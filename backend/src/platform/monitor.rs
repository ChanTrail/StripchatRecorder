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
use crate::recording::recorder::RecorderManager;
use crate::config::app_state::{AppState, StreamerData};
use crate::platform::stripchat::StripchatApi;
use crate::core::notifications::NotificationLevel;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::mpsc;

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

/// 根据 schedule 矩阵和当前 UTC 时间，计算该主播本轮使用的自适应轮询间隔。
///
/// ## 算法
///
/// 取当前 UTC 星期 + 30 分钟时段对应的活跃度值 `a ∈ [0.0, 1.0]`：
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
/// | 活跃度 | 间隔（base=60s, max=150s）|
/// |--------|--------------------------|
/// | 0.00   | 150 s                    |
/// | 0.05   | 120 s                    |
/// | 0.20   | 100 s                    |
/// | 0.40   |  80 s                    |
/// | 0.60   |  67 s                    |
/// | 0.79   |  61 s                    |
/// | ≥ 0.80 | base_secs（用户设置）     |
///
/// 无 schedule 数据时返回 `base_secs`（不节流）。
///
/// ## Parameters
/// - `schedule` – 7×48 活跃度矩阵，`None` 表示尚未获取。
/// - `base_secs` – 用户配置的轮询间隔（秒）。
/// - `max_secs`  – 低活跃时段允许的最大间隔（秒），固定为 150。
///
/// Computes the adaptive poll interval for a streamer based on its schedule matrix
/// and the current UTC time. See inline comments for the formula.
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

    let now = chrono::Utc::now();
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

    /// 构建 StripchatApi 实例（含代理、镜像站、Mouflon 密钥配置）。
    /// 出错时向前端发射 `api-error` SSE 事件并返回 Err。
    ///
    /// Build a StripchatApi instance (with proxy, mirror, Mouflon key config).
    /// Emits `api-error` SSE event on failure and returns Err.
    fn build_api(&self, emitter: &Arc<dyn Emitter>) -> Option<StripchatApi> {
        let settings = self.state.get_settings();
        match StripchatApi::new(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
            self.recorder.cdn_tld_cache(),
        ) {
            Ok(a) => Some(
                a.with_mouflon_keys(self.state.get_mouflon_keys())
                    .with_resolution_selection(
                        settings.preferred_resolution,
                        &settings.resolution_preference,
                    ),
            ),
            Err(e) => {
                tracing::error!("{}", crate::tl!("monitor.apiClientFailed", error = e));
                emitter.emit("api-error", &serde_json::json!({ "message": e.to_string() }));
                None
            }
        }
    }

    /// 监控主循环：立即轮询一次，然后按配置的间隔周期性轮询。
    /// Monitor main loop: poll once immediately, then poll periodically at the configured interval.
    async fn monitor_loop(
        self: Arc<Self>,
        emitter: Arc<dyn Emitter>,
        mut restart_rx: mpsc::Receiver<()>,
    ) {
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

        // 用 channel 收集本轮新发现的死亡主播名，最后合并成一条通知
        // Use a channel to collect newly-dead streamers from this round, then merge into one notification
        let (dead_tx, mut dead_rx) = tokio::sync::mpsc::channel::<String>(16);

        let tasks: Vec<_> = streamers
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
            .map(|streamer| {
                let api = Arc::clone(&api);
                let monitor = Arc::clone(self);
                let emitter = Arc::clone(emitter);
                let auto_record_global = settings.auto_record;
                let dead_tx = dead_tx.clone();

                tokio::spawn(async move {
                    let newly_dead = monitor
                        .poll_streamer(&api, streamer, &emitter, auto_record_global, base_secs, SCHEDULE_MAX_SECS)
                        .await;
                    if let Some(username) = newly_dead {
                        let _ = dead_tx.send(username).await;
                    }
                })
            })
            .collect();

        // 先 drop 发送端，确保 recv 能感知到所有发送端都关闭
        // Drop the producer side so the receiver can detect all senders are gone
        drop(dead_tx);

        for t in tasks {
            let _ = t.await;
        }

        // 收集本轮所有新死亡主播，合并成一条通知
        // Collect all newly-dead streamers and emit a single merged notification
        let mut newly_dead: Vec<String> = Vec::new();
        while let Ok(username) = dead_rx.try_recv() {
            newly_dead.push(username);
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
        let info = match api.get_stream_info(&username, need_playlist, streamer.model_id).await {
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
            thumbnail_url: info.thumbnail_url.clone(),
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
        // 否则由 schedule 活跃度决定间隔（0%→150 s, 80%→base, >80%→base）。
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
