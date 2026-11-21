//! 录制管理器 / Recording Manager
//!
//! 管理所有主播的录制会话生命周期，包括：
//! - 启动/停止录制（HLS 分片下载 + fMP4 转 TS，转码委托给 `recording::ffmpeg_util`）
//! - 录制完成后自动触发后处理流水线
//!
//! ffmpeg/ffprobe 底层操作见 `recording::ffmpeg_util`；空目录清理见
//! `recording::segment_merge`。
//!
//! 子模块划分 / Submodule layout:
//! - `recorder::lifecycle`：录制会话的启动、停止与文件轮转 / session start/stop and rotation
//! - `recorder::fetch_loop`：录制主循环与分片下载 / recording loop and segment download
//!
//! Manages the lifecycle of all streamer recording sessions, including:
//! - Starting/stopping recordings (HLS segment download + fMP4 to TS; transcoding is
//!   delegated to `recording::ffmpeg_util`)
//! - Automatically triggering the post-processing pipeline after recording completes
//!
//! Low-level ffmpeg/ffprobe operations live in `recording::ffmpeg_util`; empty-directory
//! cleanup lives in `recording::segment_merge`.

mod fetch_loop;
mod lifecycle;

use crate::config::app_state::AppState;
use crate::platform::stripchat::StripchatApi;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// 单个录制会话的状态 / State of a single recording session
#[derive(Debug, Clone)]
pub struct RecordingSession {
	/// 录制会话目录路径（存放 .ts 分片）/ Recording session directory path (stores .ts segments)
	pub dir_path: PathBuf,
	/// 录制开始时间 / Recording start time
	pub started_at: chrono::DateTime<chrono::Local>,
	/// 停止录制的信号发送端 / Sender to signal recording stop
	stop_tx: mpsc::Sender<()>,
}

/// "录制结束/轮转 → 后处理接手"交接状态的 RAII 守卫：drop 时把目录从 waiting_merge_dirs 移除。
/// 覆盖正常返回、流水线为空提前返回和 panic 三种情况，交接状态不会残留。
///
/// RAII guard for the "recording ended/rotated → post-processing takes over" handoff state:
/// removes the directory from waiting_merge_dirs on drop. Covers normal return, the
/// empty-pipeline early return and panics, so the handoff state can never leak.
struct HandoffGuard {
	manager: Arc<RecorderManager>,
	dir: PathBuf,
}

impl Drop for HandoffGuard {
	fn drop(&mut self) {
		self.manager.waiting_merge_dirs.write().remove(&self.dir);
	}
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

/// 响应式播放列表刷新的结果 / Outcome of a reactive playlist refresh
enum PlaylistRefresh {
	/// 拿到（新或相同但经验证可用的）播放列表 URL / Got a playlist URL (new, or identical-but-verified)
	Swapped(String),
	/// 主播当前不可录制（离线/私密等）/ Streamer currently not recordable (offline/private etc.)
	NotRecordable(String),
	/// 可录制但没有返回播放列表 URL（附带当前状态文本）/ Recordable but no playlist URL returned (carries current status text)
	NoUrl(String),
	/// 刷新请求本身失败（附带错误文本）/ The refresh request itself failed (carries error text)
	Failed(String),
}

/// 响应式播放列表刷新：先试轻量 `get_playlist_url`（CDN TLD 竞速，约 1s 完成）
/// 直取新 URL；**仅当 URL 与当前不同才算命中**——`get_playlist_url` 只解析
/// master 列表、从不验证媒体 URL 本身，主播转私密/下播时它会照常返回
/// 同一个 URL；相同 URL 意味着媒体列表仍处于刚失败的状态，不构成进展，
/// 此时继续走慢路径 `get_stream_info` 拿状态诊断（能识别不可录制并正常
/// 结束，而不是拿死 URL 做 300ms 级热循环）。快路径失败也回退慢路径。
///
/// Reactive playlist refresh: try the light `get_playlist_url` first (CDN TLD
/// race, ~1s) for a fresh URL — **only a DIFFERENT url counts as a hit**:
/// `get_playlist_url` merely parses the master playlist and never validates
/// the media URL itself, so when a streamer goes private/offline it happily
/// returns the identical URL. An identical URL means the media playlist is in
/// the same just-failed state — no progress — so fall through to the slow
/// `get_stream_info` path for status diagnostics (which detects
/// not-recordable and exits cleanly instead of hot-looping on a dead URL
/// every ~300ms). Fast-path failure also falls back to the slow path.
async fn refresh_playlist_target(
	api: &StripchatApi,
	username: &str,
	model_id: Option<i64>,
	current_url: &str,
) -> PlaylistRefresh {
	// 快路径：model_id 已知且拿到**不同** URL 才算命中
	// Fast path: only a DIFFERENT url (with known model_id) counts
	if let Some(mid) = model_id
		&& let Ok(url) = api.get_playlist_url(username, mid).await
		&& url != current_url
	{
		return PlaylistRefresh::Swapped(url);
	}
	// 慢路径兜底：cam API 往返换取状态诊断 / Slow fallback: cam-API roundtrip for status diagnostics
	match api.get_stream_info(username, true, model_id, false).await {
		Ok(info) => match info.playlist_url {
			Some(url) => PlaylistRefresh::Swapped(url),
			None if !info.is_recordable => PlaylistRefresh::NotRecordable(info.status),
			None => PlaylistRefresh::NoUrl(info.status),
		},
		Err(e) => PlaylistRefresh::Failed(e.to_string()),
	}
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
	pub fn get_settings(&self) -> crate::config::app_state::Settings {
		self.state.get_settings()
	}

	/// 获取应用状态的共享引用（供 recording 模块内其他子模块，如启动扫描，复用）。
	/// Get a shared reference to the application state (for reuse by other recording
	/// submodules, e.g. startup scanning).
	pub fn app_state(&self) -> Arc<AppState> {
		Arc::clone(&self.state)
	}

	/// 判断指定目录是否处于"录制结束 → 后处理接手"的交接状态（在 waiting_merge_dirs 中）。
	/// Check whether a directory is in the "recording ended → post-processing takes over"
	/// handoff state (present in waiting_merge_dirs).
	pub fn is_pending_handoff(&self, path: &std::path::Path) -> bool {
		self.waiting_merge_dirs.read().contains(path)
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
	pub fn get_active_sessions(&self) -> Vec<(PathBuf, chrono::DateTime<chrono::Local>)> {
		self.sessions
			.read()
			.values()
			.map(|s| (s.dir_path.clone(), s.started_at))
			.collect()
	}
}
