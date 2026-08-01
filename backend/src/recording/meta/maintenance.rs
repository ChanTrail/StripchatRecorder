//! Meta 定时维护与孤立清理 / Meta Scheduled Maintenance and Orphan Cleanup
//!
//! 提供孤立 meta 文件清理、输出目录维护主流程（[`maintain_output_dir`]）及其
//! 定时调度封装。程序启动时的一次性检查与周期性维护共用同一套逻辑
//! （见 [`maintain_output_dir`] 的调用方式）。
//!
//! Provides orphaned meta file cleanup, the output-directory maintenance main flow
//! ([`maintain_output_dir`]), and its scheduled wrappers. The one-shot startup check
//! and periodic maintenance share identical logic (see how [`maintain_output_dir`] is
//! invoked).

use super::model::{VideoMeta, meta_dir};
use super::scan::{ensure_meta_files, ts_merge_output_dir};
use super::store::{read_meta, write_meta};
use std::path::Path;
use std::sync::Arc;

/// 扫描集中 meta/ 目录，删除所有对应视频文件（或 session_dir）已不存在的孤立 meta 文件。
///
/// 孤立判断的唯一依据：`video_path` 字段指向的路径（目录或视频文件）是否存在。
/// 不再叠加任何基于 `status` 的前置过滤——`status` 是否为 "recording"/"pp_waiting"/
/// "pp_running" 与孤立判断无关：只要该路径确实存在（无论录制还是处理中都会持续占用该
/// 路径），meta 自然不会被判定为孤立；只要该路径不存在，无论 status 停留在什么值
/// （包括因进程重启等原因卡在中间状态的陈旧记录），都应视为孤立并清理。
/// 之前基于 status 的前置跳过会掩盖这类陈旧记录，导致孤立 meta 无法被清理。
///
/// Scan the centralized meta/ directory and delete orphaned meta files whose
/// corresponding video file or session_dir no longer exists.
///
/// The sole criterion for "orphaned": whether the path referenced by `video_path`
/// (a directory or video file) exists. No status-based pre-filter is applied anymore —
/// whether `status` is "recording"/"pp_waiting"/"pp_running" is irrelevant to the orphan
/// check: as long as the path genuinely exists (recording or processing both keep the path
/// present), the meta naturally won't be flagged as orphaned; as long as the path doesn't
/// exist, it's orphaned and should be cleaned up regardless of what `status` says (including
/// stale records stuck mid-state due to a process restart). The previous status-based
/// pre-filter masked exactly these stale records, preventing orphaned meta from being cleaned.
pub fn cleanup_orphaned_meta_files() -> usize {
    let dir = meta_dir();
    if !dir.exists() {
        return 0;
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.ends_with(".json") => n.to_string(),
            _ => continue,
        };

        let meta_content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: VideoMeta = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // 没有 video_path 字段时无法判断，保守跳过（旧版 meta 无此字段）
        // Cannot determine without video_path; skip conservatively (old meta format lacks it)
        let video_path_str = match meta.video_path.as_deref() {
            Some(p) => p.to_string(),
            None => continue,
        };

        // 唯一依据：路径存在则不是孤立 / Sole criterion: path exists → not orphaned
        if std::path::Path::new(&video_path_str).exists() {
            continue;
        }

        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!("Meta cleanup: failed to delete {}: {}", name, e);
        } else {
            tracing::info!("Meta cleanup: deleted orphaned meta {}", name);
            count += 1;
        }
    }

    if count > 0 {
        tracing::info!("Meta cleanup: deleted {} orphaned meta file(s)", count);
    }
    count
}

/// 启动孤立 meta 清理调度器：立即执行一次，之后每小时执行一次。
/// Start the orphaned meta cleanup scheduler: run once immediately, then once every hour.
pub async fn schedule_meta_cleanup() {
    tokio::task::spawn_blocking(cleanup_orphaned_meta_files).await.ok();
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        tokio::task::spawn_blocking(cleanup_orphaned_meta_files).await.ok();
    }
}

/// 执行一次完整的输出目录维护：清理空目录、重建缺失/损坏的 meta、触发遗漏的后处理。
/// 程序启动时和定时任务共用同一份逻辑，行为完全一致。
///
/// 不再单独"合并遗留 TS 分片"——是否需要合并完全交给流水线首节点（ts_merge）
/// 自行判断：只要触发后处理时把 session_dir 同时作为 `initial_path` 传入，
/// ts_merge 发现输入是目录就会合并，是文件就直接透传，无需在这里重复实现。
///
/// 执行顺序：
/// 1. 扫描输出目录（含 ts_merge 自定义输出目录），重建缺失/损坏/版本过旧的 meta，
///    收集所有需要（重新）触发后处理的路径（可能是视频文件，也可能是未合并的 session_dir）
/// 2. 流水线为空时将待处理任务的 meta 回退为 `finish`；否则逐个串行触发后处理
/// 3. 删除扫描过程中可能遗留的空目录
///
/// Run one full output-directory maintenance pass: remove empty directories, rebuild
/// missing/corrupt meta, and trigger missed post-processing.
/// Shared by both the startup path and the periodic scheduler so behavior stays identical.
///
/// No longer merges leftover TS segments as a separate step — whether merging is needed is
/// entirely up to the pipeline's first node (ts_merge): as long as the session_dir is passed
/// as `initial_path` when triggering post-processing, ts_merge merges it if it's a directory
/// or passes it through if it's already a file. No need to duplicate that logic here.
///
/// Execution order:
/// 1. Scan the output directory (plus ts_merge's custom output dir), rebuild missing/
///    corrupt/outdated meta, and collect all paths needing post-processing (re-)triggered
///    (either video files or unmerged session_dirs)
/// 2. Revert pending tasks' meta to `finish` when the pipeline is empty; otherwise trigger
///    post-processing for them one at a time
/// 3. Remove any empty directories left over from the scan
pub async fn maintain_output_dir(
    app_state: Arc<crate::config::settings::AppState>,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
) {
    let settings = app_state.get_settings();
    let output_dir = std::path::PathBuf::from(&settings.output_dir);

    // 步骤 1：同步阻塞操作，在 spawn_blocking 内执行
    // Step 1: synchronous blocking operations, run inside spawn_blocking
    let (pp_pending, pipeline) = tokio::task::spawn_blocking({
        let app_state = Arc::clone(&app_state);
        let recorder = Arc::clone(&recorder);
        let output_dir = output_dir.clone();
        move || {
            let ts_merge_extra = ts_merge_output_dir(&app_state);
            let extra_refs: Vec<&Path> = ts_merge_extra.iter().map(|p| p.as_path()).collect();
            let pp_pending = ensure_meta_files(&output_dir, &extra_refs, &app_state, &recorder);
            let pipeline = app_state.get_pipeline();
            (pp_pending, pipeline)
        }
    })
    .await
    .unwrap_or_default();

    // 步骤 2：流水线为空时回退 meta 状态；否则逐个串行触发后处理。
    // 按录制开始时间（meta.started_at）升序排序，保证最旧的任务优先处理，
    // 与录制结束后自动触发的行为一致（两者共用同一 pp_queue.serial_lock，
    // 任何手动触发或录制结束触发都会被追加到队列末尾，不会与启动扫描的任务并发）。
    //
    // Step 2: revert meta status when pipeline is empty; otherwise trigger pp serially.
    // Sort by recording start time (meta.started_at) ascending so the oldest task runs first,
    // consistent with the auto-trigger behavior after recording ends (both paths share the same
    // pp_queue.serial_lock, so any manual or post-recording trigger appends to the queue and
    // doesn't run concurrently with startup-scan tasks).
    let mut pp_pending = pp_pending;
    pp_pending.sort_by_key(|p| {
        crate::recording::meta::read_meta(p)
            .map(|m| m.started_at)
            .unwrap_or_default()
    });

    if pp_pending.is_empty() {
        // 步骤 3：仍需清理可能遗留的空目录 / Step 3: still clean up any leftover empty dirs
        let _ = tokio::task::spawn_blocking(move || {
            crate::recording::startup_scan::startup_remove_empty_dirs(&output_dir);
        })
        .await;
        return;
    }

    if !pipeline.nodes.iter().any(|n| n.enabled) {
        tracing::info!(
            "Meta scan: {} video(s) need post-processing but pipeline is empty, skipping",
            pp_pending.len()
        );
        let _ = tokio::task::spawn_blocking(move || {
            for path in &pp_pending {
                if let Some(mut meta) = read_meta(path) {
                    meta.status = "finish".to_string();
                    write_meta(path, &meta);
                }
            }
            // 步骤 3 / Step 3
            crate::recording::startup_scan::startup_remove_empty_dirs(&output_dir);
        })
        .await;
        return;
    }

    for video_path in pp_pending {
        let pp_state = Arc::clone(&app_state);
        let pp_emitter = Arc::clone(&emitter);
        let pp_pipeline = pipeline.clone();
        tokio::task::spawn_blocking(move || {
            crate::commands::postprocess_cmd::run_postprocess_for_path(
                &video_path,
                &video_path,
                &pp_pipeline,
                &pp_emitter,
                &pp_state,
            );
        })
        .await
        .ok();
    }

    // 步骤 3：后处理（含 ts_merge 合并）可能遗留空目录，统一清理一次
    // Step 3: post-processing (including ts_merge merges) may leave empty directories; clean up once
    let _ = tokio::task::spawn_blocking(move || {
        crate::recording::startup_scan::startup_remove_empty_dirs(&output_dir);
    })
    .await;
}

/// 启动 meta 版本检查轮询调度器：立即执行一次，之后每隔指定秒数执行一次。
/// 每次执行都是一次完整的 [`maintain_output_dir`] 维护流程，与程序启动时的
/// 一次性检查逻辑完全一致，因此启动时无需再单独执行一遍。
///
/// Start the meta version-check polling scheduler: run once immediately, then at the
/// specified interval. Each run is a full [`maintain_output_dir`] pass, identical to the
/// one-shot check performed at startup — so startup no longer needs a separate pass.
pub async fn schedule_meta_version_check(
    app_state: Arc<crate::config::settings::AppState>,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
    interval_secs: u64,
) {
    maintain_output_dir(
        Arc::clone(&app_state),
        Arc::clone(&emitter),
        Arc::clone(&recorder),
    )
    .await;
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
        maintain_output_dir(
            Arc::clone(&app_state),
            Arc::clone(&emitter),
            Arc::clone(&recorder),
        )
        .await;
    }
}
