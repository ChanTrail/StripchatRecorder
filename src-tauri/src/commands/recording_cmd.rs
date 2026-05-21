//! 录制文件管理 Tauri 命令 / Recording File Management Tauri Commands
//!
//! 提供录制文件列表查询、合并状态查询、文件打开/删除、输出目录打开等命令。
//! 文件列表查询完全基于 meta 文件（`.{stem}.json`），无需 ffprobe 或目录遍历视频文件。
//!
//! Provides commands for querying recording file lists, merge status, opening/deleting files,
//! and opening the output directory.
//! File list queries are entirely based on meta files (`.{stem}.json`),
//! requiring no ffprobe calls or video file directory traversal.

use crate::core::error::Result;
use crate::recording::recorder::RecorderManager;
use crate::config::settings::AppState;
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
    /// 视频实际时长（秒，由 ffprobe 获取并写入 meta）/ Actual video duration (seconds, from ffprobe via meta)
    pub video_duration_secs: Option<u64>,
    /// 当前处理状态（来自 meta 文件）/ Current processing status (from meta file)
    /// recording / merging_waiting / merging / pp_waiting / pp_running / pp_error / finish
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 各模块后处理结果（来自 meta 文件）/ Per-module post-processing results (from meta file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_results: Option<Vec<crate::recording::meta::PpModuleResult>>,
    /// 模块输出路径（来自 meta 文件）/ Module output paths (from meta file)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_outputs: Option<std::collections::HashMap<String, String>>,
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
///
/// 完全基于 meta 文件（`.{stem}.json`）构建文件列表，不扫描视频文件本身，不调用 ffprobe。
/// ffprobe 仅在合并完成时写入 meta，此处直接读取结果。
///
/// Core implementation of recording file list query (synchronous, called in a blocking thread).
///
/// Builds the file list entirely from meta files (`.{stem}.json`), without scanning video files
/// or calling ffprobe. ffprobe is only called when writing meta after merge; here we just read it.
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

    collect_from_meta(
        output_dir,
        &mut files,
        &sessions,
        &all_merging,
        &settings.merge_format,
    )?;

    files.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(files)
}

/// 递归遍历目录，仅扫描 meta 文件（`.{stem}.json`）构建录制文件列表。
///
/// 对每个 meta 文件：
/// - 推断对应的视频文件路径
/// - 若视频文件存在：检查实际大小是否与 meta 记录一致，不一致则更新 meta
/// - 若视频文件不存在且对应会话目录正在录制：生成录制中的占位记录
/// - 若视频文件不存在且无活跃会话（合并中/等待合并）：跳过（由前端通过 merging 状态显示）
///
/// Recursively traverse the directory, scanning only meta files (`.{stem}.json`) to build
/// the recording file list.
///
/// For each meta file:
/// - Infer the corresponding video file path
/// - If video exists: check if actual size matches meta; update meta if different
/// - If video doesn't exist and session is recording: generate an in-progress placeholder record
/// - If video doesn't exist and no active session (merging/waiting): skip (frontend shows via merging state)
fn collect_from_meta(
    dir: &std::path::Path,
    files: &mut Vec<RecordingFile>,
    sessions: &[(PathBuf, chrono::DateTime<chrono::Utc>)],
    merging: &std::collections::HashSet<PathBuf>,
    merge_format: &str,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // 跳过隐藏目录和正在合并的会话目录
            // Skip hidden directories and session directories currently being merged
            if name.starts_with('.') || merging.contains(&path) {
                continue;
            }
            collect_from_meta(&path, files, sessions, merging, merge_format)?;
        } else if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // 只处理 meta 文件：以 '.' 开头，以 '.json' 结尾
            // Only process meta files: start with '.' and end with '.json'
            if !name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }

            // 从 meta 文件名提取 stem：去掉前导 '.' 和 '.json' 后缀
            // Extract stem from meta filename: strip leading '.' and '.json' suffix
            let stem = match name.strip_prefix('.').and_then(|s| s.strip_suffix(".json")) {
                Some(s) if !s.is_empty() => s,
                _ => continue,
            };

            // 读取 meta 内容
            // Read meta content
            let meta = match crate::recording::meta::read_meta(
                &path.parent().unwrap_or(dir).join(format!("{}.{}", stem, merge_format)),
            ) {
                Some(m) => m,
                None => continue,
            };

            let video_path = path
                .parent()
                .unwrap_or(dir)
                .join(format!("{}.{}", stem, merge_format));

            if video_path.exists() {
                // 视频文件已存在：检查大小是否变化（如后处理替换了文件）
                // Video file exists: check if size changed (e.g., post-processing replaced it)
                let actual_size = fs::metadata(&video_path).map(|m| m.len()).unwrap_or(0);
                let effective_size = if meta.size_bytes != actual_size && actual_size > 0 {
                    let mut updated = meta.clone();
                    updated.size_bytes = actual_size;
                    crate::recording::meta::write_meta(&video_path, &updated);
                    actual_size
                } else {
                    meta.size_bytes
                };

                files.push(RecordingFile {
                    name: format!("{}.{}", stem, merge_format),
                    path: video_path.to_string_lossy().to_string(),
                    size_bytes: effective_size,
                    started_at: meta.started_at,
                    is_recording: false,
                    record_duration_secs: None,
                    video_duration_secs: meta.video_duration_secs,
                    status: Some(meta.status),
                    pp_results: meta.pp_results,
                    module_outputs: meta.module_outputs,
                });
            } else {
                // 视频文件不存在：根据 meta 的 status 决定如何生成记录
                // Video file doesn't exist: decide how to generate a record based on meta status
                let session_dir = path.parent().unwrap_or(dir).join(stem);

                if let Some((_, dt)) = sessions.iter().find(|(sp, _)| sp == &session_dir) {
                    // 有活跃录制会话：生成录制中的占位记录，使用实时时长和目录大小
                    // Active recording session: generate in-progress placeholder with live duration and dir size
                    let local: chrono::DateTime<chrono::Local> = (*dt).into();
                    let elapsed = chrono::Utc::now()
                        .signed_duration_since(*dt)
                        .num_seconds()
                        .max(0) as u64;
                    let size_bytes = crate::recording::recorder::dir_size_bytes(&session_dir)
                        .unwrap_or(0);

                    files.push(RecordingFile {
                        name: format!("{}.{}", stem, merge_format),
                        path: video_path.to_string_lossy().to_string(),
                        size_bytes,
                        started_at: local.to_rfc3339(),
                        is_recording: true,
                        record_duration_secs: Some(elapsed),
                        video_duration_secs: None,
                        status: Some("recording".to_string()),
                        pp_results: None,
                        module_outputs: None,
                    });
                } else {
                    // 无活跃会话（merging_waiting / merging 等过渡态）：
                    // 直接用 meta 中的信息生成记录，让前端根据 status 显示对应状态
                    //
                    // No active session (merging_waiting / merging transient states):
                    // Generate a record from meta info; frontend displays based on status field
                    files.push(RecordingFile {
                        name: format!("{}.{}", stem, merge_format),
                        path: video_path.to_string_lossy().to_string(),
                        size_bytes: meta.size_bytes,
                        started_at: meta.started_at,
                        is_recording: false,
                        record_duration_secs: None,
                        video_duration_secs: None,
                        status: Some(meta.status),
                        pp_results: meta.pp_results,
                        module_outputs: meta.module_outputs,
                    });
                }
            }
        }
    }
    Ok(())
}

/// 从文件名 stem（格式：`{name}_{YYYYMMDD}_{HHmmss}`）中解析录制开始时间。
/// Parse the recording start time from a filename stem (format: `{name}_{YYYYMMDD}_{HHmmss}`).
pub fn parse_timestamp_from_stem_pub(stem: &str) -> Option<String> {
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

/// 用系统默认程序打开指定录制文件。
/// Open the specified recording file with the system default application.
#[tauri::command]
pub async fn open_recording(path: String) -> Result<()> {
    opener::open(&path).map_err(|e| crate::core::error::AppError::Other(e.to_string()))
}

/// 删除指定录制文件或会话目录，同时清理相关的后处理状态和旁路文件（封面图、meta 等）。
/// Delete the specified recording file or session directory, cleaning up related
/// post-processing state and sidecar files (cover images, meta, etc.).
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
            // 已设置取消标志，后处理子进程会在 100ms 内被终止。
            // 不等待 pp_lock，直接继续删除文件，避免阻塞用户操作。
            // Cancel flag is set; the post-processing subprocess will be killed within 100ms.
            // Do not wait for pp_lock — proceed with deletion immediately to avoid blocking the user.
            state.pp_tasks.write().remove(path);
        }
        Some("waiting") => {
            state.pp_tasks.write().remove(path);
        }
        _ => {}
    }

    if p.is_dir() {
        fs::remove_dir_all(p)?;
    } else {
        // 对文件删除进行重试（最多 20 次，间隔 200ms）
        // Retry file deletion up to 20 times with 200ms intervals
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
        // 删除同名的封面图旁路文件 / Delete sidecar cover image files with the same stem
        if let Some(parent) = p.parent()
            && let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            for ext in &["webp", "jpg", "jpeg", "png"] {
                let sidecar = parent.join(format!("{}.{}", stem, ext));
                if sidecar.exists() {
                    let _ = fs::remove_file(&sidecar);
                }
            }
        }
        // 删除 meta 文件 / Delete meta file
        crate::recording::meta::delete_meta(p);
    }

    // 清理后处理记录和任务状态 / Clean up post-processing records and task status
    {
        let mut data = state.data.write();
        let before = data.pp_results.len();
        data.pp_results.retain(|p| p != path);
        if data.pp_results.len() != before {
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
    opener::open(&settings.output_dir)
        .map_err(|e| crate::core::error::AppError::Other(e.to_string()))
}
