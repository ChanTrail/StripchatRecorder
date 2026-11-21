//! 输出目录维护主流程与定时调度子模块。
//! Output-directory maintenance main flow and its scheduled wrapper.

use super::super::scan::{ensure_meta_files, segment_merge_output_dir};
use super::super::store::{read_meta, write_meta};
use super::cleanup::cleanup_stale_tmp;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 输出目录维护是否正在执行（保证 [`maintain_output_dir`] 单实例运行）。
/// Whether output-directory maintenance is running (keeps [`maintain_output_dir`] single-instance).
static MAINTENANCE_RUNNING: AtomicBool = AtomicBool::new(false);

/// [`MAINTENANCE_RUNNING`] 的 RAII 守卫：获取成功时置位，drop 时（含 future 被取消）复位。
/// RAII guard for [`MAINTENANCE_RUNNING`]: set on successful acquire, cleared on drop
/// (including when the future is cancelled).
struct MaintenanceGuard;

impl MaintenanceGuard {
    /// 尝试获取维护执行权；已有实例在执行时返回 `None`。
    /// Try to acquire the right to run maintenance; returns `None` if another run is in progress.
    fn try_acquire() -> Option<Self> {
        MAINTENANCE_RUNNING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| MaintenanceGuard)
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        MAINTENANCE_RUNNING.store(false, Ordering::Release);
    }
}

/// 执行一次完整的输出目录维护：清理空目录、重建缺失/损坏的 meta、触发遗漏的后处理。
/// 程序启动时和定时任务共用同一份逻辑，行为完全一致。
///
/// 不再单独"合并遗留 TS 分片"——是否需要合并完全交给流水线首节点（segment_merge）
/// 自行判断：只要触发后处理时把 session_dir 同时作为 `initial_path` 传入，
/// segment_merge 发现输入是目录就会合并，是文件就直接透传，无需在这里重复实现。
///
/// 执行顺序：
/// 1. 扫描输出目录（含 segment_merge 自定义输出目录），重建缺失/损坏/版本过旧的 meta，
///    收集所有需要（重新）触发后处理的路径（可能是视频文件，也可能是未合并的 session_dir）
/// 2. 流水线为空时逐条按录制身份占位后，将待处理任务的 meta 回退为 `finish`（拿不到
///    占位的条目本轮跳过，由下一轮维护处理）；否则逐个触发后处理，
///    任务在后台运行（实际并发度由 pp_queue 信号量控制，幂等由录制身份 claim 保证），
///    本函数不等待它们完成
/// 3. 删除空目录（可能与后处理并行，跳过 60 秒内修改过的目录，避免误删 segment_merge
///    刚创建的输出目录）
/// 4. 清理 tmp 目录中的过期文件
///
/// 同一时刻最多只有一个实例在执行：上一次维护尚未结束时（如启动扫描耗时超过定时间隔），
/// 本次调用直接记录日志并返回。
///
/// Run one full output-directory maintenance pass: remove empty directories, rebuild
/// missing/corrupt meta, and trigger missed post-processing.
/// Shared by both the startup path and the periodic scheduler so behavior stays identical.
///
/// No longer merges leftover TS segments as a separate step — whether merging is needed is
/// entirely up to the pipeline's first node (segment_merge): as long as the session_dir is passed
/// as `initial_path` when triggering post-processing, segment_merge merges it if it's a directory
/// or passes it through if it's already a file. No need to duplicate that logic here.
///
/// Execution order:
/// 1. Scan the output directory (plus segment_merge's custom output dir), rebuild missing/
///    corrupt/outdated meta, and collect all paths needing post-processing (re-)triggered
///    (either video files or unmerged session_dirs)
/// 2. Revert pending tasks' meta to `finish` when the pipeline is empty, reserving each
///    recording identity first (entries whose reservation fails are skipped this pass and
///    handled by the next one); otherwise trigger
///    post-processing for each of them; the tasks run in the background (parallelism is
///    governed by the pp_queue semaphore, idempotency by the recording-identity claim) and
///    this function does not wait for them to finish
/// 3. Remove empty directories (may run in parallel with post-processing; directories
///    modified within the last 60 seconds are skipped so freshly created segment_merge output
///    directories aren't removed)
/// 4. Clean up stale files in the tmp directory
///
/// At most one instance runs at a time: if the previous pass hasn't finished yet (e.g. the
/// startup scan takes longer than the periodic interval), this call logs and returns.
pub async fn maintain_output_dir(
    app_state: Arc<crate::config::app_state::AppState>,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
    is_startup: bool,
) {
    // 单实例：守卫是 async fn 的局部变量，正常返回或 future 被 drop 时都会释放
    // Single instance: the guard is a local of the async fn, released on return or when the
    // future is dropped
    let Some(_guard) = MaintenanceGuard::try_acquire() else {
        tracing::info!("{}", crate::tl!("meta.maintenanceAlreadyRunning"));
        return;
    };
    let settings = app_state.get_settings();
    let output_dir = std::path::PathBuf::from(&settings.output_dir);

    // 步骤 1：同步阻塞操作，在 spawn_blocking 内执行
    // Step 1: synchronous blocking operations, run inside spawn_blocking
    let (pp_pending, pipeline) = tokio::task::spawn_blocking({
        let app_state = Arc::clone(&app_state);
        let recorder = Arc::clone(&recorder);
        let output_dir = output_dir.clone();
        move || {
            let merge_extra = segment_merge_output_dir(&app_state);
            let extra_refs: Vec<&Path> = merge_extra.iter().map(|p| p.as_path()).collect();
            let pp_pending =
                ensure_meta_files(&output_dir, &extra_refs, &app_state, &recorder, is_startup);
            let pipeline = app_state.get_pipeline();
            (pp_pending, pipeline)
        }
    })
    .await
    .unwrap_or_default();

    // 步骤 2：流水线为空时回退 meta 状态；否则触发后处理（实际并发度由 pp_queue 信号量控制）。
    // 按录制开始时间（meta.started_at）升序排序，保证最旧的任务优先处理。
    // 重复触发由 run_postprocess_for_path 的录制身份 claim 幂等拦截：同一录制已在排队或
    // 执行时（无论来自录制结束、手动还是扫描触发）直接跳过；每条任务在触发前还会复核
    // 状态，已完成或路径已不存在的条目不再触发。
    //
    // Step 2: revert meta status when pipeline is empty; otherwise trigger post-processing
    // (actual parallelism is governed by the pp_queue semaphore).
    // Sort by recording start time (meta.started_at) ascending so the oldest task runs first.
    // Duplicate triggers are rejected idempotently by run_postprocess_for_path's
    // recording-identity claim: if the same recording is already queued or running (whether
    // from recording end, manual, or scan triggers) the call is skipped; each entry is also
    // re-checked right before triggering, skipping ones already finished or no longer present.
    let mut pp_pending = pp_pending;
    pp_pending.sort_by_key(|p| {
        crate::recording::meta::read_meta(p)
            .map(|m| m.started_at)
            .unwrap_or_default()
    });

    if pp_pending.is_empty() {
        // 步骤 3/4 和 startup-scan-done 走统一收尾路径
        // Fall through to unified cleanup and startup-scan-done
    } else {
        // pp_pending 不为空说明有上次进程退出时遗留的未完成任务，通知用户
        // Non-empty pp_pending means there are leftover unfinished tasks from a previous run; notify the user
        if is_startup {
            // 按 meta 状态分类：pp_error（失败重试）与其他（崩溃遗留）
            // Classify by meta status: pp_error (retry after failure) vs others (crash leftover)
            let mut stale_names: Vec<String> = Vec::new();
            let mut retry_names: Vec<String> = Vec::new();
            for path in &pp_pending {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let status = read_meta(path).map(|m| m.status).unwrap_or_default();
                if status == "pp_error" {
                    retry_names.push(name);
                } else {
                    stale_names.push(name);
                }
            }
            use std::collections::HashMap;
            if !stale_names.is_empty() {
                let count = stale_names.len();
                let joined = stale_names.join(", ");
                let (message, key, args) = if count == 1 {
                    let mut a = HashMap::new();
                    a.insert("name".to_string(), serde_json::json!(&stale_names[0]));
                    (
                        format!(
                            "Found 1 unfinished post-processing task from a previous crash, re-triggering: {}",
                            stale_names[0]
                        ),
                        "notifications.backend.ppRemainingOne",
                        a,
                    )
                } else {
                    let mut a = HashMap::new();
                    a.insert("count".to_string(), serde_json::json!(count));
                    a.insert("names".to_string(), serde_json::json!(joined));
                    (
                        format!(
                            "Found {} unfinished post-processing tasks from a previous crash, re-triggering: {}",
                            count, joined
                        ),
                        "notifications.backend.ppRemainingMany",
                        a,
                    )
                };
                app_state.notification_store.emit_i18n(
                    emitter.as_ref(),
                    crate::core::notifications::NotificationLevel::Warning,
                    "output_dir_maintenance",
                    message,
                    key,
                    Some(args),
                );
            }
            if !retry_names.is_empty() {
                let count = retry_names.len();
                let joined = retry_names.join(", ");
                let (message, key, args) = if count == 1 {
                    let mut a = HashMap::new();
                    a.insert("name".to_string(), serde_json::json!(&retry_names[0]));
                    (
                        format!(
                            "1 post-processing task failed last time, auto-retrying on startup: {}",
                            retry_names[0]
                        ),
                        "notifications.backend.ppRetryOne",
                        a,
                    )
                } else {
                    let mut a = HashMap::new();
                    a.insert("count".to_string(), serde_json::json!(count));
                    a.insert("names".to_string(), serde_json::json!(joined));
                    (
                        format!(
                            "{} post-processing tasks failed last time, auto-retrying on startup: {}",
                            count, joined
                        ),
                        "notifications.backend.ppRetryMany",
                        a,
                    )
                };
                app_state.notification_store.emit_i18n(
                    emitter.as_ref(),
                    crate::core::notifications::NotificationLevel::Info,
                    "output_dir_maintenance",
                    message,
                    key,
                    Some(args),
                );
            }
        }

        if !pipeline.nodes.iter().any(|n| n.enabled) {
            tracing::info!(
                "{}",
                crate::tl!("meta.scanSkipEmpty", count = pp_pending.len())
            );
            let pp_pending_clone = pp_pending.clone();
            let state = Arc::clone(&app_state);
            let _ = tokio::task::spawn_blocking(move || {
                for path in &pp_pending_clone {
                    // 先读后写前按录制身份占位（与扫描 A3、列表 size 回写 C1 一致）：已被后处理
                    // claim、已有其他占位或正在删除时占位失败，跳过本条，下一轮维护会重新收集并
                    // 回退，因此不会永久停留在非 finish 状态。占位持有到写回结束，期间新的 claim
                    // 会等待，删除方也会等它释放后才删文件与 meta，因此不会覆盖运行中任务的 meta，
                    // 也不会在删除后写回幽灵 meta。拿到占位后复查路径仍存在，再重新读取最新 meta，
                    // 只修改 status 后写回。
                    // Reserve the recording identity before the read-modify-write (same as the scan's
                    // A3 and the listing's size write-back C1): the reservation fails while a
                    // post-processing claim, another reservation or a removal is in progress, and this
                    // entry is skipped; the next maintenance pass collects and reverts it again, so it
                    // never stays in a non-finish state for good. The reservation is held until the
                    // write-back finishes; new claims wait meanwhile and a deletion waits for it to be
                    // released before removing the files and meta, so neither a running task's meta
                    // is overwritten nor a ghost meta written back after a deletion. After reserving,
                    // re-check that the path still exists, then re-read the latest meta and only
                    // change status.
                    let Some(_reservation) = state
                        .pp_queue
                        .try_reserve(&crate::postprocess::queue::recording_key(path))
                    else {
                        continue;
                    };
                    if !path.exists() {
                        continue;
                    }
                    if let Some(mut meta) = read_meta(path) {
                        meta.status = "finish".to_string();
                        write_meta(path, &meta);
                    }
                }
            })
            .await;
        } else {
            // 同时启动所有任务，由信号量控制实际并发度；任务在后台运行，本次维护不再等待
            // 它们完成（幂等由录制身份 claim 保证），因此后续空目录清理与 tmp 清理可能与
            // 后处理并行，空目录清理靠最小年龄保护（见 segment_merge::startup_remove_empty_dirs）
            // Launch all tasks concurrently; the semaphore controls actual parallelism. The tasks
            // run in the background and this pass no longer waits for them (idempotency is
            // guaranteed by the recording-identity claim), so the following empty-dir and tmp
            // cleanup may run in parallel with post-processing; empty-dir cleanup is protected by a
            // minimum age (see segment_merge::startup_remove_empty_dirs)
            for video_path in pp_pending {
                let pp_state = Arc::clone(&app_state);
                let pp_emitter = Arc::clone(&emitter);
                let pp_pipeline = pipeline.clone();
                drop(tokio::task::spawn_blocking(move || {
                    // 触发前复核：扫描到触发之间可能已有其他入口完成了该录制，或路径已被移除
                    // Re-check before triggering: another entry may have finished this recording,
                    // or the path may have been removed, between scan and trigger
                    if !video_path.exists()
                        || read_meta(&video_path).is_some_and(|m| m.status == "finish")
                    {
                        tracing::info!(
                            "{}",
                            crate::tl!(
                                "meta.scanSkipFinishedBeforeTrigger",
                                path = video_path.display()
                            )
                        );
                        return;
                    }
                    crate::postprocess::service::run_postprocess_for_path(
                        &video_path,
                        &video_path,
                        &pp_pipeline,
                        &pp_emitter,
                        &pp_state,
                    );
                }));
            }
        }
    } // end else pp_pending non-empty

    // 步骤 3：清理空目录（之前的后处理可能遗留空目录）。此时本次触发的后处理可能仍在
    // 运行，清理会跳过 60 秒内修改过的目录，剩余的空目录由下一次维护清理
    // Step 3: remove empty directories (earlier post-processing may have left some). Post-
    // processing triggered by this pass may still be running; cleanup skips directories
    // modified within the last 60 seconds, and the rest is handled by the next pass
    let recorder_for_cleanup = Arc::clone(&recorder);
    let _ = tokio::task::spawn_blocking(move || {
        crate::recording::segment_merge::startup_remove_empty_dirs(
            &output_dir,
            &recorder_for_cleanup,
        );
    })
    .await;

    // 步骤 4：清理 tmp 目录中超过 1 小时未修改的残留文件和空子目录，
    // 跳过正在排队/执行后处理的录制的临时文件
    // Step 4: clean up stale files (older than 1 h) and empty subdirectories in the tmp dir,
    // skipping temp files of recordings that are queued for or running post-processing
    let max_tmp_gb = app_state.get_settings().max_tmp_dir_gb;
    let tmp_dir = crate::config::app_state::exe_dir().join("tmp");
    let protected_stems = app_state.pp_queue.active_recording_stems();
    let _ = tokio::task::spawn_blocking(move || {
        cleanup_stale_tmp(&tmp_dir, max_tmp_gb, &protected_stems);
    })
    .await;

    // 启动扫描完成后（不等待扫描触发的后处理结束），通知写入 store
    // （不推 SSE，前端通过 startup-scan-done 触发 fetch 后拉取）
    // After the startup scan completes (without waiting for the post-processing it triggered),
    // write notification to store (no SSE push here; the frontend fetches it after receiving
    // startup-scan-done)
    if is_startup {
        use crate::core::emitter::EmitterExt;
        // 静默写入 Info 通知（不推 notification-created SSE，避免弹 toast）
        // Silently write Info notification (no notification-created SSE, no toast)
        app_state.notification_store.push_i18n(
            crate::core::notifications::NotificationLevel::Info,
            "startup_scan",
            "Startup scan completed",
            "notifications.backend.startupScanDone",
            None,
        );
        // 推 startup-scan-done 信号，前端收到后 fetch 通知列表（只加面板，不弹 toast）
        // Signal the frontend to fetch the notification list (panel only, no toast)
        emitter.emit("startup-scan-done", &serde_json::json!({}));
    }
}

/// 启动 meta 版本检查轮询调度器：立即执行一次，之后每隔指定秒数执行一次。
/// 每次执行都是一次完整的 [`maintain_output_dir`] 维护流程，与程序启动时的
/// 一次性检查逻辑完全一致，因此启动时无需再单独执行一遍。
///
/// Start the meta version-check polling scheduler: run once immediately, then at the
/// specified interval. Each run is a full [`maintain_output_dir`] pass, identical to the
/// one-shot check performed at startup — so startup no longer needs a separate pass.
pub async fn schedule_meta_version_check(
    app_state: Arc<crate::config::app_state::AppState>,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
    interval_secs: u64,
) {
    maintain_output_dir(
        Arc::clone(&app_state),
        Arc::clone(&emitter),
        Arc::clone(&recorder),
        true, // 启动时首次扫描，发通知 / First run on startup, emit notifications
    )
    .await;
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
        maintain_output_dir(
            Arc::clone(&app_state),
            Arc::clone(&emitter),
            Arc::clone(&recorder),
            false, // 定时扫描，静默 / Periodic scan, silent
        )
        .await;
    }
}
