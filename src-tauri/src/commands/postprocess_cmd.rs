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

    // 更新 meta status = "pp_waiting" / Update meta status = "pp_waiting"
    crate::recording::meta::set_status(&video_path, "pp_waiting");

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

    // 更新元数据文件中的后处理状态 / Update post-processing status in metadata file
    crate::recording::meta::set_status(video_path, "pp_waiting");

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

    // 从 meta 读取上次的后处理结果，用于跳过已成功的模块（重新后处理时复用）
    // Read previous pp_results from meta to skip already-succeeded modules on re-run
    let meta_snapshot = crate::recording::meta::read_meta(video_path);
    tracing::info!(
        "pp re-run: video={:?}, meta_status={:?}, meta_pp_results_count={}",
        video_path,
        meta_snapshot.as_ref().map(|m| &m.status),
        meta_snapshot.as_ref().and_then(|m| m.pp_results.as_ref()).map(|r| r.len()).unwrap_or(0),
    );
    let prev_results: Vec<crate::recording::meta::PpModuleResult> = meta_snapshot
        .and_then(|m| m.pp_results)
        .unwrap_or_default();

    tracing::info!(
        "pp re-run: prev_results={:?}",
        prev_results.iter().map(|r| format!("{}={}", r.module_id, r.success)).collect::<Vec<_>>()
    );

    // 构建实际需要执行的节点列表：跳过上次已成功的模块
    // Build the list of nodes to actually run: skip modules that succeeded last time
    let effective_pipeline = {
        let mut p = pipeline.clone();
        p.nodes.retain(|n| {
            if !n.enabled {
                return true; // 保留禁用节点（run_pipeline 会自动跳过）/ Keep disabled nodes (run_pipeline skips them)
            }
            let skip = prev_results.iter().any(|r| r.module_id == n.module_id && r.success);
            tracing::info!("pp re-run: node module_id={} enabled={} skip={}", n.module_id, n.enabled, skip);
            !skip
        });
        p
    };

    // 预检：确认所有启用节点的模块都存在，缺失则直接报错
    // Pre-check: verify all enabled nodes have their modules available, fail fast if not
    let missing: Vec<&str> = effective_pipeline
        .nodes
        .iter()
        .filter(|n| n.enabled)
        .filter(|n| !modules.iter().any(|m| m.id == n.module_id))
        .map(|n| n.module_id.as_str())
        .collect();

    if !missing.is_empty() {
        let msg = format!(
            "后处理模块缺失：{}，请检查 modules/ 目录",
            missing.join(", ")
        );
        state.pp_task_finish(&path_str, false);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({ "path": path_str, "results": [{
                "nodeId": "",
                "moduleId": missing[0],
                "success": false,
                "message": msg,
                "output": null
            }] }),
        );
        return;
    }

    let total = effective_pipeline.nodes.iter().filter(|n| n.enabled).count();

    state.pp_task_start(&path_str, total);
    let cancel_flag = state.pp_task_make_cancel_flag(&path_str);
    emitter.emit(
        "postprocess-started",
        &serde_json::json!({ "path": path_str }),
    );

    // 更新元数据文件：标记为运行中 / Update metadata file: mark as running
    crate::recording::meta::set_status(video_path, "pp_running");

    let max_tmp_dir_gb = state.get_settings().max_tmp_dir_gb;

    let new_results: Vec<NodeResult> = run_pipeline(
        video_path,
        &effective_pipeline,
        &modules,
        Some(cancel_flag),
        max_tmp_dir_gb,
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

    // 合并本次结果与上次已成功的结果，按原始 pipeline 节点顺序排列
    // Merge new results with previously succeeded results, ordered by original pipeline node order
    let results: Vec<NodeResult> = {
        let mut merged: Vec<NodeResult> = Vec::new();
        for node in pipeline.nodes.iter().filter(|n| n.enabled) {
            // 优先使用本次执行结果
            // Prefer result from this run
            if let Some(r) = new_results.iter().find(|r| r.module_id == node.module_id) {
                merged.push(r.clone());
            } else if let Some(prev) = prev_results.iter().find(|r| r.module_id == node.module_id && r.success) {
                // 复用上次成功的结果（构造一个虚拟 NodeResult）
                // Reuse previously succeeded result (construct a synthetic NodeResult)
                merged.push(NodeResult {
                    node_id: node.node_id.clone(),
                    module_id: prev.module_id.clone(),
                    success: true,
                    message: prev.message.clone(),
                    output: None,
                    delete_input: false,
                });
            }
        }
        merged
    };

    let all_ok = results.iter().all(|r| r.success);

    // 若视频文件已不存在（被删除命令删除，或被模块如 filter_short 删除），
    // 跳过 meta 写入并清理 meta 文件，避免写入孤立的 meta。
    //
    // If the video file no longer exists (deleted by delete command or by a module like filter_short),
    // skip meta write and clean up the meta file to avoid leaving an orphaned meta.
    if !video_path.exists() {
        crate::recording::meta::delete_meta(video_path);
        // 从 pp_results 目录文件中移除该路径 / Remove path from pp_results directory file
        {
            let mut data = state.data.write();
            data.pp_results.retain(|p| p != &path_str);
        }
        let _ = state.save();
        state.pp_tasks.write().remove(&path_str);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({ "path": path_str, "results": results }),
        );
        return;
    }

    state.pp_task_finish(&path_str, all_ok);

    // 更新元数据文件：写入后处理结果 / Update metadata file: write post-processing results
    {
        let final_status = if all_ok { "finish" } else { "pp_error" };
        let pp_module_results: Vec<crate::recording::meta::PpModuleResult> = results
            .iter()
            .map(|r| crate::recording::meta::PpModuleResult {
                module_id: r.module_id.clone(),
                success: r.success,
                message: r.message.clone(),
            })
            .collect();
        let mut module_outputs = std::collections::HashMap::new();
        // 先继承上次 meta 中已有的模块输出路径
        // First inherit existing module output paths from previous meta
        if let Some(prev_meta) = crate::recording::meta::read_meta(video_path)
            && let Some(prev_outputs) = prev_meta.module_outputs {
            module_outputs.extend(prev_outputs);
        }
        // 本次执行结果覆盖（优先级更高）：使用 NodeResult.output 字段（已从 OUTPUT: 行解析）
        // Override with results from this run (higher priority): use NodeResult.output field
        // (already parsed from the OUTPUT: protocol line, not from message)
        for r in &results {
            if r.success
                && let Some(ref out_path) = r.output {
                module_outputs.insert(
                    r.module_id.clone(),
                    out_path.to_string_lossy().to_string(),
                );
            }
        }
        crate::recording::meta::set_pp_done(
            video_path,
            final_status,
            pp_module_results,
            module_outputs,
        );
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
