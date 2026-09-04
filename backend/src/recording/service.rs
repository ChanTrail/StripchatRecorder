//! 录制文件管理业务逻辑 / Recording File Management Service
//!
//! 提供录制文件列表查询、合并状态查询、文件删除等功能。
//! 被 `server_mod/routes/recording.rs`、`recording/recorder.rs`、
//! `recording/startup_scan.rs` 调用。
//!
//! Provides recording file list queries, merge status queries, and file deletion.
//! Called by `server_mod/routes/recording.rs`, `recording/recorder.rs`,
//! and `recording/startup_scan.rs`.

use crate::core::error::Result;
use crate::recording::recorder::RecorderManager;
use crate::config::settings::AppState;
use std::fs;
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
    pub video_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_execution: Option<Vec<crate::recording::meta::PpExecutionEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_progress: Option<crate::recording::meta::PpNodeProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_downloaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_failed: Option<u64>,
    pub username: String,
    /// 模块输出路径（如 contact_sheet 生成的预览图），按 module_id 建立映射。
    /// 只包含节点执行结果为 `"ok"` 且路径当前确实存在于磁盘上的条目（见
    /// [`crate::recording::meta::extract_verified_module_outputs`]）——前端应仅
    /// 依据此字段判断预览图按钮是否显示，而非自行推断路径或仅凭 meta 中的路径
    /// 字符串就假定文件存在。
    ///
    /// Module output paths (e.g. contact_sheet's generated preview image), keyed by
    /// module_id. Only includes entries whose node result is `"ok"` and whose path
    /// currently exists on disk (see
    /// [`crate::recording::meta::extract_verified_module_outputs`]) — the frontend
    /// should rely solely on this field to decide whether to show a preview button,
    /// rather than inferring the path itself or assuming a file exists just because
    /// meta records a path string for it.
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty", default)]
    pub module_outputs: std::collections::HashMap<String, String>,
}

/// 录制文件列表查询的核心实现（同步，在阻塞线程中调用）。
/// 数据源：活跃录制会话 + meta/ 目录扫描 + pp_tasks（进行中的后处理）。
///
/// Core implementation of recording file list query (synchronous, called in a blocking thread).
/// Data sources: active recording sessions + meta/ directory scan + pp_tasks (in-progress).
pub fn list_recordings_inner(
    state: &Arc<AppState>,
    recorder: &Arc<RecorderManager>,
) -> std::io::Result<Vec<RecordingFile>> {
    let sessions = recorder.get_active_sessions();
    let live_segment_stats = recorder.segment_stats.read().clone();

    // 活跃录制中的 session_dir stem 集合（用于去重）
    // Set of stems for active recording session_dirs (for deduplication)
    let active_stems: std::collections::HashSet<String> = sessions
        .iter()
        .filter_map(|(sd, _)| sd.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
        .collect();

    let mut files: Vec<RecordingFile> = Vec::new();

    // 1. 活跃录制中的 session_dir（实时进度）
    // 1. Currently active recording session_dirs (real-time progress)
    for (session_dir, started_dt) in &sessions {
        let session_dir_str = session_dir.to_string_lossy().to_string();
        let local: chrono::DateTime<chrono::Local> = (*started_dt).into();
        let elapsed = chrono::Utc::now()
            .signed_duration_since(*started_dt)
            .num_seconds()
            .max(0) as u64;
        let size_bytes = crate::recording::ffmpeg_util::dir_size_bytes(session_dir).unwrap_or(0);
        let username = session_dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let stem = session_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let (seg_dl, seg_fail) = live_segment_stats.get(&session_dir_str).copied().unwrap_or((0, 0));

        files.push(RecordingFile {
            name: stem.to_string(),
            path: session_dir_str,
            size_bytes,
            started_at: local.to_rfc3339(),
            is_recording: true,
            record_duration_secs: Some(elapsed),
            video_duration_secs: None,
            video_resolution: None,
            status: Some("recording".to_string()),
            pp_execution: None,
            pp_progress: None,
            segments_downloaded: Some(seg_dl),
            segments_failed: Some(seg_fail),
            username: username.to_string(),
            module_outputs: std::collections::HashMap::new(),
        });
    }

    // 2. 扫描 meta/ 目录（含所有主播子目录），获取所有已完成/后处理中的录制
    // 2. Scan meta/ directory (including all per-streamer subdirectories) to get all
    //    completed/post-processed recordings
    for meta_path in crate::recording::meta::list_all_meta_paths() {
        let content = match std::fs::read_to_string(&meta_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: crate::recording::meta::VideoMeta = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // 跳过正在录制的（由活跃会话处理）/ Skip actively recording (handled by active sessions)
        if meta.status == "recording" { continue; }

        // video_path 是对应的视频文件或 session_dir 路径
        let vp_str = match meta.video_path.as_deref() {
            Some(p) => p.to_string(),
            None => continue,
        };
        let video_path = std::path::PathBuf::from(&vp_str);
        let stem = video_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");

        // 跳过活跃录制中的 stem（避免重复）/ Skip stems currently being recorded
        if active_stems.contains(stem) { continue; }

        // 从 pp_queue 获取更精确的运行时状态（若有）
        // Use runtime status from pp_queue if available (more accurate)
        let runtime_status = state.pp_queue.get_status(&vp_str);

        let is_dir = video_path.is_dir();
        let size_bytes = if is_dir {
            crate::recording::ffmpeg_util::dir_size_bytes(&video_path).unwrap_or(meta.size_bytes)
        } else if video_path.exists() {
            fs::metadata(&video_path).map(|m| m.len()).unwrap_or(meta.size_bytes)
        } else {
            meta.size_bytes
        };

        let username = video_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let name = if is_dir {
            stem.to_string()
        } else {
            video_path.file_name().and_then(|n| n.to_str()).unwrap_or(stem).to_string()
        };

        // 若 meta 中 size_bytes 与实际不符，顺手更新 / Update size_bytes in meta if stale
        if !is_dir && video_path.exists() && size_bytes != meta.size_bytes && size_bytes > 0 {
            let mut updated = meta.clone();
            updated.size_bytes = size_bytes;
            crate::recording::meta::write_meta(&video_path, &updated);
        }

        let module_outputs = crate::recording::meta::extract_verified_module_outputs(
            meta.pp_execution.as_deref(),
        );

        files.push(RecordingFile {
            name,
            path: vp_str,
            size_bytes,
            started_at: meta.started_at,
            is_recording: false,
            record_duration_secs: None,
            video_duration_secs: meta.video_duration_secs,
            video_resolution: meta.video_resolution,
            status: Some(runtime_status.unwrap_or(meta.status)),
            pp_execution: meta.pp_execution,
            pp_progress: meta.pp_progress,
            segments_downloaded: meta.segments_downloaded,
            segments_failed: meta.segments_failed,
            username,
            module_outputs,
        });
    }

    files.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(files)
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

    state.pp_queue.cancel(path);

    let task_status = state.pp_queue.get_status(path);

    match task_status.as_deref() {
        Some("running") | Some("waiting") => {
            state.pp_queue.remove(path);
        }
        _ => {}
    }

    if p.is_dir() {
        fs::remove_dir_all(p)?;
        crate::recording::meta::delete_meta(p);
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

    state.pp_queue.remove(path);
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
