//! 后处理路由 handler / Post-processing route handlers

use crate::core::emitter::EmitterExt;
use crate::server_mod::error::{ApiError, ApiResult};
use crate::server_mod::routes::recording::PathBody;
use crate::server_mod::server::ServerState;
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
    let initial_path = crate::commands::postprocess_cmd::infer_initial_path(&video_path);
    let emitter = Arc::clone(&s.emitter);
    let state = Arc::clone(&s.app_state);
    tokio::task::spawn_blocking(move || {
        crate::commands::postprocess_cmd::run_postprocess_for_path(
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
        let initial_path = crate::commands::postprocess_cmd::infer_initial_path(&video_path);
        let emitter = Arc::clone(&s.emitter);
        let state = Arc::clone(&s.app_state);
        let pipeline = pipeline.clone();
        tokio::task::spawn_blocking(move || {
            crate::commands::postprocess_cmd::run_postprocess_for_path(
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

pub async fn get_module_outputs(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let video_path = std::path::Path::new(&body.path);
    let pipeline = s.app_state.get_pipeline();
    let mut outputs: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for node in &pipeline.nodes {
        if !node.enabled {
            continue;
        }
        if node.module_id == "contact_sheet" {
            let format = node
                .params
                .get("format")
                .and_then(|v: &serde_json::Value| v.as_str())
                .unwrap_or("webp");
            if let (Some(parent), Some(stem)) = (
                video_path.parent(),
                video_path.file_stem().and_then(|s| s.to_str()),
            ) {
                let img_path = parent.join(format!("{}.{}", stem, format));
                if img_path.exists() {
                    outputs.insert(
                        node.module_id.clone(),
                        img_path.to_string_lossy().to_string(),
                    );
                }
            }
        }
    }

    Ok(Json(serde_json::to_value(outputs).unwrap()))
}
