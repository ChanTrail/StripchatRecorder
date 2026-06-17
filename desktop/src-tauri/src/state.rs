//! 桌面应用全局状态 / Desktop Application Global State
//!
//! 将 backend 的 `AppState`、`RecorderManager`、`StatusMonitor` 封装为 Tauri 托管状态，
//! 同时持有 `AppHandle` 以便 Tauri command 访问。
//!
//! Wraps the backend's `AppState`, `RecorderManager`, and `StatusMonitor`
//! as Tauri-managed state, along with an `AppHandle` for access in Tauri commands.

use std::sync::Arc;
use stripchat_recorder_lib::{
    config::settings::AppState,
    core::emitter::Emitter,
    recording::recorder::RecorderManager,
    streaming::monitor::StatusMonitor,
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
}
