//! ffmpeg/ffprobe 底层工具函数 / Low-level ffmpeg/ffprobe Utility Functions
//!
//! 提供分片转码（fMP4→TS）、m3u8 维护、目录大小计算、遗留分片合并、
//! 视频时长/分辨率探测等纯 ffmpeg/ffprobe 操作，不涉及录制会话生命周期管理
//! （会话生命周期见 `recording::recorder`）。
//!
//! Provides low-level ffmpeg/ffprobe operations: segment transcoding (fMP4→TS),
//! m3u8 maintenance, directory size calculation, leftover segment merging, and
//! video duration/resolution probing. Does not manage recording session lifecycle
//! (see `recording::recorder` for that).

use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock};
use tokio::sync::Semaphore;

/// 全局 ffmpeg 并发信号量，限制同时运行的 ffmpeg 进程数（最多 4 个）。
/// Global ffmpeg concurrency semaphore, limiting simultaneous ffmpeg processes (max 4).
static FFMPEG_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(4));

/// 检查 ffmpeg 是否在 PATH 中可用。
/// Check if ffmpeg is available on PATH.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// 使用 ffmpeg 将 fMP4 数据转换为 MPEG-TS 格式（通过 stdin 管道传入）。
/// Convert fMP4 data to MPEG-TS format using ffmpeg (piped via stdin).
pub(crate) async fn convert_to_ts(fmp4_data: Vec<u8>, ts_path: &PathBuf) -> Result<()> {
    let _permit = FFMPEG_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| AppError::Other(format!("ffmpeg semaphore: {}", e)))?;

    let mut child = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-i", "pipe:0", "-c", "copy", "-f", "mpegts"])
        .arg(ts_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(&fmp4_data)
            .await
            .map_err(|e| AppError::Other(format!("ffmpeg stdin write: {}", e)))?;
    }

    let status = child
        .wait()
        .await
        .map_err(|e| AppError::Other(format!("ffmpeg wait: {}", e)))?;

    if !status.success() {
        return Err(AppError::Other(format!("ffmpeg exited with {}", status)));
    }
    Ok(())
}

/// 将 TS 分片文件名追加到会话目录的 playlist.m3u8（标准 HLS 格式）。
/// Append a TS segment filename to the session directory's playlist.m3u8 (standard HLS format).
///
/// 首次写入时自动添加 M3U8 文件头（`#EXTM3U` 和 `#EXT-X-VERSION:3`）。
/// Automatically writes the M3U8 header (`#EXTM3U` and `#EXT-X-VERSION:3`) on first write.
pub(crate) fn append_to_m3u8(session_dir: &std::path::Path, ts_path: &std::path::Path) {
    let m3u8_path = session_dir.join("playlist.m3u8");
    let Some(filename) = ts_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };

    // 首次创建时写入 M3U8 文件头 / Write M3U8 header on first creation
    let needs_header = !m3u8_path.exists();
    let mut file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&m3u8_path)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to open playlist.m3u8: {}", e);
            return;
        }
    };

    if needs_header
        && let Err(e) = file.write_all(b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-MEDIA-SEQUENCE:0\n")
    {
        tracing::error!("Failed to write M3U8 header: {}", e);
        return;
    }

    // 写入分片条目（时长占位为 0，实际时长未知）/ Write segment entry (duration placeholder 0, actual duration unknown)
    let line = format!("#EXTINF:0,\n{}\n", filename);
    if let Err(e) = file.write_all(line.as_bytes()) {
        tracing::error!("Failed to update playlist.m3u8: {}", e);
    }
}

/// 计算目录中所有文件的总大小（字节）。
/// Calculate the total size of all files in a directory (bytes).
pub fn dir_size_bytes(dir: &PathBuf) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

/// 使用 ffmpeg 将会话目录中的所有 TS 分片合并为单个视频文件。
/// 合并过程中定期发送 `merge-progress` 事件，合并完成后删除会话目录。
/// 此函数仅在启动时合并遗留片段时使用（正常录制流程由 ts_merge 模块负责合并）。
///
/// Merge all TS segments in the session directory into a single video file using ffmpeg.
/// Periodically emits `merge-progress` events; deletes the session directory after completion.
/// This function is only used for merging leftover segments on startup
/// (the ts_merge module handles merging in the normal recording flow).
///
/// # 参数 / Parameters
/// - `session_dir`: 分片所在目录（可能在 tmp_dir 下）/ Segment directory (may be under tmp_dir)
/// - `output_dir`: 合并后视频的输出父目录（始终在 output_dir 下）/ Parent dir for merged video (always under output_dir)
///
/// # 返回值 / Returns
/// 合并后视频的时长（秒），失败时返回 `None`。
/// Duration of the merged video (seconds), or `None` on failure.
#[allow(dead_code)]
pub(crate) fn merge_segments(
    session_dir: &PathBuf,
    output_dir: &PathBuf,
    username: &str,
    merge_format: &str,
    emitter: &Arc<dyn Emitter>,
    session_dir_str: &str,
) -> Option<u64> {
    let m3u8_path = session_dir.join("playlist.m3u8");
    if !m3u8_path.exists() {
        tracing::warn!(
            "playlist.m3u8 not found in {:?}, skipping merge",
            session_dir
        );
        return None;
    }

    // 在合并前写入 #EXT-X-ENDLIST 标记，使 M3U8 成为完整的 VOD 播放列表
    // Write #EXT-X-ENDLIST before merging to finalize the M3U8 as a complete VOD playlist
    if let Err(e) = fs::OpenOptions::new()
        .append(true)
        .open(&m3u8_path)
        .and_then(|mut f| f.write_all(b"#EXT-X-ENDLIST\n"))
    {
        tracing::warn!("Failed to write #EXT-X-ENDLIST: {}", e);
    }

    let stem = session_dir.file_name().and_then(|n| n.to_str())?;
    let output_path = output_dir.join(format!("{}.{}", stem, merge_format));

    tracing::info!("Merging {} → {:?}", username, output_path);

    let total_bytes: u64 = fs::read_dir(session_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) == Some("ts") {
                        fs::metadata(&p).ok().map(|m| m.len())
                    } else {
                        None
                    }
                })
                .sum()
        })
        .unwrap_or(0);

    let _permit = tokio::runtime::Handle::current()
        .block_on(FFMPEG_SEMAPHORE.acquire())
        .expect("ffmpeg semaphore closed");

    let mut child = match Command::new("ffmpeg")
        .args(["-y", "-allowed_extensions", "ALL", "-i"])
        .arg(&m3u8_path)
        .args(["-c", "copy"])
        .arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to spawn ffmpeg → merge: {}", e);
            return None;
        }
    };

    let poll_interval = std::time::Duration::from_millis(500);
    loop {
        std::thread::sleep(poll_interval);
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                let out_bytes = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
                emitter.emit(
                    "merge-progress",
                    &serde_json::json!({
                        "session_dir": session_dir_str,
                        "video_path": output_path.to_string_lossy(),
                        "out_bytes": out_bytes,
                        "total_bytes": total_bytes,
                    }),
                );
            }
            Err(e) => {
                tracing::error!("ffmpeg wait error: {}", e);
                break;
            }
        }
    }

    let status = child.wait();
    match status {
        Ok(s) if s.success() => {
            tracing::info!("Merge complete: {:?}", output_path);
            emitter.emit(
                "merge-progress",
                &serde_json::json!({
                    "session_dir": session_dir_str,
                    "video_path": output_path.to_string_lossy(),
                    "out_bytes": total_bytes,
                    "total_bytes": total_bytes,
                }),
            );
            if let Err(e) = fs::remove_dir_all(session_dir) {
                tracing::error!("Failed to remove segment dir: {}", e);
            }
            let duration = get_video_duration(&output_path);
            let resolution = get_video_resolution(&output_path);

            // 更新 meta：填入实际大小、时长、分辨率，status 暂设为 "merging"（调用方会进一步更新）
            // Update meta: fill in actual size, duration, and resolution; status temporarily "merging"
            // (caller will update it further)
            let size_bytes = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
            if let Some(mut meta) = crate::recording::meta::read_meta(&output_path) {
                meta.size_bytes = size_bytes;
                meta.video_duration_secs = duration;
                meta.video_resolution = resolution;
                // 保留 status 不变，由调用方根据是否有后处理流水线决定下一个状态
                // Keep status unchanged; caller decides next status based on pipeline
                crate::recording::meta::write_meta(&output_path, &meta);
            }

            duration
        }
        Ok(s) => {
            tracing::warn!("ffmpeg merge exited with {}", s);
            None
        }
        Err(e) => {
            tracing::error!("Failed to spawn ffmpeg → merge: {}", e);
            None
        }
    }
}

/// 使用 ffprobe 获取视频文件的时长（秒）。
/// Get the duration of a video file in seconds using ffprobe.
pub fn get_video_duration(path: &std::path::Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<f64>().ok().map(|d| d as u64)
}

/// 使用 ffprobe 获取视频文件的分辨率（如 "1920x1080"）。
/// Get the resolution of a video file (e.g. "1920x1080") using ffprobe.
pub fn get_video_resolution(path: &std::path::Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=s=x:p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    let trimmed = s.trim();
    // 格式为 "WxH"，过滤无效值（含 0 的结果） / Format is "WxH"; filter out invalid results (containing 0)
    if trimmed.is_empty() || trimmed == "x" || trimmed.starts_with('x') || trimmed.ends_with('x') {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('x').collect();
    if parts.len() == 2 && parts.iter().all(|p| p.parse::<u32>().map(|v| v > 0).unwrap_or(false)) {
        Some(trimmed.to_string())
    } else {
        None
    }
}
