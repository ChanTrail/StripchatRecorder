//! 主播状态监控器 / Streamer Status Monitor
//!
//! 定期轮询所有追踪主播的直播状态，并在状态变化时：
//! - 向前端发送 `status-update` 事件
//! - 自动开始/停止录制（根据 auto_record 设置）
//!
//! Periodically polls the live status of all tracked streamers and on status changes:
//! - Emits `status-update` events to the frontend
//! - Automatically starts/stops recordings (based on auto_record settings)

use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use crate::recording::recorder::RecorderManager;
use crate::config::app_state::{AppState, Settings, StreamerData};
use crate::platform::stripchat::{StreamInfo, StripchatApi};
use crate::core::notifications::NotificationLevel;
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
}

impl ApiNetConfig {
    fn from_settings(s: &Settings) -> Self {
        Self {
            api_proxy_url: s.api_proxy_url.clone(),
            cdn_proxy_url: s.cdn_proxy_url.clone(),
            sc_mirror_url: s.sc_mirror_url.clone(),
            sc_mirror_scheme: s.sc_mirror_scheme.clone(),
        }
    }
}

// ─── CamGirlFinder schedule helpers ──────────────────────────────────────────

/// 从 camgirlfinder.net 获取指定 StripChat 主播的历史在线规律（schedule）。
///
/// 接口：`GET https://api.camgirlfinder.net/models/sc/{username}`
///
/// 返回的 `schedule` 字段是 7×48 的 float 矩阵（UTC 时区）：
/// - 第一维 0–6：星期（0 = 周日）
/// - 第二维 0–47：每天的 48 个 30 分钟时段
/// - 值域 [0.0, 1.0]：过去 28 天内该时段有在线记录的频率
///
/// 网络失败或用户在 CGF 不存在时静默返回 `None`，不影响正常录制流程。
///
/// Fetches the historical online schedule for a StripChat streamer from
/// camgirlfinder.net. Returns `None` on network failure or if the account
/// is not found in CGF — caller should treat this as "no schedule data".
pub async fn cgf_fetch_schedule(username: &str, proxy: Option<&str>) -> Option<Vec<Vec<f32>>> {
    // 用户名已经过 Stripchat API 验证，只含字母数字和下划线，无需额外 URL 编码
    // Username was already validated by Stripchat API and only contains
    // alphanumeric characters and underscores — no URL encoding needed.
    let url = format!("https://api.camgirlfinder.net/models/sc/{}", username);

    let mut builder = reqwest::Client::builder()
        .user_agent("StripchatRecorder/1.0")
        .timeout(std::time::Duration::from_secs(20));

    if let Some(proxy_url) = proxy.filter(|s| !s.is_empty()) {
        match reqwest::Proxy::all(proxy_url) {
            Ok(p) => { builder = builder.proxy(p); }
            Err(e) => {
                tracing::warn!("{}", crate::tl!("monitor.cgfProxyInvalid", error = e));
            }
        }
    }

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("{}", crate::tl!("monitor.cgfClientFailed", username = username, error = e));
            return None;
        }
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("{}", crate::tl!("monitor.cgfRequestFailed", username = username, error = e));
            return None;
        }
    };

    if !resp.status().is_success() {
        tracing::debug!("{}", crate::tl!("monitor.cgfHttpError", status = resp.status().as_u16(), username = username));
        return None;
    }

    // 只取 schedule 字段，避免反序列化整个响应体
    // Only extract the schedule field to avoid deserializing the full response body
    #[derive(serde::Deserialize)]
    struct CgfProfile {
        schedule: Option<Vec<Vec<f32>>>,
    }

    let profile: CgfProfile = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("{}", crate::tl!("monitor.cgfParseFailed", username = username, error = e));
            return None;
        }
    };

    let raw = match profile.schedule {
        Some(s) => s,
        None => {
            tracing::debug!("{}", crate::tl!("monitor.cgfScheduleNull", username = username));
            return None;
        }
    };

    // 必须恰好 7 行（7 天）；每行截断或补零到 48 个时段以容错边界情况
    // Must be exactly 7 rows (days); each row is truncated or zero-padded to 48 slots
    if raw.len() != 7 {
        return None;
    }
    let normalized: Vec<Vec<f32>> = raw
        .into_iter()
        .map(|mut row| {
            row.resize(48, 0.0); // 不足48补零；超出48截断
            row.truncate(48);
            row
        })
        .collect();
    Some(normalized)
}

/// 根据 schedule 矩阵和当前本地时间，计算该主播本轮使用的自适应轮询间隔。
///
/// ## 算法
///
/// 取当前本地时间的星期 + 30 分钟时段对应的活跃度值 `a ∈ [0.0, 1.0]`：
///
/// - `a ≥ 0.8`：活跃度足够高，直接使用用户配置的 `base_secs`，不做节流。
/// - `a ∈ [0.0, 0.8)`：通过平方根曲线将活跃度映射到 `[max_secs, base_secs]`：
///
/// ```text
/// interval = max_secs - (max_secs - base_secs) × √(a / 0.8)
/// ```
///
/// 平方根曲线使低活跃段间隔更激进地拉长，靠近 0.8 时平滑收敛到 `base_secs`。
///
/// | 活跃度 | 间隔（base=60s, max=200s）|
/// |--------|--------------------------|
/// | 0.00   | 200 s                    |
/// | 0.05   | 165 s                    |
/// | 0.20   | 130 s                    |
/// | 0.40   | 101 s                    |
/// | 0.60   |  79 s                    |
/// | 0.79   |  61 s                    |
/// | ≥ 0.80 | base_secs（用户设置）     |
///
/// 无 schedule 数据时返回 `base_secs`（不节流）。
///
/// ## Parameters
/// - `schedule` – 7×48 活跃度矩阵，`None` 表示尚未获取。
/// - `base_secs` – 用户配置的轮询间隔（秒）。
/// - `max_secs`  – 低活跃时段允许的最大间隔（秒），调用方固定传入 200（`SCHEDULE_MAX_SECS`）。
///
/// Computes the adaptive poll interval for a streamer based on its schedule matrix
/// and the current local time (weekday + 30-minute bucket). `max_secs` is the longest
/// interval allowed in low-activity buckets; the caller always passes 200
/// (`SCHEDULE_MAX_SECS`). See inline comments for the formula.
pub fn schedule_poll_interval(
    schedule: Option<&[Vec<f32>]>,
    base_secs: u64,
    max_secs: u64,
) -> u64 {
    use chrono::{Datelike as _, Timelike as _};

    let sched = match schedule {
        Some(s) => s,
        None => return base_secs, // 无数据：不节流
    };

    let now = chrono::Local::now();
    // chrono weekday: Mon=0…Sun=6;  CGF convention: Sun=0…Sat=6
    let cgf_day = now.weekday().num_days_from_sunday() as usize;
    let bucket = (now.hour() * 2 + now.minute() / 30) as usize;
    let activity = sched.get(cgf_day).and_then(|row| row.get(bucket)).copied().unwrap_or(1.0);

    // 活跃度 ≥ 0.8 直接用用户设置值，不节流
    if activity >= 0.8 {
        return base_secs;
    }

    // 平方根曲线映射：√(a / 0.8) ∈ [0, 1)
    let t = (activity / 0.8).sqrt();
    // interval = max - (max - base) × t，随 t 增大从 max 收敛到 base
    let interval = max_secs as f64 - (max_secs as f64 - base_secs as f64) * t as f64;
    interval.round() as u64
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
        Self { date: today, attempts, done: success }
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
        let prefers_higher = settings.resolution_preference == "higher";
        self.statuses
            .read()
            .get(username)
            .filter(|s| {
                s.playlist_resolution == settings.preferred_resolution
                    && s.playlist_prefers_higher == prefers_higher
            })
            .and_then(|s| s.playlist_url.clone())
    }

    /// 内部版本：直接接受已创建的 restart_rx（供 server 模式使用）。
    /// Internal version: accepts a pre-created restart_rx (used by server mode).
    pub async fn start_with_emitter_inner(self: Arc<Self>, emitter: Arc<dyn Emitter>, restart_rx: mpsc::Receiver<()>) {
        self.monitor_loop(emitter, restart_rx).await;
    }

    /// 取得本轮使用的 StripchatApi（含代理、镜像站、Mouflon 密钥、分辨率配置）。
    ///
    /// HTTP 客户端跨轮复用（见 `api_cache`），只在代理或镜像站设置变化时重建；Mouflon 密钥与
    /// 分辨率每轮按当前设置更新到复用的客户端上。构建失败时向前端发射 `api-error` SSE 事件并
    /// 返回 None。
    ///
    /// Get the StripchatApi for this round (with proxy, mirror, Mouflon key and resolution
    /// config).
    ///
    /// The HTTP clients are reused across rounds (see `api_cache`) and only rebuilt when the
    /// proxy or mirror settings change; Mouflon keys and resolution are applied to the reused
    /// clients from the current settings every round. Emits an `api-error` SSE event and returns
    /// None when building fails.
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
                            emitter.emit("api-error", &serde_json::json!({ "message": e.to_string() }));
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
                .with_resolution_selection(settings.preferred_resolution, &settings.resolution_preference),
        )
    }

    /// 查询主播直播状态；遇到网络类错误（见 [`is_retryable_network_error`]）时在本轮内按
    /// [`POLL_NETWORK_RETRY_DELAYS`] 短间隔重试，其他错误直接返回。
    ///
    /// Query a streamer's live status; on a network error (see [`is_retryable_network_error`])
    /// retry within this round after the short [`POLL_NETWORK_RETRY_DELAYS`]; other errors are
    /// returned as is.
    ///
    /// 不在这里顺带请求离线预览图（`fetch_offline_preview` 固定为 false）：它只在主播离线时
    /// 才会请求，而离线预览图不论在线与否都要刷新，由 `poll_streamer` 单独请求。
    /// The offline preview isn't requested here (`fetch_offline_preview` is always false): that
    /// path only fetches it while offline, but the offline preview must be refreshed regardless of
    /// online status, so `poll_streamer` requests it separately.
    async fn get_stream_info_with_retry(
        api: &StripchatApi,
        username: &str,
        fetch_playlist: bool,
        model_id: Option<i64>,
    ) -> Result<StreamInfo> {
        let mut attempt = 0usize;
        loop {
            match api
                .get_stream_info(username, fetch_playlist, model_id, false)
                .await
            {
                Err(e) if attempt < POLL_NETWORK_RETRY_DELAYS.len() && is_retryable_network_error(&e) => {
                    let delay = POLL_NETWORK_RETRY_DELAYS[attempt];
                    attempt += 1;
                    tracing::info!(
                        "{}",
                        crate::tl!(
                            "monitor.pollRetrying",
                            username = username,
                            attempt = attempt,
                            max = POLL_NETWORK_RETRY_DELAYS.len(),
                            secs = delay.as_secs(),
                            error = e
                        )
                    );
                    tokio::time::sleep(delay).await;
                }
                result => return result,
            }
        }
    }

    /// 监控主循环：延迟 10 秒后轮询一次，然后按配置的间隔周期性轮询。
    /// Monitor main loop: wait 10 s then poll once, then poll periodically at the configured interval.
    async fn monitor_loop(
        self: Arc<Self>,
        emitter: Arc<dyn Emitter>,
        mut restart_rx: mpsc::Receiver<()>,
    ) {
        // 延迟 10 秒等网络稳定后再发起首次轮询，避免启动瞬间网络不稳导致报错
        // Wait 10 s for network to stabilize before the first poll, avoiding errors on startup
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        self.poll_all_with_emitter(&emitter).await;

        loop {
            let poll_interval =
                tokio::time::Duration::from_secs(self.state.get_settings().poll_interval_secs);

            tokio::select! {
                _ = restart_rx.recv() => {
                    // poll_interval_secs 已变更，立即以新间隔重新开始计时（不立即轮询）
                    // poll_interval_secs changed; restart timer with new interval (no immediate poll)
                    tracing::info!("{}", crate::tl!("monitor.pollIntervalRestarted"));
                    continue;
                }
                _ = tokio::time::sleep(poll_interval) => {
                    self.poll_all_with_emitter(&emitter).await;
                }
            }
        }
    }

    /// 并发轮询所有追踪主播的状态（通用版本）。
    /// Concurrently poll the status of all tracked streamers (generic version).
    pub async fn poll_all_with_emitter(self: &Arc<Self>, emitter: &Arc<dyn Emitter>) {
        let settings = self.state.get_settings();
        let streamers = self.state.get_streamers();

        if streamers.is_empty() {
            return;
        }

        let api = match self.build_api(emitter) {
            Some(a) => Arc::new(a),
            None => return,
        };

        // schedule 自适应间隔的上限（固定 200 s），下限为用户设置的 base
        // Upper bound for schedule-adaptive interval (fixed 200 s); lower bound = user base
        const SCHEDULE_MAX_SECS: u64 = 200;
        let base_secs = settings.poll_interval_secs;
        let now = std::time::Instant::now();

        // 限制同时进行中的 API 请求数，避免主播过多时同时打出大量请求触发限流。
        // 5 路并发足以在正常延迟下及时完成一轮轮询，同时对 SC 服务器友好。
        //
        // Limit concurrent in-flight API requests to prevent rate-limiting when
        // there are many tracked streamers. 5 concurrent requests is enough to
        // finish a round quickly under normal latency while remaining polite to SC.
        const POLL_CONCURRENCY: usize = 5;
        let sem = Arc::new(tokio::sync::Semaphore::new(POLL_CONCURRENCY));

        let active_streamers: Vec<_> = streamers
            .into_iter()
            // 过滤已确认失效的主播（永久跳过）
            // Filter permanently-dead streamers
            .filter(|s| !self.dead_streamers.read().contains(&s.username))
            // Schedule 自适应间隔检查：
            // 若距上次轮询尚未到本次应有的间隔，且主播不在录制中，则跳过本轮。
            // 录制中的主播始终参与轮询（检测断流）。
            // 首次轮询（next_poll_at 中无记录）无条件执行。
            //
            // Schedule-adaptive interval check:
            // Skip if the per-streamer interval has not elapsed yet and the
            // streamer is not currently recording. Recording streamers always
            // participate (to detect stream drops). First-ever poll (no entry
            // in next_poll_at) always executes.
            .filter(|s| {
                if self.recorder.is_recording(&s.username) {
                    return true;
                }
                match self.next_poll_at.read().get(&s.username) {
                    Some(&next) => now >= next,
                    None => true, // 从未轮询过，立即执行
                }
            })
            .collect();

        // 每个任务直接返回本轮新确认失效的主播名，等待任务结束时收集。
        // 之前用容量 16 的 channel 收集、且要等所有任务结束后才读取：同一轮新失效的主播
        // 超过 16 个时，第 17 个任务会一直阻塞在 send 上，本轮永远结束不了，轮询整体停摆。
        // 改为从 JoinHandle 取返回值，没有容量上限，也不会互相等待。
        //
        // Each task returns the username newly confirmed dead this round, collected when the
        // task is awaited. Previously a capacity-16 channel was used and only read after all
        // tasks finished: with more than 16 newly-dead streamers in one round, the 17th task
        // blocked on send forever, the round never finished and polling stopped entirely.
        // Taking the value from the JoinHandle has no capacity limit and no mutual waiting.
        let tasks: Vec<_> = active_streamers
            .into_iter()
            .map(|streamer| {
                let api = Arc::clone(&api);
                let monitor = Arc::clone(self);
                let emitter = Arc::clone(emitter);
                let auto_record_global = settings.auto_record;
                let sem = Arc::clone(&sem);

                tokio::spawn(async move {
                    // 在发起 API 请求前获取信号量许可，限制并发数
                    // Acquire a semaphore permit before making the API request to cap concurrency
                    let _permit = sem.acquire().await;
                    monitor
                        .poll_streamer(&api, streamer, &emitter, auto_record_global, base_secs, SCHEDULE_MAX_SECS)
                        .await
                })
            })
            .collect();

        // 收集本轮所有新死亡主播，合并成一条通知
        // Collect all newly-dead streamers and emit a single merged notification
        let mut newly_dead: Vec<String> = Vec::new();
        for t in tasks {
            if let Ok(Some(username)) = t.await {
                newly_dead.push(username);
            }
        }

        if !newly_dead.is_empty() {
            newly_dead.sort();
            use std::collections::HashMap;
            let (message, key, args) = if newly_dead.len() == 1 {
                let mut a = HashMap::new();
                a.insert("username".to_string(), serde_json::json!(&newly_dead[0]));
                (
                    format!(
                        "Streamer {} cannot be found by username or internal ID (possibly renamed, deleted, or banned). Future polls will be skipped.",
                        newly_dead[0]
                    ),
                    "notifications.backend.streamerDeadOne",
                    a,
                )
            } else {
                let usernames = newly_dead.join(", ");
                let mut a = HashMap::new();
                a.insert("count".to_string(), serde_json::json!(newly_dead.len()));
                a.insert("usernames".to_string(), serde_json::json!(usernames));
                (
                    format!(
                        "{} streamers cannot be found (possibly renamed, deleted, or banned). Future polls will be skipped: {}",
                        newly_dead.len(),
                        newly_dead.join(", ")
                    ),
                    "notifications.backend.streamerDeadMany",
                    a,
                )
            };
            self.state.notification_store.emit_i18n_with_action(
                emitter,
                NotificationLevel::Warning,
                "streamer_dead",
                message,
                key,
                Some(args),
                Some(crate::core::notifications::NotificationAction {
                    action_type: "remove_streamers".to_string(),
                    targets: newly_dead,
                }),
            );
        }
    }

    /// 轮询单个主播的状态，更新缓存，并根据状态变化触发自动录制逻辑。
    /// 轮询完成后根据 schedule 计算下次可轮询时刻并写入 `next_poll_at`。
    /// 若该主播本轮被确认失效（首次），返回其用户名；否则返回 None。
    ///
    /// Poll a single streamer's status, update the cache, and trigger auto-recording logic.
    /// After polling, computes the next eligible poll time from the schedule and stores it
    /// in `next_poll_at`. Returns the username if newly confirmed dead; otherwise None.
    async fn poll_streamer(
        self: &Arc<Self>,
        api: &StripchatApi,
        streamer: StreamerData,
        emitter: &Arc<dyn Emitter>,
        auto_record_global: bool,
        base_secs: u64,
        max_secs: u64,
    ) -> Option<String> {
        let mut username = streamer.username.clone();

        let is_recording = self.recorder.is_recording(&username);
        let (was_online, was_recording) = self
            .statuses
            .read()
            .get(&username)
            .map(|s| (s.is_online, s.is_recording))
            .unwrap_or((false, false));

        if !self.statuses.read().contains_key(&username) {
            self.statuses
                .write()
                .entry(username.clone())
                .or_insert_with(|| StreamerStatus {
                    username: username.clone(),
                    is_online: false,
                    is_recording,
                    is_recordable: false,
                    status: String::new(),
                    thumbnail_url: None,
                    playlist_url: None,
                    playlist_resolution: 0,
                    playlist_prefers_higher: false,
                });
        }

        // 仅当确实可能触发自动录制时才拉取 playlist URL：
        // 未在录制 + 该主播启用了自动录制 + 全局自动录制也开启。
        // 手动录制按钮不依赖此处的缓存 URL，点击时会即时拉取。
        //
        // Only fetch the playlist URL when auto-recording could actually trigger:
        // not currently recording + this streamer has auto-record on + global auto-record is on.
        // The manual-record button does not rely on this cached URL; it fetches on demand.
        let need_playlist = !is_recording && streamer.auto_record && auto_record_global;
        let info = match Self::get_stream_info_with_retry(
            api,
            &username,
            need_playlist,
            streamer.model_id,
        )
        .await
        {
            Ok(i) => i,
            Err(crate::core::error::AppError::UserNotFound(_)) => {
                // 用户名查不到，且 model_id 反查也失败（get_stream_info 已处理改名回退）
                // Username not found and model_id reverse-lookup also failed
                // (get_stream_info already handles rename fallback).
                let already_dead = self.dead_streamers.read().contains(&username);
                if !already_dead {
                    // 写入内存 dead set + 持久化到 streamers.json
                    // Add to in-memory dead set + persist to streamers.json
                    self.dead_streamers.write().insert(username.clone());
                    self.state.mark_streamer_dead(&username);
                    tracing::warn!("{}", crate::tl!("monitor.streamerDead", username = username)
                    );
                    // 返回用户名，由 poll_all_with_emitter 统一合并通知
                    // Return username so poll_all_with_emitter can merge notifications
                    return Some(username);
                }
                return None;
            }
            Err(e) => {
                tracing::error!("{}", crate::tl!("monitor.pollFailed", username = username, error = e));
                // 网络/API 错误时仍以 base_secs 重试，不拉长间隔
                // On network/API error, retry at base_secs — don't extend the interval
                let next = std::time::Instant::now()
                    + std::time::Duration::from_secs(base_secs);
                self.next_poll_at.write().insert(username.clone(), next);
                return None;
            }
        };

        // 首次成功查询且此前尚无 model_id（升级前的旧数据）时，回填 model_id，
        // 便于日后改名反查有据可依。
        // On first successful lookup with no model_id yet (pre-upgrade data), backfill
        // it so future rename lookups have something to fall back on.
        if streamer.model_id.is_none()
            && let Some(mid) = info.model_id
        {
            self.state.backfill_model_id(&username, mid);
        }

        if let Some(ref new_username) = info.renamed_to
            && !is_recording
        {
            match self.state.rename_streamer(&username, new_username) {
                Ok(()) => {
                    tracing::info!("{}", crate::tl!("monitor.streamerRenamed", oldUsername = username, newUsername = new_username));
                    // 重新绑定 statuses 缓存的 key，避免旧 key 下的缓存永久残留。
                    //
                    // 注意：必须先将 remove 结果存到局部变量再做 insert，不能把两个
                    // write() 调用写在同一个 `if let` 的条件和 body 里——条件表达式
                    // 产生的临时值（这里是第一个 write() 的锁守卫）生命周期会延续到
                    // 整个 if let 语句结束，包括 body，届时 body 里的第二个 write()
                    // 会试图重新获取同一把已持有的锁，导致自死锁。
                    //
                    // Must store the `remove` result in a local first, then `insert` —
                    // can't have both `write()` calls inside the same `if let`'s
                    // condition and body: the temporary produced in the condition
                    // (the first `write()`'s lock guard) lives through the entire
                    // `if let` including the body, causing a self-deadlock when the
                    // body tries to acquire the same lock again.
                    let old_status = self.statuses.write().remove(&username);
                    if let Some(old_status) = old_status {
                        self.statuses.write().insert(new_username.clone(), old_status);
                    }
                    emitter.emit(
                        "streamer-renamed",
                        &serde_json::json!({ "old_username": username, "new_username": new_username }),
                    );
                    username = new_username.clone();
                }
                Err(e) => {
                    // 新用户名已存在于追踪列表中，放弃改名，本轮按旧用户名继续处理。
                    // New username already tracked; abandon rename and keep old username for this round.
                    tracing::warn!("{}", crate::tl!("monitor.streamerRenameSkipped", username = username, error = e));
                }
            }
        }

        // 离线预览图随轮询刷新，与在线状态无关：在线时前端展示实时截图，但离线预览图作为
        // 兜底图片也要保持最新（与原来的单独刷新任务一致）。该主播今天（本地日期）还没取到时
        // 请求一次 cam 接口，取到后写入缓存（未变化时不写盘）；取不到时保留原有缓存，下一轮
        // 再试，每天最多 PREVIEW_MAX_ATTEMPTS_PER_DAY 次。放在构建状态之前，离线时本轮就能用上
        // 新地址。仍持有轮询并发名额，不会突破 5 路并发上限。
        //
        // Refresh the offline preview as part of polling, regardless of online status: while
        // online the frontend shows the live snapshot, but the offline preview as the fallback
        // image must stay current too (same as the former separate refresh task). When this
        // streamer has no URL yet today (local date), request the cam endpoint once and cache the
        // result (nothing is written to disk when unchanged); on failure keep the existing cache
        // and retry next round, at most PREVIEW_MAX_ATTEMPTS_PER_DAY times a day. Done before
        // building the status so an offline streamer uses the new URL this round. Still holds the
        // poll concurrency slot, so the 5-way cap is respected.
        let today = chrono::Local::now().date_naive();
        let mut fresh_preview: Option<String> = None;
        if let Some(model_id) = info.model_id.or(streamer.model_id)
            && PreviewRefresh::needed(self.preview_refresh.read().get(&username), today)
        {
            fresh_preview = api.get_cam_preview_url(&username, model_id).await;
            let progress = self.preview_refresh.read().get(&username).copied();
            let progress = PreviewRefresh::record(progress, today, fresh_preview.is_some());
            self.preview_refresh.write().insert(username.clone(), progress);
            match &fresh_preview {
                Some(url) => {
                    tracing::debug!(
                        "{}",
                        crate::tl!("monitor.previewRefreshed", username = username, url = url)
                    );
                    self.state.set_cached_preview_url(&username, Some(url.clone()));
                }
                None => tracing::debug!(
                    "{}",
                    crate::tl!(
                        "monitor.previewRefreshFailed",
                        username = username,
                        attempt = progress.attempts,
                        max = PREVIEW_MAX_ATTEMPTS_PER_DAY
                    )
                ),
            }
        }

        let status = StreamerStatus {
            username: username.clone(),
            is_online: info.is_online,
            is_recording,
            // 直接用 API 返回的 is_recordable（is_live && public），不依赖是否拉取了 playlist URL。
            // 录制中时保留缓存值（正常情况下此时 API 仍返回 true；保留缓存仅作额外防护，
            // 避免录制过程中状态短暂抖动导致按钮被错误禁用）。
            //
            // Use is_recordable directly from the API response (is_live && public),
            // independent of whether a playlist URL was fetched.
            // While recording, preserve the cached value as an extra guard against
            // transient status flicker that could incorrectly disable the button.
            is_recordable: if is_recording {
                self.statuses
                    .read()
                    .get(&username)
                    .map(|s| s.is_recordable)
                    .unwrap_or(info.is_recordable)
            } else {
                info.is_recordable
            },
            status: info.status.clone(),
            // 离线时 API 不返回缩略图，用本轮刚刷新的离线预览图，没有时用 StreamerData 中
            // 持久化的缓存值兜底，保证前端始终有预览图可显示。
            // 在线时直接用 API 返回的直播缩略图，不使用缓存。
            //
            // When offline the API returns no thumbnail; use the offline preview refreshed this
            // round, falling back to the cached value from StreamerData, so the frontend always
            // has a preview. When online, use the live thumbnail from the API directly.
            thumbnail_url: if info.is_online {
                info.thumbnail_url.clone()
            } else {
                info.thumbnail_url
                    .clone()
                    .or_else(|| fresh_preview.clone())
                    .or_else(|| streamer.cached_preview_url.clone())
            },
            playlist_url: info.playlist_url.clone(),
            playlist_resolution: api.preferred_resolution(),
            playlist_prefers_higher: api.prefers_higher_resolution(),
        };

        emitter.emit("status-update", &status);

        self.statuses.write().insert(username.clone(), status);

        let stream_no_longer_recordable = is_recording && !info.is_recordable;
        if stream_no_longer_recordable {
            tracing::info!("{}", crate::tl!("monitor.streamNotRecordable", username = username, isOnline = info.is_online, isRecordable = info.is_recordable, status = info.status)
            );
            let _ = self.recorder.stop_recording_auto(&username).await;
        }

        let recording_dropped = was_recording && !is_recording && info.is_online;
        let just_came_online = info.is_online && !was_online;
        let naturally_stopped = self.recorder.naturally_stopped.write().remove(&username);
        let should_be_recording =
            info.is_recordable && !is_recording && streamer.auto_record && auto_record_global;
        if (just_came_online || recording_dropped || naturally_stopped || should_be_recording)
            && streamer.auto_record
            && auto_record_global
            && !is_recording
            && let Some(ref playlist_url) = info.playlist_url
            && {
                let settings = self.state.get_settings();
                let prefers_higher = settings.resolution_preference == "higher";
                // 分辨率设置已通过 build_api 传入 api，此处校验缓存 playlist 是否仍与当前设置匹配
                // Resolution was passed into api via build_api; verify the cached playlist still matches current settings
                self.statuses.read().get(&username).is_none_or(|s| {
                    s.playlist_resolution == settings.preferred_resolution
                        && s.playlist_prefers_higher == prefers_higher
                })
            }
        {
            tracing::info!("{}", crate::tl!("monitor.autoStart", username = username, justOnline = just_came_online, dropped = recording_dropped, naturalStop = naturally_stopped, shouldBe = should_be_recording)
            );
            let _ = self
                .recorder
                .start_recording_with_emitter(&username, playlist_url, Arc::clone(emitter))
                .await;
        }

        // 计算下次应轮询该主播的最早时刻，写入 next_poll_at。
        // 若当前处于录制中，始终用 base_secs（不拉长间隔，保持对断流的快速响应）。
        // 否则由 schedule 活跃度决定间隔（0%→200 s, 80%→base, >80%→base）。
        //
        // Compute and store the earliest next-poll time for this streamer.
        // Use base_secs when recording (fast stream-drop detection).
        // Otherwise, derive the interval from the schedule activity.
        {
            let interval_secs = if self.recorder.is_recording(&username) {
                base_secs
            } else {
                schedule_poll_interval(
                    streamer.schedule.as_deref(),
                    base_secs,
                    max_secs,
                )
            };
            let next = std::time::Instant::now()
                + std::time::Duration::from_secs(interval_secs);
            self.next_poll_at.write().insert(username.clone(), next);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 离线预览图刷新：成功后当天不再请求；失败后下一轮再试，每天最多 3 次；跨天重新开始。
    /// Offline preview refresh: no more requests that day after a success; retried after a
    /// failure, at most 3 times a day; starts over on a new day.
    #[test]
    fn preview_refresh_attempts_per_day() {
        let day1 = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).expect("date");
        let day2 = day1.succ_opt().expect("next day");
        assert!(PreviewRefresh::needed(None, day1));

        let ok = PreviewRefresh::record(None, day1, true);
        assert!(!PreviewRefresh::needed(Some(&ok), day1));
        assert!(PreviewRefresh::needed(Some(&ok), day2));

        let mut p = PreviewRefresh::record(None, day1, false);
        for _ in 1..PREVIEW_MAX_ATTEMPTS_PER_DAY {
            assert!(PreviewRefresh::needed(Some(&p), day1));
            p = PreviewRefresh::record(Some(p), day1, false);
        }
        assert_eq!(p.attempts, PREVIEW_MAX_ATTEMPTS_PER_DAY);
        assert!(!PreviewRefresh::needed(Some(&p), day1));
        assert!(PreviewRefresh::needed(Some(&p), day2));
        assert_eq!(PreviewRefresh::record(Some(p), day2, false).attempts, 1);
    }

    /// HTTP 状态错误（如 403）与"主播不存在"不重试。
    /// HTTP status errors (e.g. 403) and "streamer not found" are not retried.
    #[test]
    fn status_and_not_found_errors_are_not_retried() {
        assert!(!is_retryable_network_error(&AppError::Other("API return 403 (id=1)".into())));
        assert!(!is_retryable_network_error(&AppError::UserNotFound("x".into())));
    }

    /// 超时不重试；连接建立后被对端关闭（与代理握手失败同类的发送失败）重试。
    /// Timeouts are not retried; a connection closed by the peer (a send failure of the same
    /// kind as a failed proxy handshake) is retried.
    #[tokio::test]
    async fn timeouts_are_not_retried_but_dropped_connections_are() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .no_proxy()
            .build()
            .expect("client");

        // 接受连接但从不响应 → 超时 / Accepts connections but never responds → timeout
        let silent = std::net::TcpListener::bind("127.0.0.1:0").expect("bind silent");
        let err = client
            .get(format!("http://{}/", silent.local_addr().expect("addr")))
            .send()
            .await
            .expect_err("should time out");
        assert!(err.is_timeout());
        assert!(!is_retryable_network_error(&AppError::Reqwest(err)));

        // 接受连接后立即关闭 → 发送请求失败 / Closes right after accepting → sending fails
        let closing = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind closing");
        let addr = closing.local_addr().expect("addr");
        tokio::spawn(async move {
            while let Ok((stream, _)) = closing.accept().await {
                drop(stream);
            }
        });
        let err = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("should fail to send");
        assert!(is_retryable_network_error(&AppError::Reqwest(err)));
    }

    /// 网络错误在同一轮内共尝试 1 + 2 次后才返回错误（每次都新建连接）。
    /// A network error is attempted 1 + 2 times within the same round before the error is
    /// returned (each attempt opens a new connection).
    #[tokio::test]
    async fn network_errors_are_retried_within_the_round() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let accepted = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&accepted);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });

        // 镜像站指向本地监听：stripchat.com 替换为 127.0.0.1:port，协议改为 http
        // Point the mirror at the local listener: stripchat.com → 127.0.0.1:port over http
        let api = StripchatApi::new_api_only(None, None, Some(&addr.to_string()), Some("http"))
            .expect("api");
        let result = StatusMonitor::get_stream_info_with_retry(&api, "x", false, Some(1)).await;
        assert!(result.is_err());
        assert_eq!(accepted.load(Ordering::SeqCst), 1 + POLL_NETWORK_RETRY_DELAYS.len());
    }
}
