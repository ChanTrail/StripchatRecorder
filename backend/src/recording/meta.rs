//! 视频元数据文件管理 / Video Metadata File Management
//!
//! 每个录制对应一个隐藏的 JSON 元数据文件，文件名格式为 `.{stem}.json`。
//! 元数据文件在录制开始时创建，并随录制/合并/后处理各阶段持续更新 `status` 字段。
//!
//! Each recording has a hidden JSON metadata file named `.{stem}.json`.
//! The file is created when recording starts and its `status` field is updated
//! throughout the recording / merging / post-processing lifecycle.
//!
//! ## Status 状态流转 / Status lifecycle
//!
//! ```
//! recording → merging_waiting → merging → pp_waiting → pp_running → finish
//!                                       ↘ finish (no pipeline)    ↘ pp_error
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 当前 meta 文件格式版本。
/// 每次对 `VideoMeta` 结构做不向后兼容的变更时递增此值，
/// 轮询扫描器会对版本不匹配的 meta 文件执行重建。
///
/// Current meta file format version.
/// Increment this whenever a breaking change is made to `VideoMeta`;
/// the periodic scanner will rebuild any meta file whose version doesn't match.
pub const META_VERSION: u32 = 2;

/// 从文件名 stem（格式：`{name}_{YYYYMMDD}_{HHmmss}`）中解析录制开始时间。
/// Parse the recording start time from a filename stem (format: `{name}_{YYYYMMDD}_{HHmmss}`).
pub fn parse_timestamp_from_stem(stem: &str) -> Option<String> {
    use chrono::TimeZone;
    let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
    if parts.len() < 2 {
        return None;
    }
    let time_part = parts[0];
    let date_part = parts[1];
    if date_part.len() == 8 && time_part.len() == 6 {
        let combined = format!("{}{}", date_part, time_part);
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&combined, "%Y%m%d%H%M%S") {
            let local = chrono::Local.from_local_datetime(&dt).single()?;
            return Some(local.to_rfc3339());
        }
    }
    None
}

/// 后处理模块的执行结果 / Execution result of a post-processing module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpModuleResult {
    /// 模块 ID / Module ID
    pub module_id: String,
    /// 是否成功 / Whether succeeded
    pub success: bool,
    /// 结果消息 / Result message
    pub message: String,
}

/// 视频元数据，持久化到 `.{stem}.json` 文件。
/// Video metadata persisted to `.{stem}.json`.
///
/// `status` 字段记录当前处理阶段，是前端展示的唯一依据：
/// - `"recording"`       — 正在录制
/// - `"merging_waiting"` — 等待合并（排队中）
/// - `"merging"`         — 正在合并 TS 分片
/// - `"pp_waiting"`      — 等待后处理（排队中）
/// - `"pp_running"`      — 后处理执行中
/// - `"pp_error"`        — 后处理失败
/// - `"finish"`          — 全部完成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMeta {
    /// meta 格式版本，用于检测结构变更后的重建需求。
    /// 缺失时反序列化为 0，触发重建。
    ///
    /// Meta format version, used to detect when a rebuild is needed after structural changes.
    /// Deserializes to 0 when absent, triggering a rebuild.
    #[serde(default)]
    pub meta_version: u32,

    /// 当前处理状态 / Current processing status
    pub status: String,

    /// 录制开始时间（RFC 3339 格式）/ Recording start time (RFC 3339 format)
    pub started_at: String,

    /// 文件大小（字节）/ File size (bytes)
    pub size_bytes: u64,

    /// 视频实际时长（秒，合并完成后由 ffprobe 填入）/ Actual video duration (seconds, filled by ffprobe after merge)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_duration_secs: Option<u64>,

    /// 各模块的后处理结果（后处理完成后填充）/ Per-module post-processing results (filled after completion)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_results: Option<Vec<PpModuleResult>>,

    /// 模块输出路径（模块 ID -> 输出文件路径）/ Module output paths (module ID -> output file path)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_outputs: Option<std::collections::HashMap<String, String>>,
}

/// 根据视频文件路径计算对应的元数据文件路径（`.{stem}.json`）。
/// Compute the metadata file path for a given video file path (`.{stem}.json`).
pub fn meta_path_for(video_path: &Path) -> Option<PathBuf> {
    let parent = video_path.parent()?;
    let stem = video_path.file_stem()?.to_str()?;
    Some(parent.join(format!(".{}.json", stem)))
}

/// 读取视频文件对应的元数据，若文件不存在或解析失败则返回 `None`。
/// Read the metadata for a video file; returns `None` if the file doesn't exist or fails to parse.
pub fn read_meta(video_path: &Path) -> Option<VideoMeta> {
    let meta_path = meta_path_for(video_path)?;
    let content = std::fs::read_to_string(&meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// 将元数据写入视频文件对应的 `.{stem}.json` 文件。
/// 写入前自动将 `meta_version` 设置为当前版本常量。
///
/// Write metadata to the `.{stem}.json` file for a video file.
/// Automatically sets `meta_version` to the current version constant before writing.
pub fn write_meta(video_path: &Path, meta: &VideoMeta) {
    let Some(meta_path) = meta_path_for(video_path) else {
        return;
    };
    let mut meta = meta.clone();
    meta.meta_version = META_VERSION;
    match serde_json::to_string_pretty(&meta) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&meta_path, json) {
                tracing::warn!("Failed to write meta {:?}: {}", meta_path, e);
            }
        }
        Err(e) => {
            tracing::warn!("Failed to serialize meta for {:?}: {}", video_path, e);
        }
    }
}

/// 删除视频文件对应的元数据文件（若存在）。
/// Delete the metadata file for a video file (if it exists).
pub fn delete_meta(video_path: &Path) {
    if let Some(meta_path) = meta_path_for(video_path)
        && meta_path.exists() {
        let _ = std::fs::remove_file(&meta_path);
    }
}

/// 仅更新 meta 文件的 `status` 字段，其余字段保持不变。
/// 若 meta 文件不存在则从视频文件信息重建后再写入。
///
/// Update only the `status` field of the meta file, leaving other fields unchanged.
/// If the meta file doesn't exist, rebuilds it from video file info before writing.
pub fn set_status(video_path: &Path, status: &str) {
    let mut meta = match read_meta(video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
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
                pp_results: None,
                module_outputs: None,
            }
        }
    };
    meta.status = status.to_string();
    write_meta(video_path, &meta);
}

/// 后处理完成时更新 meta：写入最终状态、各模块结果和输出路径。
/// Update meta when post-processing completes: write final status, module results, and output paths.
pub fn set_pp_done(
    video_path: &Path,
    status: &str,
    results: Vec<PpModuleResult>,
    module_outputs: std::collections::HashMap<String, String>,
) {
    let mut meta = match read_meta(video_path) {
        Some(m) => m,
        None => {
            let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
            let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
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
                pp_results: None,
                module_outputs: None,
            }
        }
    };
    meta.status = status.to_string();
    meta.pp_results = Some(results);
    if !module_outputs.is_empty() {
        meta.module_outputs = Some(module_outputs);
    }
    write_meta(video_path, &meta);
}

/// 若 meta 文件已存在则不覆盖，否则创建初始 meta（用于启动时遗留片段的保险创建）。
/// Does not overwrite if the meta file already exists; otherwise creates an initial meta
/// (used as a safety net for leftover segments on startup).
pub fn ensure_meta(video_path: &Path, started_at: &str) {
    if let Some(meta_path) = meta_path_for(video_path)
        && meta_path.exists() {
        return;
    }
    let size_bytes = std::fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
    let meta = VideoMeta {
        meta_version: META_VERSION,
        status: "merging_waiting".to_string(),
        started_at: started_at.to_string(),
        size_bytes,
        video_duration_secs: None,
        pp_results: None,
        module_outputs: None,
    };
    write_meta(video_path, &meta);
}

/// 启动时扫描输出目录，为所有缺少 meta 文件的视频补写，并补全缺失的 `video_duration_secs`。
/// On startup, scan the output directory and write missing meta files for all videos,
/// also filling in missing `video_duration_secs` via ffprobe.
pub fn startup_ensure_meta_files(
    output_dir: &Path,
    merge_format: &str,
) {
    ensure_meta_files(output_dir, merge_format);
}

/// 扫描输出目录，为所有缺少或版本过旧的 meta 文件执行创建/重建，
/// 并补全缺失的 `video_duration_secs`。
/// 跳过仍处于活跃状态（录制中/合并中/后处理中）的 meta，避免干扰正在进行的任务。
///
/// Scan the output directory and create/rebuild meta files that are missing or have an
/// outdated version, also filling in missing `video_duration_secs` via ffprobe.
/// Skips meta files in active states (recording/merging/post-processing) to avoid
/// interfering with ongoing tasks.
pub fn ensure_meta_files(output_dir: &Path, merge_format: &str) {
    if !output_dir.exists() {
        return;
    }
    let mut count_created = 0usize;
    let mut count_updated = 0usize;
    scan_and_ensure_meta(
        output_dir,
        merge_format,
        &mut count_created,
        &mut count_updated,
    );
    if count_created > 0 || count_updated > 0 {
        tracing::info!(
            "Meta scan: created {} new, rebuilt/updated {} existing meta files",
            count_created,
            count_updated
        );
    }
}

fn scan_and_ensure_meta(
    dir: &Path,
    merge_format: &str,
    count_created: &mut usize,
    count_updated: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') {
                continue;
            }
            scan_and_ensure_meta(&path, merge_format, count_created, count_updated);
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || ext != merge_format {
                continue;
            }

            match read_meta(&path) {
                None => {
                    // meta 不存在：新建
                    // Meta missing: create it
                    let size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
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
                    let video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                    let meta = VideoMeta {
                        meta_version: META_VERSION,
                        status: "finish".to_string(),
                        started_at,
                        size_bytes,
                        video_duration_secs,
                        pp_results: None,
                        module_outputs: None,
                    };
                    write_meta(&path, &meta);
                    *count_created += 1;
                }
                Some(mut meta) => {
                    // 活跃状态的 meta 跳过，避免干扰正在进行的任务
                    // Skip active meta to avoid interfering with ongoing tasks
                    if matches!(
                        meta.status.as_str(),
                        "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                    ) {
                        continue;
                    }

                    let mut changed = false;

                    // 版本不匹配：重建 meta，保留可复用的字段
                    // Version mismatch: rebuild meta, preserving reusable fields
                    if meta.meta_version != META_VERSION {
                        tracing::info!(
                            "Meta version mismatch for {:?}: found {}, expected {} — rebuilding",
                            path,
                            meta.meta_version,
                            META_VERSION
                        );
                        // 重新从磁盘读取 size_bytes 和 video_duration_secs
                        // Re-read size_bytes and video_duration_secs from disk
                        meta.size_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(meta.size_bytes);
                        if meta.video_duration_secs.is_none() {
                            meta.video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                        }
                        // 过渡态修正为 finish（版本升级时崩溃遗留）
                        // Correct transient status to finish (crash remnant during version upgrade)
                        if matches!(
                            meta.status.as_str(),
                            "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                        ) {
                            meta.status = "finish".to_string();
                        }
                        // meta_version 由 write_meta 自动设置为 META_VERSION
                        // meta_version is set to META_VERSION automatically by write_meta
                        write_meta(&path, &meta);
                        *count_updated += 1;
                        continue;
                    }

                    // 版本匹配：仅补全缺失字段
                    // Version matches: only fill in missing fields
                    if meta.video_duration_secs.is_none() {
                        meta.video_duration_secs = crate::recording::recorder::get_video_duration(&path);
                        changed = true;
                    }
                    if changed {
                        write_meta(&path, &meta);
                        *count_updated += 1;
                    }
                }
            }
        }
    }
}

/// 扫描输出目录，删除所有对应视频文件已不存在的孤立 meta 文件（`.{stem}.json`）。
/// Scan the output directory and delete orphaned meta files (`.{stem}.json`) whose
/// corresponding video files no longer exist.
///
/// 返回删除的文件数量。
/// Returns the number of deleted files.
pub fn cleanup_orphaned_meta_files(output_dir: &Path) -> usize {
    if !output_dir.exists() {
        return 0;
    }
    let mut count = 0usize;
    cleanup_orphaned_meta_recursive(output_dir, &mut count);
    if count > 0 {
        tracing::info!("Meta cleanup: deleted {} orphaned meta file(s)", count);
    }
    count
}

fn cleanup_orphaned_meta_recursive(dir: &Path, count: &mut usize) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                cleanup_orphaned_meta_recursive(&path, count);
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // meta 文件格式：以 '.' 开头，以 '.json' 结尾
        // Meta file format: starts with '.', ends with '.json'
        if !name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }

        // 从 meta 文件名还原视频文件 stem：去掉首字符 '.' 和末尾 '.json'
        // Recover video stem from meta filename: strip leading '.' and trailing '.json'
        let stem = &name[1..name.len() - 5]; // ".{stem}.json" → "{stem}"
        let parent = match path.parent() {
            Some(p) => p,
            None => continue,
        };

        // 检查是否存在任意扩展名的同名视频文件
        // Check whether a video file with the same stem exists (any extension)
        let video_exists = std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let p = e.path();
                if !p.is_file() {
                    return false;
                }
                let vname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if vname.starts_with('.') {
                    return false;
                }
                p.file_stem().and_then(|s| s.to_str()) == Some(stem)
            });

        if !video_exists {
            // 跳过仍处于活跃状态的 meta（录制中/合并中/后处理中），
            // 此时视频文件尚未生成，不应视为孤立文件。
            // Skip meta files that are still in an active state (recording / merging / post-processing).
            // The video file doesn't exist yet at these stages, so they are not truly orphaned.
            let is_active = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| serde_json::from_str::<VideoMeta>(&c).ok())
                .map(|m| {
                    matches!(
                        m.status.as_str(),
                        "recording" | "merging_waiting" | "merging" | "pp_waiting" | "pp_running"
                    )
                })
                .unwrap_or(false);

            if is_active {
                tracing::debug!("Meta cleanup: skipping active meta {:?}", path);
                continue;
            }

            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!("Meta cleanup: failed to delete {:?}: {}", path, e);
            } else {
                tracing::info!("Meta cleanup: deleted orphaned meta {:?}", path);
                *count += 1;
            }
        }
    }
}

/// 启动孤立 meta 文件清理调度器：立即执行一次，之后每小时执行一次。
/// Start the orphaned meta cleanup scheduler: run once immediately, then once every hour.
pub async fn schedule_meta_cleanup(output_dir: std::path::PathBuf) {
    // 立即执行一次 / Run once immediately
    let dir = output_dir.clone();
    tokio::task::spawn_blocking(move || {
        cleanup_orphaned_meta_files(&dir);
    })
    .await
    .ok();

    // 之后每小时执行一次 / Then run every hour
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        let dir = output_dir.clone();
        tokio::task::spawn_blocking(move || {
            cleanup_orphaned_meta_files(&dir);
        })
        .await
        .ok();
    }
}

/// 启动 meta 版本检查轮询调度器：立即执行一次，之后每隔指定秒数执行一次。
/// 对版本不匹配或缺失的 meta 文件执行重建，跳过活跃状态的录制。
///
/// Start the meta version-check polling scheduler: run once immediately, then at the
/// specified interval. Rebuilds meta files with missing or mismatched versions,
/// skipping active recordings.
pub async fn schedule_meta_version_check(
    output_dir: std::path::PathBuf,
    merge_format: String,
    interval_secs: u64,
) {
    // 立即执行一次 / Run once immediately
    {
        let dir = output_dir.clone();
        let fmt = merge_format.clone();
        tokio::task::spawn_blocking(move || {
            ensure_meta_files(&dir, &fmt);
        })
        .await
        .ok();
    }

    // 之后按间隔循环执行 / Then run at the specified interval
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
        let dir = output_dir.clone();
        let fmt = merge_format.clone();
        tokio::task::spawn_blocking(move || {
            ensure_meta_files(&dir, &fmt);
        })
        .await
        .ok();
    }
}
