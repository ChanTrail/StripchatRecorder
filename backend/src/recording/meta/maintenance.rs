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

use super::model::{
    VideoMeta, legacy_truncated_meta_target, list_all_meta_paths, list_all_meta_paths_in, meta_dir,
    meta_dir_for, username_from_path,
};
use super::scan::{ensure_meta_files, ts_merge_output_dir};
use super::store::{read_meta, write_meta};
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

/// 一次性迁移：将 `meta_dir()` 根目录下的旧版扁平 meta 文件（升级前生成，直接平铺
/// 存放、不含主播子目录）移动到按主播分子目录的新结构（`meta_dir()/{username}/{stem}.json`）。
///
/// 每个文件的目标子目录从其自身 `video_path` 字段推断用户名（[`username_from_path`]）；
/// 若文件内容无法解析或缺少 `video_path` 字段，保守跳过（保留在原位，不强行猜测，
/// 后续若确实孤立会被 [`cleanup_orphaned_meta_files`] 处理，但由于本函数仅基于文件
/// 内容判断而非路径存在性，跳过不会误删任何数据）。已经位于子目录下的文件不受影响。
///
/// 应在程序启动时、任何其他 meta 扫描/读取发生之前调用一次；重复调用是幂等的——
/// 已迁移的文件不再存在于根目录，不会被再次处理。
///
/// One-shot migration: move legacy flat meta files directly under `meta_dir()` root
/// (generated before this change, with no per-streamer subdirectory) into the new
/// per-streamer layout (`meta_dir()/{username}/{stem}.json`).
///
/// Each file's target subdirectory is inferred from its own `video_path` field
/// ([`username_from_path`]); files that fail to parse or lack `video_path` are skipped
/// conservatively (left in place rather than guessed at — if genuinely orphaned they'll
/// later be handled by [`cleanup_orphaned_meta_files`], and since this function only
/// acts based on file content rather than path existence, skipping never deletes data).
/// Files already inside a subdirectory are untouched.
///
/// Must be called once at startup, before any other meta scan/read happens; repeated
/// calls are idempotent — migrated files no longer exist at the root and won't be
/// reprocessed.
pub fn migrate_flat_meta_files() -> usize {
    let dir = meta_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut migrated = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        // 子目录（新结构）或非 json 文件，跳过 / Subdirectory (new layout) or non-json file, skip
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: VideoMeta = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let video_path_str = match meta.video_path.as_deref() {
            Some(p) => p,
            None => continue,
        };

        let username = username_from_path(Path::new(video_path_str));
        let target_dir = meta_dir_for(&username);
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            tracing::warn!("{}", crate::tl!("meta.migrationCreateDirFailed", dir = target_dir.display(), error = e));
            continue;
        }
        let Some(file_name) = path.file_name() else { continue };
        let target_path = target_dir.join(file_name);
        if target_path.exists() {
            tracing::warn!("{}", crate::tl!("meta.migrationTargetExists", path = path.display()));
            continue;
        }
        match std::fs::rename(&path, &target_path) {
            Ok(()) => {
                tracing::info!("{}", crate::tl!("meta.migrationMoved", from = path.display(), to = target_path.display()));
                migrated += 1;
            }
            Err(e) => {
                tracing::warn!("{}", crate::tl!("meta.migrationMoveFailed", path = path.display(), error = e));
            }
        }
    }

    if migrated > 0 {
        tracing::info!("{}", crate::tl!("meta.migrationDone", count = migrated)
        );
    }
    migrated
}

/// 一次性迁移：把按旧 stem 规则（`file_stem()`）截断文件名的 meta 重命名为新规则
/// （[`super::model::recording_stem`]）下的文件名（B16 兼容）。
///
/// 旧规则会把含 `.` 用户名的 session_dir（如 `a.b_20240101_120000`）截断成 `a.json`。
/// 判断依据是 meta 自身的 `video_path`（见 [`legacy_truncated_meta_target`]）；普通用户名
/// 不受影响。只在同一目录内 rename，不删除任何文件；目标已存在时保留原文件并记录警告。
/// 必须在首次扫描前调用（两端启动时都会调用），重复调用是幂等的。
///
/// One-shot migration: rename meta files whose names were truncated by the old stem rule
/// (`file_stem()`) to the name under the new rule ([`super::model::recording_stem`]) (B16
/// compatibility).
///
/// The old rule truncated a session_dir with a `.` in the username (e.g.
/// `a.b_20240101_120000`) to `a.json`. The decision is based on the meta's own
/// `video_path` (see [`legacy_truncated_meta_target`]); plain usernames are unaffected.
/// Only renames within the same directory and never deletes files; when the target
/// already exists the original is kept and a warning is logged. Must be called before the
/// first scan (both ends call it at startup); repeated calls are idempotent.
pub fn migrate_truncated_stem_meta_files() -> usize {
    migrate_truncated_stem_meta_files_in(&meta_dir())
}

/// [`migrate_truncated_stem_meta_files`] 的实现，作用于指定 meta 根目录。
/// Implementation of [`migrate_truncated_stem_meta_files`] for the given meta root.
fn migrate_truncated_stem_meta_files_in(root: &Path) -> usize {
    let mut count = 0usize;
    for file in list_all_meta_paths_in(root) {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<VideoMeta>(&content) else {
            continue;
        };
        let Some(vp) = meta.video_path.as_deref() else {
            continue;
        };
        let Some(target) = legacy_truncated_meta_target(&file, Path::new(vp)) else {
            continue;
        };
        if target.exists() {
            tracing::warn!(
                "{}",
                crate::tl!("meta.migrationStemTargetExists", path = file.display(), to = target.display())
            );
            continue;
        }
        match std::fs::rename(&file, &target) {
            Ok(()) => {
                tracing::info!(
                    "{}",
                    crate::tl!("meta.migrationStemMoved", from = file.display(), to = target.display())
                );
                count += 1;
            }
            Err(e) => {
                tracing::warn!(
                    "{}",
                    crate::tl!("meta.migrationStemMoveFailed", path = file.display(), error = e)
                );
            }
        }
    }
    if count > 0 {
        tracing::info!("{}", crate::tl!("meta.migrationStemDone", count = count));
    }
    count
}

/// 扫描 meta/ 目录（含所有主播子目录），删除所有对应视频文件（或 session_dir）
/// 已不存在的孤立 meta 文件；之后每轮都会移除已空置超过 60 秒的主播子目录
/// （见 [`remove_empty_meta_subdirs`]，本轮刚变空的子目录由下一轮清理）。
///
/// 孤立判断的唯一依据：`video_path` 字段指向的路径（目录或视频文件）是否存在。
/// 不再叠加任何基于 `status` 的前置过滤——`status` 是否为 "recording"/"pp_waiting"/
/// "pp_running" 与孤立判断无关：只要该路径确实存在（无论录制还是处理中都会持续占用该
/// 路径），meta 自然不会被判定为孤立；只要该路径不存在，无论 status 停留在什么值
/// （包括因进程重启等原因卡在中间状态的陈旧记录），都应视为孤立并清理。
/// 之前基于 status 的前置跳过会掩盖这类陈旧记录，导致孤立 meta 无法被清理。
///
/// Scan the meta/ directory (including all per-streamer subdirectories) and delete
/// orphaned meta files whose corresponding video file or session_dir no longer exists;
/// afterwards, every pass removes streamer subdirectories that have been empty for more than
/// 60 seconds (see [`remove_empty_meta_subdirs`]; subdirectories emptied in this pass are
/// removed by the next one).
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
    let paths = list_all_meta_paths();

    let mut count = 0usize;
    for path in paths {
        let name = path.to_string_lossy().to_string();

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
            tracing::warn!("{}", crate::tl!("meta.cleanupDeleteFailed", name = name, error = e));
        } else {
            tracing::info!("{}", crate::tl!("meta.cleanupDeleted", name = name));
            count += 1;
        }
    }

    if count > 0 {
        tracing::info!("{}", crate::tl!("meta.cleanupDone", count = count));
    }
    // 每轮都清理空子目录（不只在本轮删过 meta 时）：有最小年龄保护后，本轮因删掉最后一个
    // meta 而变空的子目录修改时间刚被刷新，会被本轮跳过，需由下一轮清理
    // Clean up empty subdirectories on every pass (not only when this pass deleted meta): with
    // the minimum-age protection, a subdirectory emptied by deleting its last meta in this pass
    // has a fresh mtime and is skipped now, so a later pass has to remove it
    remove_empty_meta_subdirs();
    count
}

/// 移除 meta 根目录下已变空的主播子目录（如该主播的所有录制都已被删除/清理）。
///
/// 复用输出目录空目录清理的最小年龄保护（`segment_merge::EMPTY_DIR_MIN_AGE`）：修改时间
/// 不足 60 秒或读不到修改时间的目录跳过，删除前再确认目录为空，meta 根目录本身不删。
/// 保护的是 `write_meta` 在 `create_dir_all` 之后、写临时文件之前的窗口：新主播的子目录
/// 刚建好、尚未写入 meta 时不会被删掉。本轮跳过的旧空目录由后续轮次清理。
///
/// Remove now-empty streamer subdirectories under the meta root (e.g. all of a
/// streamer's recordings have been deleted/cleaned up).
///
/// Reuses the output-dir empty-dir cleanup's minimum-age protection
/// (`segment_merge::EMPTY_DIR_MIN_AGE`): directories modified within the last 60 seconds, or
/// whose mtime can't be read, are skipped; emptiness is confirmed right before removal, and
/// the meta root itself is never removed. This protects `write_meta`'s window between
/// `create_dir_all` and writing the temp file: a new streamer's subdirectory that was just
/// created and has no meta yet isn't removed. Old empty directories skipped in this pass are
/// cleaned up by later passes.
fn remove_empty_meta_subdirs() {
    remove_empty_meta_subdirs_in(&meta_dir());
}

/// [`remove_empty_meta_subdirs`] 的实现，作用于指定 meta 根目录，返回删除的目录数。
/// meta 子目录只有一层，递归版本的效果等同于一层检查。
///
/// Implementation of [`remove_empty_meta_subdirs`] for the given meta root, returning the
/// number of directories removed. Meta subdirectories are a single level deep, so the
/// recursive helper behaves the same as a one-level check.
fn remove_empty_meta_subdirs_in(root: &Path) -> usize {
    crate::recording::segment_merge::remove_empty_dirs_recursive(
        root,
        false,
        &|_| false,
        crate::recording::segment_merge::EMPTY_DIR_MIN_AGE,
    )
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
/// 2. 流水线为空时逐条按录制身份占位后，将待处理任务的 meta 回退为 `finish`（拿不到
///    占位的条目本轮跳过，由下一轮维护处理）；否则逐个触发后处理，
///    任务在后台运行（实际并发度由 pp_queue 信号量控制，幂等由录制身份 claim 保证），
///    本函数不等待它们完成
/// 3. 删除空目录（可能与后处理并行，跳过 60 秒内修改过的目录，避免误删 ts_merge
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
/// entirely up to the pipeline's first node (ts_merge): as long as the session_dir is passed
/// as `initial_path` when triggering post-processing, ts_merge merges it if it's a directory
/// or passes it through if it's already a file. No need to duplicate that logic here.
///
/// Execution order:
/// 1. Scan the output directory (plus ts_merge's custom output dir), rebuild missing/
///    corrupt/outdated meta, and collect all paths needing post-processing (re-)triggered
///    (either video files or unmerged session_dirs)
/// 2. Revert pending tasks' meta to `finish` when the pipeline is empty, reserving each
///    recording identity first (entries whose reservation fails are skipped this pass and
///    handled by the next one); otherwise trigger
///    post-processing for each of them; the tasks run in the background (parallelism is
///    governed by the pp_queue semaphore, idempotency by the recording-identity claim) and
///    this function does not wait for them to finish
/// 3. Remove empty directories (may run in parallel with post-processing; directories
///    modified within the last 60 seconds are skipped so freshly created ts_merge output
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
            let ts_merge_extra = ts_merge_output_dir(&app_state);
            let extra_refs: Vec<&Path> = ts_merge_extra.iter().map(|p| p.as_path()).collect();
            let pp_pending = ensure_meta_files(&output_dir, &extra_refs, &app_state, &recorder, is_startup);
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
            let name = path.file_name()
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
                    format!("Found 1 unfinished post-processing task from a previous crash, re-triggering: {}", stale_names[0]),
                    "notifications.backend.ppRemainingOne",
                    a,
                )
            } else {
                let mut a = HashMap::new();
                a.insert("count".to_string(), serde_json::json!(count));
                a.insert("names".to_string(), serde_json::json!(joined));
                (
                    format!("Found {} unfinished post-processing tasks from a previous crash, re-triggering: {}", count, joined),
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
                    format!("1 post-processing task failed last time, auto-retrying on startup: {}", retry_names[0]),
                    "notifications.backend.ppRetryOne",
                    a,
                )
            } else {
                let mut a = HashMap::new();
                a.insert("count".to_string(), serde_json::json!(count));
                a.insert("names".to_string(), serde_json::json!(joined));
                (
                    format!("{} post-processing tasks failed last time, auto-retrying on startup: {}", count, joined),
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
        tracing::info!("{}", crate::tl!("meta.scanSkipEmpty", count = pp_pending.len()));
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
                        crate::tl!("meta.scanSkipFinishedBeforeTrigger", path = video_path.display())
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
        crate::recording::segment_merge::startup_remove_empty_dirs(&output_dir, &recorder_for_cleanup);
    })
    .await;

    // 步骤 4：清理 tmp 目录中超过 24 小时未访问的残留文件和空子目录
    // Step 4: clean up stale files (older than 24 h) and empty subdirectories in the tmp dir
    let max_tmp_gb = app_state.get_settings().max_tmp_dir_gb;
    let tmp_dir = crate::config::app_state::exe_dir().join("tmp");
    let _ = tokio::task::spawn_blocking(move || {
        cleanup_stale_tmp(&tmp_dir, max_tmp_gb);
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
        true,  // 启动时首次扫描，发通知 / First run on startup, emit notifications
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

/// 清理 tmp 目录中的过期内容：
///
/// 1. **过期文件**：修改时间超过 24 小时的文件直接删除（不受大小限制影响）。
///    这类文件是后处理任务失败/中断时遗留的中间产物，不再被任何运行中任务使用。
/// 2. **空子目录**：删除文件后遗留的空子目录一并清理（跳过 60 秒内修改过的目录）。
/// 3. **大小上限兜底**：若清理过期文件后目录仍超出 `max_tmp_gb` 限制，
///    再按修改时间从旧到新继续删文件，直到大小低于上限。
///
/// `max_tmp_gb` 为 0 时跳过大小兜底逻辑（不限制大小，但仍清理 24 小时过期文件）。
///
/// Clean up stale content in the tmp directory:
///
/// 1. **Stale files**: files not modified within the last 24 hours are deleted unconditionally
///    (regardless of size limit). These are leftover intermediates from failed/interrupted
///    post-processing tasks that no running task is using anymore.
/// 2. **Empty subdirectories**: empty subdirectories left after file deletion are also removed
///    (directories modified within the last 60 seconds are skipped).
/// 3. **Size cap fallback**: if the directory still exceeds `max_tmp_gb` after removing
///    stale files, continue deleting from oldest to newest until under the limit.
///
/// When `max_tmp_gb` is 0, the size cap fallback is skipped (size is unlimited, but
/// 24-hour stale file cleanup still runs).
fn cleanup_stale_tmp(tmp: &std::path::Path, max_tmp_gb: f64) {
    if !tmp.exists() {
        return;
    }

    let max_age = std::time::Duration::from_secs(24 * 3600);
    let now = std::time::SystemTime::now();

    // 递归收集所有文件（含子目录内文件）
    // Recursively collect all files (including inside subdirectories)
    let mut all_files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
    collect_files_recursive(tmp, &mut all_files);

    let mut removed_bytes: u64 = 0;
    let mut removed_count: usize = 0;

    // 第一轮：删除超过 24 小时未修改的文件
    // Round 1: delete files not modified within the last 24 hours
    let mut remaining_files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for (path, size, modified) in all_files {
        let age = now.duration_since(modified).unwrap_or(max_age);
        if age >= max_age {
            if std::fs::remove_file(&path).is_ok() {
                removed_bytes += size;
                removed_count += 1;
            }
        } else {
            remaining_files.push((path, size, modified));
        }
    }

    // 第二轮：大小兜底——若仍超出上限，从旧到新继续删
    // Round 2: size fallback — if still over cap, delete from oldest to newest
    if max_tmp_gb > 0.0 {
        let max_bytes = (max_tmp_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        let current: u64 = remaining_files.iter().map(|(_, s, _)| s).sum();
        if current > max_bytes {
            remaining_files.sort_by_key(|(_, _, t)| *t);
            let mut total = current;
            for (path, size, _) in &remaining_files {
                if total <= max_bytes {
                    break;
                }
                if std::fs::remove_file(path).is_ok() {
                    removed_bytes += size;
                    removed_count += 1;
                    total = total.saturating_sub(*size);
                }
            }
        }
    }

    // 清理空子目录（递归，从深到浅，不删 tmp 根目录）。本函数可能与维护触发的后台后处理
    // 并行，因此复用输出目录空目录清理的最小年龄保护：跳过 60 秒内修改过的目录，避免误删
    // notify_telegram 刚创建、ffmpeg 尚未写入的 split_* 目录。本轮刚删掉文件或子目录的
    // 目录修改时间会被刷新，由下一次维护（5 分钟后）清理。
    // Remove empty subdirectories (recursive, deepest first, keeping the tmp root). This may
    // run in parallel with background post-processing triggered by maintenance, so reuse the
    // output dir cleanup's minimum-age protection: skip directories modified within the last
    // 60 seconds, so split_* dirs just created by notify_telegram and not yet written by ffmpeg
    // aren't removed. Directories whose files/subdirs were removed in this pass get a fresh
    // mtime and are cleaned up by the next maintenance pass (5 minutes later).
    crate::recording::segment_merge::remove_empty_dirs_recursive(
        tmp,
        false,
        &|_| false,
        crate::recording::segment_merge::EMPTY_DIR_MIN_AGE,
    );

    if removed_count > 0 {
        tracing::info!(
            "{}",
            crate::tl!("maintenance.tmpCleanup", count = removed_count, mb = format!("{:.1}", removed_bytes as f64 / 1024.0 / 1024.0))
        );
    }
}

/// 递归收集目录下所有文件及其元数据。
fn collect_files_recursive(
    dir: &std::path::Path,
    out: &mut Vec<(std::path::PathBuf, u64, std::time::SystemTime)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, out);
        } else if let Ok(meta) = std::fs::metadata(&path) {
            let size = meta.len();
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push((path, size, modified));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写一份只含必填字段与 video_path 的 meta / Write a meta with required fields plus video_path
    fn write_test_meta(file: &Path, video_path: &str) {
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        let json = serde_json::json!({
            "status": "finish",
            "started_at": "",
            "size_bytes": 0,
            "video_path": video_path,
        });
        std::fs::write(file, json.to_string()).expect("write meta");
    }

    /// 截断文件名的旧 meta 被重命名为新 stem 规则下的文件名；普通用户名的 meta 不动。
    /// A legacy meta with a truncated name is renamed to the new-rule name; plain-username meta is untouched.
    #[test]
    fn truncated_stem_meta_is_renamed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let old = root.join("a.b").join("a.json");
        write_test_meta(&old, "X:/ts/a.b/a.b_20240101_120000");
        let plain = root.join("alice").join("alice_20240101_120000.json");
        write_test_meta(&plain, "X:/ts/alice/alice_20240101_120000");

        assert_eq!(migrate_truncated_stem_meta_files_in(root), 1);
        assert!(!old.exists());
        assert!(root.join("a.b").join("a.b_20240101_120000.json").is_file());
        assert!(plain.is_file());
        // 幂等 / Idempotent
        assert_eq!(migrate_truncated_stem_meta_files_in(root), 0);
    }

    /// 目标已存在时两个文件都保留，返回 0。
    /// When the target already exists both files are kept and 0 is returned.
    #[test]
    fn truncated_stem_meta_kept_when_target_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let old = root.join("a.b").join("a.json");
        write_test_meta(&old, "X:/ts/a.b/a.b_20240101_120000");
        let target = root.join("a.b").join("a.b_20240101_120000.json");
        write_test_meta(&target, "X:/rec/a.b/a.b_20240101_120000.mp4");

        assert_eq!(migrate_truncated_stem_meta_files_in(root), 0);
        assert!(old.is_file());
        assert!(target.is_file());
    }

    /// 维护守卫单实例：持有期间再次获取失败，drop 后可再次获取。
    /// （唯一使用 MAINTENANCE_RUNNING 静态变量的测试，避免并行测试互相干扰）
    /// Maintenance guard is single-instance: a second acquire fails while held, succeeds after drop.
    /// (The only test touching the MAINTENANCE_RUNNING static, so parallel tests don't interfere)
    #[test]
    fn maintenance_guard_is_single_instance() {
        let first = MaintenanceGuard::try_acquire();
        assert!(first.is_some());
        assert!(MaintenanceGuard::try_acquire().is_none());
        drop(first);
        let again = MaintenanceGuard::try_acquire();
        assert!(again.is_some());
    }

    /// tmp 清理不删除刚创建的空子目录（如 notify_telegram 刚建好、ffmpeg 尚未写入的
    /// split_* 目录），也不删除新文件与 tmp 根目录。
    /// The tmp cleanup keeps freshly created empty subdirectories (e.g. a split_* dir just
    /// created by notify_telegram and not yet written by ffmpeg), new files and the tmp root.
    #[test]
    fn stale_tmp_cleanup_keeps_fresh_empty_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tmp = dir.path();
        let split = tmp.join("split_alice_1_2");
        let nested = split.join("_split_tmp_alice_0");
        std::fs::create_dir_all(&nested).expect("mkdir split");
        let other = tmp.join("other");
        std::fs::create_dir_all(&other).expect("mkdir other");
        std::fs::write(other.join("file.bin"), b"x").expect("write file");

        cleanup_stale_tmp(tmp, 0.0);
        assert!(nested.is_dir());
        assert!(split.is_dir());
        assert!(other.join("file.bin").is_file());
        assert!(tmp.is_dir());
    }

    /// meta 空子目录清理不删除刚创建的空主播子目录（write_meta 刚建好、尚未写入 meta），
    /// 也不删除含 meta 的子目录与 meta 根目录。
    /// The meta empty-subdirectory cleanup keeps a freshly created empty streamer subdirectory
    /// (just created by write_meta, no meta written yet), subdirectories holding meta, and the
    /// meta root.
    #[test]
    fn meta_subdir_cleanup_keeps_fresh_empty_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let alice = root.join("alice");
        std::fs::create_dir_all(&alice).expect("mkdir alice");
        let bob_meta = root.join("bob").join("x.json");
        write_test_meta(&bob_meta, "X:/rec/bob/x.mp4");

        assert_eq!(remove_empty_meta_subdirs_in(root), 0);
        assert!(alice.is_dir());
        assert!(bob_meta.is_file());
        assert!(root.is_dir());
    }
}
