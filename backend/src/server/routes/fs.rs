//! 文件系统工具 handler（目录浏览、磁盘空间）
//! File system utility handlers (directory browser, disk space)

use crate::server::error::{ApiError, ApiResult};
use crate::server::router::ServerState;
use axum::{Json, extract::State as AxumState};
use serde::Deserialize;

pub async fn get_disk_space_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let state = std::sync::Arc::clone(&s.app_state);
    let result = tokio::task::spawn_blocking(move || {
        crate::system::disk::get_disk_space_inner(&state.get_settings().output_dir)
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
        crate::system::fs_browser::list_dir_inner(&path)
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
        crate::system::fs_browser::create_dir_inner(&body.parent, &body.name)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "path": result })))
}

/// 列出系统所有可用驱动器（"此电脑"），供前端目录浏览器的顶层导航使用。
/// List all available system drives ("This PC"), for the frontend directory browser's top-level navigation.
pub async fn list_drives_handler() -> ApiResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(crate::system::fs_browser::list_drives_inner)
        .await
        .map_err(|e| ApiError(e.to_string()))?
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}
