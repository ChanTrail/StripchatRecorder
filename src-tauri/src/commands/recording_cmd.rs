//! 录制文件管理 Tauri 命令 / Recording File Management Tauri Commands
//!
//! 提供录制文件列表查询、合并状态查询、文件打开/删除、输出目录打开等命令。
//! 文件列表查询使用并发 ffprobe 探测视频时长，并通过缓存避免重复探测。
//!
//! Provides commands for querying recording file lists, merge status, opening/deleting files,
//! and opening the output directory.
//! File list queries use concurrent ffprobe to probe video duration with caching to avoid re-probing.

use crate::core::error::Result;
use crate::recording::recorder::{dir_size_bytes, get_video_duration, RecorderManager};
use crate::config::settings::AppState;
use chrono::TimeZone;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// 录制文件元数据（序列化后返回给前端）/ Recording file metadata (serialized and returned to the frontend)
#[derive(serde::Serialize)]
pub struct RecordingFile {
    /// 文件名（含扩展名）/ Filename (with extension)
    pub name: String,
    /// 文件完整路径 / Full file path
    pub path: String,
    /// 文件大小（字节）/ File size (bytes)
    pub size_bytes: u64,
    /// 录制开始时间（RFC 3339 格式）/ Recording start time (RFC 3339 format)
    pub started_at: String,
    /// 是否正在录制 / Whether currently recording
    pub is_recording: bool,
    /// 已录制时长（秒）/ Recorded duration (seconds)
    pub record_duration_secs: Option<u64>,
    /// 视频实际时长（秒，由 ffprobe 获取）/ Actual video duration (seconds, from ffprobe)
    pub video_duration_secs: Option<u64>,
}

/// 列出所有录制文件（在阻塞线程池中执行，避免阻塞异步运行时）。
/// List all recording files (executed in a blocking thread pool to avoid blocking the async runtime).
#[tauri::command]
pub async fn list_recordings(
    state: State<'_, Arc<AppState>>,
    recorder: State<'_, Arc<RecorderManager>>,
) -> Result<Vec<RecordingFile>> {
    let state = Arc::clone(&state);
    let recorder = Arc::clone(&recorder);
    tokio::task::spawn_blocking(move || list_recordings_inner(&state, &recorder))
        .await
        .map_err(|e| crate::core::error::AppError::Other(e.to_string()))?
        .map_err(Into::into)
}

/// 获取当前正在合并和等待合并的会话目录列表。
/// Get the list of session directories currently merging or waiting to merge.
#[tauri::command]
pub async fn get_merging_dirs(
    recorder: State<'_, Arc<RecorderManager>>,
) -> Result<Vec<serde_json::Value>> {
    let settings = recorder.get_settings();
    let merge_format = settings.merge_format.clone();

    let make_entry = |path: &PathBuf, status: &str| {
        let path_str = path.to_string_lossy().to_string();
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let parent = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let sep = if path_str.contains('\\') { "\\" } else { "/" };
        let merged_path = format!("{}{}{}.{}", parent, sep, stem, merge_format);
        serde_json::json!({
            "session_dir": path_str,
            "merged_path": merged_path,
            "merge_format": merge_format,
            "username": username,
            "status": status,
        })
    };

    let mut result: Vec<serde_json::Value> = recorder
        .merging_dirs
        .read()
        .iter()
        .map(|p| make_entry(p, "merging"))
        .collect();
    result.extend(
        recorder
            .waiting_merge_dirs
            .read()
            .iter()
            .map(|p| make_entry(p, "waiting")),
    );
    Ok(result)
}

/// 录制文件列表查询的核心实现（同步，在阻塞线程中调用）。
/// 遍历输出目录，收集所有录制文件，并并发探测视频时长（带缓存）。
///
/// Core implementation of recording file list query (synchronous, called in a blocking thread).
/// Traverses the output directory, collects all recording files, and concurrently probes video duration (with caching).
pub fn list_recordings_inner(
    state: &Arc<AppState>,
    recorder: &Arc<RecorderManager>,
) -> std::io::Result<Vec<RecordingFile>> {
    let settings = state.get_settings();
    let output_dir = std::path::Path::new(&settings.output_dir);

    if !output_dir.exists() {
        return Ok(Vec::new());
    }

    let sessions = recorder.get_active_sessions();
    let merging = recorder.merging_dirs.read().clone();
    let waiting_merging = recorder.waiting_merge_dirs.read().clone();
    let all_merging: std::collections::HashSet<PathBuf> =
        merging.union(&waiting_merging).cloned().collect();

    let mut files: Vec<RecordingFile> = Vec::new();
    // 需要 ffprobe 探测的文件列表：(文件索引, 路径, 大小, 修改时间)
    // Files that need ffprobe probing: (file index, path, size, mtime)
    let mut needs_probe: Vec<(usize, PathBuf, u64, u64)> = Vec::new();
    let mut new_cache: HashMap<(String, u64, u64), Option<u64>> = HashMap::new();

    {
        let old_cache = state.duration_cache.read();
        collect_recordings(
            output_dir,
            &mut files,
            &sessions,
            &all_merging,
            &old_cache,
            &mut new_cache,
            &mut needs_probe,
        )?;
    }

    // 并发探测视频时长（按 CPU 核心数分块）/ Concurrently probe video duration (chunked by CPU count)
    if !needs_probe.is_empty() {
        let concurrency = num_cpus().min(needs_probe.len());
        let chunk_size = (needs_probe.len() + concurrency - 1) / concurrency;

        let mut probe_results: Vec<(usize, Option<u64>, PathBuf, u64, u64)> =
            Vec::with_capacity(needs_probe.len());

        std::thread::scope(|s| {
            let chunks: Vec<_> = needs_probe.chunks(chunk_size).collect();
            let handles: Vec<_> = chunks
                .into_iter()
                .map(|chunk| {
                    let chunk: Vec<_> = chunk.to_vec();
                    s.spawn(move || -> Vec<(usize, Option<u64>, PathBuf, u64, u64)> {
                        chunk
                            .into_iter()
                            .map(|(idx, path, size, mtime)| {
                                let dur = get_video_duration(&path);
                                (idx, dur, path, size, mtime)
                            })
                            .collect()
                    })
                })
                .collect();

            for handle in handles {
                if let Ok(results) = handle.join() {
                    probe_results.extend(results);
                }
            }
        });

        for (file_idx, dur, path, size, mtime) in probe_results {
            files[file_idx].video_duration_secs = dur;
            let key = (path.to_string_lossy().to_string(), size, mtime);
            new_cache.insert(key, dur);
        }
    }

    // 更新时长缓存 / Update duration cache
    *state.duration_cache.write() = new_cache;
    files.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(files)
}

/// 获取可用的 CPU 核心数，用于并发探测。
/// Get the number of available CPU cores for concurrent probing.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// 递归遍历目录，收集录制文件和会话目录，填充 `files`、`new_cache` 和 `needs_probe`。
/// Recursively traverse the directory, collecting recording files and session directories,
/// populating `files`, `new_cache`, and `needs_probe`.
fn collect_recordings(
    dir: &std::path::Path,
    files: &mut Vec<RecordingFile>,
    sessions: &[(PathBuf, chrono::DateTime<chrono::Utc>)],
    merging: &std::collections::HashSet<PathBuf>,
    old_cache: &HashMap<(String, u64, u64), Option<u64>>,
    new_cache: &mut HashMap<(String, u64, u64), Option<u64>>,
    needs_probe: &mut Vec<(usize, PathBuf, u64, u64)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // 跳过正在合并的会话目录 / Skip session directories currently being merged
            if merging.contains(&path) {
                continue;
            }

            let segments = count_ts_segments(&path)?;
            let is_active_session = sessions.iter().any(|(sp, _)| sp == &path);

            if segments > 0 || is_active_session {
                // 这是一个录制会话目录 / This is a recording session directory
                let total_size = dir_size_bytes(&path).unwrap_or(0);
                let is_recording = is_active_session;

                let (started_at, record_duration_secs) =
                    if let Some((_, dt)) = sessions.iter().find(|(sp, _)| sp == &path) {
                        let local: chrono::DateTime<chrono::Local> = (*dt).into();
                        let elapsed = chrono::Utc::now()
                            .signed_duration_since(*dt)
                            .num_seconds()
                            .max(0) as u64;
                        (local.to_rfc3339(), Some(elapsed))
                    } else {
                        // 从目录名解析时间戳 / Parse timestamp from directory name
                        let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        let ts = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                            fs::metadata(&path)
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .map(|t| {
                                    let dt: chrono::DateTime<chrono::Local> = t.into();
                                    dt.to_rfc3339()
                                })
                                .unwrap_or_default()
                        });
                        (ts, None)
                    };

                files.push(RecordingFile {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    path: path.to_string_lossy().to_string(),
                    size_bytes: total_size,
                    started_at,
                    is_recording,
                    record_duration_secs,
                    video_duration_secs: None,
                });
            } else {
                // 普通子目录（主播目录），递归遍历 / Regular subdirectory (streamer dir), recurse
                collect_recordings(
                    &path,
                    files,
                    sessions,
                    merging,
                    old_cache,
                    new_cache,
                    needs_probe,
                )?;
            }
        } else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "mp4" | "mkv" | "ts") {
                let size_bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let started_at = parse_timestamp_from_stem(stem).unwrap_or_else(|| {
                    fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Local> = t.into();
                            dt.to_rfc3339()
                        })
                        .unwrap_or_default()
                });

                let meta = fs::metadata(&path).ok();
                let mtime = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let key = (path.to_string_lossy().to_string(), size_bytes, mtime);

                // 优先使用缓存的时长，否则加入待探测列表
                // Prefer cached duration; otherwise add to the probe list
                let video_duration_secs = if let Some(&cached) = old_cache.get(&key) {
                    new_cache.insert(key, cached);
                    cached
                } else {
                    let idx = files.len();
                    needs_probe.push((idx, path.clone(), size_bytes, mtime));
                    None
                };

                files.push(RecordingFile {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    path: path.to_string_lossy().to_string(),
                    size_bytes,
                    started_at,
                    is_recording: false,
                    record_duration_secs: None,
                    video_duration_secs,
                });
            }
        }
    }
    Ok(())
}

/// 统计目录中 .ts 分片文件的数量。
/// Count the number of .ts segment files in a directory.
fn count_ts_segments(dir: &std::path::Path) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext == "ts" {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 从文件名 stem（格式：`{name}_{YYYYMMDD}_{HHmmss}`）中解析录制开始时间。
/// Parse the recording start time from a filename stem (format: `{name}_{YYYYMMDD}_{HHmmss}`).
///
/// # 返回值 / Returns
/// RFC 3339 格式的时间字符串，解析失败时返回 `None`。
/// RFC 3339 time string, or `None` if parsing fails.
fn parse_timestamp_from_stem(stem: &str) -> Option<String> {
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

/// 用系统默认程序打开指定录制文件。
/// Open the specified recording file with the system default application.
#[tauri::command]
pub async fn open_recording(path: String) -> Result<()> {
    opener::open(&path).map_err(|e| crate::core::error::AppError::Other(e.to_string()))
}

/// 删除指定录制文件或会话目录，同时清理相关的后处理状态和旁路文件（封面图等）。
/// Delete the specified recording file or session directory, cleaning up related post-processing state and sidecar files (cover images, etc.).
#[tauri::command]
pub async fn delete_recording(
    path: String,
    recorder: State<'_, Arc<RecorderManager>>,
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
) -> Result<()> {
    let recorder = Arc::clone(&recorder);
    let state = Arc::clone(&state);
    let path_clone = path.clone();
    tokio::task::spawn_blocking(move || delete_recording_inner(&path_clone, &recorder, &state))
        .await
        .map_err(|e| crate::core::error::AppError::Other(e.to_string()))??;
    let _ = app_handle.emit("recording-deleted", serde_json::json!({ "path": path }));
    Ok(())
}

/// 删除录制文件的核心实现（同步，在阻塞线程中调用）。
/// 处理文件锁检查、后处理取消、重试删除和旁路文件清理。
///
/// Core implementation of recording file deletion (synchronous, called in a blocking thread).
/// Handles file lock checks, post-processing cancellation, retry deletion, and sidecar file cleanup.
pub fn delete_recording_inner(
    path: &str,
    recorder: &Arc<RecorderManager>,
    state: &Arc<AppState>,
) -> Result<()> {
    let p = std::path::Path::new(path);
    if recorder.is_file_locked(p) {
        return Err(crate::core::error::AppError::Other(
            "录制中，无法删除".to_string(),
        ));
    }

    // 请求取消正在进行的后处理 / Request cancellation of any in-progress post-processing
    state.pp_task_cancel(path);

    let task_status = state.pp_tasks.read().get(path).map(|t| t.status.clone());

    match task_status.as_deref() {
        Some("running") => {
            // 等待后处理锁释放（确保后处理已停止）/ Wait for pp_lock to be released (ensures post-processing has stopped)
            let _guard = state.pp_lock.lock().unwrap_or_else(|e| e.into_inner());
            drop(_guard);
        }
        Some("waiting") => {
            // 直接从队列中移除等待中的任务 / Remove waiting task directly from the queue
            state.pp_tasks.write().remove(path);
        }
        _ => {}
    }

    if p.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        // 对文件删除进行重试（最多 20 次，间隔 200ms），处理文件被短暂锁定的情况
        // Retry file deletion up to 20 times with 200ms intervals to handle brief file locks
        let mut last_err = None;
        for _ in 0..20 {
            match fs::remove_file(p) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
        if let Some(e) = last_err {
            return Err(crate::core::error::AppError::Other(e.to_string()));
        }
        // 同时删除同名的封面图旁路文件 / Also delete sidecar cover image files with the same stem
        if let Some(parent) = p.parent() {
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                for ext in &["webp", "jpg", "jpeg", "png"] {
                    let sidecar = parent.join(format!("{}.{}", stem, ext));
                    if sidecar.exists() {
                        let _ = fs::remove_file(&sidecar);
                    }
                }
            }
        }
    }
    // 清理后处理记录和任务状态 / Clean up post-processing records and task status
    {
        let mut data = state.data.write();
        if data.pp_results.remove(path).is_some() {
            drop(data);
            let _ = state.save();
        }
    }
    state.pp_tasks.write().remove(path);
    Ok(())
}

/// 用系统默认文件管理器打开录制输出目录。
/// Open the recording output directory with the system default file manager.
#[tauri::command]
pub async fn open_output_dir(state: State<'_, Arc<AppState>>) -> Result<()> {
    let settings = state.get_settings();
    opener::open(&settings.output_dir).map_err(|e| crate::core::error::AppError::Other(e.to_string()))
}
