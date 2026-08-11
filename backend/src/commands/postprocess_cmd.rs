//! 后处理流水线命令 / Post-processing Pipeline Commands
//!
//! 提供模块发现、流水线配置读写、后处理任务触发/取消、进度查询等功能。
//! Provides module discovery, pipeline config read/write,
//! post-processing task triggering/cancellation, and progress queries.
//!
//! ## pp_execution 写入时机 / pp_execution write timing
//!
//! - 节点开始前：追加 PpExecutionEntry（finished_at/result 为 null，outputs 为空数组）
//! - 节点完成后：更新对应条目（填入 finished_at、result、outputs）
//! - 每次写入后立即通过 SSE 推送 `postprocess-execution-update` 事件

use crate::core::emitter::{Emitter, EmitterExt};
use crate::postprocess::pipeline::{
    discover_modules, run_pipeline, NodeResult, PipelineConfig, PipelineNode, RecordingContext,
};
use crate::recording::meta::{
    PpExecCode, PpExecResult, PpExecutionEntry, PpNodeProgress,
};
use crate::config::settings::AppState;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
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
    // path_str_ref 用于 SSE 事件的 path 字段，初始值为 video_path，
    // ts_merge 成功后会更新为新生成的视频文件路径
    // path_str_ref for SSE event path field, initial value is video_path,
    // updated to newly generated video file path after ts_merge succeeds
    let path_str_ref = std::sync::Mutex::new(path_str.clone());
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
    ///
    /// 同时附带 `module_outputs`：从 `meta.pp_execution` 中提取、且已验证路径当前
    /// 确实存在于磁盘上的模块输出（如 contact_sheet 预览图），与 `list_recordings`
    /// 接口的 `RecordingFile.module_outputs` 使用同一套验证逻辑
    /// （[`crate::recording::meta::extract_verified_module_outputs`]）。前端应统一
    /// 依据这个已验证字段判断预览图按钮是否显示，无论数据来源于初次加载还是
    /// 这里的实时 SSE 快照，都不会出现"路径已记录但文件其实不存在"的情况。
    ///
    /// Read meta and push a snapshot via SSE so the frontend gets the full picture without
    /// relying on individual event fields.
    ///
    /// Also includes `module_outputs`: module outputs extracted from `meta.pp_execution`
    /// and verified to currently exist on disk (e.g. contact_sheet's preview image),
    /// using the exact same verification logic
    /// ([`crate::recording::meta::extract_verified_module_outputs`]) as the
    /// `list_recordings` endpoint's `RecordingFile.module_outputs`. The frontend should
    /// uniformly rely on this verified field to decide whether to show a preview button,
    /// so it never shows a stale "path recorded but file doesn't actually exist" state,
    /// regardless of whether the data came from the initial load or this real-time SSE
    /// snapshot.
    fn emit_meta_update(
        video_path: &std::path::Path,
        emitter: &Arc<dyn Emitter>,
        path_str: &str,
    ) {
        use crate::core::emitter::EmitterExt;
        if let Some(meta) = crate::recording::meta::read_meta(video_path) {
            let module_outputs = crate::recording::meta::extract_verified_module_outputs(
                meta.pp_execution.as_deref(),
            );
            emitter.emit(
                "postprocess-meta-update",
                &serde_json::json!({ "path": path_str, "meta": meta, "module_outputs": module_outputs }),
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
        &|effective_id, module_id, inputs| {
            let now = chrono::Local::now().to_rfc3339();
            // 在 pipeline.nodes 中查找节点时使用 effective_id() 匹配
            // Find node in pipeline.nodes using effective_id()
            let fingerprint = pipeline
                .nodes
                .iter()
                .find(|n| n.effective_id() == effective_id)
                .map(node_config_fingerprint)
                .unwrap_or_default();
            // effective_id 对应 module_id（普通节点）或 node_id（可复用内置节点）
            // effective_id corresponds to module_id (regular) or node_id (reusable built-in)
            let is_reusable_builtin = module_id.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX)
                && effective_id != module_id;
            let entry = PpExecutionEntry {
                module_id: module_id.to_string(),
                node_id: if is_reusable_builtin { Some(effective_id.to_string()) } else { None },
                started_at: now,
                finished_at: None,
                result: None,
                inputs: group_paths_by_bundle(inputs),
                outputs: Vec::new(),
                config_fingerprint: fingerprint,
            };
            crate::recording::meta::pp_execution_start(&video_path_buf, entry);

            *current_node_module_id.lock().unwrap() = module_id.to_string();
            *current_node_id.lock().unwrap() = effective_id.to_string();

            crate::recording::meta::set_pp_progress(
                &video_path_buf,
                PpNodeProgress {
                    module_id: module_id.to_string(),
                    node_id: if is_reusable_builtin { Some(effective_id.to_string()) } else { None },
                    mod_done: 0,
                },
            );
            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref.lock().unwrap());
        },
        // on_node_done：完成 pp_execution 条目，清空 pp_progress，更新整体进度，推送 meta 快照
        // on_node_done: finish pp_execution entry, clear pp_progress, update overall progress, push meta snapshot
        &|result: NodeResult| {
            let now = chrono::Local::now().to_rfc3339();
            let pp_result = PpExecResult {
                code: result.code.clone(),
                message: if result.message.is_empty() { None } else { Some(result.message.clone()) },
            };
            let outputs = group_paths_by_bundle(&result.outputs);
            crate::recording::meta::pp_execution_finish(
                &video_path_buf,
                &result.effective_id,
                now,
                pp_result,
                outputs,
            );

            // ts_merge 成功后更新 meta.video_path，切换 SSE path。
            //
            // 视频时长/分辨率的探测和写入不在此处进行——原因见 run_postprocess_inner
            // 末尾 backfill_video_probe_fields 调用点的说明：本次 ts_merge 若因"已成功
            // 且配置未变"被跳过（见 compute_dirty_nodes/build_effective_pipeline），
            // on_node_done 根本不会为它触发，写在这里会导致老录制的 meta 永远补不上
            // 这两个字段。
            //
            // After ts_merge succeeds, update meta.video_path and switch the SSE path.
            //
            // Video duration/resolution probing and writing does NOT happen here — see the
            // comment at the backfill_video_probe_fields call site near the end of
            // run_postprocess_inner for why: if this run's ts_merge was skipped because it
            // "already succeeded with unchanged config" (see compute_dirty_nodes/
            // build_effective_pipeline), on_node_done never fires for it at all, so writing
            // here would leave these two fields permanently unfilled for any recording that
            // was already merged before this logic existed.
            if result.module_id == "ts_merge" && result.is_success() {
                if let Some(output_path) = result.outputs.first() {
                    if let Some(mut meta) = crate::recording::meta::read_meta(&video_path_buf) {
                        meta.video_path = Some(output_path.to_string_lossy().to_string());
                        crate::recording::meta::write_meta(&video_path_buf, &meta);
                    }
                    *path_str_ref.lock().unwrap() = output_path.to_string_lossy().to_string();
                }
            }

            crate::recording::meta::clear_pp_progress(&video_path_buf);
            *current_node_module_id.lock().unwrap() = String::new();
            *current_node_id.lock().unwrap() = String::new();

            let mut done = node_done_count.lock().unwrap();
            *done += 1;
            let done_val = *done;

            let pct = if total == 0 { 100.0f64 } else { (done_val as f64 * 100.0 / total as f64).min(100.0) };
            state_ref.pp_queue.progress(
                &path_str_ref.lock().unwrap(),
                pct,
                0,
                &result.module_id,
                done_val,
                total,
            );
            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref.lock().unwrap());
        },
        // on_progress：直接用共享变量中的 module_id，无需 read_meta，大幅降低磁盘 I/O
        // on_progress: use module_id from shared variable directly, no read_meta needed,
        // significantly reducing disk I/O during high-frequency progress reporting
        &|effective_id, mod_done, _mod_total, _status_text| {
            let module_id = current_node_module_id.lock().unwrap().clone();
            let is_reusable_builtin = module_id.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX)
                && effective_id != module_id;
            crate::recording::meta::set_pp_progress(
                &video_path_buf,
                PpNodeProgress {
                    module_id,
                    node_id: if is_reusable_builtin { Some(effective_id.to_string()) } else { None },
                    mod_done,
                },
            );
            emit_meta_update(&video_path_buf, &emitter_ref, &path_str_ref.lock().unwrap());
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
            // 优先取本次执行的条目 / Prefer the entry from this run
            if let Some(entry) = current_execution.iter().rfind(|e| e.effective_id() == r.effective_id) {
                return Some(entry.clone());
            }
            // 回退到上次成功的条目（被跳过的节点）/ Fall back to previously succeeded entry (skipped nodes)
            prev_execution.iter().find(|e| e.effective_id() == r.effective_id).cloned()
        })
        .collect();

    let final_status = if all_ok { "finish" } else { "pp_error" };
    crate::recording::meta::set_pp_done(video_path, final_status, final_execution.clone());

    // ts_merge 产生的输出与 session_dir 的 stem 相同，meta 文件名不变，无需迁移。
    // ts_merge output has the same stem as the session_dir — meta filename is unchanged, no migration needed.

    // 无条件补写视频时长/分辨率（若尚未写入）：不依赖本次 ts_merge 是否真的执行过。
    //
    // 之前的实现把 ffprobe 探测放在 on_node_done 的 ts_merge 分支里，只有该节点
    // 本次真正跑过才会触发；但重新后处理一个"ts_merge 已成功且配置未变"的历史
    // 视频时，compute_dirty_nodes 会把 ts_merge 标记为非 dirty，
    // build_effective_pipeline 直接把它从本次要执行的节点列表中过滤掉，
    // on_node_done 根本不会为它调用——导致这两个字段永远停留在 null，
    // 无论重新触发多少次后处理都补不上。
    //
    // 这里改为在流水线整个跑完之后、按 surviving_path 是否已存在探测结果统一检查
    // 一次：只要 meta 里这两个字段任一为 None，且 surviving_path 是一个真实存在的
    // 视频文件（而非目录——若流水线尚未跑到 ts_merge，surviving_path 仍是原始
    // session_dir，ffprobe 对目录必然失败，get_video_duration/get_video_resolution
    // 内部的 Command 调用会静默返回 None，不会报错也不会误写），就补一次 ffprobe。
    // 已有值时不重新探测，避免徒增磁盘 IO。
    //
    // Unconditionally backfill video duration/resolution (if not already set) —
    // independent of whether this run's ts_merge actually executed.
    //
    // The previous implementation put the ffprobe probing inside on_node_done's ts_merge
    // branch, which only fires when that node genuinely runs this time. But re-processing
    // a historical video whose ts_merge "already succeeded with unchanged config" causes
    // compute_dirty_nodes to mark ts_merge as not dirty, and build_effective_pipeline
    // filters it out of this run's node list entirely — on_node_done is never called for
    // it, leaving these two fields permanently null no matter how many times
    // post-processing is re-triggered.
    // This is now checked once after the whole pipeline finishes, based on whether
    // surviving_path already has probed values in meta: as long as either field is still
    // None and surviving_path is a real video file (not a directory — if the pipeline never
    // reached ts_merge, surviving_path is still the original session_dir; ffprobe on a
    // directory simply fails and the get_video_duration/get_video_resolution helpers'
    // internal Command calls silently return None, without erroring or writing anything
    // wrong), a single ffprobe backfill pass runs. Fields already set are left untouched to
    // avoid redundant disk I/O.
    if surviving_path.is_file()
        && let Some(mut meta) = crate::recording::meta::read_meta(video_path)
        && (meta.video_duration_secs.is_none() || meta.video_resolution.is_none())
    {
        meta.video_duration_secs = meta.video_duration_secs
            .or_else(|| crate::recording::ffmpeg_util::get_video_duration(surviving_path));
        meta.video_resolution = meta.video_resolution
            .or_else(|| crate::recording::ffmpeg_util::get_video_resolution(surviving_path));
        crate::recording::meta::write_meta(video_path, &meta);
    }

    state.pp_queue.finish(&path_str, all_ok);
    // `postprocess-done` 的 path 字段必须用 path_str_ref 的最终值，而不是本函数
    // 开头就固定下来的 path_str。
    //
    // 当流水线从 TS 分片目录开始执行时，ts_merge 成功后会把 path_str_ref 更新为
    // 合并后的视频文件路径（`postprocess-meta-update` 事件全程用的都是这个会变化
    // 的值），但 path_str 从函数开始到结束始终是最初传入的 video_path（分片目录）
    // 不变。前端在收到第一个"新路径"的 postprocess-meta-update 时会尝试把
    // ppStatus/ppProgress 从旧路径迁移到新路径，但迁移只有在 `files.value` 已经
    // 不包含旧路径时才会触发——如果此时页面缓存的文件列表还没来得及刷新（仍包含
    // 旧的分片目录路径），迁移就会被跳过。这种情况下若 `postprocess-done` 仍然
    // 上报旧路径，就会把 "done" 状态写到一个再也没人读取的 key 上（页面显示的
    // 那一行早已用新路径渲染），导致进度列永久卡在最后一次 meta 快照（模块已全部
    // 完成但因为没有正在运行的节点，被误渲染成"处理中 0%"），必须手动刷新页面
    // 重新从 meta 加载 ppStatus 才能恢复。改成用 path_str_ref 的最终值，保证
    // "任务完成" 信号总是发到与 `postprocess-meta-update` 相同的 key 上。
    //
    // `postprocess-done`'s path field must use path_str_ref's final value, not
    // path_str which was fixed at the start of this function.
    //
    // When the pipeline starts from a TS segment directory, path_str_ref gets updated
    // to the merged video file path once ts_merge succeeds (postprocess-meta-update
    // uses this evolving value throughout), but path_str stays as the originally
    // passed-in video_path (the segment directory) for the entire function. The
    // frontend tries to migrate ppStatus/ppProgress from the old path to the new one
    // upon the first "new path" postprocess-meta-update event, but that migration only
    // fires if `files.value` no longer contains the old path — if the cached file list
    // hasn't been refreshed yet at that moment (still contains the old segment
    // directory path), migration is skipped. In that case, if `postprocess-done` still
    // reports the old path, it writes the "done" status to a key nobody reads anymore
    // (the row on screen has long since rendered under the new path), leaving the
    // progress column permanently stuck on the last meta snapshot (all nodes finished,
    // but with no running node it gets misrendered as "processing 0%") until a manual
    // page refresh reloads ppStatus fresh from meta. Using path_str_ref's final value
    // ensures the "task done" signal always lands on the same key as
    // `postprocess-meta-update`.
    let final_path_str = path_str_ref.lock().unwrap().clone();
    emitter.emit(
        "postprocess-done",
        &serde_json::json!({
            "path": final_path_str,
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

/// 计算节点当前参数 + 连线的指纹（sha256 十六进制字符串），用于与上次执行记录比较
/// 是否发生变更。不直接比较/存储原始 params/wiring——那些已是 `pipeline.json` 的
/// 权威数据，重复保存到 meta 属于冗余；这里只保留一个用于变更检测的哈希摘要。
/// BTreeMap 保证键顺序稳定，使指纹不受 HashMap 迭代顺序影响。
///
/// Compute a fingerprint (sha256 hex string) of a node's current params + wiring, for
/// comparison against the previous execution record to detect changes. Does not compare/
/// store the raw params/wiring directly — those are already authoritative in
/// `pipeline.json`; duplicating them in meta would be redundant. Only a hash digest for
/// change detection is kept. BTreeMap guarantees stable key ordering so the fingerprint
/// doesn't depend on HashMap iteration order.
fn node_config_fingerprint(node: &PipelineNode) -> String {
    let params: BTreeMap<&String, &serde_json::Value> = node.params.iter().collect();
    let wiring: BTreeMap<usize, (String, usize)> = node
        .inputs
        .iter()
        .map(|(port, input_ref)| (*port, (input_ref.node_id.clone(), input_ref.port)))
        .collect();
    // enabled 纳入指纹：禁用节点现在会被执行引擎当作"跳过并透传"处理（见
    // exec.rs 的文档说明），这是一种与"正常执行"截然不同的行为，因此启用/禁用
    // 状态的切换必须被视为"自身配置变更"，强制重新评估——否则在启用状态切换后
    // 重新触发后处理时，指纹不变会导致该节点被误判为"未变更"而跳过重新执行，
    // 继续复用切换前的旧结果（如把"已禁用透传"的旧输出当作"现在应该真正执行"
    // 的结果直接复用）。
    //
    // enabled is included in the fingerprint: a disabled node is now treated by the
    // execution engine as "skip and pass through" (see exec.rs's doc comments), a
    // fundamentally different behavior from "run normally" — so toggling enabled must
    // count as a "self config change" and force re-evaluation. Otherwise, re-triggering
    // post-processing after flipping enabled would see an unchanged fingerprint and
    // wrongly treat the node as "unchanged", skipping re-execution and continuing to
    // reuse the stale pre-toggle result (e.g. reusing an old "disabled pass-through"
    // output as if it were the result of "now actually running").
    let combined = serde_json::json!({ "params": params, "wiring": wiring, "enabled": node.enabled });
    let serialized = serde_json::to_string(&combined).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// 判断某节点相对于上次执行记录是否"自身发生变更"（模块 ID、参数、连线任意
/// 一项不同，或上次记录的输出文件已不再存在于磁盘上）。
/// 找不到上次记录（新增节点）或上次未成功，也视为自身变更。
///
/// 关于"输出文件已不再存在"这一判断依据：不能仅凭 meta 中记录过某个输出路径
/// 就认为该输出现在依然可用——文件可能在两次后处理之间被 cleanup 模块删除、
/// 被用户手动删除、或被外部磁盘清理工具误删。若该节点因"配置未变"被跳过
/// （见 `build_effective_pipeline`），它在 meta 中记录的旧输出路径会被
/// `build_pre_collected` 原样当作真实数据传给下游节点——下游节点收到一个根本
/// 不存在的路径，读取时才会失败，报错信息与真实原因（"上游产物已丢失"）完全
/// 脱节。将这种情况纳入"自身变更"判断，会让该节点被重新执行（重新生成缺失的
/// 输出），而不是假装它仍然成功过。
///
/// Determine whether a node has "changed on its own" relative to its previous execution
/// record (module ID, params, or wiring differ, OR the previously recorded output
/// file(s) no longer exist on disk). A missing previous record (newly added node) or a
/// non-successful previous run also counts as self-changed.
///
/// On the "output files no longer exist" criterion: a path having been recorded in meta
/// doesn't mean that output is still available now — the file could have been deleted by
/// the cleanup module, by the user manually, or by an external disk-cleanup tool between
/// two post-processing runs. If this node were skipped because its "config is unchanged"
/// (see `build_effective_pipeline`), its stale output path recorded in meta would be
/// handed to downstream nodes as real data by `build_pre_collected` — the downstream node
/// receives a path that simply doesn't exist, failing only once it tries to read it, with
/// an error message completely disconnected from the real cause ("the upstream artifact
/// is gone"). Folding this into "self-changed" makes the node re-execute (regenerating
/// the missing output) instead of pretending it's still successful.
fn node_self_changed(node: &PipelineNode, prev_execution: &[PpExecutionEntry]) -> bool {
    let eid = node.effective_id();
    let Some(prev) = prev_execution.iter().find(|e| e.effective_id() == eid) else {
        return true;
    };
    let was_success = matches!(
        prev.result.as_ref().map(|r| &r.code),
        Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
    );
    if !was_success { return true; }
    if prev.module_id != node.module_id { return true; }
    if prev.config_fingerprint != node_config_fingerprint(node) { return true; }
    if !all_recorded_outputs_exist(&prev.outputs) { return true; }
    false
}

/// 检查 meta 中按端口分组记录的输出路径（`Vec<Vec<String>>`，见 [`PpExecutionEntry::outputs`]
/// 的文档说明）是否每一个都仍然真实存在于磁盘上。空列表（节点无输出）视为通过。
///
/// Check whether every output path recorded in meta, grouped by port
/// (`Vec<Vec<String>>`, see [`PpExecutionEntry::outputs`]'s doc comment), still actually
/// exists on disk. An empty list (node produced no output) is considered passing.
fn all_recorded_outputs_exist(outputs: &[Vec<String>]) -> bool {
    outputs.iter().flatten().all(|p| std::path::Path::new(p).exists())
}

/// 将执行引擎按端口顺序返回的扁平路径列表（`inputs`/`outputs`）转换为 meta 存储格式：
/// 按端口分组的二维数组，每个端口内部按 `BUNDLE_SEP`（`\n`）拆分为多个路径字符串。
///
/// 流水线执行时的实际连线格式完全不受影响——`MediaBundle` 端口传递的仍然是
/// "视频路径\n图片路径" 的单一 PathBuf；这里只是让 meta JSON 中的表达更清晰，
/// 调用方可以直接按数组下标取路径，无需各自重复实现按 `\n` 拆分的逻辑。
///
/// Convert the flat, port-ordered path list returned by the execution engine
/// (`inputs`/`outputs`) into the meta storage format: a 2D array grouped by port, with
/// each port's single string split on `BUNDLE_SEP` (`\n`) into multiple path strings.
///
/// The actual pipeline wiring format is completely unaffected — a `MediaBundle` port
/// still carries a single "video_path\nimage_path" PathBuf during execution; this only
/// makes the meta JSON representation clearer so callers can index into the array
/// directly instead of each re-implementing `\n`-splitting themselves.
fn group_paths_by_bundle(paths: &[std::path::PathBuf]) -> Vec<Vec<String>> {
    paths
        .iter()
        .map(|p| {
            p.to_string_lossy()
                .split(crate::postprocess::builtin_nodes::BUNDLE_SEP)
                .map(|s| s.to_string())
                .collect()
        })
        .collect()
}

/// `group_paths_by_bundle` 的逆操作：将 meta 中按端口分组的二维数组重新合并为
/// 扁平的单路径-per-port 列表（同端口内的多个字符串重新用 `BUNDLE_SEP` 拼接），
/// 用于将历史 meta 记录重新接入执行引擎（如 `build_pre_collected`/`merge_with_prev_results`）。
///
/// Inverse of `group_paths_by_bundle`: re-joins the meta's per-port grouped arrays back
/// into a flat single-path-per-port list (strings within the same port are re-joined with
/// `BUNDLE_SEP`), for feeding historical meta records back into the execution engine
/// (e.g. `build_pre_collected`/`merge_with_prev_results`).
fn ungroup_paths_from_bundle(grouped: &[Vec<String>]) -> Vec<std::path::PathBuf> {
    let sep = crate::postprocess::builtin_nodes::BUNDLE_SEP.to_string();
    grouped
        .iter()
        .map(|port_paths| std::path::PathBuf::from(port_paths.join(&sep)))
        .collect()
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
        .map(|n| n.effective_id().to_string())
        .collect();

    loop {
        let mut added = false;
        for edge in &edges {
            if dirty.contains(&edge.from_node_id) && !dirty.contains(&edge.to_node_id) {
                dirty.insert(edge.to_node_id.clone());
                added = true;
            }
        }
        if !added { break; }
    }
    dirty
}

fn build_effective_pipeline(pipeline: &PipelineConfig, dirty_nodes: &HashSet<String>) -> PipelineConfig {
    let mut p = pipeline.clone();
    p.nodes.retain(|n| {
        if !n.enabled { return true; }
        let skip = !dirty_nodes.contains(n.effective_id());
        if skip {
            tracing::info!(
                "pp re-run: skipping node {} (module: {}) — succeeded previously and unchanged",
                n.effective_id(), n.module_id
            );
        }
        !skip
    });
    p
}

fn build_pre_collected(
    pipeline: &PipelineConfig,
    prev_execution: &[PpExecutionEntry],
    dirty_nodes: &HashSet<String>,
) -> HashMap<String, Vec<(usize, std::path::PathBuf)>> {
    let mut pre: HashMap<String, Vec<(usize, std::path::PathBuf)>> = HashMap::new();
    let edges = pipeline.resolved_edges();

    for entry in prev_execution {
        if dirty_nodes.contains(entry.effective_id()) { continue; }
        let is_success = matches!(
            entry.result.as_ref().map(|r| &r.code),
            Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
        );
        if !is_success { continue; }
        if entry.outputs.is_empty() { continue; }
        // 二次防御：只信任"仍然存在于磁盘上"的历史输出路径，不能仅凭 meta 中
        // 记录过就当作现在依然有效——`node_self_changed` 已经会把输出缺失的
        // 节点标记为脏节点并从这里排除（见其文档说明），此处是防止未来改动
        // 绕过那层判断而重新引入同一个 bug 的保险。
        //
        // Defense in depth: only trust historical output paths that "still exist on
        // disk" — meta having recorded a path once doesn't mean it's still valid now.
        // `node_self_changed` already marks nodes with missing outputs as dirty and
        // excludes them from reaching here (see its doc comment); this is a safety net
        // against a future change accidentally bypassing that check and reintroducing
        // the same bug.
        if !all_recorded_outputs_exist(&entry.outputs) {
            tracing::warn!(
                "pp re-run: node {} (module: {}) was expected unchanged but its recorded \
                 output(s) no longer exist on disk; skipping stale pre-fill (should have \
                 been caught as dirty already)",
                entry.effective_id(), entry.module_id
            );
            continue;
        }
        let outputs = ungroup_paths_from_bundle(&entry.outputs);
        for edge in edges.iter().filter(|e| e.from_node_id == entry.effective_id()) {
            if let Some(output_path) = outputs.get(edge.from_port) {
                pre.entry(edge.to_node_id.clone())
                    .or_default()
                    .push((edge.to_port, output_path.clone()));
            }
        }
    }
    pre
}

fn merge_with_prev_results(
    new_results: Vec<NodeResult>,
    prev_execution: &[PpExecutionEntry],
    pipeline: &PipelineConfig,
    dirty_nodes: &HashSet<String>,
) -> Vec<NodeResult> {
    let mut merged: Vec<NodeResult> = Vec::new();
    for node in pipeline.nodes.iter().filter(|n| n.enabled) {
        let eid = node.effective_id();
        if let Some(r) = new_results.iter().find(|r| r.effective_id == eid) {
            merged.push(r.clone());
            continue;
        }
        if dirty_nodes.contains(eid) { continue; }
        if let Some(prev) = prev_execution.iter().find(|e| {
            e.effective_id() == eid
                && matches!(
                    e.result.as_ref().map(|r| &r.code),
                    Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
                )
        }) {
            // 二次防御：与 build_pre_collected 的同名检查一样——`node_self_changed`
            // 已经会把输出缺失的节点标记为脏并从这里排除（`dirty_nodes.contains`
            // 判断在上面已经 `continue` 掉了），这里只是防止未来改动绕过那层
            // 判断后，最终写回 meta 的 pp_execution 里出现一条"结果码是 ok，但
            // 输出文件其实已经不存在"的记录。
            //
            // Defense in depth, mirroring the same check in build_pre_collected —
            // `node_self_changed` already marks nodes with missing outputs as dirty and
            // excludes them from reaching here (the `dirty_nodes.contains` check above
            // already `continue`s past them); this only guards against a future change
            // bypassing that check and letting a "result code is ok, but the output file
            // no longer actually exists" record end up in the final pp_execution
            // written back to meta.
            if !all_recorded_outputs_exist(&prev.outputs) {
                tracing::warn!(
                    "pp re-run: node {} (module: {}) was expected unchanged but its recorded \
                     output(s) no longer exist on disk; dropping stale result (should have \
                     been caught as dirty already)",
                    eid, prev.module_id
                );
                continue;
            }
            merged.push(NodeResult {
                effective_id: prev.effective_id().to_string(),
                module_id: prev.module_id.clone(),
                code: prev.result.as_ref().map(|r| r.code.clone()).unwrap_or(PpExecCode::Ok),
                message: prev.result.as_ref().and_then(|r| r.message.clone()).unwrap_or_default(),
                outputs: ungroup_paths_from_bundle(&prev.outputs),
                inputs: ungroup_paths_from_bundle(&prev.inputs),
            });
        }
    }
    merged
}
