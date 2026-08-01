//! 后处理流水线命令 / Post-processing Pipeline Commands
//!
//! 提供模块发现、流水线配置读写、后处理任务触发/取消、进度查询等功能。
//! Provides module discovery, pipeline config read/write,
//! post-processing task triggering/cancellation, and progress queries.
//!
//! ## pp_execution 写入时机 / pp_execution write timing
//!
//! - 节点开始前：追加 PpExecutionEntry（finished_at/result/outputs 为 null）
//! - 节点完成后：更新对应条目（填入 finished_at、result、outputs）
//! - 每次写入后立即通过 SSE 推送 `postprocess-execution-update` 事件

use crate::core::emitter::{Emitter, EmitterExt};
use crate::postprocess::pipeline::{
    discover_modules, run_pipeline, NodeResult, PipelineConfig, PipelineNode, RecordingContext,
};
use crate::recording::meta::{
    PpExecCode, PpExecResult, PpExecutionEntry, PpNodeInputSnapshot, PpNodeProgress,
};
use crate::config::settings::AppState;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// ─── 公开入口 / Public Entry Point ───────────────────────────────────────────

/// 公开的后处理入口（供录制完成后自动触发使用）。
/// 将任务加入等待队列后调用内部实现。
///
/// Public post-processing entry point (used for automatic triggering after recording completes).
/// Enqueues the task and then calls the inner implementation.
pub fn run_postprocess_for_path(
    // 后处理的初始输入路径（通常是 ts_session_dir；若跳过 ts_merge 则为 video_file）
    // Initial input path for post-processing (usually ts_session_dir; video_file if ts_merge is skipped)
    initial_path: &std::path::Path,
    // 关联的视频文件路径（用于 meta 文件定位，可能与 initial_path 不同）
    // Associated video file path (for meta file location; may differ from initial_path)
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    state.pp_queue.enqueue(&path_str);
    emitter.emit(
        "postprocess-waiting",
        &serde_json::json!({ "path": path_str }),
    );

    // 更新元数据文件中的后处理状态 / Update post-processing status in metadata file
    crate::recording::meta::set_status(video_path, "pp_waiting");

    run_postprocess_inner(initial_path, video_path, pipeline, emitter, state);
}

// ─── 核心实现 / Core Implementation ──────────────────────────────────────────

/// 后处理流水线执行的核心实现（同步，在阻塞线程中调用）。
///
/// 流程：
/// 1. 获取串行锁（保证同一时刻只有一个后处理任务）
/// 2. 检查取消标志
/// 3. 从 meta 中读取上次 pp_execution，跳过已成功的节点
/// 4. 执行 DAG 流水线，每个节点开始/完成时分步写入 meta 并推送事件
/// 5. 全部完成后写入最终状态
///
/// Core implementation of post-processing pipeline execution (synchronous, called in a blocking thread).
pub fn run_postprocess_inner(
    initial_path: &std::path::Path,
    video_path: &std::path::Path,
    pipeline: &PipelineConfig,
    emitter: &Arc<dyn Emitter>,
    state: &Arc<AppState>,
) {
    let path_str = video_path.to_string_lossy().to_string();

    // 获取串行锁 / Acquire serial lock
    let _pp_guard = state.pp_queue.acquire_serial_lock();

    // 检查是否在等待锁期间已被取消 / Check if cancelled while waiting for the lock
    if state.pp_queue.is_cancelled(&path_str) {
        // 写回 pp_error，避免 meta 卡在 pp_waiting 被扫描逻辑当作陈旧任务反复重新触发
        // Persist pp_error so meta doesn't stay stuck at pp_waiting and get endlessly
        // re-triggered by scan logic as a "stale" task
        crate::recording::meta::set_status(video_path, "pp_error");
        state.pp_queue.clear_cancel_flag(&path_str);
        state.pp_queue.remove(&path_str);
        return;
    }

    let modules = discover_modules();

    // 从 meta 读取上次的 pp_execution，用于重新后处理时跳过已成功且配置未变的节点
    // Read previous pp_execution from meta to skip succeeded nodes whose config is unchanged
    let meta_snapshot = crate::recording::meta::read_meta(video_path);
    let prev_execution: Vec<PpExecutionEntry> = meta_snapshot
        .as_ref()
        .and_then(|m| m.pp_execution.clone())
        .unwrap_or_default();

    // 计算本次需要（重新）执行的节点集合：上次未成功、配置（模块/参数/连线）已变更、
    // 或下游依赖了已变更节点的，都视为"脏"节点，必须重新执行。
    // 手动触发和启动/定时扫描触发都调用本函数，因此两条路径共享同一套判断逻辑。
    //
    // Compute the set of nodes that must (re-)run this time: nodes that didn't succeed
    // last time, whose config (module/params/wiring) changed, or that depend downstream
    // on a changed node, are all "dirty" and must be re-executed. Both manual triggers
    // and startup/scheduled re-triggers call this function, so they share this same logic.
    let dirty_nodes = compute_dirty_nodes(pipeline, &prev_execution);

    // 构建实际需要执行的流水线：过滤掉未变更且已成功的节点，但保留边关系
    // Build effective pipeline: filter out unchanged, previously-succeeded nodes; preserve edges
    let effective_pipeline = build_effective_pipeline(pipeline, &dirty_nodes);

    // 预检：确认所有启用节点的模块都存在 / Pre-check: verify all enabled nodes have modules
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
        // 必须先写回 meta 的 pp_error 状态，再清理内存队列记录；
        // 否则 meta 会永久卡在 pp_waiting，重启扫描时被反复重新触发。
        // Must persist pp_error status to meta before clearing the in-memory queue record;
        // otherwise meta stays stuck at pp_waiting and gets endlessly re-triggered on restart scans.
        crate::recording::meta::set_status(video_path, "pp_error");
        state.pp_queue.finish(&path_str, false);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({ "path": path_str, "success": false, "message": msg }),
        );
        return;
    }

    let total = effective_pipeline.nodes.iter().filter(|n| n.enabled).count();
    state.pp_queue.start(&path_str, total);
    let cancel_flag = state.pp_queue.make_cancel_flag(&path_str);

    emitter.emit(
        "postprocess-started",
        &serde_json::json!({ "path": path_str }),
    );
    crate::recording::meta::set_status(video_path, "pp_running");

    // 构建录制上下文（供模块参考）/ Build recording context for modules
    let username = video_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let recording_ctx = RecordingContext {
        video_path: video_path.to_string_lossy().to_string(),
        started_at: meta_snapshot
            .as_ref()
            .map(|m| m.started_at.clone())
            .unwrap_or_default(),
        username,
    };

    let max_tmp_dir_gb = state.get_settings().max_tmp_dir_gb;

    // 执行 DAG，节点开始/完成时分步写入 meta
    // Execute DAG; write pp_execution entry incrementally on node start/done
    let state_ref = Arc::clone(state);
    let emitter_ref = Arc::clone(emitter);
    let path_str_ref = path_str.clone();
    let video_path_buf = video_path.to_path_buf();

    let node_done_count = std::sync::Mutex::new(0usize);
    // 当前正在执行的节点信息，由 on_node_start 写入，on_progress 直接读取，避免重复 read_meta
    // Current executing node info, written by on_node_start and read directly by on_progress
    // to avoid repeated read_meta calls during high-frequency progress reporting
    let current_node_module_id = std::sync::Mutex::new(String::new());
    let current_node_id = std::sync::Mutex::new(String::new());

    // 构建预填输入槽：将已成功且未变更节点的 outputs 沿边传递给下游，确保新增/受影响的
    // 节点能收到正确输入
    // Build pre-filled collected slots: propagate unchanged succeeded nodes' outputs to
    // downstream nodes so newly added/affected nodes can receive the correct inputs
    let pre_collected = build_pre_collected(pipeline, &prev_execution, &dirty_nodes);

    /// 读取 meta 并通过 SSE 推送快照，让前端无需依赖独立事件字段即可获得完整进度。
    /// Read meta and push a snapshot via SSE so the frontend gets the full picture without
    /// relying on individual event fields.
    fn emit_meta_update(
        video_path: &std::path::Path,
        emitter: &Arc<dyn Emitter>,
        path_str: &str,
    ) {
        use crate::core::emitter::EmitterExt;
        if let Some(meta) = crate::recording::meta::read_meta(video_path) {
            emitter.emit(
                "postprocess-meta-update",
                &serde_json::json!({ "path": path_str, "meta": meta }),
            );
        }
    }

    let results = run_pipeline(
        &[initial_path.to_path_buf()],
        &effective_pipeline,
        &modules,
        &recording_ctx,
        Some(cancel_flag),
        max_tmp_dir_gb,
        pre_collected,
        // on_node_start：追加 pp_execution 条目（result=null），写入初始 pp_progress，推送 meta 快照
        // on_node_start: append pp_execution entry (result=null), write initial pp_progress, push meta snapshot
        &|node_id, module_id, inputs| {
            let now = chrono::Local::now().to_rfc3339();
            let (params, wiring) = pipeline
                .nodes
                .iter()
                .find(|n| n.node_id == node_id)
                .map(node_config_snapshot)
                .unwrap_or_default();
            let entry = PpExecutionEntry {
                node_id: node_id.to_string(),
                module_id: module_id.to_string(),
                started_at: now,
                finished_at: None,
                result: None,
                inputs: inputs.iter().map(|p| p.to_string_lossy().to_string()).collect(),
                outputs: None,
                params,
                wiring,
            };
            crate::recording::meta::pp_execution_start(&video_path_buf, entry);

            // 更新共享的当前节点信息，供 on_progress 直接使用
            // Update shared current node info for direct use by on_progress
            *current_node_module_id.lock().unwrap() = module_id.to_string();
            *current_node_id.lock().unwrap() = node_id.to_string();

            let done_so_far = *node_done_count.lock().unwrap();
            crate::recording::meta::set_pp_progress(
                &video_path_buf,
                PpNodeProgress {
                    node_id: node_id.to_string(),
                    module_id: module_id.to_string(),
                    mod_done: 0,
                    mod_total: 0,
                    overall_done: done_so_far,
                    overall_total: total,
                },
            );
            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref);
        },
        // on_node_done：完成 pp_execution 条目，清空 pp_progress，更新整体进度，推送 meta 快照
        // on_node_done: finish pp_execution entry, clear pp_progress, update overall progress, push meta snapshot
        &|result: NodeResult| {
            let now = chrono::Local::now().to_rfc3339();
            let pp_result = PpExecResult {
                code: result.code.clone(),
                message: if result.message.is_empty() {
                    None
                } else {
                    Some(result.message.clone())
                },
            };
            let outputs: Option<Vec<String>> = if result.outputs.is_empty() {
                None
            } else {
                Some(result.outputs.iter().map(|p| p.to_string_lossy().to_string()).collect())
            };
            crate::recording::meta::pp_execution_finish(
                &video_path_buf,
                &result.node_id,
                now,
                pp_result,
                outputs,
            );

            // ts_merge 成功后更新 meta.video_path
            // After ts_merge succeeds, update meta.video_path
            if result.module_id == "ts_merge" && result.is_success() {
                if let Some(output_path) = result.outputs.first() {
                    if let Some(mut meta) = crate::recording::meta::read_meta(&video_path_buf) {
                        meta.video_path = Some(output_path.to_string_lossy().to_string());
                        crate::recording::meta::write_meta(&video_path_buf, &meta);
                    }
                }
            }

            // 清空进度快照和共享节点信息
            // Clear progress snapshot and shared node info
            crate::recording::meta::clear_pp_progress(&video_path_buf);
            *current_node_module_id.lock().unwrap() = String::new();
            *current_node_id.lock().unwrap() = String::new();

            let mut done = node_done_count.lock().unwrap();
            *done += 1;
            let done_val = *done;

            let pct = if total == 0 {
                100.0f64
            } else {
                (done_val as f64 * 100.0 / total as f64).min(100.0)
            };
            state_ref.pp_queue.progress(
                &path_str_ref,
                pct,
                0, 0,
                &result.module_id,
                done_val,
                total,
            );

            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref);
        },
        // on_progress：直接用共享变量中的 module_id，无需 read_meta，大幅降低磁盘 I/O
        // on_progress: use module_id from shared variable directly, no read_meta needed,
        // significantly reducing disk I/O during high-frequency progress reporting
        &|node_id, mod_done, mod_total, _status_text| {
            let module_id = current_node_module_id.lock().unwrap().clone();
            let done_so_far = *node_done_count.lock().unwrap();
            crate::recording::meta::set_pp_progress(
                &video_path_buf,
                PpNodeProgress {
                    node_id: node_id.to_string(),
                    module_id,
                    mod_done,
                    mod_total,
                    overall_done: done_so_far,
                    overall_total: total,
                },
            );
            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref);
        },
        // on_log：模块 stdout/stderr 日志，保持不变
        // on_log: module stdout/stderr log lines, unchanged
        &|module_id, stream, line| {
            emitter_ref.emit(
                "postprocess-log",
                &serde_json::json!({
                    "path": path_str_ref,
                    "moduleId": module_id,
                    "stream": stream,
                    "line": line,
                }),
            );
        },
    );

    state.pp_queue.clear_cancel_flag(&path_str);

    // 合并本次结果与上次已成功且未变更的结果，确定最终状态
    // Merge new results with previously succeeded, unchanged results to determine final status
    let merged_results = merge_with_prev_results(results, &prev_execution, pipeline, &dirty_nodes);
    let all_ok = merged_results.iter().all(|r| r.is_success());

    // 先提取 ts_merge 输出路径（若有），再判断文件是否真正被删除
    // Extract ts_merge output path first (if any), then check if the file was truly deleted
    let ts_merge_output: Option<std::path::PathBuf> = merged_results
        .iter()
        .find(|r| r.module_id == "ts_merge" && r.is_success())
        .and_then(|r| r.outputs.first().cloned());

    // 确定"实际存活路径"：ts_merge 有输出时用输出路径，否则用原始 video_path
    // Determine the "surviving path": use ts_merge output if available, else original video_path
    let surviving_path = ts_merge_output.as_deref().unwrap_or(video_path);

    // 若实际存活路径不存在，说明视频被某个模块（如 filter_short）删除，清理 meta 并退出
    // If the surviving path doesn't exist, the video was deleted by a module (e.g. filter_short)
    if !surviving_path.exists() {
        // 用原始 video_path 定位 meta（stem 相同）/ Use original video_path to locate meta (same stem)
        crate::recording::meta::delete_meta(video_path);
        state.pp_queue.remove(&path_str);
        emitter.emit(
            "postprocess-done",
            &serde_json::json!({
                "path": path_str,
                "success": true,
                "message": "Pipeline ended: input was removed by a module",
            }),
        );
        return;
    }

    // 读取当前 meta 中本次实际执行的条目（on_node_start/on_node_done 写入的）
    // Read the pp_execution entries written during this run (by on_node_start/on_node_done)
    let current_execution: Vec<PpExecutionEntry> = crate::recording::meta::read_meta(video_path)
        .and_then(|m| m.pp_execution)
        .unwrap_or_default();

    // 用 merged_results 重建最终的 pp_execution，确保每次后处理后记录都是干净的：
    // - 本次实际执行的节点：从 current_execution 中取对应条目（含 started_at/finished_at/outputs）
    // - 被跳过的节点（上次已成功）：从 prev_execution 中取对应条目
    // 只保留 merged_results 中出现的节点，丢弃历史遗留的旧条目。
    //
    // Rebuild final pp_execution from merged_results so each re-run produces a clean record:
    // - Nodes executed this run: take their entries from current_execution (with timestamps/outputs)
    // - Skipped nodes (succeeded previously): take their entries from prev_execution
    // Only keep nodes present in merged_results; discard stale entries from previous runs.
    let final_execution: Vec<PpExecutionEntry> = merged_results
        .iter()
        .filter_map(|r| {
            // 优先取本次执行的条目（有完整的 started_at/finished_at/outputs）
            // Prefer the entry from this run (has complete started_at/finished_at/outputs)
            if let Some(entry) = current_execution.iter().rfind(|e| e.node_id == r.node_id) {
                return Some(entry.clone());
            }
            // 回退到上次成功的条目（被跳过的节点）
            // Fall back to the previously succeeded entry (for skipped nodes)
            prev_execution.iter().find(|e| e.node_id == r.node_id).cloned()
        })
        .collect();

    let final_status = if all_ok { "finish" } else { "pp_error" };
    crate::recording::meta::set_pp_done(video_path, final_status, final_execution.clone());

    // ts_merge 产生的输出与 session_dir 的 stem 相同，meta 文件名不变，无需迁移。
    // ts_merge output has the same stem as the session_dir — meta filename is unchanged, no migration needed.

    state.pp_queue.finish(&path_str, all_ok);
    emitter.emit(
        "postprocess-done",
        &serde_json::json!({
            "path": path_str,
            "success": all_ok,
            "pp_execution": final_execution,
            "video_path": ts_merge_output.as_ref().map(|p| p.to_string_lossy().to_string()),
        }),
    );
}

// ─── 帮助函数 / Helper Functions ──────────────────────────────────────────────

/// 根据 video_path 推断后处理流水线的初始输入路径。
///
/// 规则：若与 video_path 同级、同名的目录（session_dir）存在，说明 TS 分片尚未被
/// ts_merge 合并，此时首节点（通常是 ts_merge）需要以 session_dir 作为输入。
/// 否则（session_dir 已删，或从未存在），直接用 video_path 作为输入。
///
/// Infer the initial input path for the post-processing pipeline from video_path.
///
/// Rule: if a directory with the same stem exists alongside video_path (session_dir),
/// the TS segments haven't been merged yet, so the first node (typically ts_merge)
/// must receive session_dir as its input.
/// Otherwise (session_dir already deleted or never existed), use video_path directly.
pub fn infer_initial_path(video_path: &std::path::Path) -> std::path::PathBuf {
    if let (Some(parent), Some(stem)) = (
        video_path.parent(),
        video_path.file_stem().and_then(|s| s.to_str()),
    ) {
        let session_dir = parent.join(stem);
        if session_dir.is_dir() {
            return session_dir;
        }
    }
    video_path.to_path_buf()
}

/// 提取一个节点当前的参数和连线快照，用于与上次执行记录比较是否发生变更。
/// Extract a node's current params and wiring snapshot, for comparison against the
/// previous execution record to detect changes.
fn node_config_snapshot(
    node: &PipelineNode,
) -> (HashMap<String, serde_json::Value>, HashMap<usize, PpNodeInputSnapshot>) {
    let params = node.params.clone();
    let wiring = node
        .inputs
        .iter()
        .map(|(port, input_ref)| {
            (
                *port,
                PpNodeInputSnapshot {
                    node_id: input_ref.node_id.clone(),
                    port: input_ref.port,
                },
            )
        })
        .collect();
    (params, wiring)
}

/// 判断某节点相对于上次执行记录是否"自身发生变更"（模块 ID、参数或连线任意一项不同）。
/// 找不到上次记录（新增节点）或上次未成功，也视为自身变更。
///
/// Determine whether a node has "changed on its own" relative to its previous execution
/// record (module ID, params, or wiring differ). A missing previous record (newly added
/// node) or a non-successful previous run also counts as self-changed.
fn node_self_changed(node: &PipelineNode, prev_execution: &[PpExecutionEntry]) -> bool {
    let Some(prev) = prev_execution.iter().find(|e| e.node_id == node.node_id) else {
        return true; // 新增节点，无历史记录 / New node, no history
    };
    let was_success = matches!(
        prev.result.as_ref().map(|r| &r.code),
        Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
    );
    if !was_success {
        return true;
    }
    if prev.module_id != node.module_id {
        return true;
    }
    let (params, wiring) = node_config_snapshot(node);
    prev.params != params || prev.wiring != wiring
}

/// 计算本次需要（重新）执行的"脏"节点集合：自身配置变更的节点，加上所有
/// 直接或间接依赖于脏节点输出的下游节点（沿 DAG 边传播，因为它们的实际输入
/// 也会随之改变，即使这些下游节点自身参数未变）。
///
/// Compute the set of "dirty" nodes that must (re-)run this time: nodes whose own
/// config changed, plus all nodes directly or indirectly downstream of a dirty node
/// (propagated along DAG edges, since their actual input also changes even if their
/// own params didn't).
fn compute_dirty_nodes(
    pipeline: &PipelineConfig,
    prev_execution: &[PpExecutionEntry],
) -> HashSet<String> {
    let edges = pipeline.resolved_edges();
    let mut dirty: HashSet<String> = pipeline
        .nodes
        .iter()
        .filter(|n| n.enabled && node_self_changed(n, prev_execution))
        .map(|n| n.node_id.clone())
        .collect();

    // 沿边向下游传播脏标记，直到不再有新节点被标记（BFS 定点）
    // Propagate the dirty mark downstream along edges until no new nodes are added (BFS fixpoint)
    loop {
        let mut added = false;
        for edge in &edges {
            if dirty.contains(&edge.from_node_id) && !dirty.contains(&edge.to_node_id) {
                dirty.insert(edge.to_node_id.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    dirty
}

/// 构建有效流水线：跳过上次已成功且未变更（不在脏节点集合中）的节点，保留边关系。
/// 为避免图结构损坏，只从节点列表中过滤，边保持原样（run_pipeline 内部会自动处理孤立边）。
///
/// Build the effective pipeline: skip nodes that succeeded previously and are unchanged
/// (not in the dirty set). Only filters from the node list; edges are kept intact
/// (run_pipeline handles dangling edges internally).
fn build_effective_pipeline(
    pipeline: &PipelineConfig,
    dirty_nodes: &HashSet<String>,
) -> PipelineConfig {
    let mut p = pipeline.clone();
    p.nodes.retain(|n| {
        if !n.enabled {
            return true; // 保留禁用节点，run_pipeline 会跳过 / Keep disabled; run_pipeline skips it
        }
        let skip = !dirty_nodes.contains(&n.node_id);
        if skip {
            tracing::info!(
                "pp re-run: skipping node {} (module: {}) — succeeded previously and unchanged",
                n.node_id, n.module_id
            );
        }
        !skip
    });
    p
}

/// 从上次执行记录中提取已成功且未变更节点的 outputs，预填到下游节点的 collected 输入槽。
/// 这样在重新后处理时，被跳过的节点的输出仍能正常传递给新增/受影响的下游节点。
/// 脏节点（即将重新执行）的历史 outputs 不预填，避免污染其重新执行后的真实输出。
///
/// Extract outputs of previously succeeded, unchanged nodes from execution records and
/// pre-fill them into downstream nodes' collected input slots. This ensures skipped nodes'
/// outputs still propagate to newly-added/affected downstream nodes. Dirty nodes (about to
/// re-run) are excluded here so their stale outputs don't leak into the fresh run.
fn build_pre_collected(
    pipeline: &PipelineConfig,
    prev_execution: &[PpExecutionEntry],
    dirty_nodes: &HashSet<String>,
) -> HashMap<String, Vec<(usize, std::path::PathBuf)>> {
    let mut pre: HashMap<String, Vec<(usize, std::path::PathBuf)>> = HashMap::new();

    let edges = pipeline.resolved_edges();

    for entry in prev_execution {
        // 脏节点即将重新执行，跳过其历史 outputs / Dirty nodes will re-run; skip their stale outputs
        if dirty_nodes.contains(&entry.node_id) {
            continue;
        }
        // 仅处理上次成功且有 outputs 的节点
        // Only handle previously succeeded nodes that produced outputs
        let is_success = matches!(
            entry.result.as_ref().map(|r| &r.code),
            Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
        );
        if !is_success {
            continue;
        }
        let outputs = match entry.outputs.as_ref() {
            Some(o) if !o.is_empty() => o,
            _ => continue,
        };

        // 遍历该节点的所有出边，把 outputs 预填到下游节点的 collected 槽
        // Traverse all outgoing edges and pre-fill downstream nodes' collected slots
        for edge in edges.iter().filter(|e| e.from_node_id == entry.node_id) {
            if let Some(output_path) = outputs.get(edge.from_port) {
                pre.entry(edge.to_node_id.clone())
                    .or_default()
                    .push((edge.to_port, std::path::PathBuf::from(output_path)));
            }
        }
    }

    pre
}

/// 将本次新执行结果与上次已成功且未变更的结果合并为最终结果列表。
/// 脏节点（本次已重新执行）一律使用新结果，即使新结果意外缺失也不回退到旧记录，
/// 避免用过期结果掩盖"本应重新执行但未执行"的问题。
///
/// Merge new execution results with previously succeeded, unchanged results.
/// Dirty nodes (re-run this time) always use the new result — even if unexpectedly
/// missing, we do not fall back to the stale record, to avoid masking a "should have
/// re-run but didn't" bug behind an outdated result.
fn merge_with_prev_results(
    new_results: Vec<NodeResult>,
    prev_execution: &[PpExecutionEntry],
    pipeline: &PipelineConfig,
    dirty_nodes: &HashSet<String>,
) -> Vec<NodeResult> {
    let mut merged: Vec<NodeResult> = Vec::new();
    for node in pipeline.nodes.iter().filter(|n| n.enabled) {
        if let Some(r) = new_results.iter().find(|r| r.node_id == node.node_id) {
            merged.push(r.clone());
            continue;
        }
        if dirty_nodes.contains(&node.node_id) {
            // 脏节点本应重新执行但未产生结果（如流水线提前终止）——不回退到旧记录
            // Dirty node should have re-run but produced no result (e.g. pipeline
            // terminated early upstream) — do not fall back to the stale record
            continue;
        }
        if let Some(prev) = prev_execution.iter().find(|e| {
            e.node_id == node.node_id
                && matches!(
                    e.result.as_ref().map(|r| &r.code),
                    Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
                )
        }) {
            // 重建为成功结果（保留上次的输出路径）/ Reconstruct as successful result
            merged.push(NodeResult {
                node_id: prev.node_id.clone(),
                module_id: prev.module_id.clone(),
                code: prev.result.as_ref().map(|r| r.code.clone()).unwrap_or(PpExecCode::Ok),
                message: prev.result.as_ref()
                    .and_then(|r| r.message.clone())
                    .unwrap_or_default(),
                outputs: prev.outputs.as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect(),
                inputs: prev.inputs.iter().map(std::path::PathBuf::from).collect(),
            });
        }
    }
    merged
}
