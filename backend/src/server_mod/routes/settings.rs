//! 设置、Mouflon Keys、启动警告、磁盘空间 handler
//! Settings, Mouflon Keys, startup warnings, and disk space handlers

use crate::core::emitter::EmitterExt;
use crate::server_mod::error::{ApiError, ApiResult};
use crate::server_mod::server::ServerState;
use axum::{
    Json,
    extract::{Path, State as AxumState},
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn get_settings(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::config::settings::Settings> {
    Ok(Json(s.app_state.get_settings()))
}

pub async fn save_settings(
    AxumState(s): AxumState<ServerState>,
    Json(new_settings): Json<crate::config::settings::Settings>,
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

pub async fn get_startup_warnings_handler(
    AxumState(_s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    // 启动警告（missing_streamers）由 run_config_check 异步推送，此接口仅返回空占位
    // Startup warnings (missing_streamers) are pushed asynchronously by run_config_check;
    // this endpoint returns an empty placeholder
    Ok(Json(serde_json::json!({
        "missing_streamers": [],
    })))
}

pub async fn get_disk_space_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let state = Arc::clone(&s.app_state);
    let result = tokio::task::spawn_blocking(move || {
        crate::commands::settings_cmd::get_disk_space_inner(&state.get_settings().output_dir)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))??;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

#[derive(Deserialize)]
pub struct ListDirQuery {
    #[serde(default)]
    pub path: String,
}

/// 列出指定路径下的子目录，供前端目录浏览器使用。
/// List subdirectories under the given path, for the frontend directory browser.
pub async fn list_dir_handler(
    axum::extract::Query(q): axum::extract::Query<ListDirQuery>,
) -> ApiResult<serde_json::Value> {
    let path = q.path;
    let result = tokio::task::spawn_blocking(move || {
        crate::commands::settings_cmd::list_dir_inner(&path)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

#[derive(Deserialize)]
pub struct CreateDirBody {
    pub parent: String,
    pub name: String,
}

/// 在指定路径下创建新子目录，供前端目录浏览器的"新建文件夹"使用。
/// Create a new subdirectory under the given path, for the frontend directory browser's "new folder" action.
pub async fn create_dir_handler(
    Json(body): Json<CreateDirBody>,
) -> ApiResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(move || {
        crate::commands::settings_cmd::create_dir_inner(&body.parent, &body.name)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "path": result })))
}

/// 列出系统所有可用驱动器（"此电脑"），供前端目录浏览器的顶层导航使用。
/// List all available system drives ("This PC"), for the frontend directory browser's top-level navigation.
pub async fn list_drives_handler() -> ApiResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(crate::commands::settings_cmd::list_drives_inner)
        .await
        .map_err(|e| ApiError(e.to_string()))?
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
