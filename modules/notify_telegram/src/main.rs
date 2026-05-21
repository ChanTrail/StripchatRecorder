//! Telegram 通知后处理模块 / Telegram Notification Post-processing Module
//!
//! 通过 MTProto 协议将录制信息、封面图和视频文件发送到 Telegram 频道或群组。
//! 支持超过 2GB 的大文件（自动分割）、HTTP/SOCKS5 代理，以及多次重连重试。
//!
//! Sends recording info, cover image, and video files to a Telegram channel or group
//! via the MTProto protocol. Supports files over 2GB (auto-split), HTTP/SOCKS5 proxy,
//! and multiple reconnect retries.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - 环境变量 `PP_INPUT`: 输入视频文件路径 / Input video file path via env var
//! - 标准输出 `OUTPUT:{path}`: 成功后输出视频路径 / Output video path on success
//! - 标准输出 `PROGRESS:{done}/{total}`: 进度上报 / Progress reporting
//! - 标准输出 `STATUS:{speed}`: 上传速度上报 / Upload speed reporting

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use grammers_client::Client;
use grammers_client::client::{ClientConfiguration, AutoSleep};
use grammers_client::media::{InputMedia, Uploaded};
use grammers_client::message::InputMessage;
use grammers_client::sender::{ConnectionParams, SenderPool};
use grammers_client::tl;
use grammers_session::storages::SqliteSession;
use grammers_session::types::{PeerAuth, PeerId, PeerRef};
use tokio::io::AsyncReadExt;
use pp_utils::{param, param_bool, format_duration, format_bytes, format_speed, parse_stem, find_cover, tmp_dir, image_dimensions, video_meta};

/// 进度上报的缩放基数 / Progress reporting scale base
const PROGRESS_SCALE: usize = 10_000;

/// 模块元数据 JSON，通过 `--describe` 参数输出。
/// Module metadata JSON, output via `--describe` argument.
const DESCRIBE: &str = r#"{
    "id": "notify_telegram",
    "name": "Telegram 通知 0.2.0",
    "description": "将录制信息、封面图和视频通过 MTProto 发送到 Telegram（支持超过 50MB 的大文件，支持 HTTP/SOCKS5 代理）",
    "params": [
        {
        "key": "api_id",
        "label": "API ID（从 my.telegram.org 获取）",
        "type": "string",
        "default": ""
        },
        {
        "key": "api_hash",
        "label": "API Hash",
        "type": "string",
        "default": ""
        },
        {
        "key": "bot_token",
        "label": "Bot Token（从 @BotFather 获取）",
        "type": "string",
        "default": ""
        },
        {
        "key": "chat_id",
        "label": "Chat ID（超级群组填 -100xxxxxxxxxx 格式）",
        "type": "string",
        "default": ""
        },
        {
        "key": "username",
        "label": "群组 Username（超级群组必填，如 mygroupname，不含 @）",
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
        "key": "send_video",
        "label": "同时发送视频文件",
        "type": "boolean",
        "default": true
        }
    ],
    "i18n": {
        "en-US": {
        "name": "Telegram Notification 0.2.0",
        "description": "Send recording info, cover image and video to Telegram via MTProto (supports files over 50 MB and HTTP/SOCKS5 proxies)",
        "params": {
            "api_id": { "label": "API ID (from my.telegram.org)" },
            "bot_token": { "label": "Bot Token (from @BotFather)" },
            "chat_id": { "label": "Chat ID (supergroup format: -100xxxxxxxxxx)" },
            "username": { "label": "Group username (required for supergroups, e.g. mygroupname, without @)" },
            "proxy": { "label": "Proxy (http:// or socks5://)" },
            "send_video": { "label": "Also send video file" }
        }
        }
    }
}"#;

/// 若封面图不满足 Telegram 限制（宽+高 < 10000 且宽高比 < 20:1），则等比缩放。
/// Resize cover image if it violates Telegram limits (w+h < 10000 and aspect ratio < 20:1).
/// Returns Some(new_path) if resized, None if no resize needed.
fn resize_cover_for_telegram(img: &Path) -> Result<Option<PathBuf>, String> {
    const MAX_PHOTO_BYTES: u64 = 10 * 1024 * 1024; // Telegram photo limit: 10 MB

    let (w, h) = match image_dimensions(img) {
        Some(d) => d,
        None => return Ok(None), // 无法获取尺寸，跳过
    };

    let file_size = fs::metadata(img).map(|m| m.len()).unwrap_or(0);

    // 检查是否需要缩放
    let sum_ok = (w + h) < 10000;
    let ratio_ok = w.max(h) < h.min(w).saturating_mul(20);
    let size_ok = file_size < MAX_PHOTO_BYTES;
    if sum_ok && ratio_ok && size_ok {
        return Ok(None);
    }

    // 计算目标尺寸：同时满足两个约束
    // 约束1: w' + h' < 10000  => scale <= 9999 / (w+h)
    // 约束2: max(w',h') / min(w',h') < 20  => 宽高比不变，只要原始比例 < 20:1 就满足
    //        若原始比例 >= 20:1，则将长边限制为短边的 19 倍
    let (tw, th) = if !ratio_ok {
        // 先修正宽高比：将长边缩到短边的 19 倍
        if w >= h {
            (h * 19, h)
        } else {
            (w, w * 19)
        }
    } else {
        (w, h)
    };

    // 再检查 sum 约束
    let (tw, th) = if tw + th >= 10000 {
        // 等比缩放使 w'+h' = 9999
        let scale = 9999.0 / (tw + th) as f64;
        let nw = ((tw as f64 * scale).floor() as u32).max(1);
        let nh = ((th as f64 * scale).floor() as u32).max(1);
        (nw, nh)
    } else {
        (tw, th)
    };

    let stem = img.file_stem().and_then(|s| s.to_str()).unwrap_or("cover");
    let out_path = tmp_dir().join(format!("{}_tg_resized.jpg", stem));

    // 若文件超过 10MB，逐步降低质量直到满足大小限制
    // If file exceeds 10MB, progressively lower quality until size limit is met
    let q_values: &[&str] = if !size_ok { &["5", "10", "15", "20", "25", "31"] } else { &["2"] };
    let mut success = false;
    for &q in q_values {
        let status = Command::new("ffmpeg")
            .args(["-y", "-i"]).arg(img)
            .args(["-vf", &format!("scale={}:{}", tw, th), "-q:v", q])
            .arg(&out_path)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .status().map_err(|e| format!("ffmpeg not found: {}", e))?;

        if !status.success() {
            return Err("ffmpeg failed to resize cover image for Telegram".to_string());
        }

        let out_size = fs::metadata(&out_path).map(|m| m.len()).unwrap_or(u64::MAX);
        if out_size < MAX_PHOTO_BYTES {
            success = true;
            break;
        }
    }

    if !success {
        return Err("cover image exceeds Telegram 10MB photo limit even after compression".to_string());
    }

    Ok(Some(out_path))
}

/// 使用 ffmpeg 从视频中提取第一帧作为缩略图（用于 Telegram 视频消息的预览图）。
/// Extract the first frame from a video as a thumbnail using ffmpeg
/// (used as preview image for Telegram video messages).
///
/// # 返回值 / Returns
/// 缩略图文件路径，失败时返回错误。
/// Thumbnail file path, or error on failure.
fn extract_video_thumbnail(input: &Path) -> Result<PathBuf, String> {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
    let thumb_path = tmp_dir().join(format!("{}.tg_thumb.png", stem));
    let status = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args(["-frames:v", "1", "-q:v", "2"])
        .arg(&thumb_path)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status()
        .map_err(|e| format!("ffmpeg not found: {}", e))?;
    if !status.success() { return Err("ffmpeg failed to extract video thumbnail".to_string()); }
    if !thumb_path.exists() { return Err("video thumbnail file was not created".to_string()); }
    Ok(thumb_path)
}

/// 将视频文件按大小分割为多个片段（用于绕过 Telegram 2GB 文件大小限制）。
/// 若文件大小未超过限制则直接返回原文件路径。
///
/// Split a video file into segments by size (to work around Telegram's 2GB file size limit).
/// Returns the original file path if it doesn't exceed the limit.
///
/// # 参数 / Parameters
/// - `input`: 输入视频路径 / Input video path
/// - `max_bytes`: 每个片段的最大字节数 / Maximum bytes per segment
///
/// 用 ffprobe 获取视频的平均比特率（bps），失败时返回 None。
///
/// Get the average bitrate of a video via ffprobe (bps), returns None on failure.
fn probe_bitrate(input: &Path) -> Option<u64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=bit_rate",
               "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(input)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<u64>().ok()
}

/// 将单个文件切分为不超过 max_bytes 的若干段，返回所有段的路径。
/// Split a single file into segments not exceeding max_bytes, returns all segment paths.
fn split_one(input: &Path, max_bytes: u64, dir: &Path, stem: &str, ext: &str, offset: usize) -> Result<Vec<PathBuf>, String> {
    let file_size = fs::metadata(input).map_err(|e| format!("stat failed: {}", e))?.len();
    let duration = pp_utils::video_duration(input).unwrap_or(0.0);
    if duration <= 0.0 { return Err("cannot split: unable to determine video duration".to_string()); }

    // 优先用 ffprobe 比特率计算，回退到文件大小比例，均留 10% 余量
    // Prefer ffprobe bitrate for calculation, fall back to file-size ratio; keep 10% margin
    let seg_duration = if let Some(bps) = probe_bitrate(input).filter(|&b| b > 0) {
        let max_bits = (max_bytes as f64 * 0.90) * 8.0;
        (max_bits / bps as f64).floor().max(1.0)
    } else {
        let ratio = (max_bytes as f64) / (file_size as f64) * 0.90;
        (duration * ratio).floor().max(1.0)
    };

    let n_segs = (duration / seg_duration).ceil() as usize + 1;

    // ffmpeg segment 模式不支持任意起始编号，先生成临时名再重命名
    // ffmpeg segment mode doesn't support arbitrary start numbers; generate then rename
    let tmp_dir_path = dir.join(format!("_split_tmp_{}", offset));
    fs::create_dir_all(&tmp_dir_path).map_err(|e| format!("mkdir failed: {}", e))?;
    let tmp_pattern = tmp_dir_path.join(format!("seg%03d.{}", ext));

    let status = Command::new("ffmpeg")
        .args(["-y", "-i"]).arg(input)
        .args(["-c", "copy", "-f", "segment",
               "-segment_time", &seg_duration.to_string(),
               "-reset_timestamps", "1"])
        .arg(&tmp_pattern)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status()
        .map_err(|e| format!("ffmpeg not found: {}", e))?;

    if !status.success() { return Err("ffmpeg failed to split video".to_string()); }

    let mut segments: Vec<PathBuf> = (0..n_segs)
        .map(|i| tmp_dir_path.join(format!("seg{:03}.{}", i, ext)))
        .filter(|p| p.exists())
        .collect();
    segments.sort();

    if segments.is_empty() { return Err("ffmpeg produced no segment files".to_string()); }

    // 重命名为最终路径，编号从 offset 开始
    // Rename to final paths starting from offset
    let mut result = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let dest = dir.join(format!("{}_part{:03}.{}", stem, offset + i, ext));
        fs::rename(seg, &dest).map_err(|e| format!("rename failed: {}", e))?;
        result.push(dest);
    }
    let _ = fs::remove_dir(&tmp_dir_path);
    Ok(result)
}

fn split_video(input: &Path, max_bytes: u64) -> Result<Vec<PathBuf>, String> {
    let file_size = fs::metadata(input).map_err(|e| format!("stat failed: {}", e))?.len();
    if file_size <= (max_bytes as f64 * 0.95) as u64 { return Ok(vec![input.to_path_buf()]); }

    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("part");
    let ext  = input.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let dir  = tmp_dir();
    // 第一次切分
    // Initial split
    let initial = split_one(input, max_bytes, &dir, stem, ext, 0)?;

    // 对仍然超大的分片递归再切，最多重试 2 次
    // Recursively re-split any oversized segments, up to 2 retries
    let mut final_segments: Vec<PathBuf> = Vec::new();
    let mut counter = initial.len();
    for seg in initial {
        let seg_size = fs::metadata(&seg).map(|m| m.len()).unwrap_or(0);
        if seg_size > max_bytes {
            // 递归切分，编号从当前 counter 开始
            let sub = split_one(&seg, max_bytes, &dir, stem, ext, counter)?;
            counter += sub.len();
            // 删除超大的中间文件
            let _ = fs::remove_file(&seg);
            final_segments.extend(sub);
        } else {
            final_segments.push(seg);
        }
    }

    let still_oversized = final_segments.iter()
        .filter(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0) > max_bytes)
        .count();
    if still_oversized > 0 {
        return Err(format!("{} segment(s) still exceed the size limit after splitting", still_oversized));
    }

    final_segments.sort();
    Ok(final_segments)
}

/// 获取 Telegram 会话文件路径（存储在系统配置目录下）。
/// Get the Telegram session file path (stored in the system config directory).
fn session_path(api_id: i32) -> PathBuf {
    let base = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("notify_telegram");
    fs::create_dir_all(&base).ok();
    base.join(format!("{}.db", api_id))
}

/// 根据数字 chat_id 构建 Telegram PeerRef（不需要网络请求）。
/// Build a Telegram PeerRef from a numeric chat_id (no network request needed).
///
/// 支持用户、普通群组和超级群组/频道的 ID 格式。
/// Supports user, regular group, and supergroup/channel ID formats.
fn build_peer_ref(chat_id: i64) -> PeerRef {
    let id = if chat_id < -1_000_000_000_000 {
        PeerId::channel_unchecked((-chat_id) - 1_000_000_000_000)
    } else if chat_id < -1_000_000_000 {
        PeerId::channel_unchecked((-chat_id) - 1_000_000_000)
    } else if chat_id < 0 {
        PeerId::chat_unchecked(-chat_id)
    } else {
        PeerId::user_unchecked(chat_id)
    };
    PeerRef { id, auth: PeerAuth::default() }
}

/// 计算字符串的 UTF-16 编码长度（Telegram 消息实体使用 UTF-16 偏移量）。
/// Calculate the UTF-16 encoded length of a string (Telegram message entities use UTF-16 offsets).
fn utf16_len(s: &str) -> usize { s.encode_utf16().count() }

/// 构建 Telegram 消息的文字内容和格式化实体（粗体标签、Hashtag、代码块、引用块）。
/// Build the text content and formatting entities for a Telegram message
/// (bold labels, hashtag, code blocks, blockquote).
///
/// # 参数 / Parameters
/// - `model_name`: 主播名（用作 Hashtag）/ Model name (used as hashtag)
/// - `timestamp`: 录制时间戳字符串 / Recording timestamp string
/// - `duration_str`: 格式化后的时长字符串 / Formatted duration string
/// - `file_name`: 文件名 / Filename
/// - `file_size`: 格式化后的文件大小字符串 / Formatted file size string
/// - `part_label`: 分片标签（当前/总数），None 表示不分片 / Part label (current/total), None if not split
fn build_caption(
    model_name: &str, timestamp: &str, duration_str: &str,
    file_name: &str, file_size: &str,
    part_label: Option<(usize, usize)>,
) -> (String, Vec<tl::enums::MessageEntity>) {
    let mut text = String::new();
    let mut entities: Vec<tl::enums::MessageEntity> = Vec::new();

    // 宏：添加一行"粗体标签: 值"并附加对应的格式化实体
    // Macro: add a "bold label: value" line with corresponding formatting entities
    macro_rules! push_line {
        ($label:expr, $value:expr, $entity:expr) => {{
            let bold_start = utf16_len(&text) as i32;
            text.push_str(&format!("{}: ", $label));
            let bold_end = utf16_len(&text) as i32;
            entities.push(tl::types::MessageEntityBold { offset: bold_start, length: bold_end - bold_start }.into());
            let val_start = utf16_len(&text) as i32;
            text.push_str($value);
            let val_end = utf16_len(&text) as i32;
            entities.push($entity(val_start, val_end - val_start));
        }};
    }

    // 第一行：主播名作为 Hashtag / First line: model name as hashtag
    let hashtag_value = format!("#{}", model_name);
    push_line!("ModelName", &hashtag_value, |offset, length| {
        tl::types::MessageEntityHashtag { offset, length }.into()
    });

    // 后续行：时间戳、时长、文件名、文件大小（代码格式）
    // Subsequent lines: timestamp, duration, filename, file size (code format)
    for (label, value) in &[("Timestamp", timestamp), ("Duration", duration_str), ("FileName", file_name), ("FileSize", file_size)] {
        text.push('\n');
        push_line!(label, value, |offset, length| {
            tl::types::MessageEntityCode { offset, length }.into()
        });
    }

    // 可选的分片标签 / Optional part label
    if let Some((cur, total)) = part_label {
        let part_value = format!("{}/{}", cur, total);
        text.push('\n');
        push_line!("Part", &part_value, |offset, length| {
            tl::types::MessageEntityCode { offset, length }.into()
        });
    }

    // 将整个消息包裹在引用块中 / Wrap the entire message in a blockquote
    let total_len = utf16_len(&text) as i32;
    entities.push(tl::types::MessageEntityBlockquote { collapsed: false, offset: 0, length: total_len }.into());
    (text, entities)
}

/// 解析 Telegram Peer（优先通过 username 解析，其次使用数字 chat_id）。
/// Resolve a Telegram Peer (prefers username resolution, falls back to numeric chat_id).
async fn resolve_peer(client: &Client, chat_id: i64, username: &str) -> Result<PeerRef, String> {
    if !username.is_empty() {
        let peer = client.resolve_username(username).await
            .map_err(|e| format!("resolve_username failed: {}", e))?
            .ok_or_else(|| format!("username @{} not found", username))?;
        return peer.to_ref().await.ok_or_else(|| format!("@{} peer_ref unavailable", username));
    }
    Ok(build_peer_ref(chat_id))
}

/// 上传文件到 Telegram，支持断点续传、进度上报。
///
/// 直接调用底层 `SaveBigFilePart` TL 函数，保持同一 `file_id`，
/// 网络中断后从上次成功的分块处继续，而不是从头重传。
///
/// Upload a file to Telegram with resumable upload and progress reporting.
///
/// Calls the underlying `SaveBigFilePart` TL function directly, keeping the same
/// `file_id` across retries so that interrupted uploads resume from the last
/// successfully uploaded part instead of restarting from the beginning.
async fn upload_with_progress(
    client: &Client, path: &Path,
    done: Arc<AtomicUsize>, total: usize,
) -> Result<Uploaded, String> {
    // 每个分块 512 KB（Telegram 最大分块大小）/ 512 KB per chunk (Telegram max chunk size)
    const CHUNK_SIZE: usize = 512 * 1024;
    // 单个分块的最大重试次数 / Max retries per chunk
    const MAX_CHUNK_RETRIES: u32 = 20;
    // 重试延迟序列（秒）/ Retry delay sequence (seconds)
    const RETRY_DELAYS: [u64; 6] = [10, 20, 30, 40, 50, 60];

    let name = path.file_name().unwrap().to_string_lossy().to_string();

    let mut file = tokio::fs::File::open(path).await
        .map_err(|e| format!("open {} failed: {}", path.display(), e))?;
    let size = file.metadata().await
        .map_err(|e| format!("metadata failed: {}", e))?.len() as usize;

    // 与 grammers 保持一致：> 10MB 用大文件接口，否则用小文件接口
    // Match grammers behavior: use big-file API for >10MB, small-file API otherwise.
    // InputMediaUploadedPhoto requires InputFile (small), not InputFileBig — using the wrong
    // one causes PHOTO_SAVE_FILE_INVALID.
    const BIG_FILE_THRESHOLD: usize = 10 * 1024 * 1024;
    let is_big = size > BIG_FILE_THRESHOLD;

    let total_parts = size.div_ceil(CHUNK_SIZE) as i32;
    // 生成一次性 file_id，整个上传过程保持不变 / Generate file_id once; keep it for the entire upload
    let file_id = {
        use std::time::{SystemTime, UNIX_EPOCH};
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        (t.as_nanos() as i64) ^ (path.as_os_str().len() as i64 * 0x9e3779b97f4a7c15_u64 as i64)
    };

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut speed_bytes: usize = 0;
    let mut speed_last = Instant::now();
    // md5 仅小文件需要 / md5 only needed for small files
    let mut md5_ctx = md5::Context::new();

    for part in 0..total_parts {
        // 读取当前分块 / Read current chunk
        let mut read = 0usize;
        while read < CHUNK_SIZE {
            match file.read(&mut buf[read..]).await {
                Ok(0) => break,
                Ok(n) => read += n,
                Err(e) => return Err(format!("read chunk {} failed: {}", part, e)),
            }
        }
        if read == 0 { break; }
        let chunk = buf[..read].to_vec();
        if !is_big { md5_ctx.consume(&chunk); }

        // 带重试的分块上传 / Upload chunk with retry
        let mut chunk_attempt = 0u32;
        loop {
            let result = if is_big {
                client.invoke(&tl::functions::upload::SaveBigFilePart {
                    file_id,
                    file_part: part,
                    file_total_parts: total_parts,
                    bytes: chunk.clone(),
                }).await
            } else {
                client.invoke(&tl::functions::upload::SaveFilePart {
                    file_id,
                    file_part: part,
                    bytes: chunk.clone(),
                }).await
            };

            match result {
                Ok(true) => break,
                Ok(false) => {
                    let err = format!("server rejected part {}", part);
                    chunk_attempt += 1;
                    if chunk_attempt >= MAX_CHUNK_RETRIES { return Err(err); }
                    let delay = RETRY_DELAYS[(chunk_attempt as usize - 1).min(RETRY_DELAYS.len() - 1)];
                    eprintln!("chunk {}/{} attempt {}/{}: {}. retrying in {}s…",
                        part + 1, total_parts, chunk_attempt, MAX_CHUNK_RETRIES, err, delay);
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
                Err(e) => {
                    let err = format!("part {} upload error: {}", part, e);
                    chunk_attempt += 1;
                    if chunk_attempt >= MAX_CHUNK_RETRIES { return Err(err); }
                    let delay = RETRY_DELAYS[(chunk_attempt as usize - 1).min(RETRY_DELAYS.len() - 1)];
                    eprintln!("chunk {}/{} attempt {}/{}: {}. retrying in {}s…",
                        part + 1, total_parts, chunk_attempt, MAX_CHUNK_RETRIES, err, delay);
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
            }
        }

        // 更新进度和速度 / Update progress and speed
        if total > 0 {
            let new_done = done.fetch_add(read, Ordering::Relaxed) + read;
            speed_bytes += read;
            let scaled = ((new_done as u128) * (PROGRESS_SCALE as u128) / (total as u128))
                .min(PROGRESS_SCALE as u128) as usize;
            println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
            use std::io::Write;
            let _ = std::io::stdout().flush();

            let elapsed = speed_last.elapsed();
            if elapsed >= Duration::from_secs(1) {
                let bps = speed_bytes as f64 / elapsed.as_secs_f64();
                println!("STATUS:{}", format_speed(bps));
                let _ = std::io::stdout().flush();
                speed_bytes = 0;
                speed_last = Instant::now();
            }
        }
    }

    Ok(Uploaded::from_raw(if is_big {
        tl::types::InputFileBig { id: file_id, parts: total_parts, name }.into()
    } else {
        tl::types::InputFile {
            id: file_id, parts: total_parts, name,
            md5_checksum: format!("{:x}", md5_ctx.finalize()),
        }.into()
    }))
}

/// 核心异步函数：建立 Telegram 连接、上传文件并发送消息。
/// Core async function: establish Telegram connection, upload files, and send messages.
///
/// 流程：
/// 1. 建立 MTProto 连接并进行 Bot 登录
/// 2. 转换封面图格式（若需要）
/// 3. 转换/重封装视频格式（若需要）
/// 4. 并行上传封面图、视频缩略图和视频文件
/// 5. 以相册形式发送（封面图 + 视频，每批最多 10 条）
///
/// Flow:
/// 1. Establish MTProto connection and bot sign-in
/// 2. Convert cover image format if needed
/// 3. Convert/remux video format if needed
/// 4. Upload cover image, video thumbnails, and video files
/// 5. Send as album (cover + videos, up to 10 per batch)
#[allow(clippy::too_many_arguments)]
async fn upload_and_send(
    api_id: i32, api_hash: &str, bot_token: &str, proxy: &str,
    chat_id: i64, username: &str,
    model_name: &str, timestamp: &str, duration_str: &str,
    file_name: &str, file_size_str: &str,
    input: &Path, cover: Option<&Path>, send_video: bool,
    video_parts: &[PathBuf],
) -> Result<(), String> {
    let (base_caption_text, base_caption_entities) =
        build_caption(model_name, timestamp, duration_str, file_name, file_size_str, None);

    // 打开或创建 SQLite 会话文件 / Open or create SQLite session file
    let session = Arc::new(
        SqliteSession::open(&session_path(api_id)).await
            .map_err(|e| format!("open session failed: {}", e))?,
    );

    let conn_params = ConnectionParams {
        proxy_url: if proxy.is_empty() { None } else { Some(proxy.to_string()) },
        ..Default::default()
    };

    // 创建连接池并在后台运行 / Create connection pool and run in background
    let pool = SenderPool::with_configuration(Arc::clone(&session), api_id, conn_params);
    let runner = pool.runner;
    tokio::spawn(async move { runner.run().await });
    // 配置激进的 retry policy：I/O 错误视为 5 秒 flood，flood wait 阈值提高到 5 分钟
    // Aggressive retry policy: treat I/O errors as 5s flood, raise flood-wait threshold to 5 min
    let client = Client::with_configuration(pool.handle, ClientConfiguration {
        retry_policy: Box::new(AutoSleep {
            threshold: Duration::from_secs(300),
            io_errors_as_flood_of: Some(Duration::from_secs(5)),
        }),
        auto_cache_peers: true,
    });

    // Bot 登录（会话已存在时跳过）/ Bot sign-in (skipped if session already exists)
    if !client.is_authorized().await.map_err(|e| format!("is_authorized failed: {}", e))? {
        client.bot_sign_in(bot_token, api_hash).await
            .map_err(|e| format!("bot_sign_in failed: {}", e))?;
    }

    let peer = resolve_peer(&client, chat_id, username).await?;

    // 处理封面图：非 jpg/png 格式需先用 ffmpeg 转换，然后检查尺寸限制
    // Handle cover image: non-jpg/png formats need ffmpeg conversion first, then check dimension limits
    let converted_cover: Option<PathBuf>;
    let resized_cover: Option<PathBuf>;
    let effective_cover: Option<&Path>;
    if let Some(img) = cover {
        let ext = img.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            converted_cover = None;
        } else {
            let tmp = tmp_dir().join(format!(
                "{}_tg_tmp.png",
                img.file_stem().and_then(|s| s.to_str()).unwrap_or("cover")
            ));
            let status = Command::new("ffmpeg")
                .args(["-y", "-i"]).arg(img).arg(&tmp)
                .stdout(Stdio::null()).stderr(Stdio::null())
                .status().map_err(|e| format!("ffmpeg not found: {}", e))?;
            if !status.success() { return Err("ffmpeg failed to convert cover image".to_string()); }
            converted_cover = Some(tmp);
        }
        let after_format: &Path = converted_cover.as_deref().unwrap_or(img);
        // 检查并等比缩放封面图以满足 Telegram 尺寸限制
        // Check and proportionally resize cover image to meet Telegram dimension limits
        resized_cover = resize_cover_for_telegram(after_format)?;
        effective_cover = resized_cover.as_deref().or(Some(after_format));
    } else {
        converted_cover = None;
        resized_cover = None;
        effective_cover = None;
    }

    // 处理视频文件：非 mp4/mkv 格式需先重封装，重封装失败则转码
    // Handle video files: non-mp4/mkv formats need remuxing, falls back to transcoding
    let mut converted_parts: Vec<PathBuf> = Vec::new();
    let effective_parts: Vec<PathBuf> = if send_video {
        let mut parts_out: Vec<PathBuf> = Vec::new();
        for part in video_parts {
            let ext = part.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            if ext == "mp4" || ext == "mkv" {
                parts_out.push(part.clone());
            } else {
                let tmp = tmp_dir().join(format!(
                    "{}_tg_tmp.mkv",
                    part.file_stem().and_then(|s| s.to_str()).unwrap_or("video")
                ));
                // 先尝试无损重封装 / Try lossless remux first
                let remux_ok = Command::new("ffmpeg")
                    .args(["-y", "-i"]).arg(part)
                    .args(["-c", "copy", "-movflags", "+faststart"]).arg(&tmp)
                    .stdout(Stdio::null()).stderr(Stdio::null())
                    .status().map_err(|e| format!("ffmpeg not found: {}", e))?.success();
                if !remux_ok {
                    // 重封装失败则转码为 H.264 + AAC / Fall back to H.264 + AAC transcoding
                    let ok = Command::new("ffmpeg")
                        .args(["-y", "-i"]).arg(part)
                        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "23",
                               "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"]).arg(&tmp)
                        .stdout(Stdio::null()).stderr(Stdio::null())
                        .status().map_err(|e| format!("ffmpeg not found: {}", e))?.success();
                    if !ok { return Err("ffmpeg failed to convert video to mkv".to_string()); }
                }
                converted_parts.push(tmp.clone());
                parts_out.push(tmp);
            }
        }
        parts_out
    } else {
        vec![input.to_path_buf()]
    };

    // 提取每个视频片段的缩略图和元数据 / Extract thumbnail and metadata for each video part
    struct PartMeta { thumb_path: Option<PathBuf>, duration: f64, w: i32, h: i32 }
    let part_metas: Vec<PartMeta> = if send_video {
        effective_parts.iter().map(|p| {
            let thumb_path = extract_video_thumbnail(p).ok();
            let (dur, w, h) = video_meta(p).unwrap_or((0.0, 1280, 720));
            PartMeta { thumb_path, duration: dur, w, h }
        }).collect()
    } else { vec![] };

    // 计算所有需要上传的文件总大小（用于进度计算）
    // Calculate total size of all files to upload (for progress calculation)
    let cover_size = effective_cover.and_then(|p| fs::metadata(p).ok()).map(|m| m.len() as usize).unwrap_or(0);
    let video_size: usize = if send_video {
        effective_parts.iter().map(|p| fs::metadata(p).ok().map(|m| m.len() as usize).unwrap_or(0)).sum()
    } else { 0 };
    let thumb_size: usize = part_metas.iter()
        .filter_map(|m| m.thumb_path.as_ref())
        .filter_map(|p| fs::metadata(p).ok())
        .map(|m| m.len() as usize).sum();
    let upload_total = cover_size + video_size + thumb_size;

    // 共享的已上传字节计数器 / Shared uploaded bytes counter
    let done = Arc::new(AtomicUsize::new(0));

    if send_video {
        // 固化后的视频分片，包含服务端永久 Media 引用和元数据
        // Committed video parts: server-side permanent Media reference + metadata
        struct CommittedPart {
            media: grammers_client::media::Media,  // 已通过 UploadMedia 固化 / committed via UploadMedia
        }

        // 上传单个文件后立即调用 messages.UploadMedia 固化到服务端，带重试。
        // 固化后的 Media 不受上传 session TTL 限制（约 10 分钟），可安全地在之后发送。
        //
        // Upload a file then immediately commit it via messages.UploadMedia, with retry.
        // Committed Media is not subject to the ~10-minute upload session TTL.
        async fn commit_media(
            client: &Client,
            peer: &PeerRef,
            raw_media: tl::enums::InputMedia,
        ) -> Result<grammers_client::media::Media, String> {
            const MAX_RETRIES: u32 = 10;
            const DELAYS: [u64; 5] = [5, 10, 20, 40, 60];
            let mut attempt = 0u32;
            loop {
                match client.invoke(&tl::functions::messages::UploadMedia {
                    business_connection_id: None,
                    peer: (*peer).into(),
                    media: raw_media.clone(),
                }).await {
                    Ok(committed) => {
                        return grammers_client::media::Media::from_raw(committed)
                            .ok_or_else(|| "UploadMedia returned unrecognized media type".to_string());
                    }
                    Err(e) => {
                        attempt += 1;
                        if attempt >= MAX_RETRIES {
                            return Err(format!("UploadMedia failed after {} attempts: {}", attempt, e));
                        }
                        let delay = DELAYS[(attempt as usize - 1).min(DELAYS.len() - 1)];
                        eprintln!("UploadMedia attempt {}/{} failed: {}. retrying in {}s…",
                            attempt, MAX_RETRIES, e, delay);
                        tokio::time::sleep(Duration::from_secs(delay)).await;
                    }
                }
            }
        }

        // 上传封面图并立即固化 / Upload cover and commit immediately
        let committed_cover: Option<grammers_client::media::Media> = if let Some(img) = effective_cover {
            let uploaded = upload_with_progress(&client, img, Arc::clone(&done), upload_total).await?;
            let raw: tl::enums::InputMedia = tl::types::InputMediaUploadedPhoto {
                spoiler: false,
                file: uploaded.raw,
                stickers: None,
                ttl_seconds: None,
            }.into();
            Some(commit_media(&client, &peer, raw).await?)
        } else { None };
        if let Some(ref tmp) = converted_cover { let _ = fs::remove_file(tmp); }
        if let Some(ref tmp) = resized_cover { let _ = fs::remove_file(tmp); }

        // 串行上传每个视频片段及其缩略图，上传后立即固化。
        // Serial upload + immediate commit for each video part and its thumbnail.
        let do_upload_and_commit = |done: Arc<AtomicUsize>| {
            let client = &client;
            let effective_parts = &effective_parts;
            let part_metas = &part_metas;
            let peer = &peer;
            async move {
                let mut parts: Vec<CommittedPart> = Vec::new();
                for (part_path, meta) in effective_parts.iter().zip(part_metas.iter()) {
                    let thumb_uploaded = if let Some(ref tp) = meta.thumb_path {
                        let t = upload_with_progress(client, tp, Arc::clone(&done), upload_total).await;
                        let _ = fs::remove_file(tp);
                        Some(t?)
                    } else { None };

                    let video_uploaded = upload_with_progress(client, part_path, Arc::clone(&done), upload_total).await?;

                    // 直接构建 InputMediaUploadedDocument，避免访问私有字段
                    // Build InputMediaUploadedDocument directly to avoid accessing private fields
                    let ext = part_path.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_lowercase();
                    let mime = if ext == "mkv" { "video/x-matroska" } else { "video/mp4" };
                    let raw_media: tl::enums::InputMedia = tl::types::InputMediaUploadedDocument {
                        nosound_video: false,
                        force_file: false,
                        spoiler: false,
                        file: video_uploaded.raw,
                        thumb: thumb_uploaded.map(|t| t.raw),
                        mime_type: mime.to_string(),
                        attributes: vec![
                            tl::types::DocumentAttributeFilename {
                                file_name: part_path.file_name().unwrap().to_string_lossy().to_string()
                            }.into(),
                            tl::types::DocumentAttributeVideo {
                                round_message: false,
                                supports_streaming: true,
                                nosound: false,
                                duration: meta.duration,
                                w: meta.w,
                                h: meta.h,
                                preload_prefix_size: None,
                                video_start_ts: None,
                                video_codec: None,
                            }.into(),
                        ],
                        stickers: None,
                        ttl_seconds: None,
                        video_cover: None,
                        video_timestamp: None,
                    }.into();

                    let committed = commit_media(client, peer, raw_media).await?;
                    parts.push(CommittedPart { media: committed });
                }
                Ok::<_, String>(parts)
            }
        };

        let mut committed_parts = do_upload_and_commit(Arc::clone(&done)).await?;
        for tmp in &converted_parts { let _ = fs::remove_file(tmp); }

        let total_parts = committed_parts.len();

        // 辅助函数：从固化的 Media 重建 InputMedia 列表（用于发送重试）
        // Helper: rebuild InputMedia list from committed Media (for send retry)
        let build_items = |cover: &Option<grammers_client::media::Media>,
                           parts: &Vec<CommittedPart>| -> Vec<InputMedia> {
            let mut items: Vec<InputMedia> = Vec::new();
            if let Some(c) = cover {
                items.push(InputMedia::new().copy_media(c));
            }
            for (idx, part) in parts.iter().enumerate() {
                let mut item = InputMedia::new().copy_media(&part.media);                if idx == total_parts - 1 {
                    item = item.fmt_entities(base_caption_entities.clone()).caption(base_caption_text.clone());
                }
                items.push(item);
            }
            items
        };

        // 每批最多 10 条发送相册。固化后的 InputMedia 不会有 FILE_PART_MISSING，
        // 但仍保留重传逻辑以防万一。
        // Send album in batches of max 10. Committed InputMedia won't get FILE_PART_MISSING,
        // but keep re-upload logic as a safety net.
        const MAX_ALBUM: usize = 10;
        const MAX_SEND: u32 = 5;
        const MAX_REUPLOAD: u32 = 3;
        const SEND_RETRY_DELAY: Duration = Duration::from_secs(30);

        let mut reupload_count = 0u32;
        loop {
            let n_batches = build_items(&committed_cover, &committed_parts).len().div_ceil(MAX_ALBUM);
            let mut need_reupload = false;
            let mut send_err = String::new();

            'batches: for batch_idx in 0..n_batches {
                let start = batch_idx * MAX_ALBUM;
                let mut send_attempt = 0u32;
                loop {
                    let batch: Vec<InputMedia> = build_items(&committed_cover, &committed_parts)
                        .into_iter().skip(start).take(MAX_ALBUM).collect();
                    match client.send_album(peer, batch).await {
                        Ok(_) => break,
                        Err(e) => {
                            let msg = format!("send_album (batch {}) failed: {}", batch_idx + 1, e);
                            if msg.contains("FILE_PART_MISSING") {
                                need_reupload = true;
                                send_err = msg;
                                break 'batches;
                            }
                            send_attempt += 1;
                            if send_attempt >= MAX_SEND { return Err(msg); }
                            eprintln!("send failed (attempt {}/{}): {}. retrying in {:?}…",
                                send_attempt, MAX_SEND, msg, SEND_RETRY_DELAY);
                            tokio::time::sleep(SEND_RETRY_DELAY).await;
                        }
                    }
                }
            }

            if !need_reupload { break; }

            reupload_count += 1;
            if reupload_count > MAX_REUPLOAD {
                return Err(format!("{} (re-upload limit reached)", send_err));
            }
            eprintln!("FILE_PART_MISSING, re-uploading all files (attempt {}/{})…",
                reupload_count, MAX_REUPLOAD);
            done.store(0, Ordering::Relaxed);
            committed_parts = do_upload_and_commit(Arc::clone(&done)).await?;
        }
    } else if let Some(img) = effective_cover {
        // 仅发送封面图和说明文字 / Send cover image with caption only
        let uploaded = upload_with_progress(&client, img, Arc::clone(&done), upload_total).await?;
        if let Some(ref tmp) = converted_cover { let _ = fs::remove_file(tmp); }
        if let Some(ref tmp) = resized_cover { let _ = fs::remove_file(tmp); }
        const MAX_SEND: u32 = 5;
        const SEND_RETRY_DELAY: Duration = Duration::from_secs(30);
        let mut send_attempt = 0u32;
        loop {
            let msg = InputMessage::new()
                .photo(uploaded.clone())
                .fmt_entities(base_caption_entities.clone())
                .text(base_caption_text.clone());
            match client.send_message(peer, msg).await {
                Ok(_) => break,
                Err(e) => {
                    send_attempt += 1;
                    let err = format!("send_message (photo) failed: {}", e);
                    if send_attempt >= MAX_SEND { return Err(err); }
                    eprintln!("send failed (attempt {}/{}): {}. retrying in {:?}…",
                        send_attempt, MAX_SEND, err, SEND_RETRY_DELAY);
                    tokio::time::sleep(SEND_RETRY_DELAY).await;
                }
            }
        }
    } else {
        // 无封面图：仅发送文字消息，发送失败立即重试（不重新上传）
        // No cover image: send text message only; retry send on failure without re-uploading
        const MAX_SEND: u32 = 3;
        const SEND_RETRY_DELAY: Duration = Duration::from_secs(30);
        let mut send_attempt = 0u32;
        loop {
            let msg = InputMessage::new()
                .fmt_entities(base_caption_entities.clone())
                .text(base_caption_text.clone());
            match client.send_message(peer, msg).await {
                Ok(_) => break,
                Err(e) => {
                    send_attempt += 1;
                    let err = format!("send_message (text) failed: {}", e);
                    if send_attempt >= MAX_SEND { return Err(err); }
                    eprintln!("send failed (attempt {}/{}): {}. retrying in {:?}…",
                        send_attempt, MAX_SEND, err, SEND_RETRY_DELAY);
                    tokio::time::sleep(SEND_RETRY_DELAY).await;
                }
            }
        }
    }

    Ok(())
}

/// 模块主逻辑：读取参数、准备文件、带重试的上传发送。
/// Main module logic: read parameters, prepare files, upload and send with retries.
fn run() -> Result<(), String> {
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);
    if !input.exists() { return Err(format!("Input file not found: {}", input.display())); }

    // 读取并验证必填参数 / Read and validate required parameters
    let api_id: i32 = {
        let s = param("api_id", "");
        if s.is_empty() { return Err("api_id is required".to_string()); }
        s.parse().map_err(|_| "api_id must be a number".to_string())?
    };
    let api_hash  = param("api_hash", "");
    if api_hash.is_empty()  { return Err("api_hash is required".to_string()); }
    let bot_token = param("bot_token", "");
    if bot_token.is_empty() { return Err("bot_token is required".to_string()); }
    let chat_id: i64 = {
        let s = param("chat_id", "");
        if s.is_empty() { return Err("chat_id is required".to_string()); }
        s.parse().map_err(|_| "chat_id must be a number".to_string())?
    };
    let proxy      = param("proxy", "");
    let username   = param("username", "");
    let send_video = param_bool("send_video", true);

    // 从文件名解析元数据 / Parse metadata from filename
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("recording");
    let (model_name, timestamp) = parse_stem(stem);
    let file_size = fs::metadata(&input).map(|m| m.len()).unwrap_or(0);
    let duration  = pp_utils::video_duration(&input).unwrap_or(0.0);
    let ts_str    = if timestamp.is_empty() { "—".to_string() } else { timestamp };
    let dur_str   = format_duration(duration);
    let name_str  = input.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let size_str  = format_bytes(file_size);

    let cover = find_cover(&input);

    // Telegram 单文件大小限制为 2GB
    // Telegram single file size limit is 2GB
    const TG_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let video_parts: Vec<PathBuf> = if send_video { split_video(&input, TG_MAX_BYTES)? } else { vec![input.clone()] };
    let is_split = video_parts.len() > 1 || video_parts.first().map(|p| p != &input).unwrap_or(false);

    // 构建 Tokio 运行时并执行异步上传，最多重试 3 次
    // Build Tokio runtime and execute async upload with up to 3 retries
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime error: {}", e))?
        .block_on(async {
            const MAX_OUTER: u32 = 3;
            const RECONNECT_DELAY: Duration = Duration::from_secs(30);
            let mut attempt = 0u32;
            loop {
                let result = upload_and_send(
                    api_id, &api_hash, &bot_token, &proxy, chat_id, &username,
                    &model_name, &ts_str, &dur_str, &name_str, &size_str,
                    &input, cover.as_deref(), send_video, &video_parts,
                ).await;
                match result {
                    Ok(()) => break Ok(()),
                    Err(e) => {
                        attempt += 1;
                        if attempt >= MAX_OUTER { break Err(e); }
                        eprintln!(
                            "connection failed (attempt {}/{}): {}. rebuilding connection in {:?}…",
                            attempt, MAX_OUTER, e, RECONNECT_DELAY
                        );
                        tokio::time::sleep(RECONNECT_DELAY).await;
                    }
                }
            }
        })?;

    // 清理分割产生的临时片段文件 / Clean up temporary segment files from splitting
    if is_split {
        for part in &video_parts {
            if part != &input { let _ = fs::remove_file(part); }
        }
    }

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
    // 确保临时目录存在 / Ensure temp directory exists
    tmp_dir();
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}