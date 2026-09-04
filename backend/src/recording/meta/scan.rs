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

use super::model::{META_VERSION, VideoMeta, meta_path_for, parse_timestamp_from_stem};
use super::store::{read_meta, write_meta};
use std::path::Path;

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
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
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
                .map(|e| e.flatten().filter_map(|f| std::fs::metadata(f.path()).ok().map(|md| md.len())).sum())
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
) -> Vec<std::path::PathBuf> {
    let mut pp_pending: Vec<std::path::PathBuf> = Vec::new();

    if output_dir.exists() {
        scan_and_ensure_meta(output_dir, &mut pp_pending, state, recorder);
    }
    for dir in extra_dirs {
        if dir.exists() && *dir != output_dir {
            scan_and_ensure_meta(dir, &mut pp_pending, state, recorder);
        }
    }

    if !pp_pending.is_empty() {
        tracing::info!(
            "Meta scan: {} path(s) need post-processing (re-)triggered",
            pp_pending.len()
        );
    }

    pp_pending
}

fn scan_and_ensure_meta(
    dir: &Path,
    pp_pending: &mut Vec<std::path::PathBuf>,
    state: &crate::config::app_state::AppState,
    recorder: &crate::recording::recorder::RecorderManager,
) {
    // 判断某路径当前状态是否"真实活跃"（不应被本次扫描触碰或重新触发）。
    //
    // - "recording"：仅当该路径确实被当前进程的活跃录制会话锁定时才算真实活跃
    //   （`recorder.is_file_locked`）。若进程崩溃重启，session_dir 的 meta 可能还
    //   停留在 "recording"，但已没有任何活跃会话——此时应视为陈旧状态。
    // - "pp_waiting" / "pp_running"：由 pp_queue 管理，用 `is_tracked` 区分
    //   真实活跃（本进程内存中确实有记录）与陈旧状态（上次异常退出遗留，无人追踪）。
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
    let is_genuinely_active = |path: &Path, status: &str| match status {
        "recording" => recorder.is_file_locked(path),
        "pp_waiting" | "pp_running" => state.pp_queue.is_tracked(&path.to_string_lossy()),
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
            let ext = path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("mp4") | Some("mkv") | Some("ts") | Some("avi") | Some("mov")) {
                continue;
            }
            let meta_path = match meta_path_for(&path) {
                Some(p) => p,
                None => continue,
            };
            if !meta_path.exists() {
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
                tracing::info!("Meta scan: created pp_waiting meta for video {:?}", path);
                pp_pending.push(path.clone());
            } else if meta_path.exists() && read_meta(&path).is_none() {
                // meta 文件存在但解析失败（JSON 损坏）→ 重新创建
                // Meta file exists but failed to parse (corrupt JSON) → recreate
                tracing::warn!("Meta scan: corrupt meta for {:?} — recreating as pp_waiting", path);
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                    std::fs::metadata(&path).ok().and_then(|m| m.modified().ok())
                        .map(|t| { let dt: chrono::DateTime<chrono::Local> = t.into(); dt.to_rfc3339() })
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
                if is_genuinely_active(&path, meta.status.as_str()) {
                    continue;
                }
                if matches!(meta.status.as_str(), "recording" | "pp_waiting" | "pp_running") {
                    tracing::warn!(
                        "Meta scan: stale {} status for {:?} (not tracked by this process) — re-triggering post-processing",
                        meta.status, path
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                // 尝试修复字段，若无法修复（status 非法）则按缺失 meta 处理（触发后处理）
                // Try to repair fields; if unrepairable (invalid status), treat as missing meta
                match repair_meta(&meta, &path) {
                    Some(repaired) if repaired.meta_version == META_VERSION
                        && repaired.started_at == meta.started_at
                        && repaired.video_path == meta.video_path
                        && repaired.size_bytes == meta.size_bytes => {
                        // 无需修改 / No changes needed
                    }
                    Some(repaired) => {
                        tracing::info!("Meta scan: repaired fields for {:?}", path);
                        write_meta(&path, &repaired);
                    }
                    None => {
                        // status 非法，无法推断 → 重建为 pp_waiting，触发后处理
                        // Invalid status, cannot infer → rebuild as pp_waiting, trigger pp
                        tracing::warn!("Meta scan: unrepairable meta for {:?} — rebuilding as pp_waiting", path);
                        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                            std::fs::metadata(&path).ok().and_then(|m| m.modified().ok())
                                .map(|t| { let dt: chrono::DateTime<chrono::Local> = t.into(); dt.to_rfc3339() })
                                .unwrap_or_default()
                        });
                        let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                        write_meta(&path, &VideoMeta {
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
                        });
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
            if recorder.is_file_locked(&path) {
                continue;
            }

            let meta_path = match meta_path_for(&path) {
                Some(p) => p,
                None => continue,
            };
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
                tracing::info!("Meta scan: created pp_waiting meta for session_dir {:?}", path);
                pp_pending.push(path.clone());
            } else if meta_path.exists() && read_meta(&path).is_none() {
                // meta 文件存在但 JSON 损坏 → 重建并加入待后处理列表
                // Meta file exists but JSON is corrupt → rebuild and add to pending list
                tracing::warn!("Meta scan: corrupt meta for session_dir {:?} — recreating as pp_waiting", path);
                let started_at = parse_timestamp_from_stem(name).unwrap_or_else(|| {
                    std::fs::metadata(&path).ok().and_then(|m| m.modified().ok())
                        .map(|t| { let dt: chrono::DateTime<chrono::Local> = t.into(); dt.to_rfc3339() })
                        .unwrap_or_default()
                });
                let size_bytes = std::fs::read_dir(&path)
                    .map(|e| e.flatten().filter_map(|f| std::fs::metadata(f.path()).ok().map(|m| m.len())).sum())
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
                if is_genuinely_active(&path, meta.status.as_str()) {
                    continue;
                }
                if matches!(meta.status.as_str(), "recording" | "pp_waiting" | "pp_running") {
                    tracing::warn!(
                        "Meta scan: stale {} status for session_dir {:?} (not tracked by this process) — re-triggering post-processing",
                        meta.status, path
                    );
                    pp_pending.push(path.clone());
                    continue;
                }
                match repair_meta(&meta, &path) {
                    Some(repaired) if repaired.meta_version == META_VERSION
                        && repaired.started_at == meta.started_at
                        && repaired.video_path == meta.video_path
                        && repaired.size_bytes == meta.size_bytes => {}
                    Some(repaired) => {
                        tracing::info!("Meta scan: repaired fields for session_dir {:?}", path);
                        write_meta(&path, &repaired);
                    }
                    None => {
                        tracing::warn!("Meta scan: unrepairable meta for session_dir {:?} — rebuilding as pp_waiting", path);
                        let started_at = parse_timestamp_from_stem(name).unwrap_or_else(|| {
                            std::fs::metadata(&path).ok().and_then(|m| m.modified().ok())
                                .map(|t| { let dt: chrono::DateTime<chrono::Local> = t.into(); dt.to_rfc3339() })
                                .unwrap_or_default()
                        });
                        let size_bytes = std::fs::read_dir(&path)
                            .map(|e| e.flatten().filter_map(|f| std::fs::metadata(f.path()).ok().map(|m| m.len())).sum())
                            .unwrap_or(0);
                        write_meta(&path, &VideoMeta {
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
                        });
                        pp_pending.push(path.clone());
                    }
                }
            }
            // session_dir 不递归内部 / Don't recurse inside session_dir
            continue;
        }

        // ── 普通子目录，递归扫描 / Regular subdirectory, recurse ──────────────
        scan_and_ensure_meta(&path, pp_pending, state, recorder);
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
pub fn ts_merge_output_dir(state: &crate::config::app_state::AppState) -> Option<std::path::PathBuf> {
    let pipeline = state.get_pipeline();
    let node = pipeline.nodes.iter().find(|n| n.module_id == "ts_merge" && n.enabled)?;
    let dir = node.params.get("output_dir")?.as_str()?.trim();
    if dir.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(dir))
    }
}
