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
        self.statuses
            .read()
            .get(username)
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
            Ok(a) => Some(a.with_mouflon_keys(self.state.get_mouflon_keys())),
            Err(e) => {
                tracing::error!("Failed to create API client: {}", e);
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
                    tracing::info!("Monitor: poll interval changed, restarting timer");
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

        // 用 channel 收集本轮新发现的死亡主播名，最后合并成一条通知
        // Use a channel to collect newly-dead streamers from this round, then merge into one notification
        let (dead_tx, mut dead_rx) = tokio::sync::mpsc::channel::<String>(16);

        let tasks: Vec<_> = streamers
            .into_iter()
            .filter(|s| !self.dead_streamers.read().contains(&s.username))
            .map(|streamer| {
                let api = Arc::clone(&api);
                let monitor = Arc::clone(self);
                let emitter = Arc::clone(emitter);
                let auto_record_global = settings.auto_record;
                let dead_tx = dead_tx.clone();

                tokio::spawn(async move {
                    let newly_dead = monitor
                        .poll_streamer(&api, streamer, &emitter, auto_record_global)
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
    /// 若该主播本轮被确认失效（首次），返回其用户名；否则返回 None。
    ///
    /// Poll a single streamer's status, update the cache, and trigger auto-recording logic.
    /// Returns the username if the streamer was newly confirmed dead this round; otherwise None.
    async fn poll_streamer(
        self: &Arc<Self>,
        api: &StripchatApi,
        streamer: StreamerData,
        emitter: &Arc<dyn Emitter>,
        auto_record_global: bool,
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
                });
        }

        let info = match api.get_stream_info(&username, !is_recording, streamer.model_id).await {
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
                    tracing::warn!(
                        "Streamer {} confirmed dead (not found by username or model_id), skipping future polls",
                        username
                    );
                    // 返回用户名，由 poll_all_with_emitter 统一合并通知
                    // Return username so poll_all_with_emitter can merge notifications
                    return Some(username);
                }
                return None;
            }
            Err(e) => {
                tracing::error!("Poll failed → {}: {}", username, e);
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
                    tracing::info!("Streamer renamed: {} -> {}", username, new_username);
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
                    tracing::warn!("Streamer rename skipped for {}: {}", username, e);
                }
            }
        }

        let status = StreamerStatus {
            username: username.clone(),
            is_online: info.is_online,
            is_recording,
            // 正在录制时不获取 playlist_url，保留上次缓存的 is_recordable 值，避免按钮被错误禁用
            // When recording, playlist_url is not fetched; preserve the last cached is_recordable
            // to avoid incorrectly disabling buttons
            is_recordable: if is_recording {
                self.statuses
                    .read()
                    .get(&username)
                    .map(|s| s.is_recordable)
                    .unwrap_or(info.playlist_url.is_some())
            } else {
                info.playlist_url.is_some()
            },
            status: info.status.clone(),
            thumbnail_url: info.thumbnail_url.clone(),
            playlist_url: info.playlist_url.clone(),
        };

        emitter.emit("status-update", &status);

        self.statuses.write().insert(username.clone(), status);

        let stream_no_longer_recordable = is_recording && !info.is_recordable;
        if stream_no_longer_recordable {
            tracing::info!(
                "Stream no longer recordable → {} (is_online={}, is_recordable={}, status={}), stopping recording",
                username, info.is_online, info.is_recordable, info.status
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
        {
            tracing::info!("Auto-starting recording → {} (just_online={}, dropped={}, natural_stop={}, should_be={})", username, just_came_online, recording_dropped, naturally_stopped, should_be_recording);
            let _ = self
                .recorder
                .start_recording_with_emitter(&username, playlist_url, Arc::clone(emitter))
                .await;
        }
        None
    }
}
