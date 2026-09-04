//! 设置与 Mouflon Keys handler
//! Settings and Mouflon Keys handlers

use crate::core::emitter::EmitterExt;
use crate::server::error::{ApiError, ApiResult};
use crate::server::router::ServerState;
use axum::{
    Json,
    extract::{Path, State as AxumState},
};
use serde::Deserialize;

pub async fn get_settings(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::config::app_state::Settings> {
    Ok(Json(s.app_state.get_settings()))
}

pub async fn save_settings(
    AxumState(s): AxumState<ServerState>,
    Json(new_settings): Json<crate::config::app_state::Settings>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .update_settings(new_settings)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("settings-updated", &s.app_state.get_settings());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_mouflon_keys(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(s.app_state.get_mouflon_keys_store()).unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct MouflonKeyBody {
    pub pkey: String,
    pub pdkey: String,
}

pub async fn add_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<MouflonKeyBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .add_mouflon_key(&body.pkey, &body.pdkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Path(pkey): Path<String>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .remove_mouflon_key(&pkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 手动触发一次 Mouflon Keys 从 Worker 同步（忽略时间间隔，强制比对 updated_at）。
/// Manually trigger a Mouflon Keys sync from the Worker (bypasses interval, still compares updated_at).
pub async fn sync_mouflon_keys(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    let url = settings
        .mouflon_sync_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError("未配置 mouflon_sync_url".into()))?
        .to_string();
    let token = settings.mouflon_sync_token.clone();

    let updated = s
        .app_state
        .sync_mouflon_keys_from_worker(&url, token.as_deref())
        .await
        .map_err(ApiError::from)?;

    if updated {
        s.emitter
            .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    }

    Ok(Json(serde_json::json!({ "updated": updated })))
}
