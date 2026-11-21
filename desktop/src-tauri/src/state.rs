//! 桌面应用全局状态 / Desktop Application Global State
//!
//! 将 backend 的 `AppState`、`RecorderManager`、`StatusMonitor` 封装为 Tauri 托管状态，
//! 同时持有 `AppHandle` 以便 Tauri command 访问。
//!
//! Wraps the backend's `AppState`, `RecorderManager`, and `StatusMonitor`
//! as Tauri-managed state, along with an `AppHandle` for access in Tauri commands.

use parking_lot::RwLock;
use std::sync::Arc;
use stripchat_recorder_lib::{
	config::app_state::AppState, core::emitter::Emitter, platform::monitor::StatusMonitor,
	recording::recorder::RecorderManager,
};

/// Tauri 托管的全局应用状态。
/// Tauri-managed global application state.
pub struct DesktopState {
	/// 应用业务状态 / Application business state
	pub app_state: Arc<AppState>,
	/// 录制管理器 / Recorder manager
	pub recorder: Arc<RecorderManager>,
	/// 主播状态监控器 / Streamer status monitor
	pub monitor: Arc<StatusMonitor>,
	/// 事件发射器（Arc<dyn Emitter>，具体实现为 TauriEmitter）
	/// Event emitter (Arc<dyn Emitter>, backed by TauriEmitter)
	pub emitter: Arc<dyn Emitter>,
	/// 最近一次 check_for_updates_cmd 检查到的可用更新（用于后续安装）。
	/// The latest available update found by check_for_updates_cmd (used for installation).
	pub pending_update: RwLock<Option<tauri_plugin_updater::Update>>,
	/// 转发会话管理器：桌面端"系统播放器打开转发流"的 worker 宿主——
	/// 本地 /stream 监听按需拉起 worker（与 Server 侧同一份实现）。
	/// Relay session manager: hosts the workers behind the desktop "open
	/// relay stream in system player" flow — the local /stream listener
	/// spawns workers on demand (same implementation as the Server side).
	pub relay_manager: Arc<stripchat_recorder_lib::relay::state::RelayManager>,
	/// 本地转发监听端口（懒绑定）：首个 open_relay_stream 调用时在
	/// 127.0.0.1:0 上启动 / Bound lazily on the first open_relay_stream
	/// call — the local relay listener binds to 127.0.0.1:0
	pub relay_port: tokio::sync::OnceCell<u16>,
}
