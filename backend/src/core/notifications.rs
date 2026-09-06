//! 进程内通知存储 / In-process Notification Store
//!
//! 提供轻量级内存通知队列，收集各定时任务产生的有意义事件。
//! 不持久化——进程重启后清空，但后续上线的用户仍可通过
//! GET /api/notifications 拉取本次进程运行期间积累的未读通知。
//!
//! Provides a lightweight in-memory notification queue that collects meaningful events
//! from scheduled tasks. Not persisted — cleared on restart — but users who connect
//! later in the same process lifetime can still pull unread notifications via
//! GET /api/notifications.

use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::core::emitter::EmitterExt;

/// 通知级别 / Notification level
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    /// 信息 / Informational
    Info,
    /// 警告 / Warning
    Warning,
    /// 错误 / Error
    Error,
}

/// 通知可携带的操作描述，前端据此渲染操作按钮。
/// Optional action attached to a notification; frontend renders an action button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAction {
    /// 操作类型（前端 switch-case 的 key）/ Action type (frontend switch-case key)
    pub action_type: String,
    /// 操作的字符串列表参数（如主播名列表、文件路径列表）
    /// String list parameter for the action (e.g. streamer usernames, file paths)
    #[serde(default)]
    pub targets: Vec<String>,
}

/// 单条通知 / A single notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// 唯一 ID（单调递增）/ Unique ID (monotonically increasing)
    pub id: u64,
    /// 通知级别 / Level
    pub level: NotificationLevel,
    /// 事件来源标签，用于前端图标/颜色区分 / Source tag for frontend icon/color distinction
    pub source: String,
    /// 消息内容（纯文本，作为 fallback）/ Message content (plain text, used as fallback)
    pub message: String,
    /// i18n 翻译键（前端优先用此键查翻译，未设置时回退到 message）
    /// i18n translation key (frontend uses this first; falls back to `message` when absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_key: Option<String>,
    /// i18n 插值参数（配合 message_key 使用）/ i18n interpolation arguments (used with message_key)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_args: Option<HashMap<String, serde_json::Value>>,
    /// 发生时间（RFC 3339）/ Timestamp (RFC 3339)
    pub created_at: String,
    /// 可选操作（有值时前端显示操作按钮）/ Optional action (frontend shows action button when present)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<NotificationAction>,
}

/// 进程内通知存储 / In-process notification store
pub struct NotificationStore {
    inner: RwLock<StoreInner>,
}

struct StoreInner {
    notifications: Vec<Notification>,
    next_id: u64,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(StoreInner {
                notifications: Vec::new(),
                next_id: 1,
            }),
        }
    }

    /// 推入一条新通知，返回克隆。
    /// Push a new notification and return a clone.
    pub fn push(
        &self,
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
    ) -> Notification {
        self.push_with_action(level, source, message, None)
    }

    /// 推入一条带操作的通知，返回克隆。
    /// Push a notification with an action attached, return a clone.
    pub fn push_with_action(
        &self,
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
        action: Option<NotificationAction>,
    ) -> Notification {
        let mut inner = self.inner.write();
        let id = inner.next_id;
        inner.next_id += 1;
        let n = Notification {
            id,
            level,
            source: source.into(),
            message: message.into(),
            message_key: None,
            message_args: None,
            created_at: Utc::now().to_rfc3339(),
            action,
        };
        inner.notifications.push(n.clone());
        n
    }

    /// 返回所有未读通知的克隆列表（按 id 升序）。
    /// Return a cloned list of all unread notifications (ascending by id).
    pub fn list(&self) -> Vec<Notification> {
        self.inner.read().notifications.clone()
    }

    /// 将给定 ID 集合中的通知标记为已读（从内存移除）。
    /// Mark the given notification IDs as read (removes them from memory).
    pub fn mark_read(&self, ids: &[u64]) {
        let mut inner = self.inner.write();
        inner.notifications.retain(|n| !ids.contains(&n.id));
    }

    /// 清除所有通知。
    /// Clear all notifications.
    pub fn clear_all(&self) {
        self.inner.write().notifications.clear();
    }

    /// 返回未读通知数量。
    /// Return the count of unread notifications.
    pub fn unread_count(&self) -> usize {
        self.inner.read().notifications.len()
    }

    /// 推入一条通知，并通过 SSE emitter 广播 `notification-created` 事件。
    /// 这是 `push` + `emitter.emit("notification-created")` 的统一封装。
    ///
    /// Push a notification and broadcast `notification-created` via SSE emitter.
    /// This is the unified wrapper for `push` + `emitter.emit("notification-created")`.
    pub fn emit(
        &self,
        emitter: &(impl EmitterExt + ?Sized),
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
    ) {
        let n = self.push(level, source, message);
        emitter.emit("notification-created", &n);
    }

    /// 推入一条带操作的通知，并通过 SSE emitter 广播 `notification-created` 事件。
    ///
    /// Push a notification with an action and broadcast `notification-created` via SSE emitter.
    pub fn emit_with_action(
        &self,
        emitter: &(impl EmitterExt + ?Sized),
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
        action: Option<NotificationAction>,
    ) -> Notification {
        let n = self.push_with_action(level, source, message, action);
        emitter.emit("notification-created", &n);
        n
    }

    /// 推入一条带 i18n 翻译键的通知，并广播 `notification-created`。
    /// `message` 作为 fallback，`message_key` + `message_args` 供前端查翻译。
    ///
    /// Push a notification with an i18n key, broadcast `notification-created`.
    /// `message` is the fallback; `message_key` + `message_args` are used by the frontend.
    pub fn emit_i18n(
        &self,
        emitter: &(impl EmitterExt + ?Sized),
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
        message_key: impl Into<String>,
        message_args: Option<HashMap<String, serde_json::Value>>,
    ) {
        self.emit_i18n_with_action(emitter, level, source, message, message_key, message_args, None);
    }

    /// 推入一条带 i18n 翻译键和操作的通知，并广播 `notification-created`。
    ///
    /// Push a notification with an i18n key and action, broadcast `notification-created`.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_i18n_with_action(
        &self,
        emitter: &(impl EmitterExt + ?Sized),
        level: NotificationLevel,
        source: impl Into<String>,
        message: impl Into<String>,
        message_key: impl Into<String>,
        message_args: Option<HashMap<String, serde_json::Value>>,
        action: Option<NotificationAction>,
    ) -> Notification {
        let mut inner = self.inner.write();
        let id = inner.next_id;
        inner.next_id += 1;
        let n = Notification {
            id,
            level,
            source: source.into(),
            message: message.into(),
            message_key: Some(message_key.into()),
            message_args,
            created_at: Utc::now().to_rfc3339(),
            action,
        };
        inner.notifications.push(n.clone());
        drop(inner);
        emitter.emit("notification-created", &n);
        n
    }
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}
