//! 后处理路由 handler / Post-processing route handlers

use crate::core::emitter::EmitterExt;
use crate::server::error::{ApiError, ApiResult};
use crate::server::routes::recording::PathBody;
use crate::server::router::ServerState;
use axum::{
    Json,
    extract::State as AxumState,
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn run_postprocess(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let pipeline = s.app_state.get_pipeline();
    if !pipeline.nodes.iter().any(|n| n.enabled) {
        return Err(ApiError("后处理流水线为空".into()));
    }
    let video_path = std::path::PathBuf::from(&body.path);
    let initial_path = crate::postprocess::service::infer_initial_path(&video_path);
    let emitter = Arc::clone(&s.emitter);
    let state = Arc::clone(&s.app_state);
    tokio::task::spawn_blocking(move || {
        crate::postprocess::service::run_postprocess_for_path(
            &initial_path,
            &video_path,
            &pipeline,
            &emitter,
            &state,
        );
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct BatchPathBody {
    pub paths: Vec<String>,
}

pub async fn run_postprocess_batch(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<BatchPathBody>,
) -> ApiResult<serde_json::Value> {
    let pipeline = s.app_state.get_pipeline();
    if !pipeline.nodes.iter().any(|n| n.enabled) {
        return Err(ApiError("后处理流水线为空".into()));
    }
    for path in body.paths {
        let video_path = std::path::PathBuf::from(&path);
        let initial_path = crate::postprocess::service::infer_initial_path(&video_path);
        let emitter = Arc::clone(&s.emitter);
        let state = Arc::clone(&s.app_state);
        let pipeline = pipeline.clone();
        tokio::task::spawn_blocking(move || {
            crate::postprocess::service::run_postprocess_for_path(
                &initial_path,
                &video_path,
                &pipeline,
                &emitter,
                &state,
            );
        });
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn cancel_postprocess(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state.pp_queue.cancel(&body.path);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_pipeline(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::postprocess::pipeline::PipelineConfig> {
    Ok(Json(s.app_state.get_pipeline()))
}

pub async fn save_pipeline(
    AxumState(s): AxumState<ServerState>,
    Json(pipeline): Json<crate::postprocess::pipeline::PipelineConfig>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .update_pipeline(pipeline)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("pipeline-updated", &s.app_state.get_pipeline());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_modules() -> ApiResult<serde_json::Value> {
    let modules: Vec<crate::postprocess::pipeline::ModuleInfo> =
        tokio::task::spawn_blocking(crate::postprocess::pipeline::discover_modules)
            .await
            .unwrap_or_default();
    Ok(Json(serde_json::to_value(modules).unwrap()))
}

pub async fn get_postprocess_tasks(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(s.app_state.pp_queue.get_all_tasks()).unwrap(),
    ))
}

/// 获取指定视频的模块输出路径（如 contact_sheet 生成的预览图路径），供前端展示
/// 预览按钮等 UI。
///
/// 数据来源为 meta 中的真实执行记录（`pp_execution[].outputs`），而非按模块参数
/// 自行拼接/猜测路径——只有 `result.code == "ok"` 的节点才被视为产出了有效输出
/// （`done` 表示无输出即终止、`error`/`cancelled` 表示失败、`skipped` 表示未执行），
/// 避免向前端返回未经确认成功、或与实际参数（如 contact_sheet 的 format 参数
/// 被后续修改）不一致的路径。
///
/// Get module output paths for a video (e.g. contact_sheet's generated preview image
/// path), for frontend UI elements like preview buttons.
///
/// The data source is the real execution record in meta (`pp_execution[].outputs`),
/// not a path assembled/guessed from module params — only nodes with
/// `result.code == "ok"` are treated as having produced valid output (`done` means no
/// output/pipeline terminated, `error`/`cancelled` mean failure, `skipped` means not
/// executed), avoiding returning paths that were never confirmed successful or that
/// no longer match the actual params (e.g. contact_sheet's `format` param changed since).
pub async fn get_module_outputs(
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let video_path = std::path::Path::new(&body.path);
    let meta = crate::recording::meta::read_meta(video_path);
    let outputs = crate::recording::meta::extract_verified_module_outputs(
        meta.as_ref().and_then(|m| m.pp_execution.as_deref()),
    );
    Ok(Json(serde_json::to_value(outputs).unwrap()))
}
