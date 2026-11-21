//! 主播状态监控器 / Streamer Status Monitor
//!
//! 定期轮询所有追踪主播的直播状态，并在状态变化时：
//! - 向前端发送 `status-update` 事件
//! - 自动开始/停止录制（根据 auto_record 设置）
//!
//! Periodically polls the live status of all tracked streamers and on status changes:
//! - Emits `status-update` events to the frontend
//! - Automatically starts/stops recordings (based on auto_record settings)
//!
//! 子模块划分 / Submodule layout:
//! - `monitor::cgf`：CamGirlFinder 排期获取与自适应轮询间隔 / CGF schedule + adaptive interval
//! - `monitor::polling`：监控主循环与单主播轮询 / monitor loop and per-streamer polling

mod cgf;
mod polling;

// 保持 `platform::monitor::cgf_fetch_schedule` / `schedule_poll_interval` 路径不变
// Keep the `platform::monitor::cgf_fetch_schedule` / `schedule_poll_interval` paths unchanged
pub use cgf::{cgf_fetch_schedule, schedule_poll_interval};

use crate::config::app_state::{AppState, Settings};
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::AppError;
use crate::platform::stripchat::{StripchatApi, normalize_video_codec};
use crate::recording::recorder::RecorderManager;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// 同一轮轮询内，网络类错误的重试间隔（共重试 2 次）。
///
/// 经代理连接 Stripchat 时 TLS 握手经常瞬时失败（实测新建连接一半以上失败，0.3 秒左右即报错），
/// 而 API 请求走 HTTP/2、同时进行的请求共用一条连接，这条连接握手失败时一批主播会同时报错。
/// 短间隔重试即可挡住这类瞬时失败，不必等到下一个轮询间隔。
///
/// Retry delays for network errors within the same poll round (2 retries in total).
///
/// TLS handshakes to Stripchat through a proxy often fail transiently (over half of new
/// connections failed in testing, erroring out after ~0.3 s), and since API requests go over
/// HTTP/2 with concurrent requests sharing one connection, a failed handshake fails a whole batch
/// of streamers at once. Retrying after a short delay absorbs these without waiting a full interval.
const POLL_NETWORK_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// 判断错误是否属于可在同一轮内立即重试的网络类错误：连接、TLS 握手、发送请求或读取响应
/// 失败。超时（已等满 30 秒，再重试会长时间占用并发名额）、HTTP 状态错误（如 403，立即重发
/// 可能加重限流）和"主播不存在"都不重试。
///
/// Whether an error is a network error worth retrying right away within the same round: a
/// connect, TLS handshake, request-send or response-read failure. Timeouts (the full 30 s has
/// already elapsed; retrying would hold a concurrency slot for a long time), HTTP status errors
/// (e.g. 403; re-sending immediately could worsen rate limiting) and "streamer not found" are not
/// retried.
fn is_retryable_network_error(e: &AppError) -> bool {
	match e {
		AppError::Reqwest(re) => {
			!(re.is_timeout() || re.is_builder() || re.is_status() || re.is_redirect())
		}
		_ => false,
	}
}

/// 构建 API HTTP 客户端所依赖的网络设置；只有这些变化时才需要重建客户端（连接池随之丢弃）。
/// Mouflon 密钥与分辨率只影响请求参数，变化时在复用的客户端上更新即可。
///
/// Network settings the API HTTP clients depend on; the clients (and their connection pools)
/// only need rebuilding when one of these changes. Mouflon keys and resolution only affect
/// request parameters and are updated on the reused clients.
#[derive(Clone, PartialEq, Eq)]
struct ApiNetConfig {
	api_proxy_url: Option<String>,
	cdn_proxy_url: Option<String>,
	sc_mirror_url: Option<String>,
	sc_mirror_scheme: String,
	/// ECH 开关改变客户端 TLS 栈，纳入客户端重建的变更比较 /
	/// The ECH toggle changes the client TLS stack — part of the rebuild comparison
	ech_enabled: bool,
}

impl ApiNetConfig {
	fn from_settings(s: &Settings) -> Self {
		Self {
			api_proxy_url: s.api_proxy_url.clone(),
			cdn_proxy_url: s.cdn_proxy_url.clone(),
			sc_mirror_url: s.sc_mirror_url.clone(),
			sc_mirror_scheme: s.sc_mirror_scheme.clone(),
			ech_enabled: s.ech_enabled,
		}
	}
}

/// 主播实时状态（序列化后通过 `status-update` 事件发送给前端）。
/// Streamer real-time status (serialized and sent to the frontend via `status-update` events).
#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamerStatus {
	pub username: String,
	pub is_online: bool,
	pub is_recording: bool,
	pub is_recordable: bool,
	/// 直播间状态文字（中文）/ Stream status text (Chinese)
	pub status: String,
	pub thumbnail_url: Option<String>,
	/// HLS 播放列表 URL（不序列化，仅供内部使用）/ HLS playlist URL (not serialized, internal use only)
	#[serde(skip)]
	pub playlist_url: Option<String>,
	/// 获取该播放列表时使用的首选分辨率 / Preferred resolution used to fetch this playlist
	#[serde(skip)]
	pub playlist_resolution: u32,
	/// 获取该播放列表时是否优先向上选择 / Whether higher resolution was preferred for this playlist
	#[serde(skip)]
	pub playlist_prefers_higher: bool,
	/// 获取该播放列表时使用的首选编码（规范化写法，空串 = 未指定）
	/// Preferred codec used to fetch this playlist (normalized spelling, empty = unspecified)
	#[serde(skip)]
	pub playlist_codec: String,
}

impl StreamerStatus {
	/// 缓存的播放列表是否仍与当前的分辨率与编码设置一致；不一致时它选出的流已经不是用户
	/// 现在想要的，不能再用来开始录制。
	///
	/// Whether the cached playlist still matches the current resolution and codec settings; when
	/// it does not, the stream it selected is no longer the one the user wants and must not be
	/// used to start a recording.
	fn playlist_matches(&self, settings: &Settings) -> bool {
		self.playlist_resolution == settings.preferred_resolution
			&& self.playlist_prefers_higher == (settings.resolution_preference == "higher")
			&& self.playlist_codec == normalize_video_codec(&settings.preferred_codec).unwrap_or("")
	}
}

/// 主播状态监控器，管理轮询循环和自动录制逻辑。
/// Streamer status monitor managing the polling loop and auto-recording logic.
pub struct StatusMonitor {
	/// 应用状态 / Application state
	state: Arc<AppState>,
	/// 录制管理器 / Recorder manager
	recorder: Arc<RecorderManager>,
	/// 各主播的最新状态缓存 / Latest status cache per streamer
	statuses: RwLock<HashMap<String, StreamerStatus>>,
	/// 已确认失效（id 也找不到）的主播集合，跳过轮询以节约带宽
	/// Streamers confirmed dead (not found even by model_id); skipped in polling to save bandwidth
	pub dead_streamers: RwLock<HashSet<String>>,
	/// 重启轮询循环的通知发送端（发送后立即中断当前 sleep，以新间隔重新开始）
	/// Sender to notify the polling loop to restart (interrupts current sleep, restarts with new interval)
	pub restart_tx: RwLock<Option<mpsc::Sender<()>>>,
	/// 各主播的下次可轮询时间（schedule 自适应间隔）。
	///
	/// 每次 `poll_streamer` 完成后，根据当前时段的 schedule 活跃度计算下次轮询的
	/// 最早时刻并写入此表。`poll_all_with_emitter` 在派发任务前检查此表，跳过尚未
	/// 到期的主播，从而实现每个主播独立的自适应轮询间隔。
	///
	/// Earliest next-poll timestamp per streamer (schedule-adaptive interval).
	/// Written after each `poll_streamer` call; checked in `poll_all_with_emitter`
	/// to skip streamers whose interval has not yet elapsed.
	next_poll_at: RwLock<HashMap<String, std::time::Instant>>,
	/// 跨轮复用的 API 客户端（连同构建它的网络设置）。复用客户端即复用其连接池，HTTP/2 连接
	/// 可以跨轮保持，不必每轮重新握手；网络设置变化时重建。
	///
	/// API client reused across rounds (with the network settings it was built from). Reusing
	/// the client reuses its connection pool, so the HTTP/2 connection survives across rounds
	/// instead of re-handshaking every round; rebuilt when the network settings change.
	api_cache: Mutex<Option<(ApiNetConfig, StripchatApi)>>,
	/// 各主播当天（本地日期）的离线预览图刷新进度。离线预览图随轮询刷新、与在线状态无关：
	/// 每个主播每天第一次轮询成功时顺带请求一次 cam 接口，取代原来启动时与每天 0 点的单独
	/// 刷新任务；请求失败时下一轮再试，每天最多 [`PREVIEW_MAX_ATTEMPTS_PER_DAY`] 次。
	///
	/// Each streamer's offline-preview refresh progress for the (local) day. The offline preview
	/// is refreshed as part of polling regardless of online status: each streamer's first
	/// successful poll of the day also requests the cam endpoint once, replacing the former
	/// separate refresh task that ran at startup and daily at midnight; on failure it's retried
	/// the next round, at most [`PREVIEW_MAX_ATTEMPTS_PER_DAY`] times a day.
	preview_refresh: RwLock<HashMap<String, PreviewRefresh>>,
}

/// 每个主播每天最多尝试刷新离线预览图的次数（每轮一次）。经代理请求经常瞬时失败，只试一次
/// 会让当天的预览图一直得不到更新；设上限避免 cam 接口持续失败时每轮都重复请求。
///
/// Max offline-preview refresh attempts per streamer per day (one per round). Requests through
/// the proxy often fail transiently, so a single try could leave the preview stale all day; the
/// cap avoids a repeat request every round while the cam endpoint keeps failing.
const PREVIEW_MAX_ATTEMPTS_PER_DAY: u8 = 3;

/// 某个主播当天的离线预览图刷新进度 / A streamer's offline-preview refresh progress for the day
#[derive(Clone, Copy)]
struct PreviewRefresh {
	/// 本地日期 / Local date
	date: chrono::NaiveDate,
	/// 当天已尝试次数 / Attempts made that day
	attempts: u8,
	/// 当天是否已成功取到地址 / Whether a URL was obtained that day
	done: bool,
}

impl PreviewRefresh {
	/// 当天是否还需要（再）刷新 / Whether a (further) refresh is needed today
	fn needed(progress: Option<&Self>, today: chrono::NaiveDate) -> bool {
		match progress {
			Some(p) if p.date == today => !p.done && p.attempts < PREVIEW_MAX_ATTEMPTS_PER_DAY,
			_ => true,
		}
	}

	/// 记录一次尝试（跨天时重新计数）/ Record one attempt (the count restarts on a new day)
	fn record(progress: Option<Self>, today: chrono::NaiveDate, success: bool) -> Self {
		let attempts = match progress {
			Some(p) if p.date == today => p.attempts.saturating_add(1),
			_ => 1,
		};
		Self {
			date: today,
			attempts,
			done: success,
		}
	}
}

impl StatusMonitor {
	/// 创建新的状态监控器实例。
	/// Create a new status monitor instance.
	pub fn new(state: Arc<AppState>, recorder: Arc<RecorderManager>) -> Arc<Self> {
		Arc::new(Self {
			state: Arc::clone(&state),
			recorder,
			statuses: RwLock::new(HashMap::new()),
			dead_streamers: RwLock::new(state.get_dead_streamers()),
			restart_tx: RwLock::new(None),
			next_poll_at: RwLock::new(HashMap::new()),
			api_cache: Mutex::new(None),
			preview_refresh: RwLock::new(HashMap::new()),
		})
	}

	/// 获取指定主播的缓存状态（若不存在则返回 `None`）。
	/// Get the cached status for a specific streamer (returns `None` if not cached).
	pub fn get_status(&self, username: &str) -> Option<StreamerStatus> {
		self.statuses.read().get(username).cloned()
	}

	/// 获取指定主播缓存的 HLS 播放列表 URL（用于快速开始录制，避免重复 API 请求）。
	/// Get the cached HLS playlist URL for a streamer (for fast recording start, avoiding repeated API requests).
	pub fn get_cached_playlist_url(&self, username: &str) -> Option<String> {
		let settings = self.state.get_settings();
		self.statuses
			.read()
			.get(username)
			.filter(|s| s.playlist_matches(&settings))
			.and_then(|s| s.playlist_url.clone())
	}

	/// 内部版本：直接接受已创建的 restart_rx（供 server 模式使用）。
	/// Internal version: accepts a pre-created restart_rx (used by server mode).
	pub async fn start_with_emitter_inner(
		self: Arc<Self>,
		emitter: Arc<dyn Emitter>,
		restart_rx: mpsc::Receiver<()>,
	) {
		self.monitor_loop(emitter, restart_rx).await;
	}

	/// 取得本轮使用的 StripchatApi（含代理、镜像站、Mouflon 密钥、分辨率与编码配置）。
	///
	/// HTTP 客户端跨轮复用（见 `api_cache`），只在代理或镜像站设置变化时重建；Mouflon 密钥、
	/// 分辨率与编码每轮按当前设置更新到复用的客户端上。构建失败时向前端发射 `api-error` SSE
	/// 事件并返回 None。
	///
	/// Get the StripchatApi for this round (with proxy, mirror, Mouflon key, resolution and codec
	/// config).
	///
	/// The HTTP clients are reused across rounds (see `api_cache`) and only rebuilt when the
	/// proxy or mirror settings change; Mouflon keys, resolution and codec are applied to the
	/// reused clients from the current settings every round. Emits an `api-error` SSE event and
	/// returns None when building fails.
	fn build_api(&self, emitter: &Arc<dyn Emitter>) -> Option<StripchatApi> {
		let settings = self.state.get_settings();
		let net = ApiNetConfig::from_settings(&settings);
		let base = {
			let mut cache = self.api_cache.lock();
			match cache.as_ref() {
				Some((cached_net, api)) if *cached_net == net => api.clone(),
				_ => {
					let rebuilding = cache.is_some();
					match StripchatApi::new(
						net.api_proxy_url.as_deref(),
						net.cdn_proxy_url.as_deref(),
						net.sc_mirror_url.as_deref(),
						Some(net.sc_mirror_scheme.as_str()),
						self.recorder.cdn_tld_cache(),
						net.ech_enabled,
					) {
						Ok(api) => {
							if rebuilding {
								tracing::info!("{}", crate::tl!("monitor.apiClientRebuilt"));
							}
							*cache = Some((net, api.clone()));
							api
						}
						Err(e) => {
							tracing::error!("{}", crate::tl!("monitor.apiClientFailed", error = e));
							emitter.emit(
								"api-error",
								&serde_json::json!({ "message": e.to_string() }),
							);
							return None;
						}
					}
				}
			}
		};
		// 克隆的 StripchatApi 共享同一组 HTTP 客户端与连接池
		// A cloned StripchatApi shares the same HTTP clients and connection pools
		Some(
			base.with_mouflon_keys(self.state.get_mouflon_keys())
				.with_resolution_selection(
					settings.preferred_resolution,
					&settings.resolution_preference,
				)
				.with_codec_selection(&settings.preferred_codec),
		)
	}
}
