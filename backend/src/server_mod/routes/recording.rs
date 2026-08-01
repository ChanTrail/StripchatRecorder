//! 录制操作路由 handler / Recording operation route handlers

use crate::core::emitter::EmitterExt;
use crate::server_mod::error::{ApiError, ApiResult};
use crate::server_mod::server::ServerState;
use axum::{
    Json,
    extract::{Query, State as AxumState},
    http::{StatusCode, header},
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn list_recordings(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let state = Arc::clone(&s.app_state);
    let recorder = Arc::clone(&s.recorder);
    let files = tokio::task::spawn_blocking(move || {
        crate::commands::recording_cmd::list_recordings_inner(&state, &recorder)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(|e| ApiError(e.to_string()))?;
    Ok(Json(serde_json::to_value(files).unwrap()))
}

pub async fn get_merging_dirs_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let make_entry = |path: &std::path::PathBuf, status: &str| {
        let path_str = path.to_string_lossy().to_string();
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        serde_json::json!({
            "session_dir": path_str,
            "username": username,
            "stem": stem,
            "status": status,
        })
    };

    let mut result: Vec<serde_json::Value> = s
        .recorder
        .merging_dirs
        .read()
        .iter()
        .map(|p| make_entry(p, "merging"))
        .collect();
    result.extend(
        s.recorder
            .waiting_merge_dirs
            .read()
            .iter()
            .map(|p| make_entry(p, "waiting")),
    );
    Ok(Json(serde_json::json!(result)))
}

#[derive(Deserialize)]
pub struct PathBody {
    pub path: String,
}

pub async fn delete_recording(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let recorder = Arc::clone(&s.recorder);
    let state = Arc::clone(&s.app_state);
    let path = body.path.clone();
    tokio::task::spawn_blocking(move || {
        crate::commands::recording_cmd::delete_recording_inner(&path, &recorder, &state)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(ApiError::from)?;
    s.emitter.emit(
        "recording-deleted",
        &serde_json::json!({ "path": body.path }),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn open_recording(Json(body): Json<PathBody>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "path": body.path })))
}

pub async fn open_output_dir(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    Ok(Json(serde_json::json!({ "path": settings.output_dir })))
}

#[derive(Deserialize)]
pub struct FileQuery {
    pub path: String,
}

pub async fn serve_output_file(
    AxumState(s): AxumState<ServerState>,
    Query(q): Query<FileQuery>,
) -> impl IntoResponse {
    let settings = s.app_state.get_settings();
    let output_dir = std::path::Path::new(&settings.output_dir);
    let requested = std::path::Path::new(&q.path);

    let canonical_output = match output_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "output dir error").into_response(),
    };
    let canonical_requested = match requested.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };
    if !canonical_requested.starts_with(&canonical_output) {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let data = match std::fs::read(&canonical_requested) {
        Ok(d) => d,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };

    let ext = canonical_requested
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mime = match ext {
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => "application/octet-stream",
    };

    ([(header::CONTENT_TYPE, mime)], data).into_response()
}
