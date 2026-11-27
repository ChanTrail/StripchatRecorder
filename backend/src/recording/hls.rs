//! HLS 播放列表解析与 Mouflon 解密 / HLS Playlist Parsing and Mouflon Decryption
//!
//! 解析 Stripchat 的 HLS m3u8 播放列表（媒体播放列表和主播放列表两种），提取分片 URL、
//! fMP4 初始化段 URL，以及主播放列表中带宽最高的变体流 URL。
//! 支持 Mouflon 加密系统：用 SHA-256(pdkey) 或直接给定的 keystream 对分片 URL 进行 XOR 解密。
//!
//! 本模块只负责纯文本解析，不涉及任何网络请求——播放列表文本的获取（含多 CDN 竞速）
//! 由 `platform::stripchat::StripchatApi` 负责，解析后再调用本模块的函数。
//!
//! Parses Stripchat's HLS m3u8 playlists (both media playlists and master playlists),
//! extracting segment URLs, fMP4 init segment URLs, and the highest-bandwidth variant
//! stream URL from a master playlist.
//! Supports the Mouflon encryption system: XOR-decrypts segment URLs using SHA-256(pdkey) or a
//! directly given keystream.
//!
//! This module performs pure text parsing only — no network requests. Fetching the
//! playlist text (including multi-CDN racing) is the responsibility of
//! `platform::stripchat::StripchatApi`, which calls into this module's functions
//! once the text is retrieved.

use crate::core::error::{AppError, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::LazyLock;

/// 用于从加密 URL 中提取加密字符串和序号的正则表达式。
/// Regex for extracting the encrypted string and sequence number from an encrypted URL.
static SEGMENT_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"_([^_]+)_(\d+(?:_part\d+)?)\.mp4(?:[?#].*)?").unwrap());

/// keystream 的最短字节数。解密时按位置循环使用密钥流，短于加密串的 keystream 会解出乱码，
/// 参考脚本 `mouflon_download.py` 同样要求至少 16 字节才足以覆盖分片文件名里的加密串。
///
/// Minimum keystream length in bytes. Decryption cycles the key stream by position, so a
/// keystream shorter than the encrypted token decrypts to garbage; the reference script
/// `mouflon_download.py` likewise requires at least 16 bytes to cover the token in a segment name.
pub const MIN_KEYSTREAM_BYTES: usize = 16;

/// 把 keystream 的十六进制文本（如 `b8a651203a2caeca73eca302e161e30e`）解析为字节，
/// 首尾空白会被忽略；不是合法十六进制、长度为奇数或短于 [`MIN_KEYSTREAM_BYTES`] 时返回原因。
///
/// Parse a keystream's hexadecimal text (e.g. `b8a651203a2caeca73eca302e161e30e`) into bytes,
/// ignoring surrounding whitespace; returns the reason when it is not valid hex, has an odd
/// length, or is shorter than [`MIN_KEYSTREAM_BYTES`].
pub fn parse_keystream_hex(text: &str) -> std::result::Result<Vec<u8>, String> {
	let text = text.trim();
	if !text.len().is_multiple_of(2) {
		return Err("hex length must be even".to_string());
	}
	let mut bytes = Vec::with_capacity(text.len() / 2);
	for pair in text.as_bytes().chunks(2) {
		let pair = std::str::from_utf8(pair).map_err(|_| "not valid hex".to_string())?;
		bytes.push(u8::from_str_radix(pair, 16).map_err(|_| "not valid hex".to_string())?);
	}
	if bytes.len() < MIN_KEYSTREAM_BYTES {
		return Err(format!(
			"keystream must be at least {} bytes, got {}",
			MIN_KEYSTREAM_BYTES,
			bytes.len()
		));
	}
	Ok(bytes)
}

/// 把 keystream 字节编码为小写十六进制文本（[`parse_keystream_hex`] 的逆操作）。
/// Encode keystream bytes as lowercase hexadecimal text (the inverse of [`parse_keystream_hex`]).
pub fn encode_keystream_hex(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 解密 Mouflon 加密分片 URL 所需的全部密钥材料，按 pkey 索引。
///
/// 一个 pkey 可以有两种形式的密钥，解密时取其一：
/// - `pdkey`：解密密钥本身，密钥流为 `SHA-256(pdkey)`（32 字节，完整）；
/// - `keystream`：已经算好的密钥流（如同步服务从已知明文恢复出来的），无需知道 pdkey。
///
/// 两者都有时优先用 pdkey：恢复出来的 keystream 可能只是前缀。
///
/// All the key material needed to decrypt Mouflon-encrypted segment URLs, indexed by pkey.
///
/// A pkey may carry either form of key; decryption uses one of them:
/// - `pdkey`: the decryption key itself, whose key stream is `SHA-256(pdkey)` (32 bytes, complete);
/// - `keystream`: an already-computed key stream (e.g. recovered by the sync service from known
///   plaintext), usable without knowing the pdkey.
///
/// When both exist the pdkey wins, since a recovered keystream may be only a prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MouflonKeys {
	/// pkey -> pdkey
	pub pdkeys: HashMap<String, String>,
	/// pkey -> keystream 字节 / pkey -> keystream bytes
	pub keystreams: HashMap<String, Vec<u8>>,
}

impl MouflonKeys {
	/// 该 pkey 是否有可用的密钥材料（pdkey 或 keystream）。
	/// Whether this pkey has usable key material (a pdkey or a keystream).
	pub fn contains(&self, pkey: &str) -> bool {
		self.pdkeys.contains_key(pkey) || self.keystreams.get(pkey).is_some_and(|ks| !ks.is_empty())
	}

	/// 该 pkey 用于解密的密钥流：有 pdkey 时为 `SHA-256(pdkey)`，否则为 keystream；都没有返回 `None`。
	/// The key stream used to decrypt for this pkey: `SHA-256(pdkey)` when a pdkey exists,
	/// otherwise the keystream; `None` when neither exists.
	pub fn key_stream(&self, pkey: &str) -> Option<Vec<u8>> {
		if let Some(pdkey) = self.pdkeys.get(pkey) {
			return Some(Sha256::digest(pdkey.as_bytes()).to_vec());
		}
		self.keystreams
			.get(pkey)
			.filter(|ks| !ks.is_empty())
			.cloned()
	}
}

/// HLS 分片信息 / HLS segment information
#[derive(Debug, Clone)]
pub struct HlsSegment {
	/// 分片的完整 URL（已解密）/ Full segment URL (decrypted)
	pub url: String,
	/// 分片序号（用于去重）/ Segment sequence number (for deduplication)
	pub sequence: u32,
	/// 分片时长（秒，来自 #EXTINF 标签，未知时为 0）/ Segment duration in seconds (from #EXTINF tag, 0 if unknown)
	pub duration_secs: f64,
	/// 该分片对应的 fMP4 初始化段 URL（来自 EXT-X-MAP 标签，None 表示与上一分片相同）。
	/// 每个 segment 精确携带自身对应的 init URL，使录制器能在分片粒度上精确切换 init，
	/// 避免 mid-stream init 变更被遗漏。
	///
	/// The fMP4 init segment URL for this segment (from the EXT-X-MAP tag; None means
	/// same as the previous segment). Each segment carries its own init URL so the
	/// recorder can switch init at exact segment boundaries, preventing mid-stream
	/// init changes from being missed.
	pub init_url: Option<String>,
}

/// 解析 HLS m3u8 播放列表，返回分片列表。每个分片在 `init_url` 字段中携带
/// 自己对应的 fMP4 初始化段 URL（仅在该分片处发生 EXT-X-MAP 变更时才为 Some；
/// None 表示沿用前一分片的 init segment）。
///
/// Parse an HLS m3u8 playlist and return the segment list. Each segment carries the
/// fMP4 init segment URL in its `init_url` field — only set to Some when an EXT-X-MAP
/// change occurs at that segment boundary; None means the previous init segment
/// continues to apply.
///
/// # 参数 / Parameters
/// - `playlist`: m3u8 文本内容 / m3u8 text content
/// - `url_prefix`: 用于将相对路径转为绝对 URL 的前缀 / Prefix for converting relative paths to absolute URLs
/// - `mouflon_keys`: Mouflon 解密密钥（pdkey 与 keystream，按 pkey 索引）/ Mouflon decryption keys (pdkeys and keystreams, indexed by pkey)
pub fn parse_playlist(
	playlist: &str,
	url_prefix: &str,
	mouflon_keys: &MouflonKeys,
) -> Result<Vec<HlsSegment>> {
	let mut segments = Vec::new();
	// 解析过程中追踪的"当前" EXT-X-MAP URL（按行推进，在 segment 入列时快照）
	// Current EXT-X-MAP URL tracked during parsing (advanced per line, snapshotted at segment push)
	let mut current_init_url: Option<String> = None;
	// 上一个推入 segments 的分片所用的 init URL（用于判断是否发生了变更）
	// The init URL used by the last pushed segment (for change detection)
	let mut last_pushed_init_url: Option<String> = None;
	// 当前 pkey 对应的密钥流（没有该 pkey 的密钥时为 None）
	// Key stream for the current pkey (None when there is no key for it)
	let mut current_key_stream: Option<Vec<u8>> = None;
	let mut pending_duration: f64 = 0.0;

	let lines: Vec<&str> = playlist.lines().collect();

	for (i, line) in lines.iter().enumerate() {
		// 解析 Mouflon 加密标签，获取当前 pkey 对应的解密密钥
		// Parse Mouflon encryption tag to get the decryption key for the current pkey
		if line.contains("#EXT-X-MOUFLON:PSCH") {
			let parts: Vec<&str> = line.split(':').collect();
			if parts.len() >= 4 {
				let pkey = parts[3];
				current_key_stream = mouflon_keys.key_stream(pkey);
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

		// 解析 fMP4 初始化段 URL（EXT-X-MAP）——按行推进，后续 segment 沿用直到下次变更
		// Parse fMP4 init segment URL (EXT-X-MAP) — advanced per-line; segments inherit
		// the current value until the next EXT-X-MAP tag overrides it.
		if line.contains("EXT-X-MAP:URI")
			&& let Some(start) = line.find('"')
			&& let Some(end) = line[start + 1..].find('"')
		{
			let header_path = &line[start + 1..start + 1 + end];
			current_init_url = Some(if header_path.starts_with("http") {
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
			if let Some(ref key_stream) = current_key_stream {
				decrypt_segment_url(&encoded_url, key_stream).unwrap_or(encoded_url)
			} else {
				encoded_url
			}
		} else if line.starts_with("http") {
			line.to_string()
		} else {
			format!("{}/{}", url_prefix, line)
		};

		let sequence = extract_sequence(&url).unwrap_or(segments.len() as u32);

		// 仅在 init URL 发生变更时才填充 Some，否则为 None（沿用缓存，不重复下载）
		// Only set Some when the init URL has changed; None means reuse the cached init.
		let init_url_changed = current_init_url != last_pushed_init_url;
		let seg_init_url = if init_url_changed {
			last_pushed_init_url.clone_from(&current_init_url);
			current_init_url.clone()
		} else {
			None
		};

		segments.push(HlsSegment {
			url,
			sequence,
			duration_secs: pending_duration,
			init_url: seg_init_url,
		});
		pending_duration = 0.0;
	}

	Ok(segments)
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
			&& let Some((scheme, key)) = rest.split_once(':')
		{
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
				variants.push(Variant {
					url: line.to_string(),
					bandwidth,
					height,
				});
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

/// 判断 master playlist 是否提供指定编码的变体流——逐个读取 `#EXT-X-STREAM-INF`
/// 的 `CODECS` 属性，与编码族的 RFC 6381 前缀比对：
/// `AV1`→`av01`、`H265`→`hvc1`/`hev1`、`h264`→`avc1`/`avc2`/`avc3`。
///
/// 返回 `false` 的唯一情形是 playlist 里至少一个变体声明了 CODECS 且没有任何
/// 变体声明该编码族——即确定服务端没给所求编码。playlist 完全没有 CODECS
/// 声明（旧格式）时无法判断，返回 `true` 以免误回退。
///
/// Whether a master playlist offers variants of the requested codec — reads the
/// `CODECS` attribute of every `#EXT-X-STREAM-INF` and matches codec-family
/// RFC 6381 prefixes: `AV1`→`av01`, `H265`→`hvc1`/`hev1`,
/// `h264`→`avc1`/`avc2`/`avc3`.
///
/// Returns `false` only when at least one variant declares CODECS and none
/// declares the requested family — i.e. the server positively did not honor
/// the codec. A playlist with no CODECS declarations at all (legacy format)
/// cannot be verified and returns `true` to avoid a false fallback.
pub fn master_playlist_offers_codec(playlist: &str, codec: &str) -> bool {
	let prefixes: &[&str] = if codec.eq_ignore_ascii_case("AV1") {
		&["av01"]
	} else if codec.eq_ignore_ascii_case("H265") {
		&["hvc1", "hev1"]
	} else {
		&["avc1", "avc2", "avc3"]
	};

	let mut declared_any = false;
	let normalized = playlist.replace("\r\n", "\n").replace('\r', "\n");
	for line in normalized.split('\n').map(str::trim) {
		let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") else {
			continue;
		};
		// CODECS 属性值本身含逗号（视频+音频 token），不能按逗号切属性串；
		// 定位 `CODECS="` 后取到下一个引号为止 / The CODECS value itself
		// contains commas (video + audio tokens), so the attribute string
		// cannot be split on commas — locate `CODECS="` and take up to the
		// next quote
		let Some(codecs_attr) = attrs
			.find("CODECS=\"")
			.and_then(|start| attrs[start + "CODECS=\"".len()..].split('"').next())
		else {
			continue;
		};
		declared_any = true;
		if codecs_attr
			.split(',')
			.map(str::trim)
			.any(|tok| prefixes.iter().any(|p| tok.starts_with(p)))
		{
			return true;
		}
	}
	!declared_any
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

/// 使用密钥流对 Mouflon 加密的分片 URL 进行 XOR 解密。
/// Decrypt a Mouflon-encrypted segment URL by XOR-ing with a key stream.
///
/// 解密流程：提取加密字符串 → Base64 解码（反转后补齐）→ 与密钥流按位置循环异或 → 替换回 URL。
/// 密钥流来自 `SHA-256(pdkey)` 或直接给定的 keystream（见 [`MouflonKeys::key_stream`]）。
/// 解出的内容不是合法 UTF-8 时返回错误（密钥不对或 keystream 太短），而不是得到一个乱码 URL。
///
/// Decryption flow: extract encrypted string → Base64 decode (reversed + padded) → XOR with the
/// key stream, cycled by position → replace in URL. The key stream comes from `SHA-256(pdkey)` or
/// a directly given keystream (see [`MouflonKeys::key_stream`]). Returns an error when the
/// result is not valid UTF-8 (wrong key or too-short keystream) instead of producing a garbled URL.
fn decrypt_segment_url(encoded_url: &str, key_stream: &[u8]) -> Result<String> {
	if key_stream.is_empty() {
		return Err(AppError::Other("Empty Mouflon key stream".to_string()));
	}

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

	let decrypted: Vec<u8> = encrypted_bytes
		.iter()
		.enumerate()
		.map(|(i, b)| b ^ key_stream[i % key_stream.len()])
		.collect();

	let decrypted_str = String::from_utf8(decrypted)
		.map_err(|_| AppError::Other("Decrypted Mouflon token is not valid UTF-8".to_string()))?;
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

#[cfg(test)]
mod tests {
	use super::master_playlist_offers_codec;

	fn master(codecs: &str) -> String {
		format!(
			"#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720,CODECS=\"{}\"\n\
             https://cdn.example/v.m3u8\n",
			codecs
		)
	}

	/// CODECS 前缀匹配与回退判定 / CODECS prefix matching and the fallback verdict
	#[test]
	fn codec_offer_detection() {
		// 视频+音频双 token（CODECS 值内含逗号，必须整体取值）/ video+audio
		// tokens — the CODECS value contains commas and must be read whole
		assert!(master_playlist_offers_codec(
			&master("av01.0.08M.08,mp4a.40.2"),
			"AV1"
		));
		assert!(master_playlist_offers_codec(
			&master("hvc1.1.6.L120.90,mp4a.40.2"),
			"H265"
		));
		assert!(master_playlist_offers_codec(
			&master("avc1.640028,mp4a.40.2"),
			"h264"
		));
		// 编码族不匹配 → false（触发向下回退）/ family mismatch → false
		// (triggers the downward fallback)
		assert!(!master_playlist_offers_codec(
			&master("avc1.640028,mp4a.40.2"),
			"AV1"
		));
		assert!(!master_playlist_offers_codec(
			&master("av01.0.08M.08,mp4a.40.2"),
			"h264"
		));
		// 无 CODECS 声明 → 无法判断，接受 / no CODECS declaration →
		// unverifiable, accept
		let legacy = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=3000000\nhttps://cdn.example/v.m3u8\n";
		assert!(master_playlist_offers_codec(legacy, "AV1"));
		// 仅音频 token 不算命中 / an audio-only token doesn't count
		assert!(!master_playlist_offers_codec(&master("mp4a.40.2"), "h264"));
	}
}
