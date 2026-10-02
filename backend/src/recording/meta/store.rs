//! Meta 文件 CRUD 操作 / Meta File CRUD Operations
//!
//! 提供 meta 文件的读/写/删除，以及针对特定字段（status、进度、执行记录等）的
//! 增量更新辅助函数。不涉及数据结构定义（见 `super::model`）或扫描重建逻辑
//! （见 `super::scan`）。
//!
//! Provides read/write/delete for meta files, plus incremental update helpers for
//! specific fields (status, progress, execution records, etc.). Does not define data
//! structures (see `super::model`) or scan/rebuild logic (see `super::scan`).

use super::model::{
    META_VERSION, PpExecResult, PpExecutionEntry, PpNodeProgress, VideoMeta,
    parse_timestamp_from_stem, recording_stem, resolve_meta_path,
};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// meta 原子写入使用的临时文件序号（与进程 ID 组合保证文件名唯一）。
/// Sequence number for meta atomic-write temp files (combined with the PID for unique names).
static META_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// 判断文件名是否是 [`write_meta`] 原子写入产生的临时文件：`{stem}.json.{pid}.{seq}.tmp`。
/// 与 `write_meta` 中的命名格式保持一致；进程在写入与 rename 之间崩溃时会残留这类文件。
///
/// Check whether a file name is a temp file produced by [`write_meta`]'s atomic write:
/// `{stem}.json.{pid}.{seq}.tmp`. Must match the naming format in `write_meta`; such files are
/// left behind when the process crashes between the write and the rename.
pub(super) fn is_meta_write_tmp_name(name: &str) -> bool {
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let Some(rest) = name.strip_suffix(".tmp") else {
        return false;
    };
    let mut parts = rest.rsplitn(3, '.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(seq), Some(pid), Some(base)) => {
            all_digits(seq) && all_digits(pid) && base.ends_with(".json")
        }
        _ => false,
    }
}

/// 读取视频文件对应的元数据，若文件不存在或解析失败则返回 `None`。
/// 本模块的读/写/删除都通过 [`resolve_meta_path`] 定位 meta（派生 meta 缺失时查找归属 meta）。
///
/// Read the metadata for a video file; returns `None` if missing or parse fails.
/// Reads/writes/deletes in this module all locate meta via [`resolve_meta_path`]
/// (falling back to the owning meta when the derived one is missing).
pub fn read_meta(video_path: &Path) -> Option<VideoMeta> {
    let meta_path = resolve_meta_path(video_path)?;
    let content = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 将元数据写入按主播分子目录存储的 `meta/{username}/{stem}.json`。
/// 写入前自动将 `meta_version` 设置为当前版本常量，并确保主播子目录存在。
///
/// Write metadata to `meta/{username}/{stem}.json` (per-streamer subdirectory).
/// Automatically sets `meta_version` to the current version constant before writing,
/// and ensures the streamer subdirectory exists.
pub fn write_meta(video_path: &Path, meta: &VideoMeta) {
    let Some(meta_path) = resolve_meta_path(video_path) else {
        return;
    };
    // 确保 meta 目录存在 / Ensure meta directory exists
    if let Some(parent) = meta_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut meta = meta.clone();
    meta.meta_version = META_VERSION;
    // 仅在 meta.video_path 尚未设置时，用参数路径作为初始值。
    // 已有值时保留不覆盖，允许上层（如 ts_merge 完成后）显式更新为新路径。
    //
    // Only set video_path from the parameter if it hasn't been set yet.
    // If it's already set (e.g. updated to ts_merge output path), preserve it.
    if meta.video_path.is_none() {
        meta.video_path = Some(video_path.to_string_lossy().to_string());
    }
    match serde_json::to_string_pretty(&meta) {
        Ok(json) => {
            // 先写同目录的唯一临时文件再 rename 原子替换，避免并发读者（扫描、列表）
            // 读到被截断的半截 JSON 而误判为损坏 meta 并重建。
            // rename 或写临时文件失败时回退为直接写入，并清理临时文件。
            //
            // Write to a unique temp file in the same directory, then rename to atomically
            // replace, so concurrent readers (scan, listing) never see truncated JSON and
            // misjudge the meta as corrupted. Falls back to a direct write (and removes the
            // temp file) if writing the temp file or the rename fails.
            let tmp = meta_path.with_extension(format!(
                "json.{}.{}.tmp",
                std::process::id(),
                META_TMP_SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            // 回退的直接写入：主播子目录可能在开头的 create_dir_all 之后被并发的空目录清理
            // 删掉（导致写临时文件失败），因此写入前再建一次目录（D2 兜底）
            // Fallback direct write: the streamer subdirectory may have been removed by a
            // concurrent empty-dir cleanup after the create_dir_all above (making the temp-file
            // write fail), so recreate it before writing (D2 safety net)
            let write_direct = || {
                if let Some(parent) = meta_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&meta_path, &json)
            };
            let result = match std::fs::write(&tmp, &json) {
                Ok(()) => match std::fs::rename(&tmp, &meta_path) {
                    Ok(()) => Ok(()),
                    Err(_) => {
                        let _ = std::fs::remove_file(&tmp);
                        write_direct()
                    }
                },
                Err(_) => {
                    let _ = std::fs::remove_file(&tmp);
                    write_direct()
                }
            };
            if let Err(e) = result {
                tracing::warn!("{}", crate::tl!("meta.writeMetaFailed", path = meta_path.display(), error = e));
            }
        }
        Err(e) => {
            tracing::warn!("{}", crate::tl!("meta.serializeMetaFailed", path = video_path.display(), error = e));
        }
    }
}

/// 删除视频文件对应的元数据文件（若存在）。
/// Delete the metadata file for a video file (if it exists).
pub fn delete_meta(video_path: &Path) {
    if let Some(meta_path) = resolve_meta_path(video_path)
        && meta_path.exists()
    {
        let _ = std::fs::remove_file(&meta_path);
    }
}

/// 仅更新 meta 文件的 `status` 字段，其余字段保持不变。
/// 若 meta 文件不存在则从路径信息重建后再写入。
///
/// Update only the `status` field of the meta file, leaving other fields unchanged.
/// If the meta file doesn't exist, rebuilds it from path info before writing.
pub fn set_status(video_path: &Path, status: &str) {
    let mut meta = match read_meta(video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            // 与 meta 路径规则一致的 stem / Stem consistent with the meta path rule
            let stem = recording_stem(video_path).unwrap_or("");
            let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                std::fs::metadata(video_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            });
            VideoMeta {
                meta_version: META_VERSION,
                status: status.to_string(),
                started_at,
                size_bytes,
                video_duration_secs: None,
                video_resolution: None,
                pp_execution: None,
                segments_downloaded: None,
                segments_failed: None,
                video_path: None,
                pp_progress: None,
            }
        }
    };
    meta.status = status.to_string();
    write_meta(video_path, &meta);
}

/// 录制完成时更新 meta 的分片统计字段（downloaded/failed）。
/// 若 meta 文件不存在则不操作。
///
/// Update segment statistics in the meta file when recording finishes.
/// Does nothing if the meta file doesn't exist.
pub fn set_segment_stats(video_path: &Path, downloaded: u64, failed: u64) {
    let mut meta = match read_meta(video_path) {
        Some(m) => m,
        None => return,
    };
    meta.segments_downloaded = Some(downloaded);
    meta.segments_failed = Some(failed);
    write_meta(video_path, &meta);
}

/// 后处理完成时更新 meta：写入最终状态和各节点执行记录。
/// Update meta when post-processing completes: write final status and node execution records.
pub fn set_pp_done(video_path: &Path, status: &str, pp_execution: Vec<PpExecutionEntry>) {
    let mut meta = match read_meta(video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            // 与 meta 路径规则一致的 stem / Stem consistent with the meta path rule
            let stem = recording_stem(video_path).unwrap_or("");
            let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                std::fs::metadata(video_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            });
            VideoMeta {
                meta_version: META_VERSION,
                status: status.to_string(),
                started_at,
                size_bytes,
                video_duration_secs: None,
                video_resolution: None,
                pp_execution: None,
                segments_downloaded: None,
                segments_failed: None,
                video_path: None,
                pp_progress: None,
            }
        }
    };
    meta.status = status.to_string();
    if !pp_execution.is_empty() {
        meta.pp_execution = Some(pp_execution);
    }
    write_meta(video_path, &meta);
}

/// 写入当前正在执行节点的模块内进度（节点开始时和 on_progress 时调用）。
/// Write the intra-module progress of the currently executing node
/// (called on node start and on each on_progress update).
pub fn set_pp_progress(video_path: &Path, mut progress: PpNodeProgress) {
    let Some(mut meta) = read_meta(video_path) else { return };
    
    // 如果新进度的 mod_done 为 0，保留之前的值以避免前端显示"等待进度"
    // If the new progress has mod_done set to 0, preserve the previous value
    // to prevent the frontend from showing "waiting for progress"
    if progress.mod_done == 0
        && let Some(prev) = &meta.pp_progress
    {
        progress.mod_done = prev.mod_done;
    }
    
    meta.pp_progress = Some(progress);
    write_meta(video_path, &meta);
}

/// 清空当前节点的进度快照（节点完成时调用，无论成功/失败）。
/// Clear the current node progress snapshot (called when a node finishes, success or failure).
pub fn clear_pp_progress(video_path: &Path) {
    let Some(mut meta) = read_meta(video_path) else { return };
    if meta.pp_progress.is_none() {
        return; // 已是 None，无需写磁盘 / Already None, skip disk write
    }
    meta.pp_progress = None;
    write_meta(video_path, &meta);
}

/// 追加一条后处理执行记录（节点开始时调用），finished_at/result/outputs 为 null。
/// Append a post-processing execution entry when a node starts.
pub fn pp_execution_start(video_path: &Path, entry: PpExecutionEntry) {
    let Some(mut meta) = read_meta(video_path) else { return };
    meta.pp_execution.get_or_insert_with(Vec::new).push(entry);
    write_meta(video_path, &meta);
}

/// 更新后处理执行记录中最后一条匹配 effective_id 的条目（节点完成时调用）。
/// Update the last matching pp_execution entry for an effective_id when the node finishes.
pub fn pp_execution_finish(
    video_path: &Path,
    effective_id: &str,
    finished_at: String,
    result: PpExecResult,
    outputs: Vec<Vec<String>>,
) {
    let Some(mut meta) = read_meta(video_path) else { return };
    if let Some(entries) = meta.pp_execution.as_mut() {
        // effective_id 对应 entry.node_id（若有）或 entry.module_id
        // effective_id corresponds to entry.node_id (if present) or entry.module_id
        if let Some(entry) = entries.iter_mut().rev().find(|e| e.effective_id() == effective_id) {
            entry.finished_at = Some(finished_at);
            entry.result = Some(result);
            entry.outputs = outputs;
        }
    }
    write_meta(video_path, &meta);
}

/// 若 meta 文件已存在则不覆盖，否则创建初始 meta（用于启动时遗留片段的保险创建）。
/// Does not overwrite if the meta file already exists; otherwise creates an initial meta
/// (used as a safety net for leftover segments on startup).
pub fn ensure_meta(video_path: &Path, started_at: &str) {
    if let Some(meta_path) = resolve_meta_path(video_path)
        && meta_path.exists()
    {
        return;
    }
    let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
    let meta = VideoMeta {
        meta_version: META_VERSION,
        status: "pp_waiting".to_string(),
        started_at: started_at.to_string(),
        size_bytes,
        video_duration_secs: None,
        video_resolution: None,
        pp_execution: None,
        segments_downloaded: None,
        segments_failed: None,
        video_path: None,
        pp_progress: None,
    };
    write_meta(video_path, &meta);
}
