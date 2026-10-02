//! Meta 扫描、修复与重建 / Meta Scanning, Repair, and Rebuild
//!
//! 扫描输出目录（及 ts_merge 自定义输出目录），为缺失/损坏/陈旧的 meta 文件
//! 执行创建、修复或重建，并收集需要（重新）触发后处理的路径列表。
//! 不涉及 meta 文件本身的读写原语（见 `super::store`）或调度/清理逻辑
//! （见 `super::maintenance`）。
//!
//! Scans the output directory (and ts_merge's custom output dir) to create, repair,
//! or rebuild meta files that are missing, corrupt, or stale, collecting the list of
//! paths needing post-processing (re-)triggered. Does not implement meta file I/O
//! primitives (see `super::store`) or scheduling/cleanup logic (see `super::maintenance`).

use super::model::{META_VERSION, VideoMeta, meta_path_for, parse_timestamp_from_stem, recording_stem};
use super::store::{read_meta, write_meta};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// meta 完整性检查：验证必须字段是否有效，同时尝试修复可以推断的缺失字段。
///
/// 若字段可以从文件系统推断（如 `started_at` 从文件名、`size_bytes` 从文件大小、
/// `video_path` 从参数路径），则直接补全后返回修复后的 meta；
/// 若字段无法推断且值非法，则返回 `None`（需要完全重建）。
///
/// Validate required fields and attempt to repair inferrable missing fields.
///
/// Fields that can be inferred from the filesystem (e.g. `started_at` from filename,
/// `size_bytes` from file size, `video_path` from the given path) are filled in and
/// the repaired meta is returned.
/// If a field cannot be inferred and its value is invalid, returns `None`
/// (caller should fully rebuild the meta).
fn repair_meta(meta: &VideoMeta, path: &Path) -> Option<VideoMeta> {
    let mut m = meta.clone();
    let mut changed = false;

    // status 必须是已知的有效值，无法推断 → 返回 None 触发完全重建
    // status must be a known valid value; cannot be inferred → return None to trigger full rebuild
    if !matches!(
        m.status.as_str(),
        "recording" | "pp_waiting" | "pp_running" | "pp_error" | "finish"
    ) {
        return None;
    }

    // started_at 为空时从文件名或文件修改时间推断
    // Infer started_at from filename stem or file modification time when empty
    if m.started_at.trim().is_empty() {
        // 与 meta 路径规则一致的 stem（含 '.' 用户名的 session_dir 不被截断）
        // Stem consistent with the meta path rule (session_dirs of dotted usernames aren't truncated)
        let stem = recording_stem(path).unwrap_or("");
        let inferred = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
            std::fs::metadata(path)
                .ok()
                .and_then(|md| md.modified().ok())
                .map(|t| {
                    let dt: chrono::DateTime<chrono::Local> = t.into();
                    dt.to_rfc3339()
                })
                .unwrap_or_default()
        });
        m.started_at = inferred;
        changed = true;
    }

    // size_bytes 为 0 时尝试从文件系统读取实际大小
    // Re-read size_bytes from filesystem when it is 0
    if m.size_bytes == 0 {
        let actual = if path.is_dir() {
            std::fs::read_dir(path)
                .map(|e| {
                    e.flatten()
                        .filter_map(|f| std::fs::metadata(f.path()).ok().map(|md| md.len()))
                        .sum()
                })
                .unwrap_or(0)
        } else {
            std::fs::metadata(path).map(|md| md.len()).unwrap_or(0)
        };
        if actual > 0 {
            m.size_bytes = actual;
            changed = true;
        }
    }

    // video_path 缺失时从参数路径补全（write_meta 通常会自动填入，但旧版 meta 可能为空）
    // Fill video_path from the given path when absent (write_meta normally fills it,
    // but older meta files may be missing this field)
    if m.video_path.is_none() {
        m.video_path = Some(path.to_string_lossy().to_string());
        changed = true;
    }

    if changed {
        Some(m)
    } else {
        // 无需改动，返回原始 meta 避免不必要写入
        // No changes needed; return original meta to avoid unnecessary write
        Some(meta.clone())
    }
}

/// 扫描输出目录（以及可选的额外目录），为所有缺少或版本过旧的 meta 文件执行创建/重建。
///
/// 独立视频文件和 session_dir（含 .ts 分片目录）采用完全一致的处理规则——是否需要
/// 合并交由流水线首节点（ts_merge）自行判断：输入是目录就合并，已经是文件就直传。
/// 本函数只负责判断"是否需要（重新）触发后处理"，不关心输入形态。
///
/// 处理规则（视频文件与 session_dir 通用）：
/// - **meta 缺失或损坏** → 创建 `pp_waiting` 状态的 meta，并加入待后处理列表
/// - **meta 存在但状态陈旧**（`recording`/`pp_waiting`/`pp_running`，且未被本进程追踪，
///   即进程重启前遗留）→ 加入待后处理列表，重新触发流水线
/// - **meta 存在且状态终态**（`finish`/`pp_error`）→ 仅修复可推断字段，不重新触发
/// - **meta 存在但活跃状态被追踪中** → 跳过，不触碰
///
/// `extra_dirs` 传入 ts_merge 模块配置的自定义输出目录。
/// `recorder` 用于判断 session_dir 的 `recording` 状态是否真实活跃
/// （`recorder.is_file_locked`），而非仅凭 meta 中的字符串。
///
/// 返回：需要（重新）触发后处理流水线的路径列表（可能是视频文件，也可能是 session_dir；
/// 调用方直接把该路径同时作为 `initial_path` 和 `video_path` 触发 `run_postprocess_for_path`，
/// 流水线首节点会自行处理合并或直传）。
///
/// Scan the output directory (and optional extra directories) to create/rebuild meta files.
///
/// Standalone video files and session_dirs (directories containing .ts segments) are handled
/// with identical rules — whether merging is needed is decided by the pipeline's first node
/// (ts_merge) itself: merge if the input is a directory, pass through if it's already a file.
/// This function only decides whether post-processing needs to be (re-)triggered, regardless
/// of the input's shape.
///
/// Rules (shared by video files and session_dirs):
/// - **meta missing or corrupt** → create `pp_waiting` meta, add to the pending list
/// - **meta exists but status is stale** (`recording`/`pp_waiting`/`pp_running`, not tracked
///   by this process, i.e. left over from a previous restart) → add to pending, re-trigger
/// - **meta exists with a terminal status** (`finish`/`pp_error`) → repair inferrable fields only
/// - **meta exists and the active status is genuinely tracked** → skip untouched
///
/// `extra_dirs` passes the custom output directory configured in the ts_merge module.
/// `recorder` is used to determine whether a session_dir's `recording` status is genuinely
/// active (`recorder.is_file_locked`), rather than trusting the meta string alone.
///
/// Returns: paths needing post-processing (re-)triggered — either video files or session_dirs;
/// callers pass the path as both `initial_path` and `video_path` to `run_postprocess_for_path`,
/// and the pipeline's first node handles merging or pass-through on its own.
pub fn ensure_meta_files(
    output_dir: &Path,
    extra_dirs: &[&Path],
    state: &crate::config::app_state::AppState,
    recorder: &crate::recording::recorder::RecorderManager,
    retry_pp_error: bool,
) -> Vec<std::path::PathBuf> {
    let mut pp_pending: Vec<std::path::PathBuf> = Vec::new();

    // 收集"已归属其他 meta"的视频路径：某个 meta 的 video_path 推导出的 meta 路径
    // 不是该 meta 自身时（如 split_by_streamer=false 时合并文件落在扁平目录），
    // 该视频已由那份 meta 管理，扫描不应再为它新建 meta 或触发后处理。
    //
    // Collect video paths "owned by another meta": when the meta path derived from a meta's
    // video_path isn't that meta file itself (e.g. merged file in a flat dir when
    // split_by_streamer=false), the video is already managed by that meta and the scan must
    // not create a new meta for it or trigger post-processing.
    let mut owned_elsewhere: HashSet<PathBuf> = HashSet::new();
    for meta_file in super::model::list_all_meta_paths() {
        let Ok(content) = std::fs::read_to_string(&meta_file) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<VideoMeta>(&content) else {
            continue;
        };
        if let Some(vp) = meta.video_path.as_deref() {
            let vp = PathBuf::from(vp);
            if meta_path_for(&vp).as_deref() != Some(meta_file.as_path()) {
                owned_elsewhere.insert(vp);
            }
        }
    }

    if output_dir.exists() {
        scan_and_ensure_meta(output_dir, &mut pp_pending, state, recorder, retry_pp_error, &owned_elsewhere);
    }
    for dir in extra_dirs {
        if dir.exists() && *dir != output_dir {
            scan_and_ensure_meta(dir, &mut pp_pending, state, recorder, retry_pp_error, &owned_elsewhere);
        }
    }

    // 按录制身份去重：同一录制（session_dir 与合并文件共用一个 meta）只保留一条，
    // 优先保留 meta.video_path 与自身相同的那条，否则保留第一条。
    // Deduplicate by recording identity: keep one entry per recording (session_dir and merged
    // file share one meta), preferring the one whose meta.video_path equals itself, else the first.
    {
        let mut chosen: Vec<(String, PathBuf)> = Vec::new();
        for p in pp_pending.drain(..) {
            let key = crate::postprocess::queue::recording_key(&p);
            let matches_meta = || {
                read_meta(&p)
                    .and_then(|m| m.video_path)
                    .is_some_and(|vp| Path::new(&vp) == p.as_path())
            };
            match chosen.iter().position(|(k, _)| *k == key) {
                Some(idx) => {
                    let existing = &chosen[idx].1;
                    let existing_matches = read_meta(existing)
                        .and_then(|m| m.video_path)
                        .is_some_and(|vp| Path::new(&vp) == existing.as_path());
                    if !existing_matches && matches_meta() {
                        chosen[idx].1 = p;
                    }
                }
                None => chosen.push((key, p)),
            }
        }
        pp_pending = chosen.into_iter().map(|(_, p)| p).collect();
    }

    if !pp_pending.is_empty() {
        tracing::info!(
            "{}",
            crate::tl!("meta.scanPpPending", count = pp_pending.len())
        );
    }

    pp_pending
}

fn scan_and_ensure_meta(
    dir: &Path,
    pp_pending: &mut Vec<std::path::PathBuf>,
    state: &crate::config::app_state::AppState,
    recorder: &crate::recording::recorder::RecorderManager,
    retry_pp_error: bool,
    owned_elsewhere: &HashSet<PathBuf>,
) {
    // 判断某路径当前状态是否"真实活跃"（不应被本次扫描触碰或重新触发）。
    //
    // - "recording"：仅当该路径确实被当前进程的活跃录制会话锁定时才算真实活跃
    //   （`recorder.is_file_locked`）。若进程崩溃重启，session_dir 的 meta 可能还
    //   停留在 "recording"，但已没有任何活跃会话——此时应视为陈旧状态。
    // - "pp_waiting" / "pp_running"：由 pp_queue 管理，用 `is_tracked` 区分
    //   真实活跃（本进程内存中确实有记录）与陈旧状态（上次异常退出遗留，无人追踪）。
    //
    //   注意：pp_queue 追踪的 key 是最初传入 run_postprocess_for_path 的 video_path
    //   字符串（session_dir 或原始视频路径）。ts_merge 完成后会在磁盘上生成同名视频
    //   文件，scan 此时可能扫描到这个新文件，用新文件路径查 is_tracked 会找不到，
    //   因为队列里存的仍是原始路径。因此对 pp_waiting/pp_running，需要额外用
    //   meta.video_path（后处理正在使用的权威路径）做兜底查询，两者任一被追踪即视为
    //   真实活跃。
    //
    // Determine whether a path's current status is "genuinely active" (should not be
    // touched or re-triggered by this scan).
    //
    // - "recording": genuinely active only if the path is actually locked by a live
    //   recording session (`recorder.is_file_locked`). After a crash/restart, a session_dir's
    //   meta may still say "recording" even though no session is actually live — that's stale.
    // - "pp_waiting" / "pp_running": managed by pp_queue; `is_tracked` distinguishes
    //   genuinely active (has an in-memory record) from stale (leftover from a previous
    //   abnormal exit, untracked).
    //
    //   Important: the key stored in pp_queue is the video_path string originally passed to
    //   run_postprocess_for_path (either a session_dir or the original video file path).
    //   After ts_merge completes it creates a same-stem video file on disk, which a
    //   concurrent scan may encounter; looking up that new file path via is_tracked returns
    //   false because the queue still holds the original path. Therefore, for pp_waiting/
    //   pp_running we also fall back to checking meta.video_path (the authoritative path
    //   used by the active post-processing task) — the status is genuinely active if either
    //   path is tracked.
    //
    // 现在的权威判断在条目入口处：扫描在读写每个条目的 meta 前用录制身份
    // （`resolve_meta_path`，即 `recording_key`）调用 `pp_queue.try_reserve`，已被 claim
    // （排队/执行中）的录制占位失败而直接跳过；session_dir 与合并文件共用同一 meta，
    // 因此任一路径都能命中。调用本闭包时该条目的占位已成功，说明此刻没有 claim，
    // 所以这里不再查询 `is_recording_active`（否则会被自己的占位命中而误判为活跃）。
    // "recording" 还需把录制结束 → 后处理接手的交接状态（`is_pending_handoff`）视为活跃。
    // 下方基于 is_tracked 的三条启发式仅作兜底保留。
    //
    // The authoritative check now happens at each entry: before reading/writing an entry's
    // meta the scan calls `pp_queue.try_reserve` with the recording identity
    // (`resolve_meta_path`, i.e. `recording_key`); a claimed (queued/running) recording fails
    // the reservation and is skipped. A session_dir and its merged file share one meta, so
    // either path hits. When this closure runs the entry's reservation has succeeded, meaning
    // there is no claim right now, so `is_recording_active` is no longer queried here (it would
    // match our own reservation and misreport the entry as active). For "recording", the
    // recording-ended → post-processing handoff state (`is_pending_handoff`) also counts as
    // active. The three is_tracked-based heuristics below are kept only as a fallback.
    let is_genuinely_active =
        |path: &Path, status: &str, meta_video_path: Option<&str>| match status {
            "recording" => recorder.is_file_locked(path) || recorder.is_pending_handoff(path),
            "pp_waiting" | "pp_running" => {
                // 兜底：先查当前扫描路径，找不到再用 meta.video_path
                // Fallback: check the scanned path first, then meta.video_path
                if state.pp_queue.is_tracked(&path.to_string_lossy()) {
                    return true;
                }
                if let Some(vp) = meta_video_path
                    && state.pp_queue.is_tracked(vp)
                {
                    return true;
                }
                // 若扫描的是视频文件（有扩展名），还需检查同名的 session_dir 是否在追踪中。
                // ts_merge 完成前后处理以 session_dir 路径为 key 入队；ts_merge 完成后
                // meta.video_path 已更新为 .mkv，但 pp_queue 里的 key 仍是 session_dir，
                // 若不检查 session_dir，scan 扫到 .mkv 时会误判为陈旧而重复触发后处理。
                //
                // If scanning a video file (has an extension), also check if the corresponding
                // session_dir (same parent and stem, no extension) is tracked.
                // Post-processing is queued with the session_dir path as key; after ts_merge
                // meta.video_path switches to .mkv, but the pp_queue key remains the session_dir.
                // Without this check, scanning a .mkv would be misclassified as stale and
                // trigger a duplicate post-processing run.
                if path.extension().is_some()
                    && let (Some(parent), Some(stem)) = (path.parent(), recording_stem(path))
                {
                    let session_dir = parent.join(stem);
                    if state.pp_queue.is_tracked(&session_dir.to_string_lossy()) {
                        return true;
                    }
                }
                false
            }
            _ => false,
        };
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue;
        }

        // ── 独立视频文件 / Standalone video files ─────────────────────────────
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_ascii_lowercase());
            if !matches!(
                ext.as_deref(),
                Some("mp4") | Some("mkv") | Some("ts") | Some("avi") | Some("mov")
            ) {
                continue;
            }
            // 实时解析录制身份（归属 meta 路径），与 recording_key(&path) 一致（A4）
            // Resolve the recording identity (owning meta path) live; equals recording_key(&path) (A4)
            let Some(meta_path) = super::model::resolve_meta_path(&path) else {
                continue;
            };
            let key = meta_path.to_string_lossy().to_string();
            // 读写 meta 前占位：已被后处理 claim（排队或执行中）、已有占位或正在删除 → 跳过；
            // 占位持有到本分支结束，关闭"检查 claim → write_meta"之间的窗口（A3）
            // Reserve before touching the meta: skip if claimed by post-processing (queued/running),
            // already reserved, or being removed; the reservation is held until the end of this
            // branch, closing the "check claim → write_meta" window (A3)
            let Some(_reservation) = state.pp_queue.try_reserve(&key) else {
                continue;
            };
            // 占位后复查：文件可能已被删除或移动 / Re-check after reserving: file may be gone
            if !path.exists() {
                continue;
            }
            if !meta_path.exists() {
                // 归属 meta 已经由 resolve_meta_path 实时反查（扁平目录合并文件的 meta 存在时
                // 不会进入此分支，A4）；下面的 owned_elsewhere 快照只作附加保护。
                // 视频已由其他 meta 的 video_path 引用（如扁平输出目录下的合并文件）→ 跳过
                // 同 stem 的录制正在后处理（ts_merge 正写入扁平目录、video_path 尚未切换）→ 同样跳过
                // （is_stem_active 只统计 Claimed，不会被本扫描自己的占位命中）
                //
                // The owning meta has already been looked up live via resolve_meta_path (a flat-dir
                // merged file whose meta exists never reaches this branch, A4); the owned_elsewhere
                // snapshot below is only an extra safeguard.
                // Video already referenced by another meta's video_path (e.g. merged file in
                // a flat output dir) → skip
                // A same-stem recording is being post-processed (ts_merge writing into a flat dir,
                // video_path not switched yet) → skip as well
                // (is_stem_active only counts Claimed entries, so this scan's own reservation
                // doesn't match)
                if owned_elsewhere.contains(&path)
                    || path.file_stem().is_some_and(|s| state.pp_queue.is_stem_active(s))
                {
                    tracing::debug!(
                        "{}",
                        crate::tl!("meta.scanSkipOwnedVideo", path = path.display())
                    );
                    continue;
                }
                // meta 缺失：创建 pp_waiting 状态，稍后触发后处理
                // Meta missing: create pp_waiting status, trigger post-processing later
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                    std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                });
                let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let meta = VideoMeta {
                    meta_version: META_VERSION,
                    status: "pp_waiting".to_string(),
                    started_at,
                    size_bytes,
                    video_duration_secs: None,
                    video_resolution: None,
                    pp_execution: None,
                    segments_downloaded: None,
                    segments_failed: None,
                    video_path: None,
                    pp_progress: None,
                };
                write_meta(&path, &meta);
                tracing::info!(
                    "{}",
                    crate::tl!("meta.scanCreatedVideo", path = path.display())
                );
                pp_pending.push(path.clone());
            } else if meta_path.exists() && read_meta(&path).is_none() {
                // meta 文件存在但解析失败（JSON 损坏）→ 重新创建
                // Meta file exists but failed to parse (corrupt JSON) → recreate
                tracing::warn!(
                    "{}",
                    crate::tl!("meta.scanCorruptVideo", path = path.display())
                );
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                    std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                });
                let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let meta = VideoMeta {
                    meta_version: META_VERSION,
                    status: "pp_waiting".to_string(),
                    started_at,
                    size_bytes,
                    video_duration_secs: None,
                    video_resolution: None,
                    pp_execution: None,
                    segments_downloaded: None,
                    segments_failed: None,
                    video_path: None,
                    pp_progress: None,
                };
                write_meta(&path, &meta);
                pp_pending.push(path.clone());
            } else if let Some(meta) = read_meta(&path) {
                // 真实活跃状态跳过；陈旧的 recording/pp_waiting/pp_running（进程重启前遗留，
                // 无人追踪）需要重新触发后处理，而不是继续等待
                // Skip genuinely active states; stale recording/pp_waiting/pp_running (leftover
                // from a previous abnormal exit, untracked) needs to be re-triggered, not left waiting
                if is_genuinely_active(&path, meta.status.as_str(), meta.video_path.as_deref()) {
                    continue;
                }
                if matches!(
                    meta.status.as_str(),
                    "recording" | "pp_waiting" | "pp_running"
                ) {
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "meta.scanStaleVideo",
                            path = path.display(),
                            status = meta.status
                        )
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                // pp_error：上次后处理失败，仅启动时自动重试（定时扫描跳过）
                // pp_error: failed last time; auto-retry only on startup, skip on periodic scan
                if meta.status == "pp_error" && retry_pp_error {
                    tracing::info!(
                        "{}",
                        crate::tl!("meta.scanRetryVideo", path = path.display())
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                // 尝试修复字段，若无法修复（status 非法）则按缺失 meta 处理（触发后处理）
                // Try to repair fields; if unrepairable (invalid status), treat as missing meta
                match repair_meta(&meta, &path) {
                    Some(repaired)
                        if repaired.meta_version == META_VERSION
                            && repaired.started_at == meta.started_at
                            && repaired.video_path == meta.video_path
                            && repaired.size_bytes == meta.size_bytes =>
                    {
                        // 无需修改 / No changes needed
                    }
                    Some(repaired) => {
                        tracing::info!(
                            "{}",
                            crate::tl!("meta.scanRepairedVideo", path = path.display())
                        );
                        write_meta(&path, &repaired);
                    }
                    None => {
                        // status 非法，无法推断 → 重建为 pp_waiting，触发后处理
                        // Invalid status, cannot infer → rebuild as pp_waiting, trigger pp
                        tracing::warn!(
                            "{}",
                            crate::tl!("meta.scanUnrepairableVideo", path = path.display())
                        );
                        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                            std::fs::metadata(&path)
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .map(|t| {
                                    let dt: chrono::DateTime<chrono::Local> = t.into();
                                    dt.to_rfc3339()
                                })
                                .unwrap_or_default()
                        });
                        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        write_meta(
                            &path,
                            &VideoMeta {
                                meta_version: META_VERSION,
                                status: "pp_waiting".to_string(),
                                started_at,
                                size_bytes,
                                video_duration_secs: None,
                                video_resolution: None,
                                pp_execution: None,
                                segments_downloaded: None,
                                segments_failed: None,
                                video_path: None,
                                pp_progress: None,
                            },
                        );
                        pp_pending.push(path.clone());
                    }
                }
            }
            continue; // 视频文件处理完毕，不走目录分支 / done with file branch
        }

        if !path.is_dir() {
            continue;
        }

        // ── session_dir（含 .ts 分片的目录）/ session_dir containing .ts segments ──
        let has_ts = std::fs::read_dir(&path)
            .map(|mut e| {
                e.any(|f| {
                    f.ok()
                        .map(|f| f.path().extension().and_then(|x| x.to_str()) == Some("ts"))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);

        if has_ts {
            // session_dir 若被当前进程的活跃录制会话锁定，直接跳过整个分支——
            // 不检查也不重建其 meta，避免与录制循环写入 meta 产生竞争。
            //
            // Skip the entire branch if the session_dir is locked by a live recording
            // session on this process — don't inspect or rebuild its meta at all, avoiding
            // a race with the recording loop's own meta writes.
            // 处于录制结束 → 后处理接手的交接状态时同样跳过。
            // Also skip while in the recording-ended → post-processing handoff state.
            if recorder.is_file_locked(&path) || recorder.is_pending_handoff(&path) {
                continue;
            }

            // 实时解析录制身份并占位，持有到本分支结束（同视频文件分支，A3/A4）
            // Resolve the recording identity live and reserve it until the end of this branch
            // (same as the video file branch, A3/A4)
            let Some(meta_path) = super::model::resolve_meta_path(&path) else {
                continue;
            };
            let key = meta_path.to_string_lossy().to_string();
            let Some(_reservation) = state.pp_queue.try_reserve(&key) else {
                continue;
            };
            // 占位后复查：目录可能已被合并后删除 / Re-check after reserving: dir may be gone after merge
            if !path.exists() {
                continue;
            }
            if !meta_path.exists() {
                // meta 缺失：创建 pp_waiting 状态，加入待后处理列表。
                // ts_merge 会自行判断输入是目录（此处）还是文件并相应处理。
                // Meta missing: create pp_waiting status, add to pending list.
                // ts_merge decides on its own whether the input is a directory (here) or a
                // file and handles it accordingly.
                let started_at = parse_timestamp_from_stem(name).unwrap_or_else(|| {
                    std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                });
                let size_bytes = std::fs::read_dir(&path)
                    .map(|e| {
                        e.flatten()
                            .filter_map(|f| std::fs::metadata(f.path()).ok().map(|m| m.len()))
                            .sum()
                    })
                    .unwrap_or(0);
                let meta = VideoMeta {
                    meta_version: META_VERSION,
                    status: "pp_waiting".to_string(),
                    started_at,
                    size_bytes,
                    video_duration_secs: None,
                    video_resolution: None,
                    pp_execution: None,
                    segments_downloaded: None,
                    segments_failed: None,
                    video_path: None,
                    pp_progress: None,
                };
                write_meta(&path, &meta);
                tracing::info!(
                    "{}",
                    crate::tl!("meta.scanCreatedSessionDir", path = path.display())
                );
                pp_pending.push(path.clone());
            } else if meta_path.exists() && read_meta(&path).is_none() {
                // meta 文件存在但 JSON 损坏 → 重建并加入待后处理列表
                // Meta file exists but JSON is corrupt → rebuild and add to pending list
                tracing::warn!(
                    "{}",
                    crate::tl!("meta.scanCorruptSessionDir", path = path.display())
                );
                let started_at = parse_timestamp_from_stem(name).unwrap_or_else(|| {
                    std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                });
                let size_bytes = std::fs::read_dir(&path)
                    .map(|e| {
                        e.flatten()
                            .filter_map(|f| std::fs::metadata(f.path()).ok().map(|m| m.len()))
                            .sum()
                    })
                    .unwrap_or(0);
                let meta = VideoMeta {
                    meta_version: META_VERSION,
                    status: "pp_waiting".to_string(),
                    started_at,
                    size_bytes,
                    video_duration_secs: None,
                    video_resolution: None,
                    pp_execution: None,
                    segments_downloaded: None,
                    segments_failed: None,
                    video_path: None,
                    pp_progress: None,
                };
                write_meta(&path, &meta);
                pp_pending.push(path.clone());
            } else if let Some(meta) = read_meta(&path) {
                // 真实活跃状态跳过（is_file_locked 已在分支入口检查过 recording，
                // 这里只需处理 pp_waiting/pp_running 的 is_tracked 判断）；
                // 陈旧状态需要重新触发后处理，而不是继续等待
                // Genuinely active states are skipped (recording via is_file_locked was
                // already checked at branch entry; here we only need pp_waiting/pp_running's
                // is_tracked check); stale states need to be re-triggered, not left waiting
                if is_genuinely_active(&path, meta.status.as_str(), meta.video_path.as_deref()) {
                    continue;
                }
                if matches!(
                    meta.status.as_str(),
                    "recording" | "pp_waiting" | "pp_running"
                ) {
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "meta.scanStaleSessionDir",
                            path = path.display(),
                            status = meta.status
                        )
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                // pp_error：上次后处理失败，仅启动时自动重试（定时扫描跳过）
                // pp_error: failed last time; auto-retry only on startup, skip on periodic scan
                if meta.status == "pp_error" && retry_pp_error {
                    tracing::info!(
                        "{}",
                        crate::tl!("meta.scanRetrySessionDir", path = path.display())
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                match repair_meta(&meta, &path) {
                    Some(repaired)
                        if repaired.meta_version == META_VERSION
                            && repaired.started_at == meta.started_at
                            && repaired.video_path == meta.video_path
                            && repaired.size_bytes == meta.size_bytes => {}
                    Some(repaired) => {
                        tracing::info!(
                            "{}",
                            crate::tl!("meta.scanRepairedSessionDir", path = path.display())
                        );
                        write_meta(&path, &repaired);
                    }
                    None => {
                        tracing::warn!(
                            "{}",
                            crate::tl!("meta.scanUnrepairableSessionDir", path = path.display())
                        );
                        let started_at = parse_timestamp_from_stem(name).unwrap_or_else(|| {
                            std::fs::metadata(&path)
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .map(|t| {
                                    let dt: chrono::DateTime<chrono::Local> = t.into();
                                    dt.to_rfc3339()
                                })
                                .unwrap_or_default()
                        });
                        let size_bytes = std::fs::read_dir(&path)
                            .map(|e| {
                                e.flatten()
                                    .filter_map(|f| {
                                        std::fs::metadata(f.path()).ok().map(|m| m.len())
                                    })
                                    .sum()
                            })
                            .unwrap_or(0);
                        write_meta(
                            &path,
                            &VideoMeta {
                                meta_version: META_VERSION,
                                status: "pp_waiting".to_string(),
                                started_at,
                                size_bytes,
                                video_duration_secs: None,
                                video_resolution: None,
                                pp_execution: None,
                                segments_downloaded: None,
                                segments_failed: None,
                                video_path: None,
                                pp_progress: None,
                            },
                        );
                        pp_pending.push(path.clone());
                    }
                }
            }
            // session_dir 不递归内部 / Don't recurse inside session_dir
            continue;
        }

        // ── 普通子目录，递归扫描 / Regular subdirectory, recurse ──────────────
        scan_and_ensure_meta(&path, pp_pending, state, recorder, retry_pp_error, owned_elsewhere);
    }
}

/// 从流水线配置中提取 ts_merge 节点的 `output_dir` 参数（非空时才返回）。
/// 若开启了 `split_by_streamer`，返回该目录本身（按主播分子目录时根目录就是扫描起点）。
/// 若未开启，直接返回固定的自定义输出目录。
/// 这是独立视频文件的唯一已知输出路径，用于 meta 扫描重建。
///
/// Extract the scan root from the ts_merge node's params in the pipeline config.
/// - `split_by_streamer=true`: returns `output_dir` (the root to scan for per-streamer subdirs)
/// - `split_by_streamer=false`: returns `output_dir` directly as the flat output location
///
/// Returns `None` if the node is absent, disabled, or `output_dir` is empty.
pub fn ts_merge_output_dir(
    state: &crate::config::app_state::AppState,
) -> Option<std::path::PathBuf> {
    let pipeline = state.get_pipeline();
    let node = pipeline
        .nodes
        .iter()
        .find(|n| n.module_id == "ts_merge" && n.enabled)?;
    let dir = node.params.get("output_dir")?.as_str()?.trim();
    if dir.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(dir))
    }
}
