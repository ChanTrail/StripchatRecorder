//! 录制文件管理命令 / Recording File Management Commands
//!
//! 提供录制文件列表查询、合并状态查询、文件删除等功能。
//! Provides recording file list queries, merge status queries, and file deletion.
//! These functions are called directly by the HTTP server handlers in server_mod/server.rs.

use crate::core::error::Result;
use crate::recording::recorder::RecorderManager;
use crate::config::settings::AppState;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// 录制文件元数据（序列化后返回给前端）/ Recording file metadata (serialized and returned to the frontend)
#[derive(serde::Serialize)]
pub struct RecordingFile {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub started_at: String,
    pub is_recording: bool,
    pub record_duration_secs: Option<u64>,
    pub video_duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_results: Option<Vec<crate::recording::meta::PpModuleResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_outputs: Option<std::collections::HashMap<String, String>>,
}

/// 录制文件列表查询的核心实现（同步，在阻塞线程中调用）。
/// Core implementation of recording file list query (synchronous, called in a blocking thread).
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
            if name.starts_with('.') || merging.contains(&path) {
                continue;
            }
            collect_from_meta(&path, files, sessions, merging, merge_format)?;
        } else if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }

            let stem = match name.strip_prefix('.').and_then(|s| s.strip_suffix(".json")) {
                Some(s) if !s.is_empty() => s,
                _ => continue,
            };

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
                let session_dir = path.parent().unwrap_or(dir).join(stem);

                if let Some((_, dt)) = sessions.iter().find(|(sp, _)| sp == &session_dir) {
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

/// 删除录制文件的核心实现（同步，在阻塞线程中调用）。
/// Core implementation of recording file deletion (synchronous, called in a blocking thread).
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

    state.pp_task_cancel(path);

    let task_status = state.pp_tasks.read().get(path).map(|t| t.status.clone());

    match task_status.as_deref() {
        Some("running") => {
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
        if let Some(parent) = p.parent()
            && let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            for ext in &["webp", "jpg", "jpeg", "png"] {
                let sidecar = parent.join(format!("{}.{}", stem, ext));
                if sidecar.exists() {
                    let _ = fs::remove_file(&sidecar);
                }
            }
        }
        crate::recording::meta::delete_meta(p);
    }

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
