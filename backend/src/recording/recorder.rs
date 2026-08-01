//! 录制管理器 / Recording Manager
//!
//! 管理所有主播的录制会话生命周期，包括：
//! - 启动/停止录制（HLS 分片下载 + fMP4 转 TS，转码委托给 `recording::ffmpeg_util`）
//! - 录制完成后自动触发后处理流水线
//!
//! ffmpeg/ffprobe 底层操作见 `recording::ffmpeg_util`；启动时的遗留分片扫描与
//! 空目录清理见 `recording::startup_scan`。
//!
//! Manages the lifecycle of all streamer recording sessions, including:
//! - Starting/stopping recordings (HLS segment download + fMP4 to TS; transcoding is
//!   delegated to `recording::ffmpeg_util`)
//! - Automatically triggering the post-processing pipeline after recording completes
//!
//! Low-level ffmpeg/ffprobe operations live in `recording::ffmpeg_util`; startup-time
//! leftover segment scanning and empty-directory cleanup live in `recording::startup_scan`.

use crate::config::settings::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use crate::recording::ffmpeg_util::{append_to_m3u8, convert_to_ts, dir_size_bytes};
use crate::recording::hls::{get_url_prefix, parse_playlist};
use crate::streaming::stripchat::StripchatApi;
use chrono::Local;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// 单个录制会话的状态 / State of a single recording session
#[derive(Debug, Clone)]
pub struct RecordingSession {
    /// 主播用户名 / Streamer username
    #[allow(dead_code)]
    pub username: String,
    /// 录制会话目录路径（存放 .ts 分片）/ Recording session directory path (stores .ts segments)
    pub dir_path: PathBuf,
    /// 录制开始时间 / Recording start time
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// 停止录制的信号发送端 / Sender to signal recording stop
    stop_tx: mpsc::Sender<()>,
}

/// 录制管理器，管理所有主播的录制会话。
/// Recording manager that manages all streamer recording sessions.
pub struct RecorderManager {
    /// 应用状态 / Application state
    state: Arc<AppState>,
    /// 活跃录制会话表（用户名 -> 会话）/ Active recording sessions (username -> session)
    sessions: RwLock<HashMap<String, RecordingSession>>,
    /// 自然结束（非手动停止）的主播集合，用于触发自动重录 / Streamers that stopped naturally (not manually), for auto-restart
    pub naturally_stopped: RwLock<HashSet<String>>,
    /// 正在手动停止录制的主播集合 / Streamers currently being manually stopped
    manually_stopping: RwLock<HashSet<String>>,
    /// 各 CDN 节点的首选 TLD 缓存（跨会话共享）/ Preferred TLD cache per CDN node (shared across sessions)
    preferred_tld_by_node: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    /// 正在合并的会话目录集合 / Set of session directories currently being merged
    pub merging_dirs: RwLock<HashSet<PathBuf>>,
    /// 等待合并的会话目录集合 / Set of session directories waiting to merge
    pub waiting_merge_dirs: RwLock<HashSet<PathBuf>>,
    /// 活跃录制会话的实时分片统计（video_path -> (downloaded, failed)）
    /// Real-time segment stats for active sessions (video_path -> (downloaded, failed))
    pub segment_stats: RwLock<HashMap<String, (u64, u64)>>,
}

impl RecorderManager {
    /// 创建新的录制管理器实例。
    /// Create a new recorder manager instance.
    pub fn new(state: Arc<AppState>) -> Arc<Self> {
        Arc::new(Self {
            state,
            sessions: RwLock::new(HashMap::new()),
            naturally_stopped: RwLock::new(HashSet::new()),
            manually_stopping: RwLock::new(HashSet::new()),
            preferred_tld_by_node: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            merging_dirs: RwLock::new(HashSet::new()),
            waiting_merge_dirs: RwLock::new(HashSet::new()),
            segment_stats: RwLock::new(HashMap::new()),
        })
    }

    /// 判断指定主播是否正在录制。
    /// Check if a specific streamer is currently being recorded.
    pub fn is_recording(&self, username: &str) -> bool {
        self.sessions.read().contains_key(username)
    }

    /// 获取 CDN TLD 缓存的共享引用（供 StripchatApi 使用）。
    /// Get a shared reference to the CDN TLD cache (for use by StripchatApi).
    pub fn cdn_tld_cache(&self) -> Arc<parking_lot::Mutex<HashMap<String, String>>> {
        Arc::clone(&self.preferred_tld_by_node)
    }

    /// 获取当前应用设置。
    /// Get the current application settings.
    pub fn get_settings(&self) -> crate::config::settings::Settings {
        self.state.get_settings()
    }

    /// 获取应用状态的共享引用（供 recording 模块内其他子模块，如启动扫描，复用）。
    /// Get a shared reference to the application state (for reuse by other recording
    /// submodules, e.g. startup scanning).
    pub fn app_state(&self) -> Arc<AppState> {
        Arc::clone(&self.state)
    }

    /// 判断指定路径是否被某个活跃录制会话锁定（路径在会话目录下）。
    /// Check if a path is locked by an active recording session (path is under a session directory).
    pub fn is_file_locked(&self, path: &std::path::Path) -> bool {
        self.sessions
            .read()
            .values()
            .any(|s| path.starts_with(&s.dir_path))
    }

    /// 获取所有活跃录制会话的目录路径和开始时间列表。
    /// Get a list of all active recording session directory paths and start times.
    pub fn get_active_sessions(&self) -> Vec<(PathBuf, chrono::DateTime<chrono::Utc>)> {
        self.sessions
            .read()
            .values()
            .map(|s| (s.dir_path.clone(), s.started_at))
            .collect()
    }

    /// 返回当前活跃录制会话数量。
    /// Return the number of currently active recording sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.read().len()
    }

    /// 启动录制（通用版本，接受任意 emitter）。
    /// 创建会话目录，启动异步录制循环，录制完成后自动合并分片并触发后处理。
    ///
    /// Start recording (generic version, accepts any emitter).
    /// Creates the session directory, starts the async recording loop,
    /// and automatically merges segments and triggers post-processing after completion.
    ///
    /// # 返回值 / Returns
    /// 录制会话目录路径 / Recording session directory path
    pub async fn start_recording_with_emitter(
        self: &Arc<Self>,
        username: &str,
        playlist_url: &str,
        emitter: Arc<dyn Emitter>,
    ) -> Result<String> {
        if self.is_recording(username) {
            return Err(AppError::AlreadyRecording(username.to_string()));
        }

        let settings = self.state.get_settings();
        if settings.max_concurrent > 0 && self.active_count() >= settings.max_concurrent {
            return Err(AppError::Other(
                "Max concurrent recordings reached".to_string(),
            ));
        }

        // 分片输出目录：直接使用 output_dir，不再有独立的合并视频目录
        // Segment output directory: use output_dir directly; no separate merged-video directory
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let session_dir = PathBuf::from(&settings.output_dir)
            .join(username)
            .join(format!("{}_{}", username, timestamp));
        fs::create_dir_all(&session_dir)?;

        let (stop_tx, stop_rx) = mpsc::channel(1);

        let session = RecordingSession {
            username: username.to_string(),
            dir_path: session_dir.clone(),
            started_at: chrono::Utc::now(),
            stop_tx,
        };

        self.sessions.write().insert(username.to_string(), session);

        // 录制开始时立即创建 meta，写入 recording 状态
        // Create meta immediately when recording starts, with "recording" status
        {
            let started_at = chrono::Local::now().to_rfc3339();
            let meta = crate::recording::meta::VideoMeta {
                meta_version: crate::recording::meta::META_VERSION,
                status: "recording".to_string(),
                started_at,
                size_bytes: 0,
                video_duration_secs: None,
                video_resolution: None,
                pp_execution: None,
                segments_downloaded: None,
                segments_failed: None,
                video_path: None, // write_meta 会自动填入 / auto-filled by write_meta
                pp_progress: None,
            };
            crate::recording::meta::write_meta(&session_dir, &meta);
        }

        emitter.emit(
            "recording-started",
            &serde_json::json!({
                "username": username,
                "dir_path": session_dir.to_string_lossy()
            }),
        );

        let result_path = session_dir.to_string_lossy().to_string();
        let manager = Arc::clone(self);
        let username = username.to_string();
        let playlist_url = playlist_url.to_string();

        tokio::spawn(async move {
            if let Err(e) = manager
                .recording_loop(
                    &username,
                    &playlist_url,
                    &session_dir,
                    stop_rx,
                    Arc::clone(&emitter),
                )
                .await
            {
                tracing::error!("Recording error → {}: {}", username, e);
            }

            let record_duration_secs = manager.sessions.read().get(&username).map(|s| {
                chrono::Utc::now()
                    .signed_duration_since(s.started_at)
                    .num_seconds()
                    .max(0) as u64
            });

            manager.sessions.write().remove(&username);

            // 录制结束后清理 segment_stats 缓存（以 session_dir 路径为 key）
            // Clean up segment_stats cache after recording ends (keyed by session_dir path)
            manager.segment_stats.write().remove(&session_dir.to_string_lossy().to_string());

            let was_manual = manager.manually_stopping.write().remove(&username);
            if !was_manual {
                manager.naturally_stopped.write().insert(username.clone());
            }

            let session_dir_clone = session_dir.clone();
            let username_clone = username.clone();
            let state_clone = Arc::clone(&manager.state);
            let emitter_clone = Arc::clone(&emitter);
            let manager_clone = Arc::clone(&manager);

            emitter.emit(
                "recording-stopped",
                &serde_json::json!({
                    "username": username,
                    "session_dir": session_dir.to_string_lossy(),
                    "record_duration_secs": record_duration_secs,
                }),
            );

            // 录制结束后触发后处理流水线（完全按照用户配置执行，不做任何自动注入）。
            // 若用户流水线中无任何启用节点，则跳过后处理。
            //
            // After recording ends, trigger the post-processing pipeline as-is per user config.
            // If no enabled nodes exist in the user pipeline, skip post-processing entirely.
            tokio::task::spawn_blocking(move || {
                let _startup_guard = state_clone
                    .startup_lock
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                let user_pipeline = state_clone.get_pipeline();

                // 若流水线中没有任何启用节点，直接跳过后处理
                // If no enabled nodes, skip post-processing entirely
                if !user_pipeline.nodes.iter().any(|n| n.enabled) {
                    return;
                }

                let stem = session_dir_clone
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let started_at =
                    crate::commands::recording_cmd::parse_timestamp_from_stem_pub(stem)
                        .unwrap_or_else(|| {
                            let local: chrono::DateTime<chrono::Local> = chrono::Utc::now().into();
                            local.to_rfc3339()
                        });

                // session_dir 作为流水线的初始输入和 meta 占位路径
                // session_dir acts as both pipeline initial input and meta placeholder path
                crate::recording::meta::ensure_meta(&session_dir_clone, &started_at);

                // 标记 session_dir 正在处理中（供前端进度显示）
                // Mark session_dir as being processed (for frontend progress display)
                manager_clone
                    .waiting_merge_dirs
                    .write()
                    .insert(session_dir_clone.clone());
                emitter_clone.emit(
                    "recording-pp-waiting",
                    &serde_json::json!({
                        "username": username_clone,
                        "session_dir": session_dir_clone.to_string_lossy(),
                        "video_path": session_dir_clone.to_string_lossy(),
                    }),
                );

                // 触发后处理流水线，initial_path = session_dir
                // Trigger post-processing pipeline; initial_path = session_dir
                crate::commands::postprocess_cmd::run_postprocess_for_path(
                    &session_dir_clone,
                    &session_dir_clone,
                    &user_pipeline,
                    &emitter_clone,
                    &state_clone,
                );

                manager_clone
                    .waiting_merge_dirs
                    .write()
                    .remove(&session_dir_clone);
            })
            .await
            .ok();
        });

        Ok(result_path)
    }

    /// 手动停止录制（标记为手动停止，防止自动重录）。
    /// Manually stop recording (marks as manually stopped to prevent auto-restart).
    pub async fn stop_recording(self: &Arc<Self>, username: &str) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(username)
            .cloned()
            .ok_or_else(|| AppError::NotRecording(username.to_string()))?;
        self.manually_stopping.write().insert(username.to_string());
        let _ = session.stop_tx.send(()).await;
        Ok(())
    }

    /// 自动停止录制（不标记为手动停止，允许自动重录）。
    /// Automatically stop recording (not marked as manually stopped, allows auto-restart).
    pub async fn stop_recording_auto(self: &Arc<Self>, username: &str) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(username)
            .cloned()
            .ok_or_else(|| AppError::NotRecording(username.to_string()))?;
        let _ = session.stop_tx.send(()).await;
        Ok(())
    }

    /// 录制主循环：持续拉取 HLS 播放列表、下载新分片、转换为 TS 格式并写入会话目录。
    /// 代理设置和 Mouflon 密钥在每次循环迭代时动态读取，变更后立即生效。
    ///
    /// Recording main loop: continuously fetches HLS playlists, downloads new segments,
    /// converts to TS format, and writes to the session directory.
    /// Proxy settings and Mouflon keys are read dynamically each iteration and take effect immediately.
    async fn recording_loop(
        &self,
        username: &str,
        playlist_url: &str,
        session_dir: &PathBuf,
        mut stop_rx: mpsc::Receiver<()>,
        emitter: Arc<dyn Emitter>,
    ) -> Result<()> {
        // 初始设置快照，用于检测变更 / Initial settings snapshot for change detection
        let mut last_settings = self.state.get_settings();
        let mut api = StripchatApi::new(
            last_settings.api_proxy_url.as_deref(),
            last_settings.cdn_proxy_url.as_deref(),
            last_settings.sc_mirror_url.as_deref(),
            Arc::clone(&self.preferred_tld_by_node),
        )?
        .with_mouflon_keys(self.state.get_mouflon_keys());
        let mut current_playlist_url = playlist_url.to_string();
        let mut url_prefix = get_url_prefix(&current_playlist_url);

        let mut downloaded_sequences: HashSet<u32> = HashSet::new();
        let mut mp4_header: Option<Vec<u8>> = None;
        let mut cached_init_url: Option<String> = None;
        let mut retry_count = 0;
        let mut playlist_refresh_failures = 0;
        let mut consecutive_cdn_failures: usize = 0;
        let mut last_size_snapshot: Option<(u64, std::time::Instant)> = None;
        // 累计成功下载的分片数 / Total successfully downloaded segments
        let mut total_downloaded: u64 = 0;
        // 累计下载失败的分片数 / Total failed segment downloads
        let mut total_failed: u64 = 0;
        const MAX_RETRIES: u32 = 10;
        const MAX_PLAYLIST_REFRESH_FAILURES: u32 = 5;
        const CDN_FAILURE_REFRESH_THRESHOLD: usize = 3;

        tracing::info!("Started recording {} → {:?}", username, session_dir);

        loop {
            // 检测代理/密钥设置变更，变更时重建 api 实例使其立即生效
            // Detect proxy/key setting changes and rebuild api instance for immediate effect
            let current_settings = self.state.get_settings();
            let current_mouflon_keys = self.state.get_mouflon_keys();
            let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
                || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
                || current_settings.sc_mirror_url != last_settings.sc_mirror_url;
            let keys_changed = current_mouflon_keys != *api.mouflon_keys();
            if proxy_changed || keys_changed {
                match StripchatApi::new(
                    current_settings.api_proxy_url.as_deref(),
                    current_settings.cdn_proxy_url.as_deref(),
                    current_settings.sc_mirror_url.as_deref(),
                    Arc::clone(&self.preferred_tld_by_node),
                ) {
                    Ok(new_api) => {
                        api = new_api.with_mouflon_keys(current_mouflon_keys);
                        tracing::info!("Recording {}: api client rebuilt due to settings change", username);
                    }
                    Err(e) => {
                        tracing::warn!("Recording {}: failed to rebuild api client: {}", username, e);
                    }
                }
                last_settings = current_settings.clone();
            }
            let mouflon_keys = api.mouflon_keys().clone();

            let mut wait_next_round = true;
            tokio::select! {
                _ = stop_rx.recv() => {
                    tracing::info!("Stop signal received → {}", username);
                    break;
                }
                result = Self::fetch_segments(
                    &api,
                    &current_playlist_url,
                    &url_prefix,
                    &mouflon_keys,
                    session_dir,
                    username,
                    &mut downloaded_sequences,
                    &mut mp4_header,
                    &mut cached_init_url,
                ) => {
                    match result {
                        Ok((n, cdn_fail)) => {
                            if cdn_fail > 0 {
                                consecutive_cdn_failures += cdn_fail;
                                total_failed += cdn_fail as u64;
                            }
                            if n > 0 {
                                consecutive_cdn_failures = 0;
                                retry_count = 0;
                                total_downloaded += n as u64;
                                let size_bytes = dir_size_bytes(session_dir).unwrap_or(0);
                                let now = std::time::Instant::now();
                                let speed_bps = last_size_snapshot.map(|(prev_size, prev_time)| {
                                    let dt = now.duration_since(prev_time).as_secs_f64();
                                    let ds = size_bytes.saturating_sub(prev_size) as f64;
                                    if dt > 0.0 { ds / dt } else { 0.0 }
                                });
                                last_size_snapshot = Some((size_bytes, now));

                                // session_dir 路径同时用于 meta 更新和前端匹配
                                // session_dir path is used for both meta update and frontend matching
                                let session_dir_str = session_dir.to_string_lossy().to_string();

                                // 将实时文件大小和分片统计写入 meta JSON
                                // Write real-time file size and segment stats to meta JSON
                                if let Some(mut meta) = crate::recording::meta::read_meta(session_dir) {
                                    let mut changed = false;
                                    if meta.size_bytes != size_bytes {
                                        meta.size_bytes = size_bytes;
                                        changed = true;
                                    }
                                    if meta.segments_downloaded != Some(total_downloaded) {
                                        meta.segments_downloaded = Some(total_downloaded);
                                        changed = true;
                                    }
                                    if meta.segments_failed != Some(total_failed) {
                                        meta.segments_failed = Some(total_failed);
                                        changed = true;
                                    }
                                    if changed {
                                        crate::recording::meta::write_meta(session_dir, &meta);
                                    }
                                }

                                // 更新管理器中的实时分片统计 / Update real-time segment stats in manager
                                self.segment_stats.write().insert(
                                    session_dir_str.clone(),
                                    (total_downloaded, total_failed),
                                );

                                let mut payload = serde_json::json!({
                                    "path": session_dir_str,
                                    "segment_count": downloaded_sequences.len(),
                                    "size_bytes": size_bytes,
                                    "segments_downloaded": total_downloaded,
                                    "segments_failed": total_failed,
                                });
                                if let Some(spd) = speed_bps {
                                    payload["speed_bps"] = serde_json::json!(spd);
                                }
                                emitter.emit("recording-file-update", &payload);
                            } else {
                                retry_count += 1;
                            }
                            if consecutive_cdn_failures >= CDN_FAILURE_REFRESH_THRESHOLD {
                                tracing::error!(
                                    "Fetch error → {}: {} consecutive CDN failures, refreshing playlist",
                                    username, consecutive_cdn_failures
                                );
                                consecutive_cdn_failures = 0;
                                match api.get_stream_info(username, true).await {
                                    Ok(info) => {
                                        if let Some(new_url) = info.playlist_url {
                                            tracing::info!("Refreshed playlist URL → {}", username);
                                            url_prefix = get_url_prefix(&new_url);
                                            current_playlist_url = new_url;
                                            playlist_refresh_failures = 0;
                                            retry_count = 0;
                                            wait_next_round = false;
                                        } else if !info.is_recordable {
                                            tracing::warn!("Stream no longer recordable → {} (status: {}), stopping", username, info.status);
                                            break;
                                        } else {
                                            playlist_refresh_failures += 1;
                                        }
                                    }
                                    Err(refresh_err) => {
                                        tracing::error!("Playlist refresh failed → {}: {}", username, refresh_err);
                                        playlist_refresh_failures += 1;
                                    }
                                }
                                if playlist_refresh_failures >= MAX_PLAYLIST_REFRESH_FAILURES {
                                    tracing::warn!("Stream ended → {} (playlist refresh failed {} times)", username, playlist_refresh_failures);
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Fetch error → {}: {}, attempting playlist refresh", username, e);
                            consecutive_cdn_failures = 0;
                            match api.get_stream_info(username, true).await {
                                Ok(info) => {
                                    if let Some(new_url) = info.playlist_url {
                                        tracing::info!("Refreshed playlist URL → {}", username);
                                        url_prefix = get_url_prefix(&new_url);
                                        current_playlist_url = new_url;
                                        playlist_refresh_failures = 0;
                                        retry_count = 0;
                                        wait_next_round = false;
                                    } else if !info.is_recordable {
                                        tracing::warn!("Stream no longer recordable → {} (status: {}), stopping", username, info.status);
                                        break;
                                    } else {
                                        tracing::warn!("No playlist URL yet → {} (status: {}), retrying", username, info.status);
                                        playlist_refresh_failures += 1;
                                    }
                                }
                                Err(refresh_err) => {
                                    tracing::error!("Playlist refresh failed → {}: {}", username, refresh_err);
                                    playlist_refresh_failures += 1;
                                }
                            }
                            if playlist_refresh_failures >= MAX_PLAYLIST_REFRESH_FAILURES {
                                tracing::warn!("Stream ended → {} (playlist refresh failed {} times)", username, playlist_refresh_failures);
                                break;
                            }
                        }
                    }
                    if retry_count >= MAX_RETRIES {
                        tracing::warn!("Stream ended → {} (max retries)", username);
                        break;
                    }
                    if wait_next_round {
                        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
                    }
                }
            }
        }

        tracing::info!("Finished recording {} → {:?}", username, session_dir);
        Ok(())
    }

    /// 拉取一次播放列表并下载所有新分片。
    /// 返回 `(写入的分片数, CDN 失败的分片数)`。
    ///
    /// Fetch the playlist once and download all new segments.
    /// Returns `(number of segments written, number of CDN failures)`.
    #[allow(clippy::too_many_arguments)]
    async fn fetch_segments(
        api: &StripchatApi,
        playlist_url: &str,
        url_prefix: &str,
        mouflon_keys: &HashMap<String, String>,
        session_dir: &std::path::Path,
        username: &str,
        downloaded_sequences: &mut HashSet<u32>,
        mp4_header: &mut Option<Vec<u8>>,
        cached_init_url: &mut Option<String>,
    ) -> Result<(usize, usize)> {
        let playlist = api.fetch_playlist(playlist_url).await?;
        let (segments, init_url) = parse_playlist(&playlist, url_prefix, mouflon_keys)?;
        let init_url_path = |u: &str| u.split('?').next().unwrap_or(u).to_string();
        let new_init_path = init_url.as_deref().map(init_url_path);
        let cached_init_path = cached_init_url.as_deref().map(init_url_path);
        if new_init_path.is_some() && new_init_path != cached_init_path
            && let Some(ref url) = init_url
        {
            match api.download_segment(url).await {
                Ok(data) => {
                    tracing::info!("Cached init segment → {} ({} bytes)", username, data.len());
                    *mp4_header = Some(data);
                    *cached_init_url = Some(url.clone());
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to download init segment: {}, skipping this round",
                        e
                    );
                    return Ok((0, 0));
                }
            }
        }

        let mut written = 0;
        let mut new_segments = 0;
        let mut cdn_failures = 0;

        for segment in segments {
            if downloaded_sequences.contains(&segment.sequence) {
                continue;
            }
            new_segments += 1;

            match api.download_segment(&segment.url).await {
                Ok(data) => {
                    if data.len() > 1000 {
                        let ts_path = session_dir
                            .join(format!("{}_segment{:06}.ts", username, segment.sequence));

                        let fmp4: Vec<u8> = match mp4_header.as_deref() {
                            Some(h) => {
                                let mut v = Vec::with_capacity(h.len() + data.len());
                                v.extend_from_slice(h);
                                v.extend_from_slice(&data);
                                v
                            }
                            None => data,
                        };

                        match convert_to_ts(fmp4, &ts_path).await {
                            Ok(_) => {
                                append_to_m3u8(session_dir, &ts_path);
                                downloaded_sequences.insert(segment.sequence);
                                written += 1;
                            }
                            Err(e) => {
                                tracing::error!(
                                    "ffmpeg convert failed → segment {}: {}",
                                    segment.sequence,
                                    e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to download segment {}: {}", segment.sequence, e);
                    cdn_failures += 1;
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        if new_segments > 0 && cdn_failures == new_segments {
            return Err(AppError::Other(format!(
                "All {} new segments failed (CDN 404 / token expired), refreshing playlist",
                new_segments
            )));
        }

        Ok((written, cdn_failures))
    }
}
