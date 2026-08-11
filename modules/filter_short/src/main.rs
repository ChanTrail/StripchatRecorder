//! 过滤短视频后处理模块 / Filter Short Videos Post-processing Module
//!
//! 检查输入视频的时长，若低于指定阈值则将其删除（流水线在此终止）；
//! 否则将视频原样传递给下游节点。
//!
//! Checks the duration of the input video; if below the threshold, deletes it
//! (pipeline terminates here). Otherwise passes the video to the next node.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - stdin: JSON 输入（inputs[0] 为 video_file）/ JSON input (inputs[0] is video_file)
//! - stdout: 进度行 + 最终 JSON 结果 / Progress lines + final JSON result

use pp_utils::{video_duration, ModuleInput, output_done, output_ok, PROGRESS_SCALE};

const DESCRIBE: &str = r#"{
    "id": "filter_short",
    "name": "过滤短视频",
    "description": "删除时长低于指定阈值的视频文件，使流水线在此终止；否则原样传递给下一节点",
    "inputTypes": ["video_file"],
    "outputTypes": ["video_file"],
    "official": true,
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
    ]
}"#;

fn run() -> Result<(), String> {
    let input = ModuleInput::read();
    let path = input.first_input()
        .ok_or_else(|| "inputs[0] (video_file) is required".to_string())?;

    if !path.exists() {
        return Err(format!("Input file not found: {}", path.display()));
    }

    let min_duration = input.param_f64("min_duration", 60.0).max(0.0);
    let dry_run = input.param_bool("dry_run", false);

    println!("PROGRESS:0/{}", PROGRESS_SCALE);

    let duration = video_duration(&path)
        .ok_or_else(|| "无法获取视频时长，请确认 ffprobe 已安装".to_string())?;

    println!("PROGRESS:{}/{}", PROGRESS_SCALE, PROGRESS_SCALE);

    if duration < min_duration {
        if dry_run {
            eprintln!(
                "DRY_RUN: would delete '{}' (duration {:.1}s < {:.1}s)",
                path.display(), duration, min_duration
            );
            // dry_run 时终止流水线但不删文件（与真实删除行为一致，只是跳过删除操作）
            // Terminate the pipeline without deleting the file in dry_run mode
            output_done(&format!(
                "DRY_RUN: duration {:.1}s < {:.1}s, would delete",
                duration, min_duration
            ));
        } else {
            // 删除文件并终止流水线 / Delete file and terminate pipeline
            if let Err(e) = std::fs::remove_file(&path) {
                return Err(format!("Failed to delete '{}': {}", path.display(), e));
            }
            eprintln!(
                "Deleted '{}' (duration {:.1}s < {:.1}s)",
                path.display(), duration, min_duration
            );
            output_done(&format!("Deleted: duration {:.1}s < {:.1}s", duration, min_duration));
        }
    } else {
        // 时长满足要求，传递给下一节点 / Duration meets requirement, pass to next node
        output_ok(
            &[&path.to_string_lossy()],
            &format!("Passed: duration {:.1}s >= {:.1}s", duration, min_duration),
        );
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
        return;
    }
    if let Err(e) = run() {
        let json = serde_json::json!({
            "code": "error",
            "message": e,
            "outputs": []
        });
        println!("{}", json);
        std::process::exit(1);
    }
}
