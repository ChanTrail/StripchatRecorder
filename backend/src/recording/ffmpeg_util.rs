//! ffmpeg/ffprobe 底层工具函数 / Low-level ffmpeg/ffprobe Utility Functions
//!
//! 提供分片转码（fMP4→TS）、m3u8 维护、目录大小计算、视频时长/分辨率探测等
//! 纯 ffmpeg/ffprobe 操作，不涉及录制会话生命周期管理
//! （会话生命周期见 `recording::recorder`）。
//!
//! Provides low-level ffmpeg/ffprobe operations: segment transcoding (fMP4→TS),
//! m3u8 maintenance, directory size calculation, and video duration/resolution probing.
//! Does not manage recording session lifecycle (see `recording::recorder` for that).

use crate::core::error::{AppError, Result};
use std::fs;
use crate::core::no_window::NoWindowExt;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
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
        .no_window()
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
        .no_window()
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
        .no_window()
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
        .no_window()
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
