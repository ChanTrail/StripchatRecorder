//! TS 分片合并模块 / TS Segment Merge Module
//!
//! 接收录制产生的 TS 分片会话目录，使用 ffmpeg 将其合并为单一视频文件。
//! 这是官方后处理流水线的首节点，其他官方模块（如 filter_short、contact_sheet）
//! 应置于本模块之后。
//!
//! Receives the TS segment session directory from a recording and merges all segments
//! into a single video file using ffmpeg.
//! This is the first node in the official post-processing pipeline; other official modules
//! (e.g. filter_short, contact_sheet) should be placed after this one.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - stdin: JSON 输入（inputs[0] 为 ts_session_dir）/ JSON input (inputs[0] is ts_session_dir)
//! - stdout 进度行: `PROGRESS:{done}/{total}` / Progress lines
//! - stdout 最终 JSON: `{"code":"ok","message":"...","outputs":["/path/to/merged.mp4"]}`

use pp_utils::PROGRESS_SCALE;
use serde::Deserialize;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
    "id": "ts_merge",
    "name": "合并 TS 分片",
    "description": "将录制产生的 TS 分片目录合并为单一视频文件。官方后处理模块应置于本模块之后。",
    "inputTypes": ["ts_session_dir"],
    "outputTypes": ["video_file"],
    "official": true,
    "params": [
        {
            "key": "format",
            "label": "输出格式",
            "type": "select",
            "default": "mp4",
            "options": ["mp4", "mkv", "ts"]
        },
        {
            "key": "output_dir",
            "label": "合并输出目录",
            "type": "dir",
            "default": "",
            "description": "合并后视频文件的输出目录。留空则与 TS 分片目录的父目录相同。"
        },
        {
            "key": "split_by_streamer",
            "label": "按主播分子目录",
            "type": "boolean",
            "default": true,
            "description": "开启后在输出目录下按主播用户名创建子目录（仅当设置了合并输出目录时有意义）。"
        }
    ]
}"#;

/// stdin 输入 JSON 结构（只解析需要的字段）/ stdin input JSON (only parses needed fields)
#[derive(Debug, Deserialize)]
struct Input {
    inputs: Vec<String>,
    #[serde(default)]
    params: serde_json::Value,
    /// 录制上下文（含主播用户名）/ Recording context (includes streamer username)
    #[serde(default)]
    recording: Option<serde_json::Value>,
}

/// 推导合并输出路径。
///
/// 输出目录优先级：
/// 1. `output_dir` 非空 → 使用该目录
/// 2. 否则 → session_dir 的父目录
///
/// 若 `split_by_streamer` 为 true 且能从 `recording.username` 中获取主播名，
/// 则在输出目录下再加一层 `{username}/` 子目录。
///
/// Derive the merged output path.
///
/// Output directory priority:
/// 1. `output_dir` non-empty → use it
/// 2. otherwise → parent of session_dir
///
/// If `split_by_streamer` is true and a username can be obtained from `recording.username`,
/// a `{username}/` subdirectory is appended to the output directory.
fn derive_output_path(
    session_dir: &std::path::Path,
    format: &str,
    output_dir: &str,
    split_by_streamer: bool,
    username: &str,
) -> Option<PathBuf> {
    let stem = session_dir.file_name()?.to_str()?;
    let base = if output_dir.is_empty() {
        session_dir.parent()?.to_path_buf()
    } else {
        PathBuf::from(output_dir)
    };
    let parent = if split_by_streamer && !username.is_empty() {
        base.join(username)
    } else {
        base
    };
    Some(parent.join(format!("{}.{}", stem, format)))
}

fn run() -> Result<serde_json::Value, String> {
    // 从 stdin 读取 JSON 输入 / Read JSON input from stdin
    let mut stdin_buf = String::new();
    io::stdin().lock().read_to_string(&mut stdin_buf).ok();

    let input: Input = serde_json::from_str(stdin_buf.trim())
        .map_err(|e| format!("Failed to parse stdin JSON: {}", e))?;

    let session_dir_str = input.inputs.first()
        .ok_or_else(|| "inputs[0] (ts_session_dir) is required".to_string())?;
    let session_dir = PathBuf::from(session_dir_str);

    if !session_dir.exists() {
        return Err(format!("Session directory not found: {}", session_dir.display()));
    }

    // 若输入已经是视频文件（重新后处理场景：session_dir 已被合并删除），直接透传给下游。
    // If the input is already a video file (re-processing: session_dir was deleted after a
    // previous merge), pass it through to downstream nodes without re-merging.
    if session_dir.is_file() {
        println!("PROGRESS:{}/{}", PROGRESS_SCALE, PROGRESS_SCALE);
        return Ok(serde_json::json!({
            "code": "ok",
            "message": format!("Input is already a merged video file, passing through: {}", session_dir.display()),
            "outputs": [session_dir.to_string_lossy()]
        }));
    }

    let format = input.params.get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("mp4")
        .to_string();

    let output_dir = input.params.get("output_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let split_by_streamer = input.params.get("split_by_streamer")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 从 recording context 中读取主播用户名（供 split_by_streamer 使用）
    // Read streamer username from recording context (for split_by_streamer)
    let username = input.recording.as_ref()
        .and_then(|r| r.get("username"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let output_path = derive_output_path(&session_dir, &format, &output_dir, split_by_streamer, &username)
        .ok_or_else(|| "Cannot determine output path from session directory".to_string())?;

    // 确保输出目录存在 / Ensure output parent directory exists
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create output dir: {}", e))?;
    }
    let m3u8_path = session_dir.join("playlist.m3u8");
    if !m3u8_path.exists() {
        return Err(format!(
            "playlist.m3u8 not found in session directory: {}",
            session_dir.display()
        ));
    }

    // 写入 #EXT-X-ENDLIST 使 M3U8 成为完整的 VOD 播放列表
    // Write #EXT-X-ENDLIST to finalize the M3U8 as a complete VOD playlist
    if let Ok(mut f) = fs::OpenOptions::new().append(true).open(&m3u8_path) {
        let _ = f.write_all(b"#EXT-X-ENDLIST\n");
    }

    // 计算分片总大小（用于进度估算）/ Calculate total segment size for progress estimation
    let total_bytes: u64 = fs::read_dir(&session_dir)
        .map(|entries| {
            entries.flatten()
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

    println!("PROGRESS:0/{}", PROGRESS_SCALE);

    // 启动 ffmpeg 合并 / Launch ffmpeg merge
    let mut child = Command::new("ffmpeg")
        .args(["-y", "-allowed_extensions", "ALL", "-protocol_whitelist",
               "file,crypto,data,http,https,tcp,tls", "-i"])
        .arg(&m3u8_path)
        .args(["-c", "copy"])
        .arg(&output_path)
        .args(["-progress", "pipe:1", "-loglevel", "error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {}", e))?;

    // 读取 ffmpeg -progress 输出行，按 total_size 估算进度
    // Read ffmpeg -progress output lines, estimate progress from total_size
    {
        let stdout = child.stdout.take().expect("stdout piped");
        let reader = BufReader::new(stdout);
        let mut last_scaled = 0u32;

        for line in reader.lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("total_size=")
                && let Ok(bytes) = val.trim().parse::<u64>()
                && total_bytes > 0
            {
                let scaled = ((bytes.min(total_bytes) as u128 * PROGRESS_SCALE as u128)
                    / total_bytes as u128) as u32;
                let scaled = scaled.min(PROGRESS_SCALE - 1); // 保留 100% 给完成时
                if scaled != last_scaled {
                    println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
                    last_scaled = scaled;
                }
            }
        }
    }

    let status = child.wait().map_err(|e| format!("ffmpeg wait failed: {}", e))?;

    if !status.success() {
        return Err(format!("ffmpeg exited with {}", status));
    }

    // 验证输出文件存在 / Verify output file exists
    if !output_path.exists() {
        return Err("ffmpeg did not produce output file".to_string());
    }

    // 删除 session 目录（分片已合并完毕）/ Remove session directory (segments merged)
    if let Err(e) = fs::remove_dir_all(&session_dir) {
        eprintln!("Warning: failed to remove session dir {:?}: {}", session_dir, e);
    }

    println!("PROGRESS:{}/{}", PROGRESS_SCALE, PROGRESS_SCALE);

    Ok(serde_json::json!({
        "code": "ok",
        "message": format!("Merged to {}", output_path.display()),
        "outputs": [output_path.to_string_lossy()]
    }))
}

fn main() {    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
        return;
    }
    match run() {
        Ok(json) => {
            println!("{}", json);
        }
        Err(e) => {
            let json = serde_json::json!({
                "code": "error",
                "message": e,
                "outputs": []
            });
            println!("{}", json);
            std::process::exit(1);
        }
    }
}
