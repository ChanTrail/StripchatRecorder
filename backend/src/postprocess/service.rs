//! 后处理流水线业务逻辑 / Post-processing Pipeline Service
//!
//! 提供模块发现、流水线配置读写、后处理任务触发/取消、进度查询等功能。
//! 被 `server/routes/postprocess.rs`、`recording/recorder.rs`、
//! `recording/meta/maintenance.rs` 调用。
//!
//! Provides module discovery, pipeline config read/write,
//! post-processing task triggering/cancellation, and progress queries.
//! Called by `server/routes/postprocess.rs`, `recording/recorder.rs`,
//! and `recording/meta/maintenance.rs`.
//!
//! - 节点开始前：追加 PpExecutionEntry（finished_at/result 为 null，outputs 为空数组）
//! - 节点完成后：更新对应条目（填入 finished_at、result、outputs）
//! - 每次写入后立即通过 SSE 推送 `postprocess-execution-update` 事件

use crate::config::app_state::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::postprocess::pipeline::{
	NodeResult, PipelineConfig, RecordingContext, discover_modules, run_pipeline,
};
use crate::postprocess::queue::ClaimRejected;
use crate::recording::meta::{PpExecResult, PpExecutionEntry, PpNodeProgress};
use std::sync::Arc;

// 增量执行辅助（脏节点计算、预填输入、历史结果合并）见 `service::incremental`
// Incremental-execution helpers (dirty-node computation, pre-filled inputs,
// merging with previous results) live in `service::incremental`
mod incremental;

use incremental::{
	build_effective_pipeline, build_pre_collected, compute_dirty_nodes, group_paths_by_bundle,
	merge_with_prev_results, node_config_fingerprint,
};
// 保持 `postprocess::service::infer_initial_path` 的公开路径不变
// Keep `postprocess::service::infer_initial_path`'s public path unchanged
pub use incremental::infer_initial_path;

// ─── 公开入口 / Public Entry Point ───────────────────────────────────────────

/// 公开的后处理入口（供录制完成后自动触发使用）。
/// 将任务加入等待队列后调用内部实现。
///
/// Public post-processing entry point (used for automatic triggering after recording completes).
/// Enqueues the task and then calls the inner implementation.
pub fn run_postprocess_for_path(
	// 后处理的初始输入路径（通常是 session_dir；若跳过 segment_merge 则为 video_file）
	// Initial input path for post-processing (usually session_dir; video_file if segment_merge is skipped)
	initial_path: &std::path::Path,
	// 关联的视频文件路径（用于 meta 文件定位，可能与 initial_path 不同）
	// Associated video file path (for meta file location; may differ from initial_path)
	video_path: &std::path::Path,
	pipeline: &PipelineConfig,
	emitter: &Arc<dyn Emitter>,
	state: &Arc<AppState>,
) {
	let path_str = video_path.to_string_lossy().to_string();

	// 以录制身份（meta 路径）原子 claim：同一录制已有任务排队或执行时直接跳过，
	// 不入队、不发事件、不改 meta。_claim 持有到流水线返回，drop 时清理队列记录。
	// Atomically claim by recording identity (meta path): if a task for the same recording
	// is already queued or running, skip without enqueuing, emitting or touching meta.
	// _claim is held until the pipeline returns and cleans up queue records on drop.
	let key = crate::postprocess::queue::recording_key(video_path);
	// 维护扫描的占位只持续毫秒级：遇到占位时短暂等待它释放（最多 5 s），避免录制结束、
	// 轮转或手动触发被吞掉；删除中的录制与重复触发直接跳过。
	// A maintenance-scan reservation only lasts milliseconds: on a reservation, wait briefly
	// for it to be released (up to 5 s) so recording-end, rotation or manual triggers aren't
	// swallowed; recordings being deleted and duplicate triggers are skipped outright.
	let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
	let _claim = loop {
		match state.pp_queue.try_claim(&key, &path_str) {
			Ok(c) => break c,
			Err(ClaimRejected::Reserved) if std::time::Instant::now() < deadline => {
				std::thread::sleep(std::time::Duration::from_millis(20));
			}
			Err(ClaimRejected::Reserved) => {
				tracing::warn!(
					"{}",
					crate::tl!(
						"postprocess.claimWaitReservationTimeout",
						key = key,
						path = path_str
					)
				);
				return;
			}
			Err(ClaimRejected::RemovalRequested) => {
				tracing::info!(
					"{}",
					crate::tl!(
						"postprocess.triggerSkippedRemoval",
						key = key,
						path = path_str
					)
				);
				return;
			}
			Err(ClaimRejected::AlreadyClaimed) => {
				tracing::info!(
					"{}",
					crate::tl!(
						"postprocess.duplicateTriggerSkipped",
						key = key,
						path = path_str
					)
				);
				return;
			}
		}
	};

	state.pp_queue.enqueue(&path_str);
	emitter.emit(
		"postprocess-waiting",
		&serde_json::json!({ "path": path_str }),
	);

	// 更新元数据文件中的后处理状态 / Update post-processing status in metadata file
	crate::recording::meta::set_status(video_path, "pp_waiting");

	run_postprocess_inner(initial_path, video_path, pipeline, emitter, &key, state);
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
	// 录制身份键（claim 时使用的 meta 路径）/ Recording identity key (meta path used for the claim)
	recording_key: &str,
	state: &Arc<AppState>,
) {
	let path_str = video_path.to_string_lossy().to_string();

	// 先取得取消标志，排队等待许可期间被取消时能立即退出
	// Get the cancel flag first so a cancel while queued for a permit exits immediately
	let cancel_flag = state.pp_queue.make_cancel_flag(&path_str);

	// 获取并发许可（阻塞直至有空闲槽位或被取消）
	// Acquire a concurrency permit (blocks until a slot is free or the task is cancelled)
	let permit = state.pp_queue.acquire_concurrency_permit(&cancel_flag);

	// 检查是否在等待许可期间已被取消 / Check if cancelled while waiting for the permit
	if permit.is_none() || state.pp_queue.is_cancelled(&path_str) {
		// 写回 pp_error，避免 meta 卡在 pp_waiting 被扫描逻辑当作陈旧任务反复重新触发
		// Persist pp_error so meta doesn't stay stuck at pp_waiting and get endlessly
		// re-triggered by scan logic as a "stale" task
		crate::recording::meta::set_status(video_path, "pp_error");
		state.pp_queue.clear_cancel_flag(&path_str);
		state.pp_queue.remove(&path_str);
		return;
	}
	let _pp_permit = permit;

	// 锁等待期间用户可能已更新流水线，获取锁后重新读取最新配置，
	// 确保本次执行使用的是当前最新的流水线，而不是入队时的快照。
	// 如果 state 里的流水线为空（nodes 全为空），保守地回退到传入参数。
	//
	// The user may have updated the pipeline while this task was waiting for the serial
	// lock. Re-read the current pipeline after acquiring the lock so this run uses the
	// latest configuration rather than the snapshot captured at enqueue time.
	// Fall back to the passed-in parameter when state's pipeline has no nodes.
	let refreshed_pipeline: PipelineConfig;
	let pipeline: &PipelineConfig = {
		let from_state = state.get_pipeline();
		if from_state.nodes.is_empty() {
			pipeline
		} else {
			refreshed_pipeline = from_state;
			&refreshed_pipeline
		}
	};

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

	// 构建本次不需要重新执行的节点集合：在完整流水线里、已启用、不在 dirty_nodes 中。
	// 这些节点的上次输出已通过 pre_collected 预填给了下游，执行引擎遇到它们时做透传。
	// 同时用 effective_pipeline（只含 dirty 节点）做模块存在性预检和计算进度总数。
	//
	// Build the set of nodes to skip this run: enabled nodes in the full pipeline that
	// are not in dirty_nodes. Their previous outputs are pre-filled via pre_collected;
	// the execution engine will pass-through when it encounters them.
	// effective_pipeline (dirty nodes only) is used solely for module existence pre-check
	// and total-count calculation.
	let skip_nodes: std::collections::HashSet<String> = pipeline
		.nodes
		.iter()
		.filter(|n| n.enabled && !dirty_nodes.contains(n.effective_id()))
		.map(|n| n.effective_id().to_string())
		.collect();
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

	let total = effective_pipeline
		.nodes
		.iter()
		.filter(|n| n.enabled)
		.count();
	state.pp_queue.start(&path_str, total);

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
	// segment_merge 成功后会更新为新生成的视频文件路径
	// path_str_ref for SSE event path field, initial value is video_path,
	// updated to newly generated video file path after segment_merge succeeds
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
		old_path: Option<&str>,
	) {
		use crate::core::emitter::EmitterExt;
		if let Some(meta) = crate::recording::meta::read_meta(video_path) {
			let module_outputs = crate::recording::meta::extract_verified_module_outputs(
				meta.pp_execution.as_deref(),
			);
			emitter.emit(
				"postprocess-meta-update",
				&serde_json::json!({
					"path": path_str,
					"old_path": old_path,
					"meta": meta,
					"module_outputs": module_outputs,
				}),
			);
		}
	}

	let results = run_pipeline(
		&[initial_path.to_path_buf()],
		pipeline,
		&modules,
		&recording_ctx,
		Some(cancel_flag),
		max_tmp_dir_gb,
		pre_collected,
		&skip_nodes,
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
			let is_reusable_builtin = module_id
				.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX)
				&& effective_id != module_id;
			let entry = PpExecutionEntry {
				module_id: module_id.to_string(),
				node_id: if is_reusable_builtin {
					Some(effective_id.to_string())
				} else {
					None
				},
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
					node_id: if is_reusable_builtin {
						Some(effective_id.to_string())
					} else {
						None
					},
					mod_done: 0,
				},
			);
			emit_meta_update(
				&video_path_buf,
				&emitter_ref,
				&path_str_ref.lock().unwrap(),
				None,
			);
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
			let outputs = group_paths_by_bundle(&result.outputs);
			crate::recording::meta::pp_execution_finish(
				&video_path_buf,
				&result.effective_id,
				now,
				pp_result,
				outputs,
			);

			// segment_merge 成功后更新 meta.video_path，切换 SSE path。
			//
			// 视频时长/分辨率的探测和写入不在此处进行——原因见 run_postprocess_inner
			// 末尾 backfill_video_probe_fields 调用点的说明：本次 segment_merge 若因"已成功
			// 且配置未变"被跳过（见 compute_dirty_nodes/build_effective_pipeline），
			// on_node_done 根本不会为它触发，写在这里会导致老录制的 meta 永远补不上
			// 这两个字段。
			//
			// After segment_merge succeeds, update meta.video_path and switch the SSE path.
			//
			// Video duration/resolution probing and writing does NOT happen here — see the
			// comment at the backfill_video_probe_fields call site near the end of
			// run_postprocess_inner for why: if this run's segment_merge was skipped because it
			// "already succeeded with unchanged config" (see compute_dirty_nodes/
			// build_effective_pipeline), on_node_done never fires for it at all, so writing
			// here would leave these two fields permanently unfilled for any recording that
			// was already merged before this logic existed.
			//
			// old_path_for_migration：路径切换时记录旧路径，随本次 postprocess-meta-update
			// 一并推送，供前端精确完成 path 迁移，避免多任务并发时靠"搜索所有 running 路径"
			// 猜测旧路径导致的状态错乱。
			//
			// old_path_for_migration: when the path changes, record the old path and include
			// it in this postprocess-meta-update push, so the frontend can perform the path
			// migration precisely rather than guessing by searching all running paths — which
			// causes state corruption when multiple tasks are running concurrently.
			let old_path_for_migration: Option<String> = if result.module_id
				== crate::postprocess::pipeline::SEGMENT_MERGE_MODULE_ID
				&& result.is_success()
				&& let Some(output_path) = result.outputs.first()
			{
				if let Some(mut meta) = crate::recording::meta::read_meta(&video_path_buf) {
					meta.video_path = Some(output_path.to_string_lossy().to_string());
					crate::recording::meta::write_meta(&video_path_buf, &meta);
				}
				let old = path_str_ref.lock().unwrap().clone();
				*path_str_ref.lock().unwrap() = output_path.to_string_lossy().to_string();
				Some(old)
			} else {
				None
			};

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
			// 队列键始终是 path_str（入队时的路径），不随 SSE 路径切换
			// The queue key is always path_str (the enqueue path); it doesn't follow SSE path changes
			state_ref
				.pp_queue
				.progress(&path_str, pct, 0, &result.module_id, done_val, total);
			emit_meta_update(
				&video_path_buf,
				&emitter_ref,
				&path_str_ref.lock().unwrap(),
				old_path_for_migration.as_deref(),
			);
		},
		// on_progress：直接用共享变量中的 module_id，无需 read_meta，大幅降低磁盘 I/O
		// on_progress: use module_id from shared variable directly, no read_meta needed,
		// significantly reducing disk I/O during high-frequency progress reporting
		&|effective_id, mod_done, _mod_total, _status_text| {
			let module_id = current_node_module_id.lock().unwrap().clone();
			let is_reusable_builtin = module_id
				.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX)
				&& effective_id != module_id;
			crate::recording::meta::set_pp_progress(
				&video_path_buf,
				PpNodeProgress {
					module_id,
					node_id: if is_reusable_builtin {
						Some(effective_id.to_string())
					} else {
						None
					},
					mod_done,
				},
			);
			emit_meta_update(
				&video_path_buf,
				&emitter_ref,
				&path_str_ref.lock().unwrap(),
				None,
			);
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

	// 先提取 segment_merge 输出路径（若有），再判断文件是否真正被删除
	// Extract segment_merge output path first (if any), then check if the file was truly deleted
	let merge_output: Option<std::path::PathBuf> = merged_results
		.iter()
		.find(|r| {
			(r.module_id == crate::postprocess::pipeline::SEGMENT_MERGE_MODULE_ID) && r.is_success()
		})
		.and_then(|r| r.outputs.first().cloned());

	// 确定"实际存活路径"：segment_merge 有输出时用输出路径，否则用原始 video_path
	// Determine the "surviving path": use segment_merge output if available, else original video_path
	let surviving_path = merge_output.as_deref().unwrap_or(video_path);

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
			if let Some(entry) = current_execution
				.iter()
				.rfind(|e| e.effective_id() == r.effective_id)
			{
				return Some(entry.clone());
			}
			// 回退到上次成功的条目（被跳过的节点），按 effective_id 匹配。
			// Fall back to the previously succeeded entry (skipped nodes),
			// matched by effective_id.
			prev_execution
				.iter()
				.find(|e| e.effective_id() == r.effective_id)
				.cloned()
		})
		.collect();

	let final_status = if all_ok { "finish" } else { "pp_error" };
	crate::recording::meta::set_pp_done(video_path, final_status, final_execution.clone());

	// segment_merge 产生的输出与 session_dir 的 stem 相同，meta 文件名不变，无需迁移。
	// segment_merge output has the same stem as the session_dir — meta filename is unchanged, no migration needed.

	// 无条件补写视频时长/分辨率（若尚未写入）：不依赖本次 segment_merge 是否真的执行过。
	//
	// 之前的实现把 ffprobe 探测放在 on_node_done 的 segment_merge 分支里，只有该节点
	// 本次真正跑过才会触发；但重新后处理一个"segment_merge 已成功且配置未变"的历史
	// 视频时，compute_dirty_nodes 会把 segment_merge 标记为非 dirty，
	// build_effective_pipeline 直接把它从本次要执行的节点列表中过滤掉，
	// on_node_done 根本不会为它调用——导致这两个字段永远停留在 null，
	// 无论重新触发多少次后处理都补不上。
	//
	// 这里改为在流水线整个跑完之后、按 surviving_path 是否已存在探测结果统一检查
	// 一次：只要 meta 里这两个字段任一为 None，且 surviving_path 是一个真实存在的
	// 视频文件（而非目录——若流水线尚未跑到 segment_merge，surviving_path 仍是原始
	// session_dir，ffprobe 对目录必然失败，get_video_duration/get_video_resolution
	// 内部的 Command 调用会静默返回 None，不会报错也不会误写），就补一次 ffprobe。
	// 已有值时不重新探测，避免徒增磁盘 IO。
	//
	// Unconditionally backfill video duration/resolution (if not already set) —
	// independent of whether this run's segment_merge actually executed.
	//
	// The previous implementation put the ffprobe probing inside on_node_done's segment_merge
	// branch, which only fires when that node genuinely runs this time. But re-processing
	// a historical video whose segment_merge "already succeeded with unchanged config" causes
	// compute_dirty_nodes to mark segment_merge as not dirty, and build_effective_pipeline
	// filters it out of this run's node list entirely — on_node_done is never called for
	// it, leaving these two fields permanently null no matter how many times
	// post-processing is re-triggered.
	// This is now checked once after the whole pipeline finishes, based on whether
	// surviving_path already has probed values in meta: as long as either field is still
	// None and surviving_path is a real video file (not a directory — if the pipeline never
	// reached segment_merge, surviving_path is still the original session_dir; ffprobe on a
	// directory simply fails and the get_video_duration/get_video_resolution helpers'
	// internal Command calls silently return None, without erroring or writing anything
	// wrong), a single ffprobe backfill pass runs. Fields already set are left untouched to
	// avoid redundant disk I/O.
	if surviving_path.is_file()
		&& let Some(mut meta) = crate::recording::meta::read_meta(video_path)
		&& (meta.video_duration_secs.is_none() || meta.video_resolution.is_none())
	{
		meta.video_duration_secs = meta
			.video_duration_secs
			.or_else(|| crate::recording::ffmpeg_util::get_video_duration(surviving_path));
		meta.video_resolution = meta
			.video_resolution
			.or_else(|| crate::recording::ffmpeg_util::get_video_resolution(surviving_path));
		crate::recording::meta::write_meta(video_path, &meta);
	}

	state.pp_queue.finish(&path_str, all_ok);
	// `postprocess-done` 的 path 字段必须用 path_str_ref 的最终值，而不是本函数
	// 开头就固定下来的 path_str。
	//
	// 当流水线从 TS 分片目录开始执行时，segment_merge 成功后会把 path_str_ref 更新为
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
	// to the merged video file path once segment_merge succeeds (postprocess-meta-update
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

	// 后处理失败时写入通知，让用户在通知面板看到报错。
	// 用户主动删除录制导致的取消不算失败，不推送通知；手动点取消的行为不变。
	// Push a notification on failure so the user sees it in the notification panel.
	// A cancellation caused by the user deleting the recording isn't a failure and gets no
	// notification; manually clicking cancel behaves as before.
	if !all_ok && !state.pp_queue.is_removal_requested(recording_key) {
		let file_name = std::path::Path::new(&final_path_str)
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_else(|| final_path_str.clone());
		// 从 merged_results 里收集所有失败节点的错误信息
		// Collect error messages from all failed nodes in merged_results
		let errors: Vec<String> = merged_results
			.iter()
			.filter(|r| !r.is_success())
			.map(|r| format!("{}: {}", r.module_id, r.message))
			.collect();
		let error_detail = errors.join("; ");
		let message = format!("{} 后处理失败：{}", file_name, error_detail);
		let mut args = std::collections::HashMap::new();
		args.insert("name".to_string(), serde_json::json!(file_name));
		args.insert("detail".to_string(), serde_json::json!(error_detail));
		state.notification_store.emit_i18n(
			emitter.as_ref(),
			crate::core::notifications::NotificationLevel::Error,
			"postprocess",
			message,
			"notifications.backend.ppFailed",
			Some(args),
		);
	}

	emitter.emit(
		"postprocess-done",
		&serde_json::json!({
			"path": final_path_str,
			"success": all_ok,
			"pp_execution": final_execution,
			"video_path": merge_output.as_ref().map(|p| p.to_string_lossy().to_string()),
		}),
	);
}
