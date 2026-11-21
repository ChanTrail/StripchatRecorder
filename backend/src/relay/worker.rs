//! 流转发 Worker / Stream Relay Worker
//!
//! 架构：主播在线时转发 HLS fMP4 分片，离线时 worker 直接退出（HTTP 连接断开）。
//! - 上游在线：HLS fMP4 分片 → converter ffmpeg → HTTP-FLV 广播
//! - 上游离线/出错：worker 退出，broadcast channel 关闭，客户端连接自然断开
//!
//! 优化：若 AppState 中已缓存该主播的 model_id，直接用它构造 CDN playlist URL，
//! 跳过 /api/front/v1/broadcasts/{username} 的 API 查询，减少不必要的 HTTP 请求。
//!
//! Optimisation: when the streamer's model_id is already cached in AppState, the CDN
//! playlist URL is constructed directly from it, bypassing the
//! /api/front/v1/broadcasts/{username} API call.

use super::state::{NewRelayWorker, RelayManager, RelayStreamParams, RelayStreamState};
use crate::config::app_state::AppState;
use crate::core::error::AppError;
use crate::core::no_window::NoWindowExt;
use crate::platform::stripchat::StripchatApi;
use crate::recording::hls::{get_url_prefix, parse_playlist};
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};

/// 为 `subscribe_or_create` 新建的会话启动 worker；通道由会话创建方提供，这里只负责 spawn。
/// worker 的所有状态写入与退出时的移除都带 `session_id`，不会影响之后新建的同名会话。
///
/// Start the worker for a session newly created by `subscribe_or_create`; the channels
/// come from the session creator and this function only spawns. Every state write and
/// the final removal carry `session_id`, so they never affect a later session of the same name.
pub fn start_streamer(
	username: String,
	session_id: u64,
	app_state: Arc<AppState>,
	relay_manager: Arc<RelayManager>,
	worker: NewRelayWorker,
) {
	tokio::spawn(worker_loop(
		username,
		session_id,
		app_state,
		relay_manager,
		worker.stop_rx,
		worker.ts_tx,
		worker.stream_params,
	));
}

/// 将会话级 URL 覆盖参数合并到程序设置上并应用到 API 客户端：
/// 每个字段缺省回退程序设置（回退逻辑与程序设置路径一致）。
///
/// Merge per-session URL overrides onto program settings and apply them to the
/// API client: every field falls back to the settings value (identical fallback
/// semantics as the settings path).
fn apply_stream_params(
	api: StripchatApi,
	params: &RelayStreamParams,
	settings: &crate::config::app_state::Settings,
) -> StripchatApi {
	api.with_resolution_selection(
		params.resolution.unwrap_or(settings.preferred_resolution),
		params
			.preference
			.as_deref()
			.unwrap_or(&settings.resolution_preference),
	)
	.with_codec_selection(params.codec.as_deref().unwrap_or(&settings.preferred_codec))
}

const IDLE_STOP_SECS: u64 = 30;

/// 生成「生效流参数 + 来源」描述供启动日志使用：每字段形如 `值(来源)`，
/// 来源为 url（播放 URL 覆盖）或 settings（程序设置回退）；codec 显示
/// 规范化后实际发往上游的值，无法识别显示 auto（上游不带编码参数）。
///
/// Build an "effective stream params + source" description for the startup
/// log: each field renders as `value(source)` where source is url (play-URL
/// override) or settings (program fallback); codec shows the normalized value
/// actually sent upstream, or auto when unrecognized (no codec param upstream).
fn describe_stream_params(
	params: &RelayStreamParams,
	settings: &crate::config::app_state::Settings,
) -> String {
	use crate::platform::stripchat::normalize_video_codec;
	let (res, res_src) = match params.resolution {
		Some(r) => (r.to_string(), "url"),
		None => (settings.preferred_resolution.to_string(), "settings"),
	};
	let (pref, pref_src) = match params.preference.as_deref() {
		Some(p) => (p.trim().to_string(), "url"),
		None => (settings.resolution_preference.clone(), "settings"),
	};
	let (codec, codec_src) = match params.codec.as_deref() {
		Some(c) => (
			normalize_video_codec(c).unwrap_or("auto").to_string(),
			"url",
		),
		None => (
			normalize_video_codec(&settings.preferred_codec)
				.unwrap_or("auto")
				.to_string(),
			"settings",
		),
	};
	format!("resolution={res}({res_src}) preference={pref}({pref_src}) codec={codec}({codec_src})")
}

/// Unix 毫秒时间戳（供跨任务共享的输出活跃时间）/ Unix-epoch millis for the cross-task output timestamp
fn unix_ms() -> u64 {
	SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.map(|d| d.as_millis() as u64)
		.unwrap_or(0)
}

/// 拉清单/下载分片等网络失败的错误类型。
/// `Transient` 计入连续失败并按既有逻辑刷新 URL；`Fatal` 直接终止 feed_live
/// （转换器死锁等无法靠刷新上游恢复的情形，退出后客户端重连即新会话）。
///
/// Error kinds for playlist/segment fetch failures: `Transient` feeds the
/// consecutive-failure counter and the existing URL-refresh logic; `Fatal` ends
/// feed_live outright (e.g. a wedged converter that no upstream refresh can fix —
/// after exit, a client reconnect spawns a fresh session).
enum PollError {
	Transient(String),
	Fatal(String),
}

impl std::fmt::Display for PollError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Transient(m) | Self::Fatal(m) => f.write_str(m),
		}
	}
}

async fn worker_loop(
	username: String,
	session_id: u64,
	app_state: Arc<AppState>,
	relay_manager: Arc<RelayManager>,
	mut stop_rx: mpsc::Receiver<()>,
	ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
	stream_params: RelayStreamParams,
) {
	// 启动日志带「生效参数+来源」，便于确认 URL 覆盖是否被识别 /
	// Startup log carries "effective params + source" so URL overrides are visible
	let settings = app_state.get_settings();
	tracing::info!(
		"{}",
		crate::tl!(
			"relay.workerStarted",
			username = username,
			params = describe_stream_params(&stream_params, &settings)
		)
	);

	if relay_manager.is_idle(&username, session_id, IDLE_STOP_SECS) {
		tracing::info!("{}", crate::tl!("relay.workerIdle", username = username));
		relay_manager.remove_if_current(&username, session_id);
		return;
	}

	let api = match StripchatApi::new_api_only(
		settings.api_proxy_url.as_deref(),
		settings.cdn_proxy_url.as_deref(),
		settings.sc_mirror_url.as_deref(),
		Some(settings.sc_mirror_scheme.as_str()),
		settings.ech_enabled,
	) {
		Ok(a) => apply_stream_params(
			a.with_mouflon_keys(app_state.get_mouflon_keys()),
			&stream_params,
			&settings,
		),
		Err(e) => {
			tracing::error!(
				"{}",
				crate::tl!("relay.apiClientError", username = username, error = e)
			);
			relay_manager.set_state(
				&username,
				session_id,
				RelayStreamState::Error {
					message: e.to_string(),
				},
			);
			relay_manager.remove_if_current(&username, session_id);
			return;
		}
	};

	relay_manager.set_state(&username, session_id, RelayStreamState::Connecting);

	// 从 AppState 取缓存的 model_id，优先用它直接构造 playlist URL
	// Try the cached model_id first to build the playlist URL without an API call
	let cached_model_id: Option<i64> = app_state
		.get_streamers()
		.into_iter()
		.find(|s| s.username == username)
		.and_then(|s| s.model_id);

	let (playlist_url, model_id) = if let Some(mid) = cached_model_id {
		// 有缓存 model_id：直接竞速 CDN 构造 playlist URL
		// Cached model_id available: build playlist URL via CDN race, no API call
		match api.get_playlist_url(&username, mid).await {
			Ok(url) => {
				tracing::info!("{}", crate::tl!("relay.upstreamLive", username = username));
				relay_manager.set_state(&username, session_id, RelayStreamState::Live);
				relay_manager.clear_prebuffer(&username, session_id);
				(url, mid)
			}
			Err(_) => {
				// CDN 失败（可能真的离线），回退到完整 API 查询做确认
				// CDN failed (possibly really offline); fall back to full API query
				match api.get_stream_info(&username, true, Some(mid), false).await {
					Ok(info) if info.playlist_url.is_some() => {
						// 同步 backfill model_id（改名后新 model_id 可能不同）
						if let Some(new_mid) = info.model_id {
							app_state.backfill_model_id(&username, new_mid);
						}
						relay_manager.set_state(&username, session_id, RelayStreamState::Live);
						relay_manager.set_streamer_status(
							&username,
							session_id,
							info.is_online,
							info.status,
						);
						relay_manager.clear_prebuffer(&username, session_id);
						let new_mid = info.model_id.unwrap_or(mid);
						(info.playlist_url.unwrap(), new_mid)
					}
					Ok(info) => {
						tracing::info!(
							"{}",
							crate::tl!(
								"relay.upstreamOffline",
								username = username,
								status = info.status
							)
						);
						relay_manager.set_state(
							&username,
							session_id,
							RelayStreamState::Offline {
								status: info.status.clone(),
							},
						);
						relay_manager.set_streamer_status(
							&username,
							session_id,
							info.is_online,
							info.status,
						);
						relay_manager.remove_if_current(&username, session_id);
						return;
					}
					Err(e) => {
						tracing::warn!(
							"{}",
							crate::tl!("relay.streamInfoFailed", username = username, error = e)
						);
						relay_manager.set_state(
							&username,
							session_id,
							RelayStreamState::Error {
								message: e.to_string(),
							},
						);
						relay_manager.remove_if_current(&username, session_id);
						return;
					}
				}
			}
		}
	} else {
		// 没有缓存 model_id：必须走完整 API 查询
		// No cached model_id: must use full API query
		match api.get_stream_info(&username, true, None, false).await {
			Ok(info) if info.playlist_url.is_some() => {
				// 回填 model_id 供下次直接使用
				// Backfill model_id for next time
				if let Some(mid) = info.model_id {
					app_state.backfill_model_id(&username, mid);
				}
				relay_manager.set_state(&username, session_id, RelayStreamState::Live);
				relay_manager.set_streamer_status(
					&username,
					session_id,
					info.is_online,
					info.status,
				);
				relay_manager.clear_prebuffer(&username, session_id);
				let mid = info.model_id.unwrap_or(0);
				(info.playlist_url.unwrap(), mid)
			}
			Ok(info) => {
				tracing::info!(
					"{}",
					crate::tl!(
						"relay.upstreamOffline",
						username = username,
						status = info.status
					)
				);
				relay_manager.set_state(
					&username,
					session_id,
					RelayStreamState::Offline {
						status: info.status.clone(),
					},
				);
				relay_manager.set_streamer_status(
					&username,
					session_id,
					info.is_online,
					info.status,
				);
				relay_manager.remove_if_current(&username, session_id);
				return;
			}
			Err(e) => {
				tracing::warn!(
					"{}",
					crate::tl!("relay.streamInfoFailed", username = username, error = e)
				);
				relay_manager.set_state(
					&username,
					session_id,
					RelayStreamState::Error {
						message: e.to_string(),
					},
				);
				relay_manager.remove_if_current(&username, session_id);
				return;
			}
		}
	};

	relay_manager.set_playlist_url(&username, session_id, Some(playlist_url.clone()));

	feed_live(
		&username,
		session_id,
		&playlist_url,
		model_id,
		&app_state,
		Arc::clone(&relay_manager),
		&ts_tx,
		&mut stop_rx,
		&stream_params,
	)
	.await;

	relay_manager.set_playlist_url(&username, session_id, None);
	relay_manager.remove_if_current(&username, session_id);
	tracing::info!("{}", crate::tl!("relay.workerStopped", username = username));
}

/// 在线阶段：fMP4 分片 → converter ffmpeg → HTTP-FLV 广播。
/// 上游离线或出错时直接返回（worker 随后退出，broadcast channel 关闭，客户端自然断连）。
///
/// `model_id`：主播内部 ID，用于失败时直接重建 playlist URL，跳过 API 查询。
/// `0` 表示未知（由无缓存 model_id 的首次 API 查询路径产生），此时退化为完整 API 刷新。
///
/// Live phase: fMP4 segments → converter ffmpeg → HTTP-FLV broadcast.
/// `model_id`: internal streamer ID for rebuilding the playlist URL on failure without
/// an API call. `0` means unknown (first-time path with no cached id), falling back to
/// full API refresh.
#[allow(clippy::too_many_arguments)]
async fn feed_live(
	username: &str,
	session_id: u64,
	initial_playlist_url: &str,
	model_id: i64,
	app_state: &AppState,
	relay_manager: Arc<RelayManager>,
	ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
	stop_rx: &mut mpsc::Receiver<()>,
	stream_params: &RelayStreamParams,
) {
	// 标记 ffmpeg 使用中：converter 在整个在线阶段存活，自动更新须等本函数结束
	// Mark ffmpeg in use: the converter lives for this whole live phase, so
	// auto-update must wait until the function returns
	let _ffmpeg_guard = crate::ffmpeg::in_use();

	// converter: fMP4 pipe:0 → HTTP-FLV pipe:1
	// -probesize / -analyzeduration 最小化，减少首帧延迟
	let mut converter = match tokio::process::Command::new(crate::ffmpeg::ffmpeg_program())
		.args([
			"-y",
			"-probesize",
			"32",
			"-analyzeduration",
			"0",
			"-f",
			"mp4",
			"-i",
			"pipe:0",
			"-c",
			"copy",
			"-f",
			"flv",
			"pipe:1",
		])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.no_window()
		.spawn()
	{
		Ok(c) => c,
		Err(e) => {
			tracing::error!(
				"{}",
				crate::tl!("relay.liveConverterFailed", username = username, error = e)
			);
			return;
		}
	};

	let converter_stdin = converter.stdin.take().unwrap();
	let mut converter_stdout = converter.stdout.take().unwrap();

	let (conv_in_tx, mut conv_in_rx) = mpsc::channel::<Vec<u8>>(64);

	let conv_stdin_task = tokio::spawn(async move {
		let mut stdin = converter_stdin;
		while let Some(data) = conv_in_rx.recv().await {
			if stdin.write_all(&data).await.is_err() {
				break;
			}
		}
		let _ = stdin.shutdown().await;
	});

	// 转换器 stdout 活跃时间戳：卡死看门狗用它区分「上游没料」与「ffmpeg 停摆」。
	// Converter stdout liveness stamp: lets the stall watchdog tell "upstream dry"
	// apart from "ffmpeg wedged".
	let last_out_ms = Arc::new(AtomicU64::new(unix_ms()));
	let last_out_conv = Arc::clone(&last_out_ms);

	let ts_tx_clone = ts_tx.clone();
	let relay_manager_clone = Arc::clone(&relay_manager);
	let username_conv = username.to_string();
	// FLV 标签按任意字节边界切片会让晚接入的播放器拿到半个标签——ffmpeg/ffplay
	// 能启发式重同步，PotPlayer 等更严格的解复用器探测到垃圾字节即判流损坏。
	// 这里把转换器输出重新按标签边界切分，每个广播块均为完整 FLV 元素 /
	// FLV tags sliced at arbitrary byte offsets hand late joiners half a tag —
	// ffmpeg/ffplay resyncs heuristically, stricter demuxers (e.g. PotPlayer)
	// reject the stream on garbage bytes. Re-chunk the converter output at tag
	// boundaries so every broadcast chunk is one complete FLV element
	let conv_stdout_task = tokio::spawn(async move {
		let mut buf = vec![0u8; 65536];
		// pending：尚未凑齐一个完整元素的残留字节；header_done：FLV 文件头块已切出 /
		// pending: leftover bytes not yet forming a whole element; header_done:
		// the FLV file header block has been cut out
		let mut pending: Vec<u8> = Vec::with_capacity(256 * 1024);
		let mut header_done = false;
		// 非 FLV 输入兜底：首块不带 "FLV" 签名则退化为原始透传 /
		// Non-FLV fallback: if the first bytes lack the "FLV" signature, pass
		// chunks through untouched
		let mut passthrough = false;
		loop {
			match converter_stdout.read(&mut buf).await {
				Ok(0) | Err(_) => break,
				Ok(n) => {
					last_out_conv.store(unix_ms(), Ordering::Relaxed);
					pending.extend_from_slice(&buf[..n]);
					if !header_done
						&& !passthrough && pending.len() >= 3
						&& !pending.starts_with(b"FLV")
					{
						passthrough = true;
					}
					// 防御：单个元素积压超过 8MB 视为数据损坏（正常标签载荷远小于
					// 此值），退化透传避免残留缓冲无限增长 /
					// Defensive: a single element pending beyond 8MB means corrupt
					// data (real tag payloads are far smaller); degrade to
					// passthrough so the leftover buffer cannot grow unbounded
					if !passthrough && pending.len() > 8 * 1024 * 1024 {
						passthrough = true;
					}
					if passthrough {
						if !pending.is_empty() {
							let chunk: Arc<Vec<u8>> = Arc::new(std::mem::take(&mut pending));
							relay_manager_clone.push_prebuffer(
								&username_conv,
								session_id,
								Arc::clone(&chunk),
								false,
							);
							let _ = ts_tx_clone.send(chunk);
						}
						continue;
					}
					while let Some(elem_len) = flv_element_len(&pending, header_done) {
						if pending.len() < elem_len {
							break;
						}
						let elem: Vec<u8> = pending.drain(..elem_len).collect();
						let is_keyframe = flv_is_video_keyframe(&elem, header_done);
						header_done = true;
						let chunk: Arc<Vec<u8>> = Arc::new(elem);
						relay_manager_clone.push_prebuffer(
							&username_conv,
							session_id,
							Arc::clone(&chunk),
							is_keyframe,
						);
						let _ = ts_tx_clone.send(chunk);
					}
				}
			}
		}
		tracing::info!(
			"{}",
			crate::tl!("relay.liveConverterClosed", username = username_conv)
		);
	});

	let mut last_settings = app_state.get_settings();
	let mut api = match StripchatApi::new_api_only(
		last_settings.api_proxy_url.as_deref(),
		last_settings.cdn_proxy_url.as_deref(),
		last_settings.sc_mirror_url.as_deref(),
		Some(last_settings.sc_mirror_scheme.as_str()),
		last_settings.ech_enabled,
	) {
		Ok(a) => apply_stream_params(
			a.with_mouflon_keys(app_state.get_mouflon_keys()),
			stream_params,
			&last_settings,
		),
		Err(_) => {
			drop(conv_in_tx);
			let _ = conv_stdin_task.await;
			let _ = conv_stdout_task.await;
			let _ = converter.wait().await;
			return;
		}
	};
	let mut last_mouflon_keys = app_state.get_mouflon_keys();

	let mut current_url = initial_playlist_url.to_string();
	let mut url_prefix = get_url_prefix(&current_url);
	let mut downloaded: HashSet<u32> = HashSet::new();
	let mut init_data: Option<Vec<u8>> = None;
	let mut cached_init_url: Option<String> = None;
	let mut consecutive_failures: u32 = 0;
	const MAX_FAILURES: u32 = 3;

	// ── 卡死看门狗 / Stall watchdog ──────────────────────────────────────────
	// 两类静默卡死（会话看似健康但不再产流，客户端重连无效、只能手动停 worker）：
	// A) 喂流停滞：清单冻结（URL 过期后 CDN 持续回旧清单）或分片持续失败被
	//    「标记跳过」吞掉 → had_new 恒为 false，连续失败计数永远不涨。
	// B) 转换器停摆：ffmpeg 不再产出（或停止消费 stdin 造成反压死锁）。
	// Two silent-deadlock shapes that leave the session "healthy" but dry —
	// reconnecting clients merely subscribe to the same wedged worker:
	// A) feed stall: a frozen manifest (CDN keeps serving the old playlist after
	//    the URL's signature lapses) or per-segment failures swallowed by the
	//    mark-and-skip path → had_new stays false while the failure counter
	//    never moves.
	// B) converter wedge: ffmpeg stops emitting (or stops draining stdin →
	//    back-pressure deadlock).
	const FEED_STALL_SECS: u64 = 30;
	const OUTPUT_STALL_SECS: u64 = 20;
	const STALL_REFRESH_TRIES: u32 = 2;
	let mut last_feed = Instant::now();
	let mut stall_refreshes: u32 = 0;

	loop {
		if stop_rx.try_recv().is_ok() {
			break;
		}
		if relay_manager.is_idle(username, session_id, IDLE_STOP_SECS) {
			tracing::info!("{}", crate::tl!("relay.liveIdle", username = username));
			break;
		}

		// 看门狗 B：最近仍在喂入但 stdout 静默超阈 → 转换器停摆，刷新上游救不回来，
		// 直接退出（会话移除后客户端重连即新 worker）
		// Watchdog B: fed recently but stdout silent past the threshold → converter
		// wedged beyond any upstream refresh — exit so the session dies and a
		// reconnect spawns a fresh worker
		let out_silent_ms = unix_ms().saturating_sub(last_out_ms.load(Ordering::Relaxed));
		if last_feed.elapsed() < Duration::from_secs(FEED_STALL_SECS)
			&& out_silent_ms > OUTPUT_STALL_SECS * 1000
		{
			tracing::warn!(
				"{}",
				crate::tl!("relay.converterSilent", username = username)
			);
			break;
		}

		// 看门狗 A：喂流停滞超阈 → 先重建 playlist URL 给一个恢复窗口，
		// 反复重建仍无新片（或重建失败）则退出
		// Watchdog A: feed stalled past the threshold → rebuild the playlist URL
		// and grant one recovery window; exit if refreshes run out or the
		// rebuild itself fails
		if last_feed.elapsed() >= Duration::from_secs(FEED_STALL_SECS) {
			if stall_refreshes >= STALL_REFRESH_TRIES {
				tracing::warn!("{}", crate::tl!("relay.stillStalled", username = username));
				break;
			}
			stall_refreshes += 1;
			tracing::warn!("{}", crate::tl!("relay.feedStalled", username = username));
			match refresh_playlist_url(&api, username, model_id, app_state).await {
				Some(new_url) => {
					url_prefix = get_url_prefix(&new_url);
					current_url = new_url;
					consecutive_failures = 0;
				}
				None => break,
			}
			last_feed = Instant::now();
			last_out_ms.store(unix_ms(), Ordering::Relaxed);
		}

		// 按需重建 API 客户端（设置变更时）
		let current_settings = app_state.get_settings();
		let current_mouflon_keys = app_state.get_mouflon_keys();
		let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
            || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
            || current_settings.sc_mirror_url != last_settings.sc_mirror_url
            || current_settings.sc_mirror_scheme != last_settings.sc_mirror_scheme
            // ECH 开关改变客户端 TLS 栈，属同一重建条件 /
            // The ECH toggle changes the client TLS stack — same rebuild condition
            || current_settings.ech_enabled != last_settings.ech_enabled;
		// URL 参数已覆盖的字段不受设置变更影响，跳过无谓的客户端重建 /
		// Fields pinned by URL params are unaffected by settings changes —
		// skip the needless client rebuild for them
		let stream_selection_changed = (stream_params.resolution.is_none()
			&& current_settings.preferred_resolution != last_settings.preferred_resolution)
			|| (stream_params.preference.is_none()
				&& current_settings.resolution_preference != last_settings.resolution_preference)
			|| (stream_params.codec.is_none()
				&& current_settings.preferred_codec != last_settings.preferred_codec);
		if proxy_changed || stream_selection_changed || current_mouflon_keys != last_mouflon_keys {
			if let Ok(new_api) = StripchatApi::new_api_only(
				current_settings.api_proxy_url.as_deref(),
				current_settings.cdn_proxy_url.as_deref(),
				current_settings.sc_mirror_url.as_deref(),
				Some(current_settings.sc_mirror_scheme.as_str()),
				current_settings.ech_enabled,
			) {
				api = apply_stream_params(
					new_api.with_mouflon_keys(current_mouflon_keys.clone()),
					stream_params,
					&current_settings,
				);
			}
			last_settings = current_settings;
			last_mouflon_keys = current_mouflon_keys;
		}

		match poll_and_feed(
			&api,
			username,
			&current_url,
			&url_prefix,
			&conv_in_tx,
			&mut downloaded,
			&mut init_data,
			&mut cached_init_url,
		)
		.await
		{
			Ok(had_new) => {
				consecutive_failures = 0;
				if had_new {
					// 有新分片送入即复位喂流计时与重建计数 /
					// Any fed segment resets both the feed timer and the refresh count
					last_feed = Instant::now();
					stall_refreshes = 0;
				} else {
					tokio::select! {
						_ = stop_rx.recv() => break,
						_ = tokio::time::sleep(tokio::time::Duration::from_millis(1000)) => {
							if relay_manager.is_idle(username, session_id, IDLE_STOP_SECS) { break; }
						}
					}
				}
			}
			Err(PollError::Fatal(m)) => {
				// 致命错误（如转换器 stdin 反压死锁）：刷新 URL 救不回，直接退出 /
				// Fatal (e.g. converter stdin back-pressure deadlock): no upstream
				// refresh can fix this — exit
				tracing::error!(
					"{}",
					crate::tl!("relay.pollFatal", username = username, error = m)
				);
				break;
			}
			Err(PollError::Transient(e)) => {
				consecutive_failures += 1;
				tracing::warn!(
					"{}",
					crate::tl!(
						"relay.livePollFailed",
						username = username,
						cur = consecutive_failures,
						max = MAX_FAILURES,
						error = e
					)
				);

				if consecutive_failures >= MAX_FAILURES {
					tracing::info!(
						"{}",
						crate::tl!(
							"relay.upstreamOffline",
							username = username,
							status = "offline"
						)
					);
					break;
				}

				// 优先用缓存 model_id 直接重建 playlist URL，跳过 API 查询
				// Prefer rebuilding playlist URL from cached model_id, skipping the API call
				let refreshed = if model_id != 0 {
					match api.get_playlist_url(username, model_id).await {
						Ok(new_url) => {
							url_prefix = get_url_prefix(&new_url);
							current_url = new_url;
							consecutive_failures = 0;
							true
						}
						// CDN 竞速全失败，走完整 API 查询确认是否真的离线
						// CDN race failed entirely; fall back to full API query to confirm offline
						Err(_) => false,
					}
				} else {
					false
				};

				if !refreshed {
					// 回退到完整 API 查询（含改名回退）
					// Fall back to full API query (with rename fallback)
					let known_mid = if model_id != 0 { Some(model_id) } else { None };
					match api.get_stream_info(username, true, known_mid, false).await {
						Ok(info) if info.playlist_url.is_some() => {
							if let Some(mid) = info.model_id {
								app_state.backfill_model_id(username, mid);
							}
							let new_url = info.playlist_url.unwrap();
							url_prefix = get_url_prefix(&new_url);
							current_url = new_url;
							consecutive_failures = 0;
						}
						Ok(info) => {
							tracing::info!(
								"{}",
								crate::tl!(
									"relay.upstreamOffline",
									username = username,
									status = info.status
								)
							);
							relay_manager.set_state(
								username,
								session_id,
								RelayStreamState::Offline {
									status: info.status.clone(),
								},
							);
							relay_manager.set_streamer_status(
								username,
								session_id,
								info.is_online,
								info.status,
							);
							break;
						}
						Err(_) => { /* 查询失败，靠 MAX_FAILURES 自然退出 */ }
					}
				}

				tokio::select! {
					_ = stop_rx.recv() => break,
					_ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
						if relay_manager.is_idle(username, session_id, IDLE_STOP_SECS) { break; }
					}
				}
			}
		}
	}

	drop(conv_in_tx);
	let _ = conv_stdin_task.await;
	let _ = conv_stdout_task.await;
	let _ = converter.kill().await;
	let _ = converter.wait().await;
}

#[allow(clippy::too_many_arguments)]
async fn poll_and_feed(
	api: &StripchatApi,
	username: &str,
	playlist_url: &str,
	url_prefix: &str,
	fmp4_tx: &mpsc::Sender<Vec<u8>>,
	downloaded: &mut HashSet<u32>,
	init_data: &mut Option<Vec<u8>>,
	cached_init_url: &mut Option<String>,
) -> Result<bool, PollError> {
	let mouflon_keys = api.mouflon_keys();

	let playlist_text = api
		.fetch_playlist(playlist_url)
		.await
		.map_err(|e| PollError::Transient(e.to_string()))?;

	let segments = parse_playlist(&playlist_text, url_prefix, mouflon_keys)
		.map_err(|e| PollError::Transient(e.to_string()))?;

	let new_init_url: Option<String> = segments.iter().find_map(|s| s.init_url.clone());

	let init_url_path = |u: &str| u.split('?').next().unwrap_or(u).to_string();
	let new_init_path = new_init_url.as_deref().map(init_url_path);
	let cached_path = cached_init_url.as_deref().map(init_url_path);

	if new_init_path.is_some()
		&& new_init_path != cached_path
		&& let Some(ref url) = new_init_url
	{
		match api.download_segment(url).await {
			Ok(data) => {
				*init_data = Some(data);
				*cached_init_url = Some(url.to_string());
			}
			Err(e) => {
				return Err(PollError::Transient(format!(
					"Failed to download init segment: {}",
					e
				)));
			}
		}
	}

	// 清单里最后一条分片的下标：它常常还没发布完（所有 CDN 节点回 404，或只拿到很小的一截）
	// Index of the last segment in the playlist: it is often not fully published yet (every CDN
	// node answers 404, or only a tiny part is available)
	let last_idx = segments.len().saturating_sub(1);

	let mut had_new = false;
	for (idx, seg) in segments.into_iter().enumerate() {
		if downloaded.contains(&seg.sequence) {
			continue;
		}
		let is_tail = idx == last_idx;

		// 分片必须按序号升序送入转换器：序号靠后的先送入、靠前的之后再补，fMP4 的 tfdt 就会
		// 倒退，播放器会倒放或跳回。因此：
		// - 最后一条没发布完：结束本轮，不做标记，下一轮重新拉清单后再按序下载它；
		// - 其余分片失败：标记为已处理，永久跳过（留一个缺口），不再在之后补送。
		//
		// Segments must be fed to the converter in ascending sequence order: feeding a later
		// segment first and an earlier one afterwards makes the fMP4 tfdt go backwards, so players
		// rewind or jump back. Therefore:
		// - the last entry not fully published: end this round without marking it, so it is
		//   downloaded in order after the next playlist fetch;
		// - any other segment that fails: mark it handled and skip it for good (leaving a gap)
		//   rather than feeding it later.
		let seg_bytes = match api.download_segment(&seg.url).await {
			Ok(d) if d.len() > 1000 => d,
			Err(AppError::HttpNotFound(_)) if is_tail => {
				tracing::debug!(
					"{}",
					crate::tl!(
						"relay.liveTailNotPublished",
						seq = seg.sequence,
						username = username
					)
				);
				break;
			}
			Err(e) if is_tail => {
				tracing::warn!(
					"{}",
					crate::tl!(
						"relay.liveSegmentFailed",
						seq = seg.sequence,
						username = username,
						error = e
					)
				);
				break;
			}
			Ok(_) if is_tail => break,
			Ok(_) => {
				tracing::warn!(
					"{}",
					crate::tl!(
						"relay.liveSegmentFailed",
						seq = seg.sequence,
						username = username,
						error = "segment body too small"
					)
				);
				downloaded.insert(seg.sequence);
				continue;
			}
			Err(e) => {
				tracing::warn!(
					"{}",
					crate::tl!(
						"relay.liveSegmentFailed",
						seq = seg.sequence,
						username = username,
						error = e
					)
				);
				downloaded.insert(seg.sequence);
				continue;
			}
		};

		let fmp4 = match init_data.as_deref() {
			Some(init) => {
				let mut v = Vec::with_capacity(init.len() + seg_bytes.len());
				v.extend_from_slice(init);
				v.extend_from_slice(&seg_bytes);
				v
			}
			None => seg_bytes,
		};

		// 转换器停止消费 stdin（ffmpeg 卡死不再读取）时 send 会永久阻塞——
		// 加超时兜底：超时即 Fatal，直接结束 feed_live 让客户端重连进新会话。
		// If the converter stops draining stdin (wedged ffmpeg no longer reading),
		// this send would block forever — bound it; on timeout report Fatal so
		// feed_live ends and the client reconnects into a fresh session.
		match tokio::time::timeout(Duration::from_secs(30), fmp4_tx.send(fmp4)).await {
			Ok(Ok(())) => {}
			Ok(Err(_)) => {
				return Err(PollError::Fatal("converter stdin channel closed".into()));
			}
			Err(_) => {
				return Err(PollError::Fatal("converter stdin send stalled".into()));
			}
		}

		downloaded.insert(seg.sequence);
		had_new = true;
	}

	Ok(had_new)
}

/// 喂流卡死时重建 playlist URL：优先用 model_id 直连 CDN 竞速，
/// 失败则回退完整 API 查询（顺带回填 model_id）。成功返回新 URL，失败返回 None。
///
/// Rebuild the playlist URL after a feed stall: prefer the direct CDN race via
/// model_id, falling back to the full API query (which also backfills model_id).
/// Returns the new URL on success, None otherwise.
async fn refresh_playlist_url(
	api: &StripchatApi,
	username: &str,
	model_id: i64,
	app_state: &AppState,
) -> Option<String> {
	if model_id != 0
		&& let Ok(url) = api.get_playlist_url(username, model_id).await
	{
		return Some(url);
	}
	let known_mid = if model_id != 0 { Some(model_id) } else { None };
	match api.get_stream_info(username, true, known_mid, false).await {
		Ok(info) => {
			if let Some(mid) = info.model_id {
				app_state.backfill_model_id(username, mid);
			}
			info.playlist_url
		}
		Err(_) => None,
	}
}

/// 计算 FLV 流中下一个完整元素的长度；`header_done` 表示 13 字节的文件头块
/// （FLV 签名+版本+标志+DataOffset+PrevTagSize0）已被切出。`None` 表示头部
/// 字节不足、需等待更多数据。
///
/// Return the length of the next complete element in an FLV stream;
/// `header_done` marks the 13-byte file header block (FLV signature + version +
/// flags + DataOffset + PrevTagSize0) as already consumed. `None` means the
/// header bytes are incomplete and more data is needed.
fn flv_element_len(buf: &[u8], header_done: bool) -> Option<usize> {
	if !header_done {
		return Some(13);
	}
	if buf.len() < 11 {
		return None;
	}
	// 标签头第 1..4 字节为 DataSize；元素总长 = 11B 头 + 载荷 + 4B PrevTagSize /
	// Bytes 1..4 of the tag header hold DataSize; total element length =
	// 11B header + payload + 4B PrevTagSize
	let data_size = u32::from_be_bytes([0, buf[1], buf[2], buf[3]]) as usize;
	Some(11 + data_size + 4)
}

/// 判定一个完整 FLV 元素是否为视频关键帧标签，用于新订阅的播放起点对齐。
/// `preceded_by_header` 为 false 表示该元素本身是文件头块。
///
/// Whether a complete FLV element is a video keyframe tag, used to align the
/// playback start point of new subscriptions. `preceded_by_header` is false
/// when the element is the file header block itself.
fn flv_is_video_keyframe(elem: &[u8], preceded_by_header: bool) -> bool {
	// 视频标签类型为 0x09；最短需要头 11B + 载荷 2B + PrevTagSize 4B /
	// Video tag type is 0x09; minimum is 11B header + 2B payload + 4B PrevTagSize
	if !preceded_by_header || elem.len() < 17 || elem[0] != 0x09 {
		return false;
	}
	let data = &elem[11..];
	// 载荷首字节高 4 位为 FrameType，1 = keyframe /
	// The high nibble of the first payload byte is FrameType; 1 = keyframe
	if data[0] >> 4 != 1 {
		return false;
	}
	match data[0] & 0x0F {
		// Enhanced-RTMP（HEVC/AV1/VP9）：低 4 位为 PacketType，1=CodedFrames、
		// 2=CodedFramesX，均携带无序列头的帧数据 /
		// Enhanced-RTMP (HEVC/AV1/VP9): low nibble is PacketType —
		// 1=CodedFrames, 2=CodedFramesX, both carrying plain frame data
		1 | 2 => true,
		// 传统编码（AVC=7、旧式 HEVC=12）：次字节 PacketType 1 = NALU 载荷
		//（0 为序列头，非关键帧画面）/
		// Legacy codecs (AVC=7, legacy HEVC=12): the next byte's PacketType
		// 1 = NALU payload (0 is the sequence header, not a keyframe picture)
		7 | 12 => data[1] == 0x01,
		_ => false,
	}
}
