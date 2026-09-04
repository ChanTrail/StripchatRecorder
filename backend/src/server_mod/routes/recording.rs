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
        crate::recording::service::list_recordings_inner(&state, &recorder)
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
        crate::recording::service::delete_recording_inner(&path, &recorder, &state)
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
    /// 录制的视频路径（meta 的主键，与 `RecordingFile.path` 一致）
    /// The recording's video path (meta's primary key, matches `RecordingFile.path`)
    pub video_path: String,
    /// 要获取的模块输出所属的 module_id（如 "contact_sheet"）
    /// The module_id whose output is being requested (e.g. "contact_sheet")
    pub module_id: String,
}

/// 提供模块输出文件（如 contact_sheet 生成的预览图）的静态文件服务。
///
/// 不接受任意文件路径作为输入，而是接受 `(video_path, module_id)`，在服务端通过
/// meta 查出该模块此刻真正的、已验证存在的输出路径（与 `RecordingFile.module_outputs`
/// / SSE `postprocess-meta-update.module_outputs` 完全同一套逻辑，见
/// [`crate::recording::meta::extract_verified_module_outputs`]）。
///
/// 这样设计的原因：此前的实现接受任意 `path` 参数，用 `canonicalize` +
/// `starts_with(output_dir)` 校验请求路径必须落在配置的输出目录内——但 ts_merge
/// 等模块支持自定义输出目录参数（`output_dir`），一旦用户配置了与 `settings.output_dir`
/// 不同的合并输出目录，contact_sheet 等下游模块的产物自然也会落在这个自定义目录下，
/// 导致校验以"access denied"失败，预览图无法加载。改为通过 meta 解析而非直接校验
/// 目录边界后，不再关心文件实际存放在哪个目录，只关心它是否是某次录制某个模块
/// 当前真实存在的合法输出——从根源上不受任何自定义输出目录设置的影响。
///
/// Serves a static file for a module's output (e.g. contact_sheet's generated preview
/// image).
///
/// Does not accept an arbitrary file path as input; instead accepts
/// `(video_path, module_id)`, and resolves that module's currently real,
/// verified-to-exist output path server-side via meta (the exact same logic as
/// `RecordingFile.module_outputs` / the SSE `postprocess-meta-update.module_outputs`,
/// see [`crate::recording::meta::extract_verified_module_outputs`]).
///
/// Why this design: the previous implementation accepted an arbitrary `path` parameter
/// and validated it with `canonicalize` + `starts_with(output_dir)`, requiring the
/// requested path to fall within the configured output directory — but modules like
/// ts_merge support a custom output directory param (`output_dir`); once a user
/// configures a merge output directory different from `settings.output_dir`, downstream
/// modules like contact_sheet naturally produce their output under that custom
/// directory too, causing the boundary check to fail with "access denied" and the
/// preview to never load. By resolving through meta instead of validating a directory
/// boundary directly, it no longer matters which directory the file actually lives in —
/// only whether it's a genuinely current, legitimate output of a specific module for a
/// specific recording — which is unaffected by any custom output directory setting.
pub async fn serve_output_file(Query(q): Query<FileQuery>) -> impl IntoResponse {
    let video_path = std::path::Path::new(&q.video_path);
    let meta = match crate::recording::meta::read_meta(video_path) {
        Some(m) => m,
        None => return (StatusCode::NOT_FOUND, "recording not found").into_response(),
    };
    let outputs = crate::recording::meta::extract_verified_module_outputs(
        meta.pp_execution.as_deref(),
    );
    let output_path = match outputs.get(&q.module_id) {
        Some(p) => p,
        None => return (StatusCode::NOT_FOUND, "module output not found").into_response(),
    };

    let data = match std::fs::read(output_path) {
        Ok(d) => d,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };

    let ext = std::path::Path::new(output_path)
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
