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

// ─── 社区模块 / Community Modules ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct UninstallModuleBody {
    /// 要卸载的模块 ID / Module ID to uninstall
    pub module_id: String,
}

/// 获取当前正在安装的社区模块列表（模块 ID → 已下载字节数）。
///
/// 前端重连 SSE 后调用此接口恢复安装中的状态，避免刷新页面后进度消失。
///
/// Get the list of currently in-progress community module installs (module ID → downloaded bytes).
///
/// The frontend calls this on SSE reconnect to restore in-progress state,
/// preventing progress from disappearing after a page refresh.
pub async fn get_install_tasks(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let tasks: std::collections::HashMap<String, u64> =
        s.app_state.install_tasks.read().clone();
    Ok(Json(serde_json::to_value(tasks).unwrap()))
}

/// 安装指定社区模块（下载 + sha256 校验 + 写入 modules/ 目录）。
///
/// 前端负责拉取 registry，将完整的模块数据传入，后端只负责文件下载和写入。
/// 安装状态写入 AppState.install_tasks，前端重连后可通过 GET /api/community-modules/tasks 恢复。
/// 安装完成或失败后推送 SSE 事件 `community-module-install-done`。
///
/// Install a community module (download + sha256 verify + write to modules/).
///
/// The frontend fetches the registry and passes the complete module data.
/// Install state is stored in AppState.install_tasks; frontend can restore it via
/// GET /api/community-modules/tasks after reconnect.
/// Pushes SSE event `community-module-install-done` on completion or failure.
pub async fn install_community_module(
    AxumState(s): AxumState<ServerState>,
    Json(module): Json<crate::postprocess::RegistryModule>,
) -> ApiResult<serde_json::Value> {
    use crate::postprocess::community;

    let settings   = s.app_state.get_settings();
    let proxy_url  = settings.community_proxy_url.clone();
    let mirror_url = settings.community_mirror_url.clone();
    let emitter    = Arc::clone(&s.emitter);
    let module_id  = module.id.clone();
    let app_state  = Arc::clone(&s.app_state);
    let app_state2 = Arc::clone(&s.app_state);

    // 写入安装任务表，记录初始已下载字节数为 0 / Record install task with 0 downloaded bytes
    app_state.install_tasks.write().insert(module_id.clone(), 0);

    let result = community::install_module(&module, proxy_url, mirror_url, |downloaded, total| {
        // 更新内存中的已下载字节数 / Update in-memory downloaded bytes
        app_state2.install_tasks.write().insert(module_id.clone(), downloaded);

        let pct = if total > 0 {
            (downloaded as f64 / total as f64 * 100.0).min(100.0)
        } else {
            -1.0
        };
        emitter.emit(
            "community-module-download-progress",
            &serde_json::json!({
                "moduleId":   module_id,
                "downloaded": downloaded,
                "total":      total,
                "pct":        pct,
            }),
        );
    })
    .await;

    // 无论成功失败都从任务表中删除 / Remove from task map regardless of outcome
    app_state.install_tasks.write().remove(&module.id);

    match result {
        Ok(()) => {
            s.emitter.emit(
                "community-module-install-done",
                &serde_json::json!({ "moduleId": module.id, "success": true }),
            );
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        Err(e) => {
            tracing::error!("社区模块 '{}' 安装失败: {}", module.id, e);
            s.emitter.emit(
                "community-module-install-done",
                &serde_json::json!({ "moduleId": module.id, "success": false, "error": e.to_string() }),
            );
            Err(ApiError(e.to_string()))
        }
    }
}

/// 卸载指定社区模块（从 modules/ 目录删除对应可执行文件）。
///
/// 幂等操作：若模块未安装，静默成功。
///
/// Uninstall a community module (remove the executable from modules/ directory).
///
/// Idempotent: silently succeeds if the module is not installed.
pub async fn uninstall_community_module(
    Json(body): Json<UninstallModuleBody>,
) -> ApiResult<serde_json::Value> {
    use crate::postprocess::community;

    community::uninstall_module(&body.module_id)
        .map_err(|e| ApiError(e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}
