//! Tauri 事件发射器 / Tauri Event Emitter
//!
//! 实现 backend 的 `Emitter` trait，将事件通过 Tauri 的 `AppHandle::emit` 广播给前端窗口。
//! Implements the backend `Emitter` trait, broadcasting events to frontend windows
//! via Tauri's `AppHandle::emit`.

use stripchat_recorder_lib::core::emitter::Emitter;
use tauri::AppHandle;
use tauri::Emitter as TauriEmitterTrait;

/// Tauri 模式的事件发射器。
/// Event emitter for Tauri mode.
pub struct TauriEmitter {
    app: AppHandle,
}

impl TauriEmitter {
    /// 创建新的 TauriEmitter 实例。
    /// Create a new TauriEmitter instance.
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl Emitter for TauriEmitter {
    fn emit_raw(&self, event: &str, payload: &str) {
        // Tauri emit 需要一个可序列化的值；payload 已经是 JSON 字符串，
        // 反序列化为 serde_json::Value 后传递，避免字符串被二次转义。
        // Tauri emit needs a serializable value; payload is already a JSON string,
        // deserialize to serde_json::Value to avoid double-escaping.
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(payload) {
            let _ = TauriEmitterTrait::emit(&self.app, event, raw);
        } else {
            let _ = TauriEmitterTrait::emit(&self.app, event, payload);
        }
    }
}
