//! Contact Sheet 后处理模块 / Contact Sheet Post-processing Module
//!
//! 每隔指定秒数从视频中截取一帧，为每帧叠加时间戳水印，
//! 然后将所有帧拼合成一张网格预览图（contact sheet）保存到视频同目录。
//!
//! Extracts frames from the video at specified intervals, overlays timestamp watermarks,
//! then tiles all frames into a grid preview image (contact sheet) saved in the video's directory.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - 环境变量 `PP_INPUT`: 输入视频文件路径 / Input video file path via env var
//! - 标准输出 `OUTPUT:{path}`: 输出视频路径（contact sheet 与视频同名）/ Output video path
//! - 标准输出 `SKIP:{reason}`: 跳过原因（contact sheet 已存在）/ Skip reason (contact sheet already exists)

use pp_utils::{emit_progress, param, param_u32, video_duration};
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
  "id": "contact_sheet",
  "name": "Contact Sheet 0.1.0",
  "description": "每隔指定秒数截帧，拼合成一张带时间戳的预览图保存到视频同目录",
  "params": [
    {
      "key": "interval",
      "label": "截帧间隔（秒）",
      "type": "number",
      "default": 30
    },
    {
      "key": "thumb_width",
      "label": "单帧宽度（px）",
      "type": "number",
      "default": 320
    },
    {
      "key": "format",
      "label": "图片格式",
      "type": "select",
      "default": "webp",
      "options": ["webp", "jpg", "png"]
    },
    {
      "key": "quality",
      "label": "图片质量（1-100，jpg/webp 有效）",
      "type": "number",
      "default": 100
    },
    {
      "key": "cols",
      "label": "列数（0=自动）",
      "type": "number",
      "default": 0
    },
    {
      "key": "rows",
      "label": "行数（0=自动）",
      "type": "number",
      "default": 0
    },
    {
      "key": "fontfile",
      "label": "字体文件路径（留空自动检测）",
      "type": "string",
      "default": ""
    },
    {
      "key": "fontsize",
      "label": "时间戳字号",
      "type": "number",
      "default": 18
    }
  ]
}"#;

/// 根据帧数计算最优列数（使网格接近正方形）。
/// 若用户指定了列数则直接使用。
///
/// Calculate the optimal number of columns for the grid (to make it roughly square).
/// Uses the user-specified value if provided.
///
/// # 参数 / Parameters
/// - `frame_count`: 总帧数 / Total frame count
/// - `forced_cols`: 用户指定的列数（0 = 自动）/ User-specified columns (0 = auto)
fn compute_cols(frame_count: u32, forced_cols: u32) -> u32 {
    if forced_cols > 0 {
        return forced_cols;
    }
    // 使用 sqrt * 1.33 使网格略宽于高 / Use sqrt * 1.33 to make grid slightly wider than tall
    (((frame_count as f64).sqrt() * 1.33).ceil() as u32).max(1)
}

/// 在常见系统路径中查找可用的字体文件。
/// 支持 Windows、macOS 和 Linux。
///
/// Find an available font file in common system paths.
/// Supports Windows, macOS, and Linux.
///
/// # 返回值 / Returns
/// ffmpeg drawtext 过滤器可用的字体路径字符串，未找到返回 `None`。
/// Font path string usable by ffmpeg drawtext filter, or `None` if not found.
fn find_font() -> Option<String> {
    let candidates: &[&str] = &[
        // Windows 字体 / Windows fonts
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\consola.ttf",
        // macOS 字体 / macOS fonts
        "/System/Library/Fonts/Helvetica.ttc",
        "/Library/Fonts/Arial.ttf",
        // Linux 字体 / Linux fonts
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    candidates
        .iter()
        .find(|p| Path::new(p).exists())
        .map(|p| ffmpeg_escape_path(p))
}

/// 将文件路径转换为 ffmpeg drawtext 过滤器可接受的格式。
/// 在 Windows 上需要将反斜杠转为正斜杠，并对驱动器号后的冒号进行转义。
///
/// Convert a file path to a format accepted by ffmpeg's drawtext filter.
/// On Windows, backslashes are converted to forward slashes and the colon after
/// the drive letter is escaped.
fn ffmpeg_escape_path(path: &str) -> String {
    let fwd = path.replace('\\', "/");
    // Windows 驱动器路径（如 C:/...）需要转义冒号为 \:
    // Windows drive paths (e.g. C:/...) need the colon escaped as \:
    if fwd.len() >= 2 && fwd.as_bytes()[1] == b':' {
        format!("{}\\:{}", &fwd[..1], &fwd[2..])
    } else {
        fwd
    }
}

/// 模块主逻辑：截帧 -> 叠加时间戳 -> 拼合网格图。
/// Main module logic: extract frames -> overlay timestamps -> tile into grid.
fn run() -> Result<(), String> {
    // 读取输入文件路径 / Read input file path
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    // 读取模块参数 / Read module parameters
    let interval = param_u32("interval", 30).max(1);
    let thumb_width = param_u32("thumb_width", 320).max(16);
    let forced_cols = param_u32("cols", 0);
    let forced_rows = param_u32("rows", 0);
    let fontsize = param_u32("fontsize", 18).max(8);
    let tile_pad = 4u32;
    let quality = param_u32("quality", 100).clamp(1, 100);
    let format = param("format", "webp");
    let fontfile_param = param("fontfile", "");

    // 确定字体文件路径（优先使用用户指定的）/ Determine font file path (user-specified takes priority)
    let fontfile = if !fontfile_param.is_empty() {
        Some(ffmpeg_escape_path(&fontfile_param))
    } else {
        find_font()
    };

    if fontfile.is_none() {
        eprintln!("Warning: no font file found, timestamp overlay will be skipped");
    }

    // 输出文件路径：与视频同目录，同名，扩展名为图片格式
    // Output path: same directory as video, same name, image format extension
    let output_path = input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "{}.{}",
            input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("contact_sheet"),
            format
        ));

    // 若 contact sheet 已存在则跳过 / Skip if contact sheet already exists
    if output_path.exists() {
        println!(
            "SKIP: contact sheet already exists: {}",
            output_path.display()
        );
        println!("OUTPUT:{}", input.display());
        return Ok(());
    }

    // 获取视频时长以计算帧数 / Get video duration to calculate frame count
    let duration = video_duration(&input)
        .ok_or_else(|| "无法获取视频时长，请确认 ffprobe 已安装".to_string())?;

    let frame_count = ((duration / interval as f64).floor() as u32).max(1);
    let cols = compute_cols(frame_count, forced_cols);
    let rows = if forced_rows > 0 {
        forced_rows
    } else {
        // 向上取整确保所有帧都能放入网格 / Ceiling division to fit all frames in the grid
        (frame_count + cols - 1) / cols
    };

    // 创建临时目录存放截取的帧 / Create temp directory for extracted frames
    let tmp_dir = std::env::temp_dir().join(format!(
        "contact_sheet_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;
    // 定义清理函数，确保临时目录在任何情况下都被删除
    // Define cleanup closure to ensure temp dir is always removed
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    };

    emit_progress(0, frame_count);

    // 构建时间戳水印过滤器（若有字体文件）/ Build timestamp watermark filter (if font available)
    let drawtext_filter = if let Some(ref font) = fontfile {
        format!(
            ",drawtext=fontfile='{font}'\
             :text='%{{pts\\:hms}}'\
             :x=w-tw-8:y=h-th-8:fontsize={fs}:fontcolor=white\
             :box=1:boxcolor=black@0.6:boxborderw=3",
            font = font,
            fs = fontsize,
        )
    } else {
        String::new()
    };

    // 构建 ffmpeg 视频过滤器：降帧率 + 缩放 + 时间戳水印
    // Build ffmpeg video filter: fps reduction + scale + timestamp overlay
    let vf = format!(
        "fps=1/{interval},scale={w}:-1{dt}",
        interval = interval,
        w = thumb_width,
        dt = drawtext_filter
    );
    let frame_pattern = tmp_dir.join("frame_%06d.png");

    // 第一步：使用 ffmpeg 截取帧并实时上报进度
    // Step 1: Extract frames with ffmpeg and report progress in real-time
    let mut child = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(&input)
        .args(["-vf", &vf])
        .args(["-frames:v", &frame_count.to_string()])
        .arg(&frame_pattern)
        .args(["-progress", "pipe:1"])
        .args(["-loglevel", "error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            cleanup();
            format!("Failed to spawn ffmpeg (extract): {}", e)
        })?;

    {
        // 从 ffmpeg 的 -progress 输出中解析帧数并上报进度
        // Parse frame count from ffmpeg's -progress output and report progress
        use std::io::{BufRead, BufReader};
        let stdout = child.stdout.take().expect("stdout piped");
        let reader = BufReader::new(stdout);
        let mut last_reported = 0u32;
        for line in reader.lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("frame=") {
                if let Ok(n) = val.trim().parse::<u32>() {
                    let clamped = n.min(frame_count);
                    if clamped != last_reported {
                        emit_progress(clamped, frame_count);
                        last_reported = clamped;
                    }
                }
            }
        }
    }

    let status = child.wait().map_err(|e| {
        cleanup();
        format!("ffmpeg extract wait failed: {}", e)
    })?;

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
        cleanup();
        return Err(format!("ffmpeg extract failed:\n{}", stderr_msg.trim()));
    }

    // 验证实际截取到的帧数 / Verify actual number of extracted frames
    let extracted = (1..=frame_count)
        .filter(|i| tmp_dir.join(format!("frame_{:06}.png", i)).exists())
        .count() as u32;

    if extracted == 0 {
        cleanup();
        return Err(
            "No frames extracted — check the video file and ffmpeg installation".to_string(),
        );
    }

    emit_progress(frame_count, frame_count);

    // 生成 ffmpeg concat 文件列表 / Generate ffmpeg concat file list
    let filelist_path = tmp_dir.join("frames.txt");
    let mut list = String::new();
    for i in 1..=frame_count {
        let p = tmp_dir.join(format!("frame_{:06}.png", i));
        if p.exists() {
            list.push_str(&format!(
                "file '{}'\n",
                p.to_string_lossy().replace('\\', "/")
            ));
        }
    }
    std::fs::write(&filelist_path, &list).map_err(|e| {
        cleanup();
        format!("Failed to write filelist: {}", e)
    })?;

    // 第二步：使用 ffmpeg tile 过滤器将帧拼合为网格图
    // Step 2: Use ffmpeg tile filter to combine frames into a grid image
    let tile_filter = format!("tile={}x{}:padding={}", cols, rows, tile_pad);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&filelist_path)
        .args(["-vf", &tile_filter, "-frames:v", "1"]);

    // 根据输出格式设置质量参数 / Set quality parameters based on output format
    match format.as_str() {
        "jpg" => {
            cmd.args(["-q:v", "3"]);
        }
        "webp" => {
            cmd.args(["-quality", &quality.to_string()]);
        }
        _ => {}
    }

    cmd.arg(&output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let out = cmd.output().map_err(|e| {
        cleanup();
        format!("Failed to spawn ffmpeg (tile): {}", e)
    })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        cleanup();
        return Err(format!("ffmpeg tile failed:\n{}", stderr.trim()));
    }

    // 清理临时帧文件 / Clean up temporary frame files
    cleanup();
    // 输出视频路径（contact sheet 路径由前端根据视频路径推断）
    // Output video path (contact sheet path is inferred by frontend from video path)
    println!("OUTPUT:{}", input.display());
    Ok(())
}

/// 程序入口：处理 `--describe` 参数或执行主逻辑。
/// Entry point: handle `--describe` argument or execute main logic.
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", DESCRIBE);
        return;
    }
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
