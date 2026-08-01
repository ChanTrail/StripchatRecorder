//! ffmpeg 帧提取 / ffmpeg Frame Extraction

use pp_utils::emit_progress;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 使用 ffmpeg 从视频中按时间间隔截取帧，并叠加时间戳水印（若提供字体文件）。
/// 截取的帧以 `frame_000001.png` … 格式保存到 `out_dir`。
/// 通过 `out_time_us` 行实时上报进度（总帧数 `frame_total` 作为比例基数）。
///
/// Extract frames from a video at regular intervals using ffmpeg, with an optional
/// timestamp drawtext overlay. Frames are saved as `frame_000001.png` … in `out_dir`.
/// Progress is reported in real-time using `out_time_us` lines from ffmpeg -progress.
///
/// # 参数 / Parameters
/// - `input`: 视频文件路径 / video file path
/// - `out_dir`: 帧输出目录 / frame output directory
/// - `interval`: 截帧间隔（秒）/ frame interval in seconds
/// - `thumb_width`: 单帧宽度（px）/ thumbnail width in pixels
/// - `fontfile`: drawtext 字体文件路径（已转义），`None` 则不叠加水印 / drawtext font path (escaped), `None` = no overlay
/// - `fontsize`: 时间戳字号 / timestamp font size
/// - `frame_total`: 期望截取的总帧数（用于进度上报）/ expected total frames (for progress reporting)
/// - `duration_secs`: 视频时长（秒，用于进度估算）/ video duration in seconds (for progress estimation)
pub fn extract_frames(
    input: &Path,
    out_dir: &Path,
    interval: u32,
    thumb_width: u32,
    fontfile: Option<&str>,
    fontsize: u32,
    frame_total: u32,
    duration_secs: f64,
) -> Result<(), String> {
    let drawtext = match fontfile {
        Some(font) => format!(
            ",drawtext=fontfile='{font}'\
             :text='%{{pts\\:hms}}'\
             :x=w-tw-8:y=h-th-8:fontsize={fs}:fontcolor=white\
             :box=1:boxcolor=black@0.6:boxborderw=3",
            font = font,
            fs = fontsize,
        ),
        None => String::new(),
    };

    // select 过滤器：选取第一帧，然后每隔 interval 秒选一帧；pts 保持不变供 drawtext 使用
    // select filter: first frame + one frame every interval seconds; pts preserved for drawtext
    let vf = format!(
        "select='isnan(prev_selected_t)+gte(t-prev_selected_t\\,{interval})',scale={w}:-1{dt}",
        interval = interval,
        w = thumb_width,
        dt = drawtext,
    );
    let frame_pattern = out_dir.join("frame_%06d.png");

    let mut child = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args(["-vf", &vf])
        .args(["-frames:v", &frame_total.to_string()])
        .arg(&frame_pattern)
        .args(["-progress", "pipe:1"])
        .args(["-loglevel", "error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg (extract): {}", e))?;

    {
        use std::io::{BufRead, BufReader};
        let stdout = child.stdout.take().expect("stdout piped");
        let reader = BufReader::new(stdout);
        let total_us = (duration_secs * 1_000_000.0) as u64;
        let mut last_reported = 0u32;
        for line in reader.lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("out_time_us=")
                && let Ok(us) = val.trim().parse::<u64>()
            {
                let progress = if total_us > 0 {
                    ((us as f64 / total_us as f64) * frame_total as f64) as u32
                } else {
                    0
                };
                let clamped = progress.min(frame_total);
                if clamped != last_reported {
                    emit_progress(clamped, frame_total);
                    last_reported = clamped;
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("ffmpeg extract wait failed: {}", e))?;

    if !status.success() {
        let stderr_msg = child
            .stderr
            .take()
            .and_then(|mut s| {
                use std::io::Read;
                let mut buf = String::new();
                s.read_to_string(&mut buf).ok()?;
                Some(buf)
            })
            .unwrap_or_default();
        return Err(format!("ffmpeg extract failed:\n{}", stderr_msg.trim()));
    }

    Ok(())
}

/// 验证并统计实际截取到的帧文件数量（`frame_000001.png` … `frame_{n}.png`）。
///
/// Count how many frame files were actually created (frame_000001.png … frame_N.png).
pub fn count_extracted_frames(out_dir: &Path, expected: u32) -> u32 {
    (1..=expected)
        .filter(|i| out_dir.join(format!("frame_{:06}.png", i)).exists())
        .count() as u32
}

/// 将帧文件列表写入 ffmpeg concat 格式的文本文件。
///
/// Write frame file list to an ffmpeg concat-format text file.
pub fn write_concat_list(out_dir: &Path, frame_count: u32) -> Result<PathBuf, String> {
    let filelist = out_dir.join("frames.txt");
    let mut list = String::new();
    for i in 1..=frame_count {
        let p = out_dir.join(format!("frame_{:06}.png", i));
        if p.exists() {
            list.push_str(&format!(
                "file '{}'\n",
                p.to_string_lossy().replace('\\', "/")
            ));
        }
    }
    std::fs::write(&filelist, &list)
        .map_err(|e| format!("Failed to write concat list: {}", e))?;
    Ok(filelist)
}

/// 使用 ffmpeg tile 过滤器将帧拼合为网格图像。
///
/// Tile extracted frames into a grid image using the ffmpeg tile filter.
///
/// - `filelist`: concat 文件路径 / path to concat file list
/// - `output`: 输出图像路径 / output image path
/// - `cols` / `rows`: 网格列/行数 / grid columns / rows
/// - `tile_pad`: 帧间距（px）/ padding between frames in pixels
/// - `format`: 输出格式（"webp" / "jpg" / "png"）/ output format
/// - `quality`: 图像质量（仅 webp/jpg 有效）/ image quality (webp/jpg only)
pub fn tile_frames(
    filelist: &Path,
    output: &Path,
    cols: u32,
    rows: u32,
    tile_pad: u32,
    format: &str,
    quality: u32,
) -> Result<(), String> {
    let tile_filter = format!("tile={}x{}:padding={}", cols, rows, tile_pad);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(filelist)
        .args(["-vf", &tile_filter, "-frames:v", "1"]);

    match format {
        "jpg" => {
            cmd.args(["-q:v", "3"]);
        }
        "webp" => {
            cmd.args(["-quality", &quality.to_string()]);
        }
        _ => {}
    }

    cmd.arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let out = cmd
        .output()
        .map_err(|e| format!("Failed to spawn ffmpeg (tile): {}", e))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("ffmpeg tile failed:\n{}", stderr.trim()));
    }

    Ok(())
}
