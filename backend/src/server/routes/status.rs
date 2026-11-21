//! 服务状态探测端点 / Service-status probing endpoint
//!
//! `GET /api/service-status`：并行测量各后端依赖的连通性与延时——SC API
//! （镜像改写生效）、SC 浏览页、GitHub、以及按角色分组的 CDN 域（列表
//! 域/分片域/节点分片/图片域）。每条目返回「普通腿 + ECH 腿」两个并行
//! 探测结果（对齐分片下载双腿竞速的形态）：普通腿走非 ECH 通道（有代理
//! 时经代理），ECH 腿走借用 ECH 通道（未启用时整条缺席）。CDN 提供商仅凭响应头内置识别
//! （各地区解析到的厂商可能不同）。浏览器侧的目标（camgirlfinder 前端
//! 直连）由状态页在前端自测，不在此端点内。
//!
//! `GET /api/service-status`: parallel connectivity + latency probes for
//! every backend dependency — the SC API (mirror rewrite applied), the SC
//! browse page, GitHub, and CDN domains grouped by role (playlist /
//! segment / node-segment / thumbnails). Each item returns two parallel
//! probe results — a plain leg (non-ECH channel, proxy-aware) plus an ECH leg (borrowed
//! ECH channel, absent entirely when disabled) — matching the segment
//! download's dual-leg race. CDN providers are identified from response
//! headers only (the answering vendor can differ per region). Browser-side
//! targets (the frontend's direct camgirlfinder link) are probed by the
//! status page itself, not here.

use crate::config::app_state::AppState;
use crate::core::error::Result;
use crate::platform::stripchat::{CDN_TLDS, ProbeResult, StripchatApi};
use crate::server::router::ServerState;
use axum::{
	Json,
	extract::{Query, State},
};
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 探测请求超时（远小于生产客户端的 30s——状态页要快速判定）/
/// Probe request timeout (well under the production client's 30s — the
/// status page wants a quick verdict)
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 探测目标 URL / Probe target URLs
const SC_API_URL: &str = "https://stripchat.com/api/front/models?limit=1";
const SC_BROWSE_URL: &str = "https://stripchat.com/";
const GITHUB_API_URL: &str = "https://api.github.com/";
const GITHUB_WEB_URL: &str = "https://github.com/";

/// 一条探测腿的结果（普通或 ECH）/ one probe leg's outcome (plain or ECH)
#[derive(Serialize)]
struct LegStat {
	/// 连通性：拿到 <500 的 HTTP 应答即视为可达 /
	/// reachable: any HTTP response with status <500 counts
	ok: bool,
	/// 耗时毫秒（失败项为 null）/ elapsed ms (null on failure)
	latency_ms: Option<u64>,
	/// 详情：HTTP 状态或错误摘要 / detail: HTTP status or error summary
	detail: String,
}

/// 单个探测条目：普通腿与 ECH 腿同时返回（对齐分片下载双腿竞速的形态，
/// 前端并列显示）/ One probed item: plain + ECH legs returned together
/// (matching the segment download's dual-leg race — the frontend shows
/// them side by side)
#[derive(Serialize)]
struct StatusItem {
	/// 稳定 id（前端 i18n 键后缀）/ stable id (frontend i18n key suffix)
	id: &'static str,
	/// 分组：backend（后端服务）/ cdn（CDN 域）/ group id: `backend` or `cdn`
	group: &'static str,
	/// CDN 条目角色（仅 CDN 组）：playlist/media/node/thumb/ech /
	/// CDN item role (cdn group only): playlist/media/node/thumb/ech
	#[serde(skip_serializing_if = "Option::is_none")]
	role: Option<&'static str>,
	/// 实际请求的 URL（镜像改写后）/ effective probed URL (post mirror)
	url: String,
	/// 目标主机 / target host
	host: String,
	/// 提供商键（前端翻译；空串 = 未识别，不显示徽标）/ provider key
	/// (translated by the frontend; empty = unrecognized, badge hidden)
	provider: &'static str,
	/// 普通腿（生产配置下的非 ECH 通道，有代理时经代理）/ plain leg (the
	/// non-ECH channel under the production config — proxy-aware)
	direct: LegStat,
	/// ECH 腿：ECH 未启用/不适用 → null（前端隐藏该腿）；启用但主机无
	/// 通道 → no-ech-channel 失败腿；有通道 → 独立计时结果。整条腿绝对
	/// 只走 ECH、绝不退回普通客户端；「解析通道 + 发请求」共享探测预算，
	/// 超时按失败腿呈现（等效超时）/ ECH leg: null when ECH is off or
	/// not applicable (the frontend hides it); a `no-ech-channel` failure
	/// leg when enabled but the host has no usable channel; an
	/// independently timed result otherwise. The leg is strictly ECH-only
	/// — never falls back to the plain client; "resolve channel + request"
	/// share the probe budget, and overrunning it is reported as a
	/// timeout-class failure leg
	#[serde(skip_serializing_if = "Option::is_none")]
	ech: Option<LegStat>,
	/// 一次性探测项（走镜像的条目前端只测一轮即冻结，省流量）/
	/// one-shot probe item (mirror-routed entries are probed in the first
	/// round only — the frontend freezes them afterwards to save traffic)
	once: bool,
	/// 展示排序（前端按 group+order 排）/ display order (frontend sorts by group+order)
	#[serde(skip)]
	order: u16,
}

/// 宽松布尔反序列化：serde_urlencoded 原生只认 "true"/"false"，这里额外
/// 接受 "1"/"yes"/"on"——URL 查询串惯用 `?flag=1` /
/// Lenient bool deserialization: serde_urlencoded natively accepts only
/// "true"/"false"; this additionally takes "1"/"yes"/"on" — the `?flag=1`
/// convention common in query strings
fn de_boolish<'de, D>(d: D) -> std::result::Result<bool, D::Error>
where
	D: serde::Deserializer<'de>,
{
	Ok(matches!(
		String::deserialize(d)?.as_str(),
		"1" | "true" | "yes" | "on"
	))
}

/// `GET /api/service-status` 的查询参数 / Query params for `GET /api/service-status`
#[derive(Deserialize)]
pub struct StatusQuery {
	/// 跳过走镜像的条目（scApi 走 sc_mirror_url 时、scBrowse 走
	/// sc_browse_mirror_url 时不重复探测）/ skip mirror-routed items
	/// (scApi when sc_mirror_url is set; scBrowse when sc_browse_mirror_url
	/// is set — they were already probed in the first round)
	#[serde(default, deserialize_with = "de_boolish")]
	skip_mirrored: bool,
}

/// 响应体 / Response payload
#[derive(Serialize)]
pub struct ServiceStatusResp {
	/// 整轮探测耗时（并行执行，约等于最慢一项）/ overall probe elapsed
	/// (parallel — roughly the slowest probe)
	elapsed_ms: u64,
	/// 全部条目 / every probed item
	items: Vec<StatusItem>,
}

/// 一个待探测的 CDN 目标 / One CDN probe target
struct CdnTarget {
	/// 条目角色 / item role
	role: &'static str,
	/// 条目 id（前端 i18n）/ item id (frontend i18n)
	id: &'static str,
	/// 目标主机 / target host
	host: String,
}

/// 枚举 CDN 探测目标：每个 TLD 的列表域(edge-hls)与分片域(media-hls)，
/// 外加 .live 的节点子域样本。缩略图域属前端竞速链路（见前端组），
/// 不在后端探测。/ Enumerates CDN targets: each TLD's playlist host
/// (edge-hls) and segment host (media-hls), plus a .live node-subdomain
/// sample. The thumbnail domain rides a frontend racing path (see the
/// frontend group) and isn't probed here.
fn cdn_targets() -> Vec<CdnTarget> {
	let mut out = Vec::new();
	for &tld in CDN_TLDS {
		out.push(CdnTarget {
			role: "playlist",
			id: "cdnPlaylist",
			host: format!("edge-hls.{tld}"),
		});
		// .live 没有 media-hls.{tld} 主机形态（生产 URL 走 b-hls-N 节点
		// 子域），该主机也不在 CF 可路由名单内（ECH 探测恒 530 zone-miss）
		// ——探测一个生产从不使用的主机没有意义，跳过 /
		// .live has no media-hls.{tld} host form (production URLs ride the
		// b-hls-N node subdomain) and the host isn't CF-routable (ECH
		// probes always 530 zone-miss) — probing a host production never
		// uses is pointless, skip it
		if tld != "doppiocdn.live" {
			out.push(CdnTarget {
				role: "media",
				id: "cdnMedia",
				host: format!("media-hls.{tld}"),
			});
		}
	}
	// .live 节点子域分片形态样本（真实节点主机）/
	// .live node-subdomain segment-form sample (a real node host)
	out.push(CdnTarget {
		role: "node",
		id: "cdnNode",
		host: "b-hls-22.doppiocdn.live".to_string(),
	});
	// 缩略图不从此处探测：真实消费者是前端（浏览器直接 <img>/fetch 加载
	// 且跨 img.doppiocdn.{com,org,live,net} 竞速），后端探测的普通+ECH
	// 双腿不代表任何真实链路——前端组用浏览器侧竞速探测代替 /
	// thumbnails are intentionally not probed here: the real consumer is
	// the frontend (the browser loads them directly via <img>/fetch,
	// racing img.doppiocdn.{com,org,live,net}), so backend plain+ECH legs
	// would measure a path nothing uses — the frontend group probes the
	// real browser-side race instead
	out
}

/// 从响应头识别 CDN 提供商（按地域解析到的厂商可能不同，谁应答就是谁）：
/// cf-ray/server:cloudflare → cloudflare；x-amz-cf-*/server:cloudfront →
/// cloudfront；x-nws-log-uuid/EO-LOG-UUID/server:nws → 腾讯；cdn77 →
/// CDN77；byte-nginx/x-bdcdn-*/x-tt-trace-tag → 字节跳动。
/// 识别不出返回 None（不显示提供商徽标）。
/// Detects the CDN provider from response headers (the answering vendor
/// can differ per region — whoever answered is the provider):
/// cf-ray/server:cloudflare → cloudflare; x-amz-cf-*/server:cloudfront →
/// cloudfront; x-nws-log-uuid/EO-LOG-UUID/server:nws → Tencent; cdn77 →
/// CDN77; byte-nginx/x-bdcdn-*/x-tt-trace-tag → ByteDance.
/// None when unrecognized (no provider badge shown).
fn provider_from_headers(h: &HeaderMap) -> Option<&'static str> {
	let server = h
		.get(reqwest::header::SERVER)
		.and_then(|v| v.to_str().ok())
		.unwrap_or("")
		.to_ascii_lowercase();
	if h.contains_key("cf-ray") || server.contains("cloudflare") {
		Some("cloudflare")
	} else if h.contains_key("x-amz-cf-id")
		|| h.contains_key("x-amz-cf-pop")
		|| server.contains("cloudfront")
	{
		Some("cloudfront")
	} else if h.contains_key("x-nws-log-uuid")
		|| h.contains_key("eo-log-uuid")
		|| server.contains("nws")
	{
		Some("tencent")
	} else if server.contains("cdn77") {
		Some("cdn77")
	} else if server.contains("byte-nginx")
		|| h.keys().any(|k| k.as_str().starts_with("x-bdcdn"))
		|| h.contains_key("x-tt-trace-tag")
	{
		Some("bytedance")
	} else {
		None
	}
}

/// 从 URL 提取主机名 / Extracts the host from a URL
fn host_of(url: &str) -> String {
	reqwest::Url::parse(url)
		.ok()
		.and_then(|u| u.host_str().map(|s| s.to_string()))
		.unwrap_or_default()
}

/// 错误摘要：取首行、截 160 字符 / Error summary: first line, capped at 160 chars
fn short_err(e: &crate::core::error::AppError) -> String {
	let s = e.to_string();
	let line = s.lines().next().unwrap_or(&s);
	line.chars().take(160).collect()
}

/// 条目静态描述（探测前已知的信息）/ Static item descriptor (everything
/// known before the probe runs)
struct ItemSpec<'a> {
	/// 条目 id（前端 i18n）/ item id (frontend i18n)
	id: &'static str,
	/// 分组 / group
	group: &'static str,
	/// CDN 角色（非 CDN 组为 None）/ CDN role (None outside the cdn group)
	role: Option<&'static str>,
	/// 探测目标 URL 提示（失败回退展示）/ probe target URL hint
	/// (fallback display on failure)
	url_hint: &'a str,
	/// 静态提供商键 / static provider key
	provider: &'static str,
	/// 是否一次性探测项 / one-shot probe item
	once: bool,
}

/// 探测结果 → 一条腿：成功记状态码与耗时，失败记错误摘要 /
/// Maps a probe result to one leg: success records the status code and
/// elapsed; failure records the error summary
fn leg_of(res: Result<ProbeResult>) -> LegStat {
	match res {
		Ok(p) => LegStat {
			ok: p.status < 500,
			latency_ms: Some(p.elapsed.as_millis() as u64),
			detail: format!("HTTP {}", p.status),
		},
		Err(e) => LegStat {
			ok: false,
			latency_ms: None,
			detail: short_err(&e),
		},
	}
}

/// ECH 探测结果 → (ECH 腿, 提供商提示)：ECH 未启用 → (None, None)
/// （前端隐藏该腿）；启用但主机无通道（借用失败/探活被拒负缓存）→
/// no-ech-channel 失败腿；有通道 → 独立计时结果（CDN 根路径正常回
/// 403/404 同样证明通道可路由，<500 即记连通）。
/// 提供商提示取 ECH 应答的响应头——借道边缘只是转发 TLS，应答仍由
/// 目标主机所在 CDN 签名，普通腿失败/签名缺失时它是有效的识别途径。
/// Maps the ECH probe to (ECH leg, provider hint): (None, None) when ECH
/// is off (frontend hides the leg); a `no-ech-channel` failure leg when
/// enabled but the host has no usable channel (borrow failed / probe-
/// rejected and negative-cached); an independently timed result
/// otherwise — CDN roots legitimately answer 403/404 which still proves
/// the channel routes, so <500 counts as reachable. The provider hint
/// reads the ECH answer's headers — the borrowed edge only forwards TLS,
/// so the answer is still signed by the target host's real CDN, a valid
/// identification path when the plain leg fails or lacks a signature.
fn ech_leg(
	res: Result<Option<ProbeResult>>,
	ech_enabled: bool,
) -> (Option<LegStat>, Option<&'static str>) {
	if !ech_enabled {
		return (None, None);
	}
	match res {
		Ok(Some(p)) => {
			let prov = provider_from_headers(&p.headers);
			(Some(leg_of(Ok(p))), prov)
		}
		Ok(None) => (
			Some(LegStat {
				ok: false,
				latency_ms: None,
				detail: "no-ech-channel".to_string(),
			}),
			None,
		),
		Err(e) => (
			Some(LegStat {
				ok: false,
				latency_ms: None,
				detail: short_err(&e),
			}),
			None,
		),
	}
}

/// 探测结果 → 展示条目：普通腿折算结果；提供商按「普通腿响应头 →
/// ECH 腿应答头 → 静态键」顺序识别（普通腿失败时同样用 ECH 应答头
/// 兜底），ECH 腿原样附带。
/// Maps a probe result to a display item: the plain leg is folded from
/// the probe result; the provider is identified in the order "plain-leg
/// headers → ECH-leg headers → static key" (the ECH headers also cover
/// a failed plain leg); the ECH leg is attached as built.
fn to_item(
	spec: ItemSpec<'_>,
	order: u16,
	res: Result<ProbeResult>,
	ech: (Option<LegStat>, Option<&'static str>),
) -> StatusItem {
	let (ech, ech_provider) = ech;
	match res {
		Ok(p) => {
			let provider = provider_from_headers(&p.headers)
				.or(ech_provider)
				.unwrap_or(spec.provider);
			let host = if p.url.is_empty() {
				host_of(spec.url_hint)
			} else {
				host_of(&p.url)
			};
			StatusItem {
				id: spec.id,
				group: spec.group,
				role: spec.role,
				url: p.url.clone(),
				host,
				provider,
				direct: leg_of(Ok(p)),
				ech,
				once: spec.once,
				order,
			}
		}
		Err(e) => StatusItem {
			id: spec.id,
			group: spec.group,
			role: spec.role,
			url: spec.url_hint.to_string(),
			host: host_of(spec.url_hint),
			// 普通腿失败也可用 ECH 应答头兜底识别（借道边缘的应答
			// 仍由目标主机所在 CDN 签名）/ the ECH answer's headers
			// still identify the provider when the plain leg fails
			// (the borrowed edge's answer is signed by the target's CDN)
			provider: ech_provider.unwrap_or(spec.provider),
			direct: leg_of(Err(e)),
			ech,
			once: spec.once,
			order,
		},
	}
}

/// GitHub 探测客户端：沿用 api_proxy_url（更新检查同路径），显式禁用
/// 环境代理避免意外回落 / Probe client for GitHub: rides api_proxy_url
/// (same path as the update check) and explicitly disables env proxies
/// when unset — no surprising fallback
fn github_client(proxy: Option<&str>) -> reqwest::Client {
	let mut builder = reqwest::Client::builder()
		.user_agent("stripchat-recorder")
		.timeout(PROBE_TIMEOUT)
		.no_proxy();
	if let Some(p) = proxy.filter(|s| !s.is_empty())
		&& let Ok(px) = reqwest::Proxy::all(p)
	{
		builder = builder.proxy(px);
	}
	builder.build().unwrap_or_default()
}

/// 通用计时 GET（GitHub 等不走 StripchatApi 通道的目标）/
/// Generic timed GET (for targets not riding a StripchatApi channel)
async fn probe_get(client: &reqwest::Client, url: &str) -> Result<ProbeResult> {
	let t0 = Instant::now();
	let resp = client.get(url).timeout(PROBE_TIMEOUT).send().await?;
	Ok(ProbeResult {
		status: resp.status().as_u16(),
		elapsed: t0.elapsed(),
		url: url.to_string(),
		headers: resp.headers().clone(),
	})
}

/// `GET /api/service-status`：并行探测全部后端依赖并返回条目清单。
/// `skip_mirrored=1` 时跳过走镜像的条目（前端首轮全量探测后改走此参，
/// 镜像条目不重复探测以省流量）。
/// `GET /api/service-status`: probes every backend dependency in parallel
/// and returns the item list. `skip_mirrored=1` skips mirror-routed items
/// (the frontend switches to it after the first full round — mirrored
/// entries need only a single probe, saving traffic).
pub async fn get_service_status(
	State(s): State<ServerState>,
	Query(q): Query<StatusQuery>,
) -> Json<ServiceStatusResp> {
	Json(collect_service_status(&s.app_state, q.skip_mirrored).await)
}

/// 探测主体（axum handler 与桌面 Tauri 命令共用）：并行探测全部后端依赖
/// 并返回条目清单。`skip_mirrored` 时跳过走镜像的条目（前端首轮全量
/// 探测后改走此参，镜像条目不重复探测以省流量）。
///
/// Probe body shared by the axum handler and the desktop Tauri command:
/// probes every backend dependency in parallel and returns the item list.
/// `skip_mirrored` skips mirror-routed items (the frontend switches to it
/// after the first full round — mirrored entries need only a single probe,
/// saving traffic).
pub async fn collect_service_status(
	app_state: &Arc<AppState>,
	skip_mirrored: bool,
) -> ServiceStatusResp {
	let settings = app_state.get_settings();
	let t0 = Instant::now();
	let mut set = tokio::task::JoinSet::new();
	let mut order: u16 = 0;
	let mut next_order = || {
		order += 1;
		order
	};

	// 镜像是否生效（非空才算）/ whether a mirror is in effect (non-empty)
	let sc_mirror_set = settings
		.sc_mirror_url
		.as_deref()
		.map(|m| !m.is_empty())
		.unwrap_or(false);
	let browse_mirror_set = settings
		.sc_browse_mirror_url
		.as_deref()
		.map(|m| !m.is_empty())
		.unwrap_or(false);

	// SC API + CDN 普通腿 + CDN ECH 共用一个生产实例（api/cdn 代理与 ECH 开关
	// 均按设置生效，探到的就是录制真实走的通道）/ One StripchatApi covers
	// the SC API, plain-CDN and CDN-ECH probes — its channels already carry
	// the configured proxies and ECH flag, so probes measure the very paths
	// recording uses
	match StripchatApi::new_api_only(
		settings.api_proxy_url.as_deref(),
		settings.cdn_proxy_url.as_deref(),
		settings.sc_mirror_url.as_deref(),
		Some(settings.sc_mirror_scheme.as_str()),
		settings.ech_enabled,
	) {
		Ok(api) => {
			let api = std::sync::Arc::new(api);
			// 镜像改写生效的条目只测一次（skip_mirrored 轮里整项缺席，
			// 前端保留首轮结果冻结展示）/ mirror-routed items are one-shot:
			// absent from skip_mirrored rounds — the frontend keeps the
			// first-round result frozen on screen
			if !(skip_mirrored && sc_mirror_set) {
				let ord = next_order();
				let ech_enabled = settings.ech_enabled;
				set.spawn({
					let api = api.clone();
					async move {
						// 普通腿与 ECH 腿并行探测（对齐分片下载双腿竞速）/
						// plain + ECH legs probed in parallel (matching
						// the segment download's dual-leg race)
						let (ch, ech) = tokio::join!(
							api.probe_api_direct(SC_API_URL),
							api.probe_api_ech(SC_API_URL),
						);
						to_item(
							ItemSpec {
								id: "scApi",
								group: "backend",
								role: None,
								url_hint: SC_API_URL,
								provider: "cloudflare",
								once: sc_mirror_set,
							},
							ord,
							ch,
							ech_leg(ech, ech_enabled),
						)
					}
				});
			}
			// CDN 域（逐域单独度量，不经 cdn_get 竞速；每域的普通腿与
			// ECH 腿并行探测）/ CDN domains (each measured alone — no
			// cdn_get race; each domain's plain + ECH legs probe in
			// parallel)
			for tgt in cdn_targets() {
				let ord = next_order();
				let api = api.clone();
				let ech_enabled = settings.ech_enabled;
				set.spawn(async move {
					let url = format!("https://{}/", tgt.host);
					let (ch, ech) =
						tokio::join!(api.probe_cdn_direct(&url), api.probe_cdn_ech(&url),);
					to_item(
						ItemSpec {
							id: tgt.id,
							group: "cdn",
							role: Some(tgt.role),
							url_hint: &url,
							provider: "",
							once: false,
						},
						ord,
						ch,
						ech_leg(ech, ech_enabled),
					)
				});
			}
		}
		Err(e) => {
			// 客户端构建失败（多为代理串非法）：SC API 与 CDN 组各放一条错误
			// 条目，页面仍能看到其余目标 / Client build failure (usually a
			// malformed proxy string): one error item each for the SC API and
			// the CDN group, other targets still show
			let detail = short_err(&e);
			set.spawn(async move {
				StatusItem {
					id: "scApi",
					group: "backend",
					role: None,
					url: SC_API_URL.to_string(),
					host: host_of(SC_API_URL),
					provider: "cloudflare",
					direct: LegStat {
						ok: false,
						latency_ms: None,
						detail: detail.clone(),
					},
					ech: None,
					once: sc_mirror_set,
					order: 1,
				}
			});
		}
	}

	// SC 浏览页：独立代理/镜像设置（sc_browse_*）/ SC browse page: dedicated
	// proxy/mirror settings (sc_browse_*)
	if !(skip_mirrored && browse_mirror_set) {
		match StripchatApi::new_for_browse(&settings) {
			Ok(api) => {
				let ord = next_order();
				let ech_enabled = settings.ech_enabled;
				set.spawn(async move {
					let (ch, ech) = tokio::join!(
						api.probe_api_direct(SC_BROWSE_URL),
						api.probe_api_ech(SC_BROWSE_URL),
					);
					to_item(
						ItemSpec {
							id: "scBrowse",
							group: "backend",
							role: None,
							url_hint: SC_BROWSE_URL,
							provider: "cloudflare",
							once: browse_mirror_set,
						},
						ord,
						ch,
						ech_leg(ech, ech_enabled),
					)
				});
			}
			Err(e) => {
				let detail = short_err(&e);
				let ord = next_order();
				set.spawn(async move {
					StatusItem {
						id: "scBrowse",
						group: "backend",
						role: None,
						url: SC_BROWSE_URL.to_string(),
						host: host_of(SC_BROWSE_URL),
						provider: "cloudflare",
						direct: LegStat {
							ok: false,
							latency_ms: None,
							detail,
						},
						ech: None,
						once: browse_mirror_set,
						order: ord,
					}
				});
			}
		}
	}

	// GitHub（api_proxy_url 通道，与更新检查一致）/ GitHub (api_proxy_url
	// channel — same as the update check)
	let gh = github_client(settings.api_proxy_url.as_deref());
	for (id, url, ord) in [
		("githubApi", GITHUB_API_URL, next_order()),
		("githubWeb", GITHUB_WEB_URL, next_order()),
	] {
		let gh = gh.clone();
		set.spawn(async move {
			to_item(
				ItemSpec {
					id,
					group: "backend",
					role: None,
					url_hint: url,
					provider: "github",
					once: false,
				},
				ord,
				probe_get(&gh, url).await,
				(None, None),
			)
		});
	}

	// 收集软时限：超时仍在探测的条目本轮缺席——JoinSet drop 会 abort 余下
	// 探测任务，但条目内部派生的 ECH 解析任务不随调用方取消、继续在后台
	// 把通道缓存写热，下一轮照常返回 / Soft collection deadline: items
	// still probing past it are absent this round — dropping the JoinSet
	// aborts the remaining probe tasks, but the detached ECH-resolve tasks
	// inside survive and keep warming the channel cache in the background,
	// so those items land on the next round
	let mut items = Vec::new();
	let deadline = tokio::time::sleep(Duration::from_secs(12));
	tokio::pin!(deadline);
	loop {
		tokio::select! {
			joined = set.join_next() => {
				match joined {
					Some(Ok(item)) => items.push(item),
					Some(_) => {}
					None => break,
				}
			}
			() = &mut deadline => break,
		}
	}
	items.sort_by_key(|i| i.order);
	ServiceStatusResp {
		elapsed_ms: t0.elapsed().as_millis() as u64,
		items,
	}
}
