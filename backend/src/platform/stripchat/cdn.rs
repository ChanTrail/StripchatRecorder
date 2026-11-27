//! CDN 请求：直取 → 多 TLD 形态感知竞速（每候选各带一条 ECH 腿），
//! 维护节点首选 TLD 记录，供播放列表与分片下载共用。
//! CDN fetching: direct try → grammar-aware multi-TLD race (one ECH leg
//! per candidate), tracking the per-node preferred TLD; shared by playlist
//! and segment downloads.

use super::net::{format_send_error, https_host};
use super::{CDN_TLDS, StripchatApi};
use crate::core::error::{AppError, Result};
use std::time::Duration;

/// 首轮直取阶段的最长等待：源 TLD 被黑洞（连接超时 ~21-30s）时若让它
/// 串行跑完请求预算，后续竞速还没开始清单窗口就已滑过多条分片——超时
/// 即进竞速，未决的源腿在竞速里重新参选（被砍断 ≠ 已判负）。
/// Max wait for the first-try phase: a black-holed source TLD (connect
/// timeout ~21-30s) would otherwise serially eat the whole request budget
/// while the playlist window slides past multiple segments — on timeout
/// fall into the race, where the unresolved source leg re-enters (an
/// aborted leg is not a verdict).
const FIRST_TRY_TIMEOUT: Duration = Duration::from_secs(4);

/// 兜底竞速收集阶段的最长等待：首个可用响应即返回；全部参选腿出结果或
/// 超时收尾。慢腿（.com 黑洞 ~21s、.live/ECH 链路超时）不应把「全灭」
/// 判决拖到半分钟——live 分片在窗口内的寿命只有 ~30-60s，判负必须快。
/// Max wait for the fallback-race collection: returns on the first usable
/// response, or ends when every leg finished / the deadline hits. Slow legs
/// (.com black-hole ~21s, .live/ECH link timeouts) must not stretch an
/// all-fail verdict to half a minute — live segments only stay fetchable
/// for ~30-60s inside the window.
const RACE_COLLECT_TIMEOUT: Duration = Duration::from_secs(12);

/// 竞速超时收尾时把结果判为 HttpNotFound 所需的最少已答 404 腿数：
/// 至少 2 条腿明确回答 404 且无一条非 404 结果，才足以认定分片已被
/// 逐出（单腿 404 可能是边缘未同步）/ Minimum answered-404 legs needed
/// to call a deadline-cut race HttpNotFound: at least two legs must have
/// explicitly answered 404 with no non-404 outcome — a lone 404 could be
/// an unsynced edge.
const NOT_FOUND_QUORUM: usize = 2;

/// ECH 腿的延迟起跑（happy-eyeballs 式）：ECH 通道命中缓存时
/// `ech_client_for` 返回已钉好 IP 的现成客户端，天然快于冷启动的直连腿
/// （DNS+TCP+TLS）——同权同刻起跑会让 ECH 几乎赢下每场竞速，违背
/// 「直连优先、ECH 兜底」的设计（钉死 CF 任播 IP 绕开 CDN 地理调度）。
/// 晚 2s 起跑：直连健康时 ECH 任务在睡眠中被 abort，不产生任何流量；
/// 直连被 SNI 黑洞挂住时 ECH 于 +2s 进场，仍远快于 ~21s 连接超时。
///
/// Delayed start for ECH legs (happy-eyeballs style): a warm ECH channel
/// makes `ech_client_for` return a ready pinned-IP client — inherently
/// faster than a cold direct leg (DNS+TCP+TLS) — so racing them at the
/// same instant lets ECH win almost every race, defeating the
/// "direct-first, ECH-fallback" design (pinned anycast IPs bypass CDN
/// geo-steering). Starting 2s late: on a healthy direct the ECH task is
/// aborted mid-sleep and produces zero traffic; when direct legs hang on
/// an SNI black-hole, ECH enters at +2s — still far ahead of the ~21s
/// connect timeout.
const ECH_LEG_DELAY: Duration = Duration::from_secs(2);

impl StripchatApi {
	/// 从 CDN URL 中提取节点 ID：优先取 media-hls 路径里的 `b-hls-{node}`
	/// 段（节点语义在路径里——`media-hls.{tld}/b-hls-{n}/...` 与
	/// `b-hls-{n}.doppiocdn.live` 共享同一节点键，首选 TLD 记录因此真正
	/// 按节点生效）；URL 形态不匹配时回退主机名首段。
	/// Extract the node ID from a CDN URL: prefer the `b-hls-{node}` path
	/// segment of media-hls URLs (the node semantics live in the path —
	/// `media-hls.{tld}/b-hls-{n}/...` and `b-hls-{n}.doppiocdn.live`
	/// share one key so the preferred-TLD record really works per node);
	/// fall back to the hostname's first label on any other URL shape.
	fn extract_node_id(url: &str) -> Option<String> {
		if let Some((node, _)) = Self::parse_media_url(url) {
			return Some(node);
		}
		let without_scheme = url.strip_prefix("https://")?;
		let host = without_scheme.split('/').next()?;
		host.split('.').next().map(|s| s.to_string())
	}

	/// 对 CDN URL 发起请求：先直取原始 URL（分片与其清单绑定在同一域名/
	/// 节点上，实测同节点各 TLD 分片字节一致），失败再对其它 TLD 做
	/// 形态感知竞速——ECH 开启时竞速里每个候选（含源 URL）都带一条 ECH
	/// 腿：借用配置是 CF 网络级共享的，且各 TLD 主机多经 CF-for-SaaS
	/// 注册可回源，不可用的腿（.net 403 / media-hls.live 530）自然落败。
	/// 返回首个成功响应，同时维护节点的首选 TLD 记录。
	///
	/// Fetch a CDN URL: try the URL as-given first — segments are bound to
	/// the host/node that served their playlist (verified: identical bytes
	/// on the same node across TLDs) — then race the remaining TLDs with
	/// grammar-aware rewrites; with ECH enabled every candidate (including
	/// the source) also carries an ECH leg — the borrowed config is
	/// CF-network-wide and most TLD hosts are CF-for-SaaS routable, while
	/// unusable legs (.net 403 / media-hls.live 530) simply lose the race.
	/// Returns the first usable response. Tracks the per-node preferred
	/// TLD.
	async fn cdn_get(&self, url: &str) -> Result<wreq::Response> {
		let src_tld = match CDN_TLDS.iter().find(|&&tld| url.contains(tld)) {
			Some(&tld) => tld,
			None => {
				return Ok(self
					.cdn_client
					.get(url)
					.header("Referer", self.referer_base())
					.send()
					.await?);
			}
		};

		let node_id = Self::extract_node_id(url).unwrap_or_else(|| "unknown".into());
		let mut errors: Vec<(String, String)> = Vec::new();
		// 是否每个已应答失败都是 HTTP 404（出现网络错误或其他状态码则为 false）
		// Whether every answered failure was an HTTP 404 (false once a
		// network error or another status shows up)
		let mut all_not_found = true;
		// 已应答且明确为 404 的腿数（竞速超时收尾时作判据）
		// Count of legs that explicitly answered 404 (deadline verdict input)
		let mut not_found_hits = 0usize;

		// 首轮：源 URL 与该节点的首选 TLD 改写并发竞速（限时）。首选 TLD
		// 是上次竞速的胜者记录——命中时直接绕开已被干扰的源 TLD（如 .com
		// 遭 RST、.net 黑洞），否则两条腿互相兜底，开销仍远小于全 TLD 竞速。
		// Phase 1: race the source URL against the node's preferred-TLD
		// rewrite (bounded). The preferred TLD is the last race's recorded
		// winner — on a hit it bypasses an interfered source outright
		// (e.g. .com RST'd, .net black-holed); otherwise the two legs cover
		// each other, still far cheaper than the full-TLD race.
		let preferred_tld = self
			.preferred_tld_by_node
			.lock()
			.get(&node_id)
			.cloned()
			.filter(|t| t != src_tld && CDN_TLDS.contains(&t.as_str()));

		let mut first = tokio::task::JoinSet::new();
		{
			let cdn_client = self.cdn_client.clone();
			let referer = self.referer_base();
			let url_owned = url.to_string();
			first.spawn(async move {
				(
					src_tld.to_string(),
					cdn_client
						.get(&url_owned)
						.header("Referer", referer)
						.send()
						.await,
				)
			});
		}
		if let Some(ref pref) = preferred_tld {
			let candidate = Self::rewrite_for_tld(url, src_tld, pref);
			let cdn_client = self.cdn_client.clone();
			let referer = self.referer_base();
			let label = pref.clone();
			first.spawn(async move {
				(
					label,
					cdn_client
						.get(&candidate)
						.header("Referer", referer)
						.send()
						.await,
				)
			});
		}

		// 收集首轮各腿结果：首个可用即胜出返回；全部失败或超时进兜底竞速。
		// 结果向量在 async 外声明——超时截断时已应答腿的记录不丢。
		// Collect phase-1 outcomes: first usable wins and returns; all
		// failed or the deadline expired → fall through to the race. The
		// results vec lives outside the async block so answers collected
		// before a deadline survive the cut.
		let mut p1_hit: Option<(String, wreq::Response)> = None;
		let mut p1_results: Vec<(String, std::result::Result<wreq::Response, wreq::Error>)> =
			Vec::new();
		{
			let phase1 = async {
				while let Some(join_result) = first.join_next().await {
					let Ok((label, resp)) = join_result else {
						continue;
					};
					if matches!(&resp, Ok(r) if Self::usable_response(r)) {
						if let Ok(r) = resp {
							p1_hit = Some((label, r));
						}
						return;
					}
					p1_results.push((label, resp));
				}
			};
			let _ = tokio::time::timeout(FIRST_TRY_TIMEOUT, phase1).await;
			first.abort_all();
		}
		if let Some((tld, resp)) = p1_hit {
			self.note_tld_win(&node_id, &tld, src_tld);
			return Ok(resp);
		}
		// 首轮已应答的腿：记录失败形态；源腿未应答（被超时砍断）时不算
		// 判负，竞速里给它一次重选机会。
		// Completed phase-1 legs: record their failure shape; a source leg
		// that never answered (cut by the deadline) is NOT a verdict — the
		// race gives it one re-entry.
		let mut src_settled = false;
		for (label, resp) in p1_results {
			if label == src_tld {
				src_settled = true;
			}
			match resp {
				Ok(r) => {
					if r.status() == wreq::StatusCode::NOT_FOUND {
						not_found_hits += 1;
					} else {
						all_not_found = false;
					}
					errors.push((label, Self::response_error(&r)));
				}
				Err(e) => {
					all_not_found = false;
					errors.push((label, format_send_error(&e)));
				}
			}
		}

		// 兜底竞速：其余 TLD 候选 +（源腿未决时的）源 URL 重试腿。竞速项
		// 返回 `Option<Result<..>>`：None 保留给「未真正参选」的 ECH 腿，
		// 收集时跳过，不占错误清单。
		// Fallback race: remaining-TLD candidates plus a source-URL re-entry
		// when its phase-1 leg went unresolved. Race items yield
		// `Option<Result<..>>`: None is reserved for ECH legs that never
		// actually entered, skipped on collection so they can't pollute the
		// error list.
		let mut tasks = tokio::task::JoinSet::new();
		if !src_settled {
			let cdn_client = self.cdn_client.clone();
			let referer = self.referer_base();
			let url_owned = url.to_string();
			tasks.spawn(async move {
				(
					src_tld.to_string(),
					Some(
						cdn_client
							.get(&url_owned)
							.header("Referer", referer)
							.send()
							.await,
					),
				)
			});
		}
		for (cand_tld, candidate) in Self::tld_candidates(url, src_tld) {
			let cdn_client = self.cdn_client.clone();
			let referer = self.referer_base();
			tasks.spawn(async move {
				let resp = cdn_client
					.get(&candidate)
					.header("Referer", referer)
					.send()
					.await;
				(cand_tld, Some(resp))
			});
		}

		// ECH 腿：源 URL 与每个候选 URL 各挂一条（标签 `{tld}+ech`）——
		// 借来的 ECH 是 Cloudflare 网络级共享配置，ech_status 对所有主机
		// 都 Accepted；doppiocdn 各 TLD 又经 CF-for-SaaS 注册（实测 .com/
		// .media/.org 及 .live 的 edge/节点形态经 ECH 返回真实清单），
		// .net 与 media-hls.live 会拿 403/530 在竞速里自然落败，无需
		// 白名单。拿不到通道的腿返回 None 不参选；5xx 降级、403/连接错误
		// 驱逐条目的回馈与 API 侧一致。
		// ECH legs: one on the source URL plus one per candidate (labelled
		// `{tld}+ech`) — the borrowed ECH config is Cloudflare-network-wide
		// (ech_status Accepts every host) and the doppiocdn TLDs are mostly
		// CF-for-SaaS registered (measured: .com/.media/.org and .live
		// edge/node forms serve real playlists via ECH), while .net and
		// media-hls.live get 403/530 and lose the race on their own — no
		// whitelist needed. Legs that can't get a channel yield None and
		// don't enter; 5xx demotion and 403/connect-error eviction mirror
		// the API side.
		if let Some(ech) = &self.cdn_ech {
			let ech_targets = std::iter::once((src_tld.to_string(), url.to_string()))
				.chain(Self::tld_candidates(url, src_tld));
			for (label, target) in ech_targets {
				let Some(host) = https_host(&target) else {
					continue;
				};
				let ech = ech.clone();
				let referer = self.referer_base();
				tasks.spawn(async move {
					// 延迟起跑：直连优先；被 abort 的睡眠不产生任何流量 /
					// Delayed start keeps direct first; an aborted sleep costs nothing
					tokio::time::sleep(ECH_LEG_DELAY).await;
					let Some(client) = ech.ech_client_for(&host).await else {
						return (format!("{label}+ech"), None);
					};
					let resp = client.get(&target).header("Referer", referer).send().await;
					match &resp {
						Ok(r) if r.status().is_server_error() => ech.note_server_error(&host).await,
						Ok(r) if r.status() == wreq::StatusCode::FORBIDDEN => {
							ech.note_forbidden(&host).await;
						}
						Err(e) if e.is_connect() => ech.evict(&host).await,
						_ => {}
					}
					(format!("{label}+ech"), Some(resp))
				});
			}
		}

		// 收集竞速结果：首个可用即胜出；限时收尾时慢腿一律中止，按已应答
		// 结果判决（状态记录同样在 async 外，截断不丢）。
		// Collect race results: first usable wins; at the deadline all
		// stragglers abort and the verdict uses the answers already in
		// (accumulators live outside the async block too).
		let mut hit: Option<(String, wreq::Response)> = None;
		{
			let collect = async {
				while let Some(join_result) = tasks.join_next().await {
					let (tld, result) = match join_result {
						Ok(r) => r,
						Err(_) => continue,
					};
					// None = ECH 候选未取得通道、未参选，跳过 /
					// None = the ECH candidate never got a channel and didn't enter
					let Some(result) = result else { continue };
					match result {
						Ok(resp) if Self::usable_response(&resp) => {
							hit = Some((tld, resp));
							return;
						}
						Ok(resp) => {
							if resp.status() == wreq::StatusCode::NOT_FOUND {
								not_found_hits += 1;
							} else {
								all_not_found = false;
							}
							errors.push((tld, Self::response_error(&resp)));
						}
						Err(e) => {
							all_not_found = false;
							errors.push((tld, format_send_error(&e)));
						}
					}
				}
			};
			let _ = tokio::time::timeout(RACE_COLLECT_TIMEOUT, collect).await;
			tasks.abort_all();
		}
		if let Some((tld, resp)) = hit {
			self.note_tld_win(&node_id, &tld, src_tld);
			return Ok(resp);
		}

		// 多个节点都明确回答 404：资源还不存在（如刚出现在清单里、尚未
		// 发布完的最后一个分片）或已被逐出——这是正常现象，不逐个节点打
		// 错误日志，交给调用方决定怎么处理。竞速被限时砍尾时，未决腿按
		// 「不足以推翻」处理：≥ [`NOT_FOUND_QUORUM`] 条腿明确 404 且无
		// 一条非 404 应答，同样认定资源不存在。
		// Several nodes explicitly answered 404: the resource does not
		// exist yet (e.g. the last segment that just appeared in the
		// playlist but is not fully published) or was already evicted.
		// This is routine, so skip the per-node error logs and let the
		// caller decide what to do. When the race was cut by the deadline,
		// unanswered legs count as "insufficient to overturn": with
		// ≥ [`NOT_FOUND_QUORUM`] explicit 404s and zero non-404 answers
		// the resource is still called missing.
		if all_not_found && not_found_hits >= NOT_FOUND_QUORUM {
			return Err(AppError::HttpNotFound(url.to_string()));
		}

		for (tld, err) in &errors {
			tracing::error!(
				"{}",
				crate::tl!("stripchat.cdnTldFailed", tld = tld, error = err)
			);
		}
		Err(AppError::Other(format!("All CDN TLDs failed → {}", url)))
	}

	/// 2xx 且非空响应才算可用：.media 边缘对未同步分片会回
	/// `200 Content-Length:0` 空体而非 404，当成功收下会写入空分片。
	/// A response is usable only when it's 2xx and non-empty: .media edges
	/// answer un-replicated segments with `200 Content-Length:0` instead of
	/// 404 — accepting those would write empty segments.
	fn usable_response(resp: &wreq::Response) -> bool {
		resp.status().is_success() && resp.content_length() != Some(0)
	}

	/// 失败响应的错误描述：区分空 200 与真实 HTTP 错误码。
	/// Describe a failed response: distinguishes empty 200s from real HTTP errors.
	fn response_error(resp: &wreq::Response) -> String {
		if resp.status().is_success() {
			format!("HTTP {} (empty body)", resp.status())
		} else {
			format!("HTTP {}", resp.status())
		}
	}

	/// 记录节点胜出的 TLD（与缓存不同时打 debug 日志并更新）。
	/// Record the node's winning TLD (debug-logs and updates on change).
	fn note_tld_win(&self, node_id: &str, winner: &str, src_tld: &str) {
		let preferred = self.preferred_tld_by_node.lock().get(node_id).cloned();
		if preferred.as_deref() != Some(winner) {
			tracing::debug!(
				"{}",
				crate::tl!(
					"stripchat.cdnTldChanged",
					node = node_id,
					from = preferred.as_deref().unwrap_or(src_tld),
					to = winner
				)
			);
			self.preferred_tld_by_node
				.lock()
				.insert(node_id.to_string(), winner.to_string());
		}
	}

	/// 生成 src_tld 之外各 TLD 的候选 (TLD, URL)：media-hls 清单/分片 URL
	/// 存在两种主机格式——`media-hls.{tld}/b-hls-{node}/{rest}` 与 .live 的
	/// 节点子域 `b-hls-{node}.doppiocdn.live/hls/{rest}`——候选按格式互转
	/// （朴素 replace 会造出 media-hls.doppiocdn.live / b-hls-{n}.doppiocdn.net
	/// 等不存在的主机）；解析不出形态时退回字符串替换兜底。
	///
	/// Build fallback (TLD, URL) candidates excluding src_tld: media-hls
	/// URLs come in two host grammars — `media-hls.{tld}/b-hls-{node}/{rest}`
	/// and .live's node-subdomain `b-hls-{node}.doppiocdn.live/hls/{rest}` —
	/// candidates are converted between them (plain replace fabricates
	/// nonexistent hosts like media-hls.doppiocdn.live or
	/// b-hls-{n}.doppiocdn.net). Falls back to string replacement when the
	/// URL matches neither grammar.
	fn tld_candidates(url: &str, src_tld: &str) -> Vec<(String, String)> {
		CDN_TLDS
			.iter()
			.filter(|&&t| t != src_tld)
			.map(|&t| (t.to_string(), Self::rewrite_for_tld(url, src_tld, t)))
			.collect()
	}

	/// 把 URL 按形态感知规则改写到指定 TLD（与 [`tld_candidates`] 同一套
	/// 语法互转；首轮「首选 TLD」腿与兜底竞速共用）。
	/// Rewrite the URL onto the given TLD with grammar-aware conversion
	/// (the same grammar as [`tld_candidates`]; shared by the phase-1
	/// "preferred TLD" leg and the fallback race).
	fn rewrite_for_tld(url: &str, src_tld: &str, target_tld: &str) -> String {
		match Self::parse_media_url(url) {
			Some((node, rest)) if target_tld == "doppiocdn.live" => {
				format!("https://{node}.doppiocdn.live/hls/{rest}")
			}
			Some((node, rest)) => {
				format!("https://media-hls.{target_tld}/{node}/{rest}")
			}
			None => url.replace(src_tld, target_tld),
		}
	}

	/// 解析 media-hls 系 URL 为 (`b-hls-{node}`, `{rest}`)：
	/// - `media-hls.{tld}/b-hls-{node}/{rest}`（.com/.media/.net/.org）
	/// - `b-hls-{node}.doppiocdn.live/hls/{rest}`（.live 节点子域形态）
	///
	/// 两种之外返回 None。
	/// Parse a media-hls-family URL into (`b-hls-{node}`, `{rest}`):
	/// - `media-hls.{tld}/b-hls-{node}/{rest}` (.com/.media/.net/.org)
	/// - `b-hls-{node}.doppiocdn.live/hls/{rest}` (.live node-subdomain form)
	///
	/// Returns None for anything else.
	fn parse_media_url(url: &str) -> Option<(String, String)> {
		// .live 节点子域形态优先（host 前缀更具体）/
		// .live node-subdomain form first (more specific host prefix)
		if let Some(rest) = url.strip_prefix("https://b-hls-")
			&& let Some((node, tail)) = rest.split_once(".doppiocdn.live/hls/")
		{
			return Some((format!("b-hls-{node}"), tail.to_string()));
		}
		// media-hls.{tld}/b-hls-{node}/{rest}
		if url.contains("media-hls.")
			&& let Some(pos) = url.find("/b-hls-")
		{
			// +1 跳过斜杠，path = "b-hls-{node}/{rest}" / skip the slash so path = "b-hls-{node}/{rest}"
			let path = &url[pos + 1..];
			if let Some((node, tail)) = path.split_once('/') {
				return Some((node.to_string(), tail.to_string()));
			}
		}
		None
	}

	/// 下载 HLS 播放列表文本内容。
	/// Download the HLS playlist text content.
	pub async fn fetch_playlist(&self, playlist_url: &str) -> Result<String> {
		let resp = self.cdn_get(playlist_url).await?;
		Ok(resp.text().await?)
	}

	/// 下载单个 HLS 分片的字节数据。
	/// Download the byte data of a single HLS segment.
	pub async fn download_segment(&self, url: &str) -> Result<Vec<u8>> {
		let resp = self.cdn_get(url).await?;
		Ok(resp.bytes().await?.to_vec())
	}

	/// 服务状态页探测：经 CDN 普通客户端 GET url——非 ECH 基线腿，一次
	/// 计时请求（不走 cdn_get 的 TLD 竞速——状态页要逐域单独度量）。
	/// Status-page probe: a single timed GET via the plain CDN client —
	/// the non-ECH baseline leg (no cdn_get TLD race — the status page
	/// measures each domain alone).
	pub(crate) async fn probe_cdn_direct(&self, url: &str) -> Result<super::ProbeResult> {
		let t0 = std::time::Instant::now();
		let resp = self
			.cdn_client
			.get(url)
			.header("Referer", self.referer_base())
			.timeout(super::PROBE_TIMEOUT)
			.send()
			.await?;
		Ok(super::ProbeResult {
			status: resp.status().as_u16(),
			elapsed: t0.elapsed(),
			url: url.to_string(),
			headers: resp.headers().clone(),
		})
	}

	/// 服务状态页探测：经 CDN ECH 通道 GET url——整条腿绝对只走 ECH：
	/// `ech_client_for` 拿不到探活通过的通道就返回 None（记
	/// no-ech-channel 失败腿），**绝不退回普通客户端**；「解析通道 +
	/// 发请求」共享一个探测预算，超出即按「ECH 不通」的超时失败返回——
	/// 状态页把该腿记为失败点（等效超时），而不会拖垮整条目本轮缺席。
	/// Status-page probe: a timed GET via the CDN ECH channel — the leg is
	/// strictly ECH-only: `ech_client_for` yields None when no
	/// probe-passed channel exists (a no-ech-channel failure leg) and
	/// **never falls back to the plain client**; "resolve channel +
	/// request" share one probe budget — past it the leg returns a
	/// timeout-class failure (the page plots a failure point, equivalent
	/// to a timeout) instead of dragging the whole item past the
	/// collection deadline.
	pub(crate) async fn probe_cdn_ech(&self, url: &str) -> Result<Option<super::ProbeResult>> {
		let (Some(ech), Some(host)) = (&self.cdn_ech, https_host(url)) else {
			return Ok(None);
		};
		tokio::time::timeout(super::PROBE_TIMEOUT, async {
			let Some(client) = ech.ech_client_for(&host).await else {
				return Ok(None);
			};
			let t0 = std::time::Instant::now();
			let resp = client
				.get(url)
				.header("Referer", self.referer_base())
				.send()
				.await?;
			Ok(Some(super::ProbeResult {
				status: resp.status().as_u16(),
				elapsed: t0.elapsed(),
				url: url.to_string(),
				headers: resp.headers().clone(),
			}))
		})
		.await
		.map_err(|_| AppError::Other("ECH 探测超时 / ECH probe timed out".into()))?
	}
}
