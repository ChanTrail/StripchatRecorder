//! HLS 播放列表解析与 Mouflon 解密 / HLS Playlist Parsing and Mouflon Decryption
//!
//! 解析 Stripchat 的 HLS m3u8 播放列表（媒体播放列表和主播放列表两种），提取分片 URL、
//! fMP4 初始化段 URL，以及主播放列表中带宽最高的变体流 URL。
//! 支持 Mouflon 加密系统：通过 SHA-256 密钥对分片 URL 进行 XOR 解密。
//!
//! 本模块只负责纯文本解析，不涉及任何网络请求——播放列表文本的获取（含多 CDN 竞速）
//! 由 `platform::stripchat::StripchatApi` 负责，解析后再调用本模块的函数。
//!
//! Parses Stripchat's HLS m3u8 playlists (both media playlists and master playlists),
//! extracting segment URLs, fMP4 init segment URLs, and the highest-bandwidth variant
//! stream URL from a master playlist.
//! Supports the Mouflon encryption system: XOR-decrypts segment URLs using SHA-256 keys.
//!
//! This module performs pure text parsing only — no network requests. Fetching the
//! playlist text (including multi-CDN racing) is the responsibility of
//! `platform::stripchat::StripchatApi`, which calls into this module's functions
//! once the text is retrieved.

use crate::core::error::{AppError, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::LazyLock;

/// 用于从加密 URL 中提取加密字符串和序号的正则表达式。
/// Regex for extracting the encrypted string and sequence number from an encrypted URL.
static SEGMENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"_([^_]+)_(\d+(?:_part\d+)?)\.mp4(?:[?#].*)?").unwrap());

/// HLS 分片信息 / HLS segment information
#[derive(Debug, Clone)]
pub struct HlsSegment {
    /// 分片的完整 URL（已解密）/ Full segment URL (decrypted)
    pub url: String,
    /// 分片序号（用于去重）/ Segment sequence number (for deduplication)
    pub sequence: u32,
    /// 分片时长（秒，来自 #EXTINF 标签，未知时为 0）/ Segment duration in seconds (from #EXTINF tag, 0 if unknown)
    pub duration_secs: f64,
}

/// 解析 HLS m3u8 播放列表，返回分片列表和 fMP4 初始化段 URL。
/// Parse an HLS m3u8 playlist, returning the segment list and fMP4 init segment URL.
///
/// # 参数 / Parameters
/// - `playlist`: m3u8 文本内容 / m3u8 text content
/// - `url_prefix`: 用于将相对路径转为绝对 URL 的前缀 / Prefix for converting relative paths to absolute URLs
/// - `mouflon_keys`: Mouflon 解密密钥表（pkey -> pdkey）/ Mouflon decryption key map (pkey -> pdkey)
///
/// # 返回值 / Returns
/// `(segments, init_url)` 元组 / Tuple of `(segments, init_url)`
pub fn parse_playlist(
    playlist: &str,
    url_prefix: &str,
    mouflon_keys: &HashMap<String, String>,
) -> Result<(Vec<HlsSegment>, Option<String>)> {
    let mut segments = Vec::new();
    let mut mp4_header_url = None;
    let mut current_pkey: Option<&str> = None;
    let mut pending_duration: f64 = 0.0;

    let lines: Vec<&str> = playlist.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        // 解析 Mouflon 加密标签，获取当前 pkey 对应的解密密钥
        // Parse Mouflon encryption tag to get the decryption key for the current pkey
        if line.contains("#EXT-X-MOUFLON:PSCH") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 {
                let pkey = parts[3];
                current_pkey = mouflon_keys.get(pkey).map(|s| s.as_str());
            }
        }

        // 解析 EXTINF 时长（格式：#EXTINF:6.00000,）
        // Parse EXTINF duration (format: #EXTINF:6.00000,)
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending_duration = rest
                .split(',')
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
        }

        // 解析 fMP4 初始化段 URL（EXT-X-MAP）
        // Parse fMP4 init segment URL (EXT-X-MAP)
        if line.contains("EXT-X-MAP:URI")
            && let Some(start) = line.find('"')
            && let Some(end) = line[start + 1..].find('"')
        {
            let header_path = &line[start + 1..start + 1 + end];
            mp4_header_url = Some(if header_path.starts_with("http") {
                header_path.to_string()
            } else {
                format!("{}/{}", url_prefix, header_path)
            });
        }

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // 检查前一行是否为 Mouflon URI 标签（加密分片的实际 URL 在标签中）
        // Check if the previous line is a Mouflon URI tag (actual URL for encrypted segments is in the tag)
        let mouflon_uri_line = if i > 0 && lines[i - 1].starts_with("#EXT-X-MOUFLON:URI:") {
            Some(lines[i - 1])
        } else {
            None
        };

        let url = if let Some(mouflon_line) = mouflon_uri_line {
            let raw_url = mouflon_line.trim_start_matches("#EXT-X-MOUFLON:URI:");
            let encoded_url = if raw_url.starts_with("https://") {
                raw_url.to_string()
            } else if raw_url.starts_with("//") {
                format!("https:{}", raw_url)
            } else {
                format!("https://{}", raw_url)
            };

            // 若有解密密钥则解密 URL，否则直接使用
            // Decrypt URL if key is available, otherwise use as-is
            if let Some(key) = current_pkey {
                decrypt_segment_url(&encoded_url, key).unwrap_or(encoded_url)
            } else {
                encoded_url
            }
        } else if line.starts_with("http") {
            line.to_string()
        } else {
            format!("{}/{}", url_prefix, line)
        };

        let sequence = extract_sequence(&url).unwrap_or(segments.len() as u32);
        segments.push(HlsSegment { url, sequence, duration_secs: pending_duration });
        pending_duration = 0.0;
    }

    Ok((segments, mp4_header_url))
}

/// 从主播放列表（master playlist）文本中按配置选择变体流 URL，
/// 以及所有 Mouflon PSCH 参数对。
///
/// - `preferred_resolution == 0`：按带宽选最高画质（原有行为）。
/// - `preferred_resolution > 0`：选最接近目标高度的流；`prefers_higher` 控制
///   目标不可用时优先向上还是向下回退。
///
/// Parse the variant stream URL from a master playlist using the configured resolution
/// preference, along with all Mouflon PSCH parameter pairs.
///
/// - `preferred_resolution == 0`: pick highest bandwidth (original behaviour).
/// - `preferred_resolution > 0`: pick the stream closest to the target height;
///   `prefers_higher` controls fallback direction when the exact target is unavailable.
pub fn parse_master_playlist(
    playlist: &str,
    preferred_resolution: u32,
    prefers_higher: bool,
) -> Option<(String, Vec<(String, String)>)> {
    struct Variant {
        url: String,
        bandwidth: u64,
        height: Option<u32>,
    }

    // 先把 \r\n 统一成 \n，再按 \n 分割
    let normalized = playlist.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').map(|l| l.trim()).collect();

    // 收集所有 Mouflon PSCH 参数对 (psch, pkey)
    let mut mouflon_pairs: Vec<(String, String)> = Vec::new();
    for &line in &lines {
        if let Some(rest) = line.strip_prefix("#EXT-X-MOUFLON:PSCH:")
            && let Some((scheme, key)) = rest.split_once(':') {
            mouflon_pairs.push((scheme.to_string(), key.to_string()));
        }
    }

    // 解析所有变体流
    let mut variants: Vec<Variant> = Vec::new();
    let mut pending: Option<(u64, Option<u32>)> = None;

    for &line in &lines {
        if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let bandwidth = attrs
                .split(',')
                .find_map(|seg| seg.trim().strip_prefix("BANDWIDTH="))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let height = attrs
                .split(',')
                .find_map(|seg| seg.trim().strip_prefix("RESOLUTION="))
                .and_then(|v| v.split_once('x'))
                .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
                .map(|(w, h)| w.min(h));
            pending = Some((bandwidth, height));
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some((bandwidth, height)) = pending.take() {
                variants.push(Variant { url: line.to_string(), bandwidth, height });
            }
        } else {
            pending = None;
        }
    }

    let selected = if preferred_resolution == 0 {
        variants.iter().max_by_key(|v| v.bandwidth)
    } else {
        let at_or_below = || {
            variants
                .iter()
                .filter(|v| v.height.is_some_and(|h| h <= preferred_resolution))
                .max_by_key(|v| (v.height.unwrap_or(0), v.bandwidth))
        };
        let at_or_above = || {
            variants
                .iter()
                .filter(|v| v.height.is_some_and(|h| h >= preferred_resolution))
                .min_by(|a, b| {
                    a.height
                        .cmp(&b.height)
                        .then_with(|| b.bandwidth.cmp(&a.bandwidth))
                })
        };

        let chosen = if prefers_higher {
            at_or_above().or_else(at_or_below)
        } else {
            at_or_below().or_else(at_or_above)
        };

        // 若所有流均缺少 RESOLUTION 属性，退回到带宽最高的流
        // If no stream has a RESOLUTION attribute, fall back to highest bandwidth
        chosen.or_else(|| variants.iter().max_by_key(|v| v.bandwidth))
    };

    selected.map(|v| (v.url.clone(), mouflon_pairs))
}

/// 从完整 URL 中提取 URL 前缀（去掉最后一个路径段）。
/// Extract the URL prefix from a full URL (removes the last path segment).
pub fn get_url_prefix(url: &str) -> String {
    let parts: Vec<&str> = url.split('/').collect();
    if parts.len() > 1 {
        parts[..parts.len() - 1].join("/")
    } else {
        url.to_string()
    }
}

/// 使用 SHA-256 密钥对 Mouflon 加密的分片 URL 进行 XOR 解密。
/// Decrypt a Mouflon-encrypted segment URL using XOR with a SHA-256 key.
///
/// 解密流程：提取加密字符串 → Base64 解码（反转后补齐）→ SHA-256(key) XOR 解密 → 替换回 URL
/// Decryption flow: extract encrypted string → Base64 decode (reversed + padded) → SHA-256(key) XOR decrypt → replace in URL
fn decrypt_segment_url(encoded_url: &str, key: &str) -> Result<String> {
    let captures = SEGMENT_REGEX
        .captures(encoded_url)
        .ok_or_else(|| AppError::Other("Cannot parse encrypted URL".to_string()))?;

    let encrypted_str = captures.get(1).unwrap().as_str();

    // 反转字符串并补齐 Base64 填充 / Reverse string and pad for Base64
    let mut reversed: String = encrypted_str.chars().rev().collect();
    while !reversed.len().is_multiple_of(4) {
        reversed.push('=');
    }

    let encrypted_bytes = STANDARD
        .decode(&reversed)
        .map_err(|e| AppError::Other(format!("Base64 decode error: {}", e)))?;

    // 使用 SHA-256(key) 作为 XOR 密钥流 / Use SHA-256(key) as XOR keystream
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let key_bytes = hasher.finalize();

    let decrypted: Vec<u8> = encrypted_bytes
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key_bytes[i % key_bytes.len()])
        .collect();

    let decrypted_str = String::from_utf8_lossy(&decrypted);
    Ok(encoded_url.replace(encrypted_str, &decrypted_str))
}

/// 从分片 URL 的文件名中提取序号（最后一个 `_` 后、`.` 前的数字）。
/// Extract the sequence number from a segment URL's filename (number after the last `_`, before `.`).
fn extract_sequence(url: &str) -> Option<u32> {
    let filename = url.split('/').next_back()?;
    let parts: Vec<&str> = filename.split('_').collect();
    let last = parts.last()?;
    let num_str = last.split('.').next()?;
    num_str.parse().ok()
}
