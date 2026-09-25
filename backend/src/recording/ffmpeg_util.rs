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
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

/// 当前 FFmpeg 信号量许可数（由 `set_ffmpeg_concurrency` 维护，用于差值更新）。
/// Current FFmpeg semaphore permit count (maintained by `set_ffmpeg_concurrency` for delta updates).
static FFMPEG_SEMAPHORE_PERMITS: AtomicUsize = AtomicUsize::new(0);

/// 全局 FFmpeg 并发信号量。
/// 由 `set_ffmpeg_concurrency` 首次调用时（`AppState::new()` 阶段）初始化，
/// 此后只通过该函数的差值逻辑动态调整，不设任何硬编码初始值。
///
/// Global FFmpeg concurrency semaphore.
/// Initialized on the first call to `set_ffmpeg_concurrency` (during `AppState::new()`),
/// then adjusted dynamically via delta logic in that function — no hardcoded initial value.
static FFMPEG_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// 获取 FFmpeg 信号量引用，若尚未初始化则 panic（调用方保证在 `AppState::new()` 之后使用）。
/// Get a reference to the FFmpeg semaphore, panicking if not yet initialized
/// (callers guarantee use only after `AppState::new()`).
fn ffmpeg_semaphore() -> &'static Semaphore {
    FFMPEG_SEMAPHORE.get().expect("ffmpeg semaphore not initialized — call set_ffmpeg_concurrency first")
}

/// 动态更新 FFmpeg 并发许可数，使其与后处理并发数（`max_pp_concurrent`）保持一致。
///
/// **初始化**：首次调用（`AppState::new()` 阶段）会创建信号量，许可数由
/// `resolve_concurrency(n)` 决定（`n=0` 时为自动，即 CPU × 2）。
///
/// **后续更新**：用差值法调整现有信号量：
/// - 新 > 旧：`add_permits(diff)` 增加许可
/// - 新 < 旧：`try_acquire_many(diff).forget()` 减少可用许可
///   （若当前全部被占用则延迟消耗，持有者 drop 时自然归还并被消耗）
///
/// Dynamically update the FFmpeg concurrency permit count to match `max_pp_concurrent`.
///
/// **Initialization**: the first call (during `AppState::new()`) creates the semaphore
/// with a permit count determined by `resolve_concurrency(n)` (auto = CPU × 2 when n=0).
///
/// **Subsequent updates**: adjusts the existing semaphore with a delta approach:
/// - new > old: `add_permits(diff)`
/// - new < old: `try_acquire_many(diff).forget()` — deferred if all permits are held
pub fn set_ffmpeg_concurrency(n: usize) {
    let new_permits = crate::postprocess::queue::resolve_concurrency(n);

    // 首次调用：初始化信号量，不做差值运算
    // First call: initialize the semaphore, no delta needed
    if FFMPEG_SEMAPHORE.get().is_none() {
        let _ = FFMPEG_SEMAPHORE.set(Semaphore::new(new_permits));
        FFMPEG_SEMAPHORE_PERMITS.store(new_permits, Ordering::Relaxed);
        tracing::debug!("ffmpeg_concurrency initialized: permits={}", new_permits);
        return;
    }

    let old_permits = FFMPEG_SEMAPHORE_PERMITS.swap(new_permits, Ordering::Relaxed);

    match new_permits.cmp(&old_permits) {
        std::cmp::Ordering::Greater => {
            let diff = new_permits - old_permits;
            ffmpeg_semaphore().add_permits(diff);
        }
        std::cmp::Ordering::Less => {
            let diff = old_permits - new_permits;
            // try_acquire_many 在许可不足时直接失败（不阻塞），forget 永久移除这些许可。
            // 若许可全被占用，持有者 drop 后归还的许可会超出 new_permits，
            // 下一次 acquire 完成后自然消耗到目标数。
            //
            // try_acquire_many fails fast if permits are insufficient; forget removes
            // them permanently. If all permits are held, the excess returned on drop
            // will be consumed naturally after the next acquire completes.
            if let Ok(permit) = ffmpeg_semaphore().try_acquire_many(diff as u32) {
                permit.forget();
            }
        }
        std::cmp::Ordering::Equal => {}
    }

    tracing::debug!(
        "ffmpeg_concurrency updated: old={}, new={}",
        old_permits,
        new_permits
    );
}

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
///
/// 录制分片转码是实时路径，不参与后处理 FFmpeg 并发信号量的排队——信号量只约束
/// 后处理模块（`pipeline/exec.rs`）启动的 FFmpeg 进程，保证录制完整性优先。
///
/// Convert fMP4 data to MPEG-TS format using ffmpeg (piped via stdin).
///
/// Segment transcoding is on the real-time recording path and does NOT go through
/// the post-processing FFmpeg concurrency semaphore — the semaphore only throttles
/// FFmpeg processes spawned by post-processing modules (`pipeline/exec.rs`),
/// ensuring recording integrity takes priority.
pub(crate) async fn convert_to_ts(fmp4_data: Vec<u8>, ts_path: &PathBuf) -> Result<()> {
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
            tracing::error!("{}", crate::tl!("ffmpegUtil.openPlaylistFailed", error = e));
            return;
        }
    };

    if needs_header
        && let Err(e) = file.write_all(b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-MEDIA-SEQUENCE:0\n")
    {
        tracing::error!("{}", crate::tl!("ffmpegUtil.writeM3u8HeaderFailed", error = e));
        return;
    }

    // 写入分片条目（时长占位为 0，实际时长未知）/ Write segment entry (duration placeholder 0, actual duration unknown)
    let line = format!("#EXTINF:0,\n{}\n", filename);
    if let Err(e) = file.write_all(line.as_bytes()) {
        tracing::error!("{}", crate::tl!("ffmpegUtil.updateM3u8Failed", error = e));
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
