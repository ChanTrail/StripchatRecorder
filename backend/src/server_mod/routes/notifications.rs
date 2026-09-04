//! 通知 API handlers / Notification API handlers

use crate::server_mod::error::ApiResult;
use crate::server_mod::server::ServerState;
use axum::{Json, extract::State as AxumState};
use serde::Deserialize;

/// GET /api/notifications — 返回所有未读通知。
/// GET /api/notifications — Return all unread notifications.
pub async fn list_notifications(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let notifications = s.app_state.notification_store.list();
    Ok(Json(serde_json::json!({
        "notifications": notifications,
        "unread_count": notifications.len(),
    })))
}

#[derive(Deserialize)]
pub struct MarkReadBody {
    /// 要标记为已读的通知 ID 列表（空列表 = 全部标记）
    /// List of notification IDs to mark as read (empty = mark all)
    pub ids: Vec<u64>,
}

/// POST /api/notifications/read — 标记指定通知（或全部）为已读。
/// POST /api/notifications/read — Mark specified (or all) notifications as read.
pub async fn mark_notifications_read(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<MarkReadBody>,
) -> ApiResult<serde_json::Value> {
    if body.ids.is_empty() {
        // 空 ids = 全部清除 / Empty ids = clear all
        s.app_state.notification_store.clear_all();
    } else {
        s.app_state.notification_store.mark_read(&body.ids);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
