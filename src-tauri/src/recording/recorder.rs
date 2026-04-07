//! 录制管理器 / Recording Manager
//!
//! 管理所有主播的录制会话生命周期，包括：
//! - 启动/停止录制（HLS 分片下载 + fMP4 转 TS + ffmpeg 合并）
//! - 录制完成后自动触发后处理流水线
//! - 启动时合并遗留的未完成录制片段
//!
//! Manages the lifecycle of all streamer recording sessions, including:
//! - Starting/stopping recordings (HLS segment download + fMP4 to TS + ffmpeg merge)
//! - Automatically triggering the post-processing pipeline after recording completes
//! - Merging leftover incomplete recording segments on startup

use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use crate::recording::hls::{get_url_prefix, parse_playlist};
use crate::config::settings::AppState;
use crate::streaming::stripchat::StripchatApi;
use chrono::Local;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock};
use tokio::sync::{mpsc, Semaphore};

/// 全局 ffmpeg 并发信号量，限制同时运行的 ffmpeg 进程数（最多 4 个）。
/// Global ffmpeg concurrency semaphore, limiting simultaneous ffmpeg processes (max 4).
static FFMPEG_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

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

    /// 启动录制（Tauri 模式，使用 AppHandle 创建 emitter）。
    /// Start recording (Tauri mode, creates emitter from AppHandle).
    pub async fn start_recording(
        self: &Arc<Self>,
        username: &str,
        playlist_url: &str,
        app_handle: tauri::AppHandle,
    ) -> Result<String> {
        let emitter = Arc::new(crate::core::emitter::TauriEmitter(app_handle));
        self.start_recording_with_emitter(username, playlist_url, emitter)
            .await
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
        let api_proxy = settings.api_proxy_url.clone();
        let cdn_proxy = settings.cdn_proxy_url.clone();
        let sc_mirror = settings.sc_mirror_url.clone();
        let mouflon_keys = self.state.get_mouflon_keys();
        let merge_format = settings.merge_format.clone();

        tokio::spawn(async move {
            if let Err(e) = manager
                .recording_loop(
                    &username,
                    &playlist_url,
                    &session_dir,
                    api_proxy.as_deref(),
                    cdn_proxy.as_deref(),
                    sc_mirror.as_deref(),
                    mouflon_keys,
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

            let was_manual = manager.manually_stopping.write().remove(&username);
            if !was_manual {
                manager.naturally_stopped.write().insert(username.clone());
            }

            let session_dir_clone = session_dir.clone();
            let username_clone = username.clone();
            let merge_format_clone = merge_format.clone();
            let state_clone = Arc::clone(&manager.state);
            let emitter_clone = Arc::clone(&emitter);

            emitter.emit(
                "recording-merge-waiting",
                &serde_json::json!({
                    "username": username,
                    "session_dir": session_dir.to_string_lossy(),
                    "merge_format": merge_format,
                }),
            );

            let video_duration_secs = tokio::task::spawn_blocking(move || {
                let _startup_guard = state_clone
                    .startup_lock
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());

                emitter_clone.emit(
                    "recording-merging",
                    &serde_json::json!({
                        "username": username_clone,
                        "session_dir": session_dir_clone.to_string_lossy(),
                        "merge_format": merge_format_clone,
                    }),
                );

                let session_dir_str = session_dir_clone.to_string_lossy().to_string();
                let duration = merge_segments(
                    &session_dir_clone,
                    &username_clone,
                    &merge_format_clone,
                    &emitter_clone,
                    &session_dir_str,
                );

                if duration.is_some() {
                    let parent = session_dir_clone.parent().unwrap_or(&session_dir_clone);
                    let stem = session_dir_clone
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    let merged_path = parent.join(format!("{}.{}", stem, merge_format_clone));
                    if merged_path.exists() {
                        let pipeline = state_clone.get_pipeline();
                        if !pipeline.nodes.is_empty() {
                            crate::commands::postprocess_cmd::run_postprocess_for_path(
                                &merged_path,
                                &pipeline,
                                &emitter_clone,
                                &state_clone,
                            );
                        }
                    }
                }

                duration
            })
            .await
            .unwrap_or(None);

            emitter.emit(
                "recording-stopped",
                &serde_json::json!({
                    "username": username,
                    "record_duration_secs": record_duration_secs,
                    "video_duration_secs": video_duration_secs,
                }),
            );
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
    /// Recording main loop: continuously fetches HLS playlists, downloads new segments,
    /// converts to TS format, and writes to the session directory.
    async fn recording_loop(
        &self,
        username: &str,
        playlist_url: &str,
        session_dir: &PathBuf,
        api_proxy: Option<&str>,
        cdn_proxy: Option<&str>,
        sc_mirror: Option<&str>,
        mouflon_keys: HashMap<String, String>,
        mut stop_rx: mpsc::Receiver<()>,
        emitter: Arc<dyn Emitter>,
    ) -> Result<()> {
        let api = StripchatApi::new(
            api_proxy,
            cdn_proxy,
            sc_mirror,
            Arc::clone(&self.preferred_tld_by_node),
        )?;
        let mut current_playlist_url = playlist_url.to_string();
        let mut url_prefix = get_url_prefix(&current_playlist_url);

        let mut downloaded_sequences: HashSet<u32> = HashSet::new();
        let mut mp4_header: Option<Vec<u8>> = None;
        let mut cached_init_url: Option<String> = None;
        let mut retry_count = 0;
        let mut playlist_refresh_failures = 0;
        let mut consecutive_cdn_failures: usize = 0;
        const MAX_RETRIES: u32 = 10;
        const MAX_PLAYLIST_REFRESH_FAILURES: u32 = 5;
        const CDN_FAILURE_REFRESH_THRESHOLD: usize = 3;

        tracing::info!("Started recording {} → {:?}", username, session_dir);

        loop {
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
                            }
                            if n > 0 {
                                consecutive_cdn_failures = 0;
                                retry_count = 0;
                                let size_bytes = dir_size_bytes(session_dir).unwrap_or(0);
                                emitter.emit("recording-file-update", &serde_json::json!({
                                    "path": session_dir.to_string_lossy(),
                                    "segment_count": downloaded_sequences.len(),
                                    "size_bytes": size_bytes,
                                }));
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
    async fn fetch_segments(
        api: &StripchatApi,
        playlist_url: &str,
        url_prefix: &str,
        mouflon_keys: &HashMap<String, String>,
        session_dir: &PathBuf,
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
        if new_init_path.is_some() && new_init_path != cached_init_path {
            if let Some(ref url) = init_url {
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
                                append_to_filelist(session_dir, &ts_path);
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

/// 检查 ffmpeg 是否在 PATH 中可用。
/// Check if ffmpeg is available on PATH.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// 使用 ffmpeg 将 fMP4 数据转换为 MPEG-TS 格式（通过 stdin 管道传入）。
/// Convert fMP4 data to MPEG-TS format using ffmpeg (piped via stdin).
async fn convert_to_ts(fmp4_data: Vec<u8>, ts_path: &PathBuf) -> Result<()> {
    let _permit = FFMPEG_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Other(format!("ffmpeg semaphore: {}", e)))?;

    let mut child = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-i", "pipe:0", "-c", "copy", "-f", "mpegts"])
        .arg(ts_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(&fmp4_data)
            .await
            .map_err(|e| AppError::Other(format!("ffmpeg stdin write: {}", e)))?;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| AppError::Other(format!("ffmpeg wait: {}", e)))?;

    if !status.success() {
        return Err(AppError::Other(format!("ffmpeg exited with {}", status)));
    }
    Ok(())
}

/// 将 TS 分片文件名追加到会话目录的 filelist.txt（供 ffmpeg concat 使用）。
/// Append a TS segment filename to the session directory's filelist.txt (for ffmpeg concat).
fn append_to_filelist(session_dir: &PathBuf, ts_path: &PathBuf) {
    let filelist_path = session_dir.join("filelist.txt");
    let Some(filename) = ts_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let line = format!("file '{}'\n", filename);
    if let Err(e) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&filelist_path)
        .and_then(|mut f| f.write_all(line.as_bytes()))
    {
        tracing::error!("Failed to update filelist.txt: {}", e);
    }
}

/// 计算目录中所有文件的总大小（字节）。
/// Calculate the total size of all files in a directory (bytes).
pub fn dir_size_bytes(dir: &PathBuf) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

/// 使用 ffmpeg 将会话目录中的所有 TS 分片合并为单个视频文件。
/// 合并过程中定期发送 `merge-progress` 事件，合并完成后删除会话目录。
///
/// Merge all TS segments in the session directory into a single video file using ffmpeg.
/// Periodically emits `merge-progress` events during merging; deletes the session directory after completion.
///
/// # 返回值 / Returns
/// 合并后视频的时长（秒），失败时返回 `None`。
/// Duration of the merged video (seconds), or `None` on failure.
fn merge_segments(
    session_dir: &PathBuf,
    username: &str,
    merge_format: &str,
    emitter: &Arc<dyn Emitter>,
    session_dir_str: &str,
) -> Option<u64> {
    let filelist_path = session_dir.join("filelist.txt");
    if !filelist_path.exists() {
        tracing::warn!(
            "filelist.txt not found in {:?}, skipping merge",
            session_dir
        );
        return None;
    }

    let parent = match session_dir.parent() {
        Some(p) => p,
        None => return None,
    };
    let stem = match session_dir.file_name().and_then(|n| n.to_str()) {
        Some(s) => s,
        None => return None,
    };
    let output_path = parent.join(format!("{}.{}", stem, merge_format));

    tracing::info!("Merging {} → {:?}", username, output_path);

    let total_bytes: u64 = fs::read_dir(session_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("ts") {
                        fs::metadata(&p).ok().map(|m| m.len())
                    } else {
                        None
                    }
                })
                .sum()
        })
        .unwrap_or(0);

    let _permit = tokio::runtime::Handle::current()
        .block_on(FFMPEG_SEMAPHORE.acquire())
        .expect("ffmpeg semaphore closed");

    let mut child = match Command::new("ffmpeg")
        .args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&filelist_path)
        .args(["-c", "copy"])
        .arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to spawn ffmpeg → merge: {}", e);
            return None;
        }
    };

    let poll_interval = std::time::Duration::from_millis(500);
    loop {
        std::thread::sleep(poll_interval);
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                let out_bytes = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
                emitter.emit(
                    "merge-progress",
                    &serde_json::json!({
                        "session_dir": session_dir_str,
                        "out_bytes": out_bytes,
                        "total_bytes": total_bytes,
                    }),
                );
            }
            Err(e) => {
                tracing::error!("ffmpeg wait error: {}", e);
                break;
            }
        }
    }

    let status = child.wait();
    match status {
        Ok(s) if s.success() => {
            tracing::info!("Merge complete: {:?}", output_path);
            if let Err(e) = fs::remove_dir_all(session_dir) {
                tracing::error!("Failed to remove segment dir: {}", e);
            }
            get_video_duration(&output_path)
        }
        Ok(s) => {
            tracing::warn!("ffmpeg merge exited with {}", s);
            None
        }
        Err(e) => {
            tracing::error!("Failed to spawn ffmpeg → merge: {}", e);
            None
        }
    }
}

/// 使用 ffprobe 获取视频文件的时长（秒）。
/// Get the duration of a video file in seconds using ffprobe.
pub fn get_video_duration(path: &std::path::Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<f64>().ok().map(|d| d as u64)
}

/// 启动时扫描输出目录，合并所有遗留的未完成录制片段，并对未后处理的视频触发后处理。
/// On startup, scan the output directory to merge all leftover incomplete recording segments,
/// and trigger post-processing for videos that haven't been processed yet.
pub fn startup_merge_leftover_segments(
    output_dir: &std::path::Path,
    merge_format: &str,
    emitter: &Arc<dyn Emitter>,
    recorder: &Arc<RecorderManager>,
) -> Vec<PathBuf> {
    if !output_dir.exists() {
        return Vec::new();
    }
    let state = Arc::clone(&recorder.state);
    let pipeline = state.get_pipeline();

    let mut segment_dirs: Vec<PathBuf> = Vec::new();
    collect_segment_dirs(output_dir, &mut segment_dirs);
    segment_dirs.sort_by_key(|p| session_dir_timestamp(p));

    let mut unprocessed_videos: Vec<PathBuf> = Vec::new();
    if !pipeline.nodes.is_empty() {
        let pp_results = state.data.read().pp_results.clone();
        collect_unprocessed_videos(
            output_dir,
            merge_format,
            &pp_results,
            &mut unprocessed_videos,
        );
        unprocessed_videos.sort_by_key(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            session_dir_timestamp_from_stem(stem)
        });
    }

    if segment_dirs.is_empty() && unprocessed_videos.is_empty() {
        return Vec::new();
    }

    let _startup_guard = state.startup_lock.lock().unwrap_or_else(|e| e.into_inner());

    let mut merged_paths = Vec::new();
    let mut pp_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

    for video_path in unprocessed_videos {
        if !pipeline.nodes.is_empty() {
            let pp_state = Arc::clone(&state);
            let pp_emitter = Arc::clone(emitter);
            let pp_pipeline = pipeline.clone();
            let handle = std::thread::spawn(move || {
                crate::commands::postprocess_cmd::run_postprocess_for_path(
                    &video_path,
                    &pp_pipeline,
                    &pp_emitter,
                    &pp_state,
                );
            });
            pp_handles.push(handle);
        }
    }

    for path in &segment_dirs {
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        recorder.waiting_merge_dirs.write().insert(path.clone());
        emitter.emit(
            "recording-merge-waiting",
            &serde_json::json!({
                "username": username,
                "session_dir": path.to_string_lossy(),
                "merge_format": merge_format,
            }),
        );
    }

    for path in segment_dirs {
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let output_path = path
            .parent()
            .unwrap_or(&path)
            .join(format!("{}.{}", stem, merge_format));
        let path_str = path.to_string_lossy().to_string();

        recorder.waiting_merge_dirs.write().remove(&path);
        recorder.merging_dirs.write().insert(path.clone());
        emitter.emit(
            "recording-merging",
            &serde_json::json!({
                "username": username,
                "session_dir": path_str,
                "merge_format": merge_format,
            }),
        );

        let video_duration_secs = merge_segments(&path, &stem, merge_format, emitter, &path_str);

        recorder.merging_dirs.write().remove(&path);
        emitter.emit(
            "recording-stopped",
            &serde_json::json!({
                "username": username,
                "session_dir": path_str,
                "record_duration_secs": serde_json::Value::Null,
                "video_duration_secs": video_duration_secs,
            }),
        );

        if output_path.exists() {
            merged_paths.push(output_path.clone());
            if !pipeline.nodes.is_empty() {
                let pp_state = Arc::clone(&state);
                let pp_emitter = Arc::clone(emitter);
                let pp_pipeline = pipeline.clone();
                let handle = std::thread::spawn(move || {
                    crate::commands::postprocess_cmd::run_postprocess_for_path(
                        &output_path,
                        &pp_pipeline,
                        &pp_emitter,
                        &pp_state,
                    );
                });
                pp_handles.push(handle);
            }
        }
    }

    for handle in pp_handles {
        let _ = handle.join();
    }

    merged_paths
}

pub fn startup_remove_empty_dirs(output_dir: &std::path::Path) {
    if !output_dir.exists() {
        return;
    }

    let removed = remove_empty_dirs_recursive(output_dir, false);
    if removed > 0 {
        tracing::info!(
            "Startup: removed {} empty directories under {:?}",
            removed,
            output_dir
        );
    }
}

fn remove_empty_dirs_recursive(dir: &std::path::Path, remove_self: bool) -> usize {
    let mut removed = 0;

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            removed += remove_empty_dirs_recursive(&path, true);
        }
    }

    if remove_self {
        let is_empty = fs::read_dir(dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if is_empty && fs::remove_dir(dir).is_ok() {
            removed += 1;
        }
    }

    removed
}

fn collect_segment_dirs(dir: &std::path::Path, result: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let has_segments = fs::read_dir(&path)
            .map(|mut e| {
                e.any(|f| {
                    f.ok()
                        .and_then(|f| {
                            f.path()
                                .extension()
                                .and_then(|x| x.to_str())
                                .map(|x| x == "ts")
                                .filter(|&b| b)
                                .map(|_| ())
                        })
                        .is_some()
                })
            })
            .unwrap_or(false);
        if has_segments {
            result.push(path);
        } else {
            collect_segment_dirs(&path, result);
        }
    }
}

fn session_dir_timestamp(path: &std::path::Path) -> String {
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    session_dir_timestamp_from_stem(stem)
}

fn session_dir_timestamp_from_stem(stem: &str) -> String {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 2 {
        let date = parts[parts.len() - 2];
        let time = parts[parts.len() - 1];
        if date.len() == 8
            && time.len() == 6
            && date.chars().all(|c| c.is_ascii_digit())
            && time.chars().all(|c| c.is_ascii_digit())
        {
            return format!("{}_{}", date, time);
        }
    }
    stem.to_string()
}

fn collect_unprocessed_videos(
    dir: &std::path::Path,
    merge_format: &str,
    pp_results: &std::collections::HashMap<String, bool>,
    result: &mut Vec<PathBuf>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_unprocessed_videos(&path, merge_format, pp_results, result);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == merge_format {
                let path_str = path.to_string_lossy().to_string();
                if !pp_results.contains_key(&path_str) {
                    result.push(path);
                }
            }
        }
    }
}
