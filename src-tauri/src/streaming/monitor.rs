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
use crate::config::settings::{AppState, StreamerData};
use crate::streaming::stripchat::StripchatApi;
use parking_lot::RwLock;
use std::collections::HashMap;
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
    pub viewers: i64,
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
    /// 停止轮询循环的发送端 / Sender to stop the polling loop
    stop_tx: RwLock<Option<mpsc::Sender<()>>>,
}

impl StatusMonitor {
    /// 创建新的状态监控器实例。
    /// Create a new status monitor instance.
    pub fn new(state: Arc<AppState>, recorder: Arc<RecorderManager>) -> Arc<Self> {
        Arc::new(Self {
            state,
            recorder,
            statuses: RwLock::new(HashMap::new()),
            stop_tx: RwLock::new(None),
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

    /// 启动监控循环（Tauri 模式，使用 AppHandle 创建 emitter）。
    /// Start the monitoring loop (Tauri mode, creates emitter from AppHandle).
    pub fn start(self: &Arc<Self>, app_handle: tauri::AppHandle) {
        let emitter: Arc<dyn Emitter> = Arc::new(crate::core::emitter::TauriEmitter(app_handle));
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            monitor.start_with_emitter(emitter).await;
        });
    }

    /// 对单个主播执行一次状态轮询（Tauri 模式）。
    /// Perform a single status poll for one streamer (Tauri mode).
    pub async fn poll_one(self: &Arc<Self>, username: &str, app_handle: &tauri::AppHandle) {
        let emitter: Arc<dyn Emitter> = Arc::new(crate::core::emitter::TauriEmitter(app_handle.clone()));
        self.poll_one_with_emitter(username, &emitter).await;
    }

    /// 尝试为所有满足条件的主播启动录制（并发限制放宽时调用，Tauri 模式）。
    /// Try to start recordings for all eligible streamers (called when concurrency limit is relaxed, Tauri mode).
    pub async fn try_start_pending(self: &Arc<Self>, app_handle: &tauri::AppHandle) {
        let emitter: Arc<dyn Emitter> = Arc::new(crate::core::emitter::TauriEmitter(app_handle.clone()));
        self.try_start_pending_with_emitter(&emitter).await;
    }

    /// 启动监控循环（通用版本，接受任意 emitter）。
    /// Start the monitoring loop (generic version, accepts any emitter).
    pub async fn start_with_emitter(self: Arc<Self>, emitter: Arc<dyn Emitter>) {
        let (stop_tx, stop_rx) = mpsc::channel(1);
        *self.stop_tx.write() = Some(stop_tx);
        self.monitor_loop(emitter, stop_rx).await;
    }

    /// 监控主循环：立即轮询一次，然后按配置的间隔周期性轮询。
    /// Monitor main loop: poll once immediately, then poll periodically at the configured interval.
    async fn monitor_loop(
        self: Arc<Self>,
        emitter: Arc<dyn Emitter>,
        mut stop_rx: mpsc::Receiver<()>,
    ) {
        self.poll_all_with_emitter(&emitter).await;

        loop {
            let poll_interval =
                tokio::time::Duration::from_secs(self.state.get_settings().poll_interval_secs);

            tokio::select! {
                _ = stop_rx.recv() => {
                    tracing::info!("Monitor stopped");
                    break;
                }
                _ = tokio::time::sleep(poll_interval) => {
                    self.poll_all_with_emitter(&emitter).await;
                }
            }
        }
    }

    /// 尝试为所有满足条件的主播启动录制（通用版本）。
    /// Try to start recordings for all eligible streamers (generic version).
    pub async fn try_start_pending_with_emitter(self: &Arc<Self>, emitter: &Arc<dyn Emitter>) {
        let settings = self.state.get_settings();
        if !settings.auto_record {
            return;
        }
        let streamers = self.state.get_streamers();

        let candidates: Vec<(String, String)> = {
            let statuses = self.statuses.read();
            streamers
                .iter()
                .filter(|s| s.auto_record && !self.recorder.is_recording(&s.username))
                .filter_map(|s| {
                    statuses.get(&s.username).and_then(|cached| {
                        if cached.is_online {
                            cached
                                .playlist_url
                                .as_ref()
                                .map(|url| (s.username.clone(), url.clone()))
                        } else {
                            None
                        }
                    })
                })
                .collect()
        };

        for (username, playlist_url) in candidates {
            if self.recorder.is_recording(&username) {
                continue;
            }
            tracing::info!("try_start_pending: auto-starting recording → {}", username);
            let _ = self
                .recorder
                .start_recording_with_emitter(&username, &playlist_url, Arc::clone(emitter))
                .await;
        }
    }

    /// 对单个主播执行一次状态轮询（通用版本）。
    /// Perform a single status poll for one streamer (generic version).
    pub async fn poll_one_with_emitter(
        self: &Arc<Self>,
        username: &str,
        emitter: &Arc<dyn Emitter>,
    ) {
        let settings = self.state.get_settings();
        let streamers = self.state.get_streamers();

        let streamer = match streamers.into_iter().find(|s| s.username == username) {
            Some(s) => s,
            None => return,
        };

        let api = match StripchatApi::new(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            self.recorder.cdn_tld_cache(),
        ) {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("Failed to create API client: {}", e);
                emitter.emit(
                    "api-error",
                    &serde_json::json!({ "message": e.to_string() }),
                );
                return;
            }
        };

        self.poll_streamer(&api, streamer, emitter, settings.auto_record)
            .await;
    }

    /// 并发轮询所有追踪主播的状态（通用版本）。
    /// Concurrently poll the status of all tracked streamers (generic version).
    pub async fn poll_all_with_emitter(self: &Arc<Self>, emitter: &Arc<dyn Emitter>) {
        let settings = self.state.get_settings();
        let streamers = self.state.get_streamers();

        if streamers.is_empty() {
            return;
        }

        let api = match StripchatApi::new(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            self.recorder.cdn_tld_cache(),
        ) {
            Ok(a) => Arc::new(a),
            Err(e) => {
                tracing::error!("Failed to create API client: {}", e);
                emitter.emit(
                    "api-error",
                    &serde_json::json!({ "message": e.to_string() }),
                );
                return;
            }
        };

        let tasks: Vec<_> = streamers
            .into_iter()
            .map(|streamer| {
                let api = Arc::clone(&api);
                let monitor = Arc::clone(self);
                let emitter = Arc::clone(emitter);
                let auto_record_global = settings.auto_record;

                tokio::spawn(async move {
                    monitor
                        .poll_streamer(&api, streamer, &emitter, auto_record_global)
                        .await;
                })
            })
            .collect();

        for t in tasks {
            let _ = t.await;
        }
    }

    /// 轮询单个主播的状态，更新缓存，并根据状态变化触发自动录制逻辑。
    /// Poll a single streamer's status, update the cache, and trigger auto-recording logic based on status changes.
    async fn poll_streamer(
        self: &Arc<Self>,
        api: &StripchatApi,
        streamer: StreamerData,
        emitter: &Arc<dyn Emitter>,
        auto_record_global: bool,
    ) {
        let username = streamer.username.clone();

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
                    viewers: 0,
                    status: String::new(),
                    thumbnail_url: None,
                    playlist_url: None,
                });
        }

        let info = match api.get_stream_info(&username, !is_recording).await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("Poll failed → {}: {}", username, e);
                return;
            }
        };

        let status = StreamerStatus {
            username: username.clone(),
            is_online: info.is_online,
            is_recording,
            is_recordable: info.playlist_url.is_some(),
            viewers: info.viewers,
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
        {
            if let Some(ref playlist_url) = info.playlist_url {
                tracing::info!("Auto-starting recording → {} (just_online={}, dropped={}, natural_stop={}, should_be={})", username, just_came_online, recording_dropped, naturally_stopped, should_be_recording);
                let _ = self
                    .recorder
                    .start_recording_with_emitter(&username, playlist_url, Arc::clone(emitter))
                    .await;
            }
        }
    }
}
