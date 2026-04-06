//! 事件发射器抽象层 / Event Emitter Abstraction Layer
//!
//! 定义统一的 `Emitter` trait，使录制器、监控器等核心模块可以在不依赖具体运行时
//! （Tauri / HTTP 服务器）的情况下发送事件。
//!
//! Defines a unified `Emitter` trait so that core modules (recorder, monitor, etc.)
//! can emit events without depending on a specific runtime (Tauri / HTTP server).

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 原始事件数据（事件名 + JSON 载荷字符串）。
/// Raw event data (event name + JSON payload string).
#[derive(Debug, Clone)]
pub struct Event {
    /// 事件名称 / Event name
    pub name: String,
    /// JSON 序列化后的载荷字符串 / JSON-serialized payload string
    pub payload: String,
}

/// 事件发射器 trait：将事件广播给所有订阅者。
/// Event emitter trait: broadcasts events to all subscribers.
pub trait Emitter: Send + Sync + 'static {
    /// 发送原始 JSON 字符串载荷的事件。
    /// Emit an event with a raw JSON string payload.
    fn emit_raw(&self, event: &str, payload: &str);
}

/// 将可序列化的载荷序列化为 JSON 后调用 `emit_raw`。
/// Serializes a serializable payload to JSON and calls `emit_raw`.
#[allow(dead_code)]
pub fn emit<T: Serialize>(emitter: &dyn Emitter, event: &str, payload: &T) {
    match serde_json::to_string(payload) {
        Ok(s) => emitter.emit_raw(event, &s),
        Err(e) => tracing::error!("emit serialize error: {}", e),
    }
}

/// 为所有实现了 `Emitter` 的类型提供泛型 `emit` 方法的扩展 trait。
/// Extension trait providing a generic `emit` method for all `Emitter` implementors.
pub trait EmitterExt {
    /// 将载荷序列化为 JSON 并发送事件。
    /// Serialize the payload to JSON and emit the event.
    fn emit<T: Serialize>(&self, event: &str, payload: &T);
}

impl<E: Emitter + ?Sized> EmitterExt for E {
    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        match serde_json::to_string(payload) {
            Ok(s) => self.emit_raw(event, &s),
            Err(e) => tracing::error!("emit serialize error: {}", e),
        }
    }
}

/// Tauri 桌面模式的事件发射器，通过 `tauri::AppHandle` 向前端发送事件。
/// Tauri desktop mode emitter; sends events to the frontend via `tauri::AppHandle`.
pub struct TauriEmitter(pub tauri::AppHandle);

impl Emitter for TauriEmitter {
    fn emit_raw(&self, event: &str, payload: &str) {
        use tauri::Emitter as TauriEmitterTrait;
        // 将 JSON 字符串解析为 Value，确保前端收到的是结构化数据而非字符串
        // Parse JSON string to Value so the frontend receives structured data, not a raw string
        let val: serde_json::Value =
            serde_json::from_str(payload).unwrap_or(serde_json::Value::Null);
        let _ = self.0.emit(event, val);
    }
}

/// HTTP 服务器模式的事件发射器，通过 `broadcast::Sender` 将事件推送到 SSE 流。
/// HTTP server mode emitter; pushes events to the SSE stream via `broadcast::Sender`.
#[derive(Clone)]
pub struct BroadcastEmitter(pub broadcast::Sender<Event>);

impl Emitter for BroadcastEmitter {
    fn emit_raw(&self, event: &str, payload: &str) {
        let _ = self.0.send(Event {
            name: event.to_string(),
            payload: payload.to_string(),
        });
    }
}

/// 空操作发射器，用于测试或不需要事件通知的场景。
/// No-op emitter for testing or scenarios where event notifications are not needed.
#[allow(dead_code)]
pub struct NoopEmitter;

impl Emitter for NoopEmitter {
    fn emit_raw(&self, _event: &str, _payload: &str) {}
}

/// 允许将 `Arc<dyn Emitter>` 直接作为 `Emitter` 使用。
/// Allows `Arc<dyn Emitter>` to be used directly as an `Emitter`.
impl Emitter for Arc<dyn Emitter> {
    fn emit_raw(&self, event: &str, payload: &str) {
        (**self).emit_raw(event, payload)
    }
}
