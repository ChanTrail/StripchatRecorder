//! 遗留分片合并与目录清理 / Leftover Segment Merge and Directory Cleanup
//!
//! 提供遗留分片合并触发和输出目录空目录清理，不涉及活跃录制会话的
//! 生命周期管理（会话生命周期见 `recording::recorder`）。
//! 由 `recording::meta::maintenance` 周期性调用（含启动时的首次立即执行）。
//!
//! 注：`startup_merge_leftover_segments` 目前仅由桌面版（desktop/src-tauri）调用——
//! Server 模式（backend）已由 `meta::ensure_meta_files` 统一处理 session_dir 和视频
//! 文件的（重新）触发逻辑，是否需要合并交给流水线首节点（ts_merge）自行判断。
//! `startup_remove_empty_dirs` 仍在 backend 的定时任务和桌面版中使用。
//!
//! Provides leftover segment merge triggering and output directory empty-dir cleanup.
//! Does not manage active recording session lifecycle (see `recording::recorder`).
//! Called periodically by `recording::meta::maintenance` (including an immediate
//! first run at startup).
//!
//! Note: `startup_merge_leftover_segments` is currently only called by the desktop
//! app (desktop/src-tauri) — Server mode (backend) now handles (re-)triggering for
//! both session_dirs and video files uniformly via `meta::ensure_meta_files`, letting
//! the pipeline's first node (ts_merge) decide whether merging is needed.
//! `startup_remove_empty_dirs` is still used by backend's scheduled tasks and
//! by the desktop app.

use crate::core::emitter::{Emitter, EmitterExt};
use crate::recording::recorder::RecorderManager;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// 启动时扫描输出目录，合并所有遗留的未完成录制片段，并对未后处理的视频触发后处理。
///
/// On startup, scan the output directory to merge all leftover incomplete recording segments,
/// and trigger post-processing for videos that haven't been processed yet.
pub fn startup_merge_leftover_segments(
    output_dir: &std::path::Path,
    emitter: &Arc<dyn Emitter>,
    recorder: &Arc<RecorderManager>,
) -> Vec<PathBuf> {
    let state = recorder.app_state();
    let pipeline = state.get_pipeline();

    // 扫描分片目录（含 .ts 文件的子目录）
    // Scan for segment directories (subdirectories containing .ts files)
    let mut segment_dirs: Vec<PathBuf> = Vec::new();
    if output_dir.exists() {
        collect_segment_dirs(output_dir, &mut segment_dirs);
    }
    // 排除正在录制中的会话目录。此函数不仅在启动时调用一次，也作为定时任务周期性执行，
    // 若不排除活跃会话，会把仍在写入的分片目录误判为"遗留分片"并尝试合并，破坏正在进行的录制。
    //
    // Exclude session directories currently being recorded. This function is not only called
    // once at startup but also periodically as a scheduled task; without this exclusion, an
    // actively-written segment directory would be misidentified as "leftover" and merged,
    // corrupting the in-progress recording.
    segment_dirs.retain(|p| !recorder.is_file_locked(p));
    segment_dirs.sort_by_key(|p| session_dir_timestamp(p));

    // 扫描已完成合并但尚未后处理的视频（从 meta/ 目录读取 status 不是 finish/pp_error 的视频文件）
    // Scan for merged-but-unprocessed videos (from meta/ dir: video files with status not finish/pp_error)
    let mut unprocessed_videos: Vec<PathBuf> = Vec::new();
    {
        for meta_path in crate::recording::meta::list_all_meta_paths() {
            let content = match std::fs::read_to_string(&meta_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta: crate::recording::meta::VideoMeta = match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(_) => continue,
            };
            // 只处理 status 为 recording（未完成）且有 video_path 的条目
            // 实际上是寻找合并完成但后处理未运行的视频：status == "finish" 且 video_path 是文件
            // We look for video files (not dirs) that exist but have no pp_execution success
            if matches!(meta.status.as_str(), "finish") {
                let has_pp = meta.pp_execution.as_ref()
                    .map(|e| !e.is_empty())
                    .unwrap_or(false);
                if has_pp { continue; } // 已后处理过 / Already post-processed
            }
            let vp_str = match meta.video_path.as_deref() {
                Some(p) => p.to_string(),
                None => continue,
            };
            let path = std::path::PathBuf::from(&vp_str);
            if !path.is_file() {
                continue;
            }
            // 排除 finish（已完成，上面已单独处理）和 recording（由活跃会话处理）。
            // pp_waiting/pp_running 若不在本进程的内存队列中追踪，说明是进程重启前遗留的
            // 陈旧状态（上次异常退出），需要重新加入 unprocessed_videos 触发后处理，
            // 而不是被当作"仍在进行"而永久跳过。
            //
            // Exclude finish (already handled above) and recording (handled by active sessions).
            // If pp_waiting/pp_running is not tracked by this process's in-memory queue, it's a
            // stale status left over from a previous abnormal exit and should be re-added to
            // unprocessed_videos to trigger post-processing, rather than being permanently
            // skipped as "still in progress".
            match meta.status.as_str() {
                "finish" | "pp_error" | "recording" => continue,
                "pp_waiting" | "pp_running" if state.pp_queue.is_tracked(&vp_str) => continue,
                _ => unprocessed_videos.push(path),
            }
        }
        unprocessed_videos.sort_by_key(|p| {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            session_dir_timestamp_from_stem(stem)
        });
    }

    if segment_dirs.is_empty() && unprocessed_videos.is_empty() {
        return Vec::new();
    }

    let _startup_guard = state.startup_lock.lock().unwrap_or_else(|e| e.into_inner());

    let mut merged_paths = Vec::new();
    let mut pp_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

    // 对已合并但未后处理的视频直接触发用户流水线（不包含 ts_merge）
    // For already-merged videos, trigger the user pipeline directly (no ts_merge)
    for video_path in unprocessed_videos {
        if !pipeline.nodes.is_empty() {
            let pp_state = Arc::clone(&state);
            let pp_emitter = Arc::clone(emitter);
            let pp_pipeline = pipeline.clone();
            let video_path_clone = video_path.clone();
            let handle = std::thread::spawn(move || {
                crate::postprocess::service::run_postprocess_for_path(
                    &video_path_clone,
                    &video_path_clone,
                    &pp_pipeline,
                    &pp_emitter,
                    &pp_state,
                );
            });
            pp_handles.push(handle);
        }
    }

    // 对遗留分片目录，触发后处理流水线（session_dir 作为初始输入和 meta 占位）
    // For leftover segment dirs, trigger pipeline (session_dir as initial input and meta placeholder)
    for path in &segment_dirs {
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        recorder.waiting_merge_dirs.write().insert(path.clone());
        emitter.emit(
            "recording-pp-waiting",
            &serde_json::json!({
                "username": username,
                "session_dir": path.to_string_lossy(),
            }),
        );

        // 预创建 session_dir 的 meta 文件
        // Pre-create meta file for the session_dir
        let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
        let started_at = crate::recording::service::parse_timestamp_from_stem_pub(stem)
            .unwrap_or_else(|| {
                fs::metadata(path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default()
            });
        crate::recording::meta::ensure_meta(path, &started_at);
    }

    for path in segment_dirs {
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        recorder.waiting_merge_dirs.write().remove(&path);

        // 若流水线中没有任何启用节点，直接跳过后处理
        // If no enabled nodes, skip post-processing entirely
        if !pipeline.nodes.iter().any(|n| n.enabled) {
            continue;
        }

        let _ = username;
        let pp_state = Arc::clone(&state);
        let pp_emitter = Arc::clone(emitter);
        let path_clone = path.clone();
        let pp_pipeline = pipeline.clone();
        let handle = std::thread::spawn(move || {
            crate::postprocess::service::run_postprocess_for_path(
                &path_clone,
                &path_clone,
                &pp_pipeline,
                &pp_emitter,
                &pp_state,
            );
        });
        pp_handles.push(handle);

        merged_paths.push(path);
    }

    for handle in pp_handles {
        let _ = handle.join();
    }

    merged_paths
}

/// 递归清理输出目录下的所有空目录。
/// Recursively remove all empty directories under the output directory.
pub fn startup_remove_empty_dirs(output_dir: &std::path::Path) {
    if !output_dir.exists() {
        return;
    }

    let removed = remove_empty_dirs_recursive(output_dir, false);
    if removed > 0 {
        tracing::info!(
            "Startup: removed {} empty directories under {:?}",
            removed,
            output_dir
        );
    }
}

fn remove_empty_dirs_recursive(dir: &std::path::Path, remove_self: bool) -> usize {
    let mut removed = 0;

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            removed += remove_empty_dirs_recursive(&path, true);
        }
    }

    if remove_self {
        let is_empty = fs::read_dir(dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if is_empty && fs::remove_dir(dir).is_ok() {
            removed += 1;
        }
    }

    removed
}

fn collect_segment_dirs(dir: &std::path::Path, result: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let has_segments = fs::read_dir(&path)
            .map(|mut e| {
                e.any(|f| {
                    f.ok()
                        .and_then(|f| {
                            f.path()
                                .extension()
                                .and_then(|x| x.to_str())
                                .map(|x| x == "ts")
                                .filter(|&b| b)
                                .map(|_| ())
                        })
                        .is_some()
                })
            })
            .unwrap_or(false);
        if has_segments {
            result.push(path);
        } else {
            collect_segment_dirs(&path, result);
        }
    }
}

fn session_dir_timestamp(path: &std::path::Path) -> String {
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    session_dir_timestamp_from_stem(stem)
}

fn session_dir_timestamp_from_stem(stem: &str) -> String {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 2 {
        let date = parts[parts.len() - 2];
        let time = parts[parts.len() - 1];
        if date.len() == 8
            && time.len() == 6
            && date.chars().all(|c| c.is_ascii_digit())
            && time.chars().all(|c| c.is_ascii_digit())
        {
            return format!("{}_{}", date, time);
        }
    }
    stem.to_string()
}
