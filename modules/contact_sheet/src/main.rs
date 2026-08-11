//! Contact Sheet 后处理模块 / Contact Sheet Post-processing Module
//!
//! 每隔指定秒数从视频中截取一帧，为每帧叠加时间戳水印，
//! 然后将所有帧拼合成一张网格预览图（contact sheet）保存到视频同目录。
//!
//! Extracts frames from the video at specified intervals, overlays timestamp watermarks,
//! then tiles all frames into a grid preview image (contact sheet) saved in the video's directory.
//!
//! # 子模块 / Sub-modules
//! - `font`    — 字体查找与 ffmpeg drawtext 路径转义 / Font discovery and drawtext path escaping
//! - `grid`    — 网格行列数计算 / Grid layout calculation
//! - `extract` — ffmpeg 截帧、帧数统计、拼图 / Frame extraction, counting, tiling
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - stdin: JSON 输入（inputs[0] 为 video_file）/ JSON input (inputs[0] is video_file)
//! - stdout: 进度行 + 最终 JSON 结果 / Progress lines + final JSON result

mod extract;
mod font;
mod grid;

use pp_utils::{emit_progress, output_ok, video_duration, ModuleInput};
use std::path::Path;

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
    "id": "contact_sheet",
    "name": "Contact Sheet",
    "description": "每隔指定秒数截帧，拼合成一张带时间戳的预览图保存到视频同目录",
    "inputTypes": ["video_file"],
    "outputTypes": ["media_bundle"],
    "official": true,
    "params": [
        { "key": "interval",    "label": "截帧间隔（秒）",            "type": "number", "default": 30 },
        { "key": "thumb_width", "label": "单帧宽度（px）",            "type": "number", "default": 320 },
        { "key": "format",      "label": "图片格式",                  "type": "select", "default": "webp", "options": ["webp","jpg","png"] },
        { "key": "quality",     "label": "图片质量（1-100）",         "type": "number", "default": 100 },
        { "key": "cols",        "label": "列数（0=自动）",            "type": "number", "default": 0 },
        { "key": "rows",        "label": "行数（0=自动）",            "type": "number", "default": 0 },
        { "key": "fontfile",    "label": "字体文件路径（留空自动）",  "type": "string", "default": "" },
        { "key": "fontsize",    "label": "时间戳字号",                "type": "number", "default": 18 }
    ]
}"#;

/// 模块主逻辑：截帧 → 叠加时间戳 → 拼合网格图。
/// Main module logic: extract frames → overlay timestamps → tile into grid.
fn run() -> Result<(), String> {
    let input_json = ModuleInput::read();
    let input = input_json
        .first_input()
        .ok_or_else(|| "inputs[0] (video_file) is required".to_string())?;

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    let interval     = input_json.param_u32("interval", 30).max(1);
    let thumb_width  = input_json.param_u32("thumb_width", 320).max(16);
    let forced_cols  = input_json.param_u32("cols", 0);
    let forced_rows  = input_json.param_u32("rows", 0);
    let fontsize     = input_json.param_u32("fontsize", 18).max(8);
    let tile_pad     = 4u32;
    let quality      = input_json.param_u32("quality", 100).clamp(1, 100);
    let format       = input_json.param_str("format", "webp");
    let fontfile_param = input_json.param_str("fontfile", "");

    // 字体路径：优先用户指定，否则自动查找 / Font: prefer user-specified, else auto-detect
    let fontfile: Option<String> = if !fontfile_param.is_empty() {
        Some(font::escape_for_drawtext(&fontfile_param))
    } else {
        font::find_font()
    };

    if fontfile.is_none() {
        eprintln!("Warning: no font file found, timestamp overlay will be skipped");
    }

    // 输出文件路径（与视频同目录同名，扩展名为图片格式）
    // Output path: same directory and stem as the video, image format extension
    let output_path = input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            "{}.{}",
            input.file_stem().and_then(|s| s.to_str()).unwrap_or("contact_sheet"),
            format
        ));

    // 若 contact sheet 已存在则跳过 / Skip if contact sheet already exists
    if output_path.exists() {
        println!("SKIP: contact sheet already exists: {}", output_path.display());
        let bundle = format!("{}\n{}", input.to_string_lossy(), output_path.display());
        output_ok(
            &[&bundle],
            &format!("Skipped: contact sheet already exists ({})", output_path.display()),
        );
        return Ok(());
    }

    // 获取视频时长以计算预期帧数 / Get video duration to calculate expected frame count
    let duration = video_duration(&input)
        .ok_or_else(|| "无法获取视频时长，请确认 ffprobe 已安装".to_string())?;

    let frame_count = ((duration / interval as f64).floor() as u32).max(1);
    let cols = grid::compute_cols(frame_count, forced_cols);
    let rows = grid::compute_rows(frame_count, cols, forced_rows);

    // 创建临时目录存放截取的帧 / Create temp directory for extracted frames
    let tmp_dir = std::env::temp_dir().join(format!(
        "contact_sheet_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;

    // 定义清理函数，确保临时目录在任何情况下都被删除
    // Cleanup closure: always remove temp dir on exit
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&tmp_dir);
    };

    emit_progress(0, frame_count);

    // 第一步：截帧 / Step 1: Extract frames
    if let Err(e) = extract::extract_frames(
        &input,
        &tmp_dir,
        interval,
        thumb_width,
        fontfile.as_deref(),
        fontsize,
        frame_count,
        duration,
    ) {
        cleanup();
        return Err(e);
    }

    // 验证实际截取帧数 / Verify extracted frame count
    let extracted = extract::count_extracted_frames(&tmp_dir, frame_count);
    if extracted == 0 {
        cleanup();
        return Err("No frames extracted — check the video file and ffmpeg installation".to_string());
    }

    emit_progress(frame_count, frame_count);

    // 第二步：写帧列表并拼图 / Step 2: Write concat list and tile
    let filelist = match extract::write_concat_list(&tmp_dir, frame_count) {
        Ok(p) => p,
        Err(e) => {
            cleanup();
            return Err(e);
        }
    };

    if let Err(e) = extract::tile_frames(&filelist, &output_path, cols, rows, tile_pad, &format, quality) {
        cleanup();
        return Err(e);
    }

    // 清理临时帧文件 / Clean up temp frames
    cleanup();

    // 输出 media_bundle（视频路径 + 图片路径）/ Output media_bundle (video + image paths)
    let bundle = format!("{}\n{}", input.to_string_lossy(), output_path.display());
    output_ok(
        &[&bundle],
        &format!("Contact sheet saved: {}", output_path.display()),
    );
    Ok(())
}

/// 程序入口：处理 `--describe` 参数或执行主逻辑。
/// Entry point: handle `--describe` or run main logic.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
        return;
    }
    if let Err(e) = run() {
        let json = serde_json::json!({ "code": "error", "message": e, "outputs": [] });
        println!("{}", json);
        std::process::exit(1);
    }
}
