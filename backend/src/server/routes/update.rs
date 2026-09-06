//! 更新检查与下载安装 handler / Update check, download, and install handlers
//!
//! GET  /api/update/info     — 查询最新 Release 信息
//! GET  /api/update/status   — 查询当前下载/安装进度（SSE 重连后前端用于恢复状态）
//! POST /api/update/download — 触发后端下载 zip 并安装（异步，进度通过 SSE 推送）

use crate::server::error::{ApiError, ApiResult};
use crate::server::router::ServerState;
use axum::{Json, extract::State as AxumState};
use serde::Deserialize;

/// GET /api/update/info — 返回当前版本、平台、Docker 状态及最新 Release 信息。
pub async fn get_update_info(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::update::UpdateInfo> {
    let proxy_url = s.app_state.get_settings().api_proxy_url;

    let (release, asset_names) =
        match crate::update::fetch_latest_release_with_assets(proxy_url.as_deref()).await {
            Ok((r, names)) => (Some(r), names),
            Err(_) => (None, vec![]),
        };

    Ok(Json(crate::update::UpdateInfo {
        current_version: crate::update::APP_VERSION.to_string(),
        platform: crate::update::current_platform().to_string(),
        is_docker: crate::update::is_docker(),
        release,
        asset_names,
    }))
}

/// GET /api/update/status — 返回当前更新进度（前端 SSE 重连后用于恢复状态）。
pub async fn get_update_status(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::update::UpdateProgress> {
    let progress = s.app_state.update_state.read().clone();
    Ok(Json(progress))
}

#[derive(Deserialize)]
pub struct StartDownloadBody {
    /// 要下载的 asset URL / Asset URL to download
    pub download_url: String,
}

/// POST /api/update/download — 在后台下载并安装更新。
///
/// 立即返回 202 Accepted；实际下载/安装进度通过 SSE `update-progress` 事件推送。
/// 若当前已有下载任务进行中，返回 400 阻止重复触发。
///
/// Returns 202 Accepted immediately; download/install progress is pushed via SSE.
/// Returns 400 if a download is already in progress to prevent duplicate triggers.
pub async fn start_download(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<StartDownloadBody>,
) -> ApiResult<serde_json::Value> {
    use crate::update::UpdateProgress;

    // 防止重复触发 / prevent duplicate triggers
    {
        let state = s.app_state.update_state.read();
        match *state {
            UpdateProgress::Downloading { .. } | UpdateProgress::Installing => {
                return Err(ApiError("更新下载已在进行中".to_string()));
            }
            _ => {}
        }
    }

    let proxy_url = s.app_state.get_settings().api_proxy_url;
    let state_store = std::sync::Arc::clone(&s.app_state.update_state);
    let emitter = std::sync::Arc::clone(&s.emitter);

    // 异步启动，立即返回 / spawn async task, return immediately
    tokio::spawn(crate::update::download_and_install(
        body.download_url,
        proxy_url,
        state_store,
        emitter,
    ));

    Ok(Json(serde_json::json!({ "ok": true })))
}
