//! 过滤短视频后处理模块 / Filter Short Videos Post-processing Module
//!
//! 检查输入视频的时长，若低于指定阈值则将其删除。
//! 支持 dry_run 模式（仅预览，不实际删除）。
//!
//! Checks the duration of the input video and deletes it if below the specified threshold.
//! Supports dry_run mode (preview only, no actual deletion).
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - 环境变量 `PP_INPUT`: 输入视频文件路径 / Input video file path via env var
//! - 环境变量 `PP_PARAM_*`: 模块参数 / Module parameters via env vars
//! - 标准输出 `OUTPUT:{path}`: 视频通过过滤时输出路径 / Output path when video passes filter
//! - 标准输出 `DELETE_INPUT`: 请求主程序删除输入文件（视频时长低于阈值时）/ Request host to delete input file (when duration is below threshold)
//! - 标准输出 `PROGRESS:{done}/{total}`: 进度上报 / Progress reporting

use pp_utils::{param_bool, param_f64, video_duration, PROGRESS_SCALE};
use std::env;
use std::path::PathBuf;

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
    "id": "filter_short",
    "name": "过滤短视频 0.2.0",
    "description": "删除时长低于指定阈值的视频文件",
    "params": [
        {
        "key": "min_duration",
        "label": "最短时长（秒）",
        "type": "number",
        "default": 60
        },
        {
        "key": "dry_run",
        "label": "仅预览，不实际删除",
        "type": "boolean",
        "default": false
        }
    ],
    "i18n": {
        "en-US": {
        "name": "Filter Short Videos 0.1.0",
        "description": "Delete video files shorter than the specified duration threshold",
        "params": {
            "min_duration": { "label": "Minimum duration (seconds)" },
            "dry_run": { "label": "Dry run (preview only, no deletion)" }
        }
        }
    }
}"#;

/// 模块主逻辑：读取参数、检查视频时长、决定是否删除。
/// Main module logic: read parameters, check video duration, decide whether to delete.
fn run() -> Result<(), String> {
    // 从环境变量读取输入文件路径 / Read input file path from environment variable
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    // 读取模块参数 / Read module parameters
    let min_duration = param_f64("min_duration", 60.0).max(0.0);
    let dry_run = param_bool("dry_run", false);

    // 上报初始进度 / Report initial progress
    println!("PROGRESS:0/{}", PROGRESS_SCALE);

    // 使用 ffprobe 获取视频时长 / Get video duration using ffprobe
    let duration = video_duration(&input)
        .ok_or_else(|| "无法获取视频时长，请确认 ffprobe 已安装".to_string())?;

    // 上报完成进度 / Report completion progress
    println!("PROGRESS:{}/{}", PROGRESS_SCALE, PROGRESS_SCALE);

    if duration < min_duration {
        // 视频时长低于阈值：请求主程序删除或预览 / Video duration below threshold: request host deletion or preview
        if dry_run {
            eprintln!(
                "DRY_RUN: would delete '{}' (duration {:.1}s < {:.1}s)",
                input.display(),
                duration,
                min_duration
            );
        } else {
            // 输出 DELETE_INPUT 协议行，由主程序负责删除文件
            // Output DELETE_INPUT protocol line; the host is responsible for deleting the file
            println!("DELETE_INPUT");
            eprintln!(
                "Requesting deletion of '{}' (duration {:.1}s < {:.1}s)",
                input.display(),
                duration,
                min_duration
            );
        }
        // 视频将被删除时不输出 OUTPUT，流水线后续模块将跳过
        // No OUTPUT when video will be deleted; subsequent pipeline modules will be skipped
    } else {
        // 视频时长满足要求，传递给下一个模块 / Video duration meets requirement, pass to next module
        println!("OUTPUT:{}", input.display());
    }

    Ok(())
}

/// 程序入口：处理 `--describe` 参数或执行主逻辑。
/// Entry point: handle `--describe` argument or execute main logic.
fn main() {
    let args: Vec<String> = env::args().collect();
    // 输出模块描述 JSON 并退出 / Output module description JSON and exit
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", DESCRIBE);
        return;
    }
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
