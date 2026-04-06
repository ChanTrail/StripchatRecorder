//! 后处理流水线 Tauri 命令 / Post-processing Pipeline Tauri Commands
//!
//! 提供模块发现、流水线配置读写、后处理任务触发/取消、进度查询和模块输出路径查询等命令。
//! Provides commands for module discovery, pipeline config read/write,
//! post-processing task triggering/cancellation, progress queries, and module output path queries.

use crate::core::emitter::{Emitter, EmitterExt, TauriEmitter};
use crate::core::error::Result;
use crate::postprocess::pipeline::{discover_modules, run_pipeline, ModuleInfo, NodeResult, PipelineConfig};
use crate::config::settings::{AppState, PpTaskStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, State};

/// 列出 modules/ 目录下所有可用的后处理模块。
/// List all available post-processing modules in the modules/ directory.
#[tauri::command]
pub async fn list_modules() -> Result<Vec<ModuleInfo>> {
    let modules: Vec<ModuleInfo> = tokio::task::spawn_blocking(discover_modules)
        .await
        .unwrap_or_default();
    Ok(modules)
}

/// 获取当前流水线配置。
/// Get the current pipeline configuration.
#[tauri::command]
pub async fn get_pipeline(state: State<'_, Arc<AppState>>) -> Result<PipelineConfig> {
    Ok(state.get_pipeline())
}

/// 保存流水线配置到磁盘。
/// Save the pipeline configuration to disk.
#[tauri::command]
pub async fn save_pipeline(
    pipeline: PipelineConfig,
    state: State<'_, Arc<AppState>>,
) -> Result<()> {
    state.update_pipeline(pipeline)
}

/// 获取所有后处理任务的状态列表。
/// Get the status list of all post-processing tasks.
#[tauri::command]
pub async fn get_postprocess_tasks(state: State<'_, Arc<AppState>>) -> Result<Vec<PpTaskStatus>> {
    Ok(state.get_pp_tasks())
}

/// 查询指定视频文件的模块输出路径（如 contact_sheet 预览图路径）。
/// Query module output paths for a specific video file (e.g., contact_sheet preview image path).
#[tauri::command]
pub async fn get_module_outputs(
    path: String,
    state: State<'_, Arc<AppState>>,
) -> Result<std::collections::HashMap<String, String>> {
    let video_path = std::path::Path::new(&path);
    let pipeline = state.get_pipeline();
    let mut outputs = std::collections::HashMap::new();

    for node in &pipeline.nodes {
        if !node.enabled {
            continue;
        }
        // 目前只有 contact_sheet 模块有可预测的输出路径
        // Currently only the contact_sheet module has a predictable output path
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

    Ok(outputs)
}

/// 触发对指定视频文件执行后处理流水线（异步，立即返回）。
/// Trigger post-processing pipeline execution for a specific video file (async, returns immediately).
#[tauri::command]
pub async fn run_postprocess_cmd(
    path: String,
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<()> {
    let pipeline = state.get_pipeline();
    if pipeline.nodes.is_empty() {
        return Err(crate::core::error::AppError::Other(
            "后处理流水线为空，请先在后处理页面添加模块".to_string(),
        ));
    }

    let video_path = PathBuf::from(&path);
    let emitter: Arc<dyn Emitter> = Arc::new(TauriEmitter(app_handle));
    let state_clone = Arc::clone(&state);

    state.pp_task_enqueue(&path);
    emitter.emit("postprocess-waiting", &serde_json::json!({ "path": path }));

    // 在阻塞线程池中执行，避免阻塞 Tokio 异步运行时
    // Execute in blocking thread pool to avoid blocking the Tokio async runtime
    tokio::task::spawn_blocking(move || {
        run_postprocess_for_path_inner(&video_path, &pipeline, &emitter, &state_clone);
    });

    Ok(())
}

/// 公开的后处理入口（供录制完成后自动触发使用）。
/// 将任务加入等待队列后调用内部实现。
///
/// Public post-processing entry point (used for automatic triggering after recording completes).
/// Enqueues the task and then calls the inner implementation.
pub fn run_postprocess_for_path(
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    state.pp_task_enqueue(&path_str);
    emitter.emit(
        "postprocess-waiting",
        &serde_json::json!({ "path": path_str }),
    );

    run_postprocess_for_path_inner(video_path, pipeline, emitter, state);
}

/// 后处理流水线执行的核心实现（同步，在阻塞线程中调用）。
/// 获取串行锁 → 检查取消标志 → 执行流水线 → 上报进度和结果。
///
/// Core implementation of post-processing pipeline execution (synchronous, called in a blocking thread).
/// Acquires serial lock → checks cancel flag → runs pipeline → reports progress and results.
pub fn run_postprocess_for_path_inner(
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    // 获取串行锁，确保同一时刻只有一个后处理任务运行
    // Acquire serial lock to ensure only one post-processing task runs at a time
    let _pp_guard = state.pp_lock.lock().unwrap_or_else(|e| e.into_inner());

    // 检查是否在等待锁期间已被取消 / Check if cancelled while waiting for the lock
    let already_cancelled = state
        .pp_cancel_flags
        .read()
        .get(&path_str)
        .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false);
    if already_cancelled {
        state.pp_task_clear_cancel_flag(&path_str);
        state.pp_tasks.write().remove(&path_str);
        return;
    }

    let modules = discover_modules();
    let total = pipeline.nodes.iter().filter(|n| n.enabled).count();

    state.pp_task_start(&path_str, total);
    let cancel_flag = state.pp_task_make_cancel_flag(&path_str);
    emitter.emit(
        "postprocess-started",
        &serde_json::json!({ "path": path_str }),
    );

    let results: Vec<NodeResult> = run_pipeline(
        video_path,
        pipeline,
        &modules,
        Some(cancel_flag),
        // 进度回调：更新状态并向前端发送进度事件
        // Progress callback: update state and emit progress event to frontend
        |node_done: usize, node_total: usize, mod_done: u32, mod_total: u32, module_name: &str, status_text: &str| {
            let pct_raw = if node_total == 0 {
                100.0
            } else if mod_total > 0 {
                let nt = node_total as f64;
                let nd = node_done as f64;
                let node_pct = (nd * 100.0) / nt;
                let slice = 100.0 / nt;
                let inner = ((mod_done as f64) * slice) / (mod_total as f64);
                (node_pct + inner).min(100.0)
            } else {
                ((node_done as f64) * 100.0) / (node_total as f64)
            };
            let pct = (pct_raw * 100.0).round() / 100.0;

            let display_name = if status_text.is_empty() {
                module_name.to_string()
            } else {
                format!("{} · {}", module_name, status_text)
            };

            state.pp_task_progress(
                &path_str,
                pct,
                mod_done,
                mod_total,
                &display_name,
                node_done,
                node_total,
            );

            emitter.emit(
                "postprocess-progress",
                &serde_json::json!({
                    "path": path_str,
                    "done": node_done,
                    "total": node_total,
                    "pct": pct,
                    "modDone": mod_done,
                    "modTotal": mod_total,
                    "moduleName": display_name,
                }),
            );
        },
        // 日志回调：将模块的 stdout/stderr 输出转发给前端
        // Log callback: forward module stdout/stderr output to the frontend
        |module_id, stream, line| {
            emitter.emit(
                "postprocess-log",
                &serde_json::json!({
                    "path": path_str,
                    "moduleId": module_id,
                    "stream": stream,
                    "line": line,
                }),
            );
        },
    );

    state.pp_task_clear_cancel_flag(&path_str);

    let all_ok = results.iter().all(|r| r.success);
    state.pp_task_finish(&path_str, all_ok);

    // 若视频文件已被模块删除（如 filter_short），清理相关记录
    // If the video file was deleted by a module (e.g., filter_short), clean up related records
    if all_ok && !video_path.exists() {
        state.data.write().pp_results.remove(&path_str);
        let _ = state.save();
        state.pp_tasks.write().remove(&path_str);
    }

    emitter.emit(
        "postprocess-done",
        &serde_json::json!({ "path": path_str, "results": results }),
    );
}

/// 请求取消指定文件的后处理任务。
/// Request cancellation of the post-processing task for the given file.
#[tauri::command]
pub async fn cancel_postprocess(path: String, state: State<'_, Arc<AppState>>) -> Result<()> {
    state.pp_task_cancel(&path);
    Ok(())
}
