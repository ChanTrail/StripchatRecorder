//! Discord 通知后处理模块 / Discord Notification Post-processing Module
//!
//! 将录制信息（主播名、时间戳、时长、文件名、文件大小）和封面图
//! 通过 Discord Webhook 发送到指定频道。
//!
//! Sends recording information (model name, timestamp, duration, filename, file size)
//! and cover image to a Discord channel via Webhook.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - 环境变量 `PP_INPUT`: 输入视频文件路径 / Input video file path via env var
//! - 标准输出 `OUTPUT:{path}`: 成功后输出视频路径 / Output video path on success
//! - 标准输出 `PROGRESS:{done}/{total}`: 进度上报 / Progress reporting
//! - 标准输出 `STATUS:{speed}`: 上传速度上报 / Upload speed reporting

use pp_utils::{
    emit_progress_step, find_cover, format_bytes, format_duration, format_speed, param, parse_stem,
    video_duration, PROGRESS_SCALE,
};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
  "id": "notify_discord",
  "name": "Discord 通知",
  "description": "将录制信息和封面图发送到 Discord Webhook",
  "params": [
    {
      "key": "webhook_url",
      "label": "Webhook URL",
      "type": "string",
      "default": ""
    },
    {
      "key": "proxy",
      "label": "代理地址（支持 http://、socks5://）",
      "type": "string",
      "default": ""
    },
    {
      "key": "username",
      "label": "Bot 显示名称",
      "type": "string",
      "default": "Recorder Bot"
    }
  ]
}"#;

/// 构建配置了超时和可选代理的 HTTP 客户端。
/// Build an HTTP agent configured with timeouts and optional proxy.
///
/// # 参数 / Parameters
/// - `proxy`: 代理地址（空字符串表示不使用代理）/ Proxy URL (empty string means no proxy)
fn build_agent(proxy: &str) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(600)));
    if !proxy.is_empty() {
        match ureq::Proxy::new(proxy) {
            Ok(p) => {
                config = config.proxy(Some(p));
            }
            Err(_) => {
                eprintln!("Warning: invalid proxy URL '{}', ignoring", proxy);
            }
        }
    }
    config.build().into()
}

/// 带进度上报的同步读取器，包装任意 `Read` 实现。
/// Synchronous reader with progress reporting, wrapping any `Read` implementation.
struct ProgressReader<R: Read> {
    inner: R,
    done: u64,
    total: u64,
    last_reported: u64,
    speed_bytes: u64,
    speed_last: Instant,
}

impl<R: Read> ProgressReader<R> {
    /// 创建新的进度读取器。
    /// Create a new progress reader.
    ///
    /// # 参数 / Parameters
    /// - `inner`: 被包装的读取器 / Wrapped reader
    /// - `total`: 总字节数（用于计算百分比）/ Total bytes (for percentage calculation)
    fn new(inner: R, total: u64) -> Self {
        Self {
            inner,
            done: 0,
            total,
            last_reported: u64::MAX,
            speed_bytes: 0,
            speed_last: Instant::now(),
        }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 && self.total > 0 {
            self.done += n as u64;
            self.speed_bytes += n as u64;
            // 计算缩放后的进度值并上报 / Calculate scaled progress and report
            let scaled = ((self.done as u128 * PROGRESS_SCALE as u128) / self.total as u128)
                .min(PROGRESS_SCALE as u128) as u64;
            if scaled != self.last_reported {
                self.last_reported = scaled;
                println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
            }
            // 每秒上报一次上传速度 / Report upload speed once per second
            let elapsed = self.speed_last.elapsed();
            if elapsed >= Duration::from_secs(1) {
                let bps = self.speed_bytes as f64 / elapsed.as_secs_f64();
                println!("STATUS:{}", format_speed(bps));
                self.speed_bytes = 0;
                self.speed_last = Instant::now();
            }
        }
        Ok(n)
    }
}

/// 模块主逻辑：收集录制信息 -> 构建消息 -> 发送到 Discord Webhook。
/// Main module logic: collect recording info -> build message -> send to Discord Webhook.
fn run() -> Result<(), String> {
    // 读取输入文件路径 / Read input file path
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    // 读取模块参数 / Read module parameters
    let webhook_url = param("webhook_url", "");
    if webhook_url.is_empty() {
        return Err("webhook_url is required".to_string());
    }
    let proxy = param("proxy", "");
    let bot_name = param("username", "Recorder Bot");

    emit_progress_step(0, 3);

    // 从文件名解析主播名和时间戳 / Parse model name and timestamp from filename
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let (model_name, timestamp) = parse_stem(stem);
    let file_size = fs::metadata(&input).map(|m| m.len()).unwrap_or(0);
    let duration = video_duration(&input).unwrap_or(0.0);

    // 构建 Discord 消息内容（Markdown 格式）/ Build Discord message content (Markdown format)
    let content = format!(
        "**ModelName:** `#{model}`\n\
         **Timestamp:** `{ts}`\n\
         **Duration:** `{dur}`\n\
         **FileName:** `{name}`\n\
         **FileSize:** `{size}`",
        model = model_name,
        ts = if timestamp.is_empty() {
            "—".to_string()
        } else {
            timestamp
        },
        dur = format_duration(duration),
        name = input.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        size = format_bytes(file_size),
    );

    emit_progress_step(1, 3);

    let agent = build_agent(&proxy);
    let cover = find_cover(&input);

    if let Some(ref img_path) = cover {
        // 有封面图：使用 multipart/form-data 同时发送消息和图片
        // Has cover image: send message and image together using multipart/form-data
        let img_bytes =
            fs::read(img_path).map_err(|e| format!("Failed to read cover image: {}", e))?;
        let img_name = img_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cover.jpg");
        let mime = if img_name.ends_with(".png") {
            "image/png"
        } else if img_name.ends_with(".webp") {
            "image/webp"
        } else {
            "image/jpeg"
        };

        // 手动构建 multipart 请求体 / Manually build multipart request body
        let payload_json =
            serde_json::json!({ "username": bot_name, "content": content }).to_string();
        let boundary = "----RustBoundary7f3a9b2c";
        let mut body: Vec<u8> = Vec::new();

        // payload_json 部分 / payload_json part
        let pj_header = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"payload_json\"\r\nContent-Type: application/json\r\n\r\n",
            b = boundary
        );
        body.extend_from_slice(pj_header.as_bytes());
        body.extend_from_slice(payload_json.as_bytes());
        body.extend_from_slice(b"\r\n");

        // 图片文件部分 / Image file part
        let file_header = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{n}\"\r\nContent-Type: {m}\r\n\r\n",
            b = boundary, n = img_name, m = mime
        );
        body.extend_from_slice(file_header.as_bytes());
        body.extend_from_slice(&img_bytes);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

        let content_type = format!("multipart/form-data; boundary={}", boundary);
        let body_len = body.len() as u64;
        // 使用进度读取器包装请求体以上报上传进度
        // Wrap request body with progress reader to report upload progress
        let mut progress_reader = ProgressReader::new(io::Cursor::new(body), body_len);
        let send_body = ureq::SendBody::from_reader(&mut progress_reader);

        println!("PROGRESS:0/{}", PROGRESS_SCALE);
        let upload_start = Instant::now();

        let resp = agent
            .post(&webhook_url)
            .header("Content-Type", &content_type)
            .send(send_body)
            .map_err(|e| format!("Discord request failed: {}", e))?;

        // 上报最终上传速度 / Report final upload speed
        let elapsed = upload_start.elapsed();
        if elapsed.as_secs_f64() > 0.0 {
            println!(
                "STATUS:{}",
                format_speed(body_len as f64 / elapsed.as_secs_f64())
            );
        }

        if resp.status() != 200 && resp.status() != 204 {
            let status = resp.status();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            return Err(format!("Discord returned {}: {}", status, body));
        }
    } else {
        // 无封面图：仅发送文字消息 / No cover image: send text message only
        emit_progress_step(2, 3);
        let payload = serde_json::json!({ "username": bot_name, "content": content });
        let resp = agent
            .post(&webhook_url)
            .header("Content-Type", "application/json")
            .send(payload.to_string())
            .map_err(|e| format!("Discord request failed: {}", e))?;

        if resp.status() != 200 && resp.status() != 204 {
            let status = resp.status();
            let body = resp.into_body().read_to_string().unwrap_or_default();
            return Err(format!("Discord returned {}: {}", status, body));
        }
    }

    emit_progress_step(3, 3);
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
