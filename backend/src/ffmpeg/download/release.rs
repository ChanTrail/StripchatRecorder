//! FFmpeg 发布源元数据 / FFmpeg Release Metadata
//!
//! - [`build_platform`] / [`platform_pair`] / [`archive_file_name`]：宿主平台到
//!   FFmpeg-Builds-Full release 资产的映射（`windows-x86_64.7z`、
//!   `linux-arm64.tar.xz` 等）
//! - [`BuildMarker`] / [`read_marker`] / [`write_marker`]：`tools/ffmpeg-build.json`
//!   安装标记——只带标记的 tools 副本才是"本程序安装"，自动更新才敢替换
//! - [`parse_sha256_file`]：sha256sum 格式校验文件的解析（小写 64 位十六进制）
//!
//! - [`build_platform`] / [`platform_pair`] / [`archive_file_name`]: host-platform
//!   to FFmpeg-Builds-Full release asset mapping (`windows-x86_64.7z`,
//!   `linux-arm64.tar.xz`, etc.)
//! - [`BuildMarker`] / [`read_marker`] / [`write_marker`]: the
//!   `tools/ffmpeg-build.json` install marker — only a marked tools copy is
//!   ours, and auto-update only replaces those
//! - [`parse_sha256_file`]: sha256sum checksum-file parsing (lowercase 64-hex)

use super::MARKER_FILE_NAME;
use crate::core::error::{AppError, Result};
use crate::ffmpeg::tools_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// ─── 平台识别 / Platform detection ───────────────────────────────────────────

/// FFmpeg-Builds-Full 的构建变体后缀（拼在平台标识后）：
/// `""` 静态（默认）、`-shared` 共享库旁位自解析、`-debug` 全符号、
/// `-shared-debug` 两形态叠加。linux 的 `-legacy` 是平台轨而非变体。
///
/// FFmpeg-Builds-Full variant suffixes appended after the platform id:
/// `""` static (default), `-shared` side-by-side shared libs, `-debug`
/// full symbols, `-shared-debug` both combined. Linux `-legacy` is a
/// platform track, not a variant.
pub(super) const VARIANT_SUFFIXES: [&str; 4] = ["", "-shared", "-debug", "-shared-debug"];

/// 当前宿主机可下载的完整选项（`GET /api/ffmpeg/locations` 下发给前端渲染
/// 选择器）；不支持下载的平台返回 `None`。
///
/// The full download choices for this host (sent to the frontend by
/// `GET /api/ffmpeg/locations` to render its selectors); `None` on
/// unsupported platforms.
#[derive(Debug, Clone, Serialize)]
pub struct FFmpegDownloadOptions {
	/// 可选平台轨（linux 为默认轨 + `-legacy` 兼容轨，其余单轨；首元素即宿主默认轨）
	/// Selectable platform tracks (default + `-legacy` on linux, single-track
	/// elsewhere; the first element is the host's default track)
	pub platforms: Vec<String>,
	/// 可选变体后缀 / Selectable variant suffixes
	pub variants: Vec<String>,
}

/// 手动下载请求中的构建选择（`POST /api/ffmpeg/download` 请求体；
/// `download_ffmpeg` Tauri 命令参数）。两项均可缺省 → 自动解析
/// （保留已安装标记的平台轨/变体，否则回退宿主默认轨 + 静态）。
///
/// Optional build selection in a manual download request (body of
/// `POST /api/ffmpeg/download`; args of the `download_ffmpeg` Tauri
/// command). Both fields may be omitted → auto-resolution (keep the
/// installed marker's track/variant, else host default + static).
#[derive(Debug, Default, Clone, Deserialize)]
pub struct FFmpegDownloadChoice {
	/// 平台轨标识（如 `linux-x86_64-legacy`）；缺省自动解析
	/// Platform track id (e.g. `linux-x86_64-legacy`); auto-resolved when absent
	pub platform: Option<String>,
	/// 变体后缀（`""`/`-shared`/`-debug`/`-shared-debug`）；缺省自动解析
	/// Variant suffix (`""`/`-shared`/`-debug`/`-shared-debug`); auto-resolved when absent
	pub variant: Option<String>,
}

/// 一次解析完成的下载目标：平台轨 + 变体后缀 + 压缩包扩展名。
/// One resolved download target: platform track + variant suffix + archive ext.
pub(super) struct AssetSpec {
	/// 平台轨标识 / Platform track id
	pub(super) platform: String,
	/// 变体后缀（`""` = 静态）/ Variant suffix (`""` = static)
	pub(super) variant: String,
	/// 压缩包扩展名 / Archive extension
	pub(super) ext: &'static str,
}

impl AssetSpec {
	/// 远端压缩包文件名：`ffmpeg-latest-{platform}{variant}.{ext}`。
	/// Remote archive file name: `ffmpeg-latest-{platform}{variant}.{ext}`.
	pub(super) fn archive_name(&self) -> String {
		format!(
			"ffmpeg-latest-{}{}.{}",
			self.platform, self.variant, self.ext
		)
	}
}

/// 返回当前编译目标对应的 `(默认平台标识, 压缩包扩展名)`；不支持的平台返回
/// `None`。
/// Returns `(default platform id, archive extension)` for the current compile
/// target; `None` on unsupported platforms.
pub fn build_platform() -> Option<(&'static str, &'static str)> {
	platform_pair(std::env::consts::OS, std::env::consts::ARCH)
}

/// [`build_platform`] 的纯函数实现，便于单元测试覆盖全部平台组合。
/// Pure implementation of [`build_platform`], unit-testable for every os/arch pair.
pub(super) fn platform_pair(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
	match (os, arch) {
		("windows", "x86_64") => Some(("windows-x86_64", "7z")),
		("windows", "aarch64") => Some(("windows-arm64", "7z")),
		("linux", "x86_64") => Some(("linux-x86_64", "tar.xz")),
		("linux", "aarch64") => Some(("linux-arm64", "tar.xz")),
		("macos", "aarch64") => Some(("macos-arm64", "tar.xz")),
		_ => None,
	}
}

/// 当前宿主机可选的平台轨列表（首元素 = 宿主默认轨）；linux 有默认/`-legacy`
/// 双轨，其余平台仅默认轨。不支持下载时返回 `None`。
/// Selectable platform tracks for this host (first element = host default);
/// linux offers default + `-legacy`, other platforms only the default.
/// `None` on unsupported platforms.
fn available_platforms() -> Option<Vec<&'static str>> {
	match (std::env::consts::OS, std::env::consts::ARCH) {
		("windows", "x86_64") => Some(vec!["windows-x86_64"]),
		("windows", "aarch64") => Some(vec!["windows-arm64"]),
		("linux", "x86_64") => Some(vec!["linux-x86_64", "linux-x86_64-legacy"]),
		("linux", "aarch64") => Some(vec!["linux-arm64", "linux-arm64-legacy"]),
		("macos", "aarch64") => Some(vec!["macos-arm64"]),
		_ => None,
	}
}

/// `GET /api/ffmpeg/locations` 需要的下载可选项；不支持下载的平台返回 `None`。
/// The download choices for `GET /api/ffmpeg/locations`; `None` on
/// unsupported platforms.
pub fn download_options() -> Option<FFmpegDownloadOptions> {
	Some(FFmpegDownloadOptions {
		platforms: available_platforms()?
			.iter()
			.map(|p| p.to_string())
			.collect(),
		variants: VARIANT_SUFFIXES.iter().map(|v| v.to_string()).collect(),
	})
}

/// 解析下载目标：显式请求的平台轨/变体先经白名单校验（非法值报错），缺省时
/// 按"已安装标记 → 宿主默认"回落——手动重下与自动补装/更新由此保持同一轨，
/// 手动放置（无标记）副本的自动维护则永远取宿主默认轨。
///
/// Resolve the download target: an explicit track/variant is validated
/// against the allowlist (invalid values error out); omitted fields fall back
/// to the installed marker's values then to host defaults — so manual
/// re-downloads and automatic refills/updates stay on the same track, while
/// a markerless (manually placed) copy is always maintained on the default.
pub(super) fn resolve_asset(
	requested_platform: Option<&str>,
	requested_variant: Option<&str>,
	marker: Option<&BuildMarker>,
) -> Result<AssetSpec> {
	let avail = available_platforms().ok_or_else(|| {
		AppError::Other(format!(
			"当前平台不支持自动下载 FFmpeg（{}/{}）",
			std::env::consts::OS,
			std::env::consts::ARCH
		))
	})?;
	let (_, ext) = platform_pair(std::env::consts::OS, std::env::consts::ARCH)
		.expect("available_platforms 与 platform_pair 的平台集合一致");

	let platform = match requested_platform {
		Some(p) if avail.contains(&p) => p.to_string(),
		Some(p) => return Err(AppError::Other(format!("当前主机不支持的平台轨: {}", p))),
		None => marker
			.map(|m| m.platform.as_str())
			.filter(|p| avail.contains(p))
			.map(str::to_string)
			.unwrap_or_else(|| avail[0].to_string()),
	};
	let variant = match requested_variant {
		Some(v) if VARIANT_SUFFIXES.contains(&v) => v.to_string(),
		Some(v) => return Err(AppError::Other(format!("不支持的构建变体: {}", v))),
		None => marker
			.map(|m| m.variant.as_str())
			.filter(|v| VARIANT_SUFFIXES.contains(v))
			.map(str::to_string)
			.unwrap_or_default(),
	};
	Ok(AssetSpec {
		platform,
		variant,
		ext,
	})
}

// ─── 安装标记 / Install marker ───────────────────────────────────────────────

/// `tools/ffmpeg-build.json` 的内容：标记该 tools 副本由本程序下载维护。
/// Contents of `tools/ffmpeg-build.json`: marks the tools copy as managed by us.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BuildMarker {
	/// 已安装压缩包的 SHA-256（与远端 `.sha256` 比对判断是否有更新）
	/// SHA-256 of the installed archive (compared with the remote `.sha256` for updates)
	pub(super) archive_sha256: String,
	/// 下载时使用的平台轨标识（如 `linux-x86_64-legacy`）
	/// Platform track id used for the download (e.g. `linux-x86_64-legacy`)
	pub(super) platform: String,
	/// 下载时使用的变体后缀（`""` = 静态；旧标记无此字段，按静态处理）
	/// Variant suffix used for the download (`""` = static; absent in old
	/// markers, treated as static)
	#[serde(default)]
	pub(super) variant: String,
	/// 本次安装换入 tools/ 的文件名清单（旧标记无此字段——按三件套处理）；
	/// 换轨/换形态时用于清掉上一版遗留的旁位库等受管文件
	/// File names swapped into tools/ by this install (absent in old markers —
	/// treated as the three binaries); used to purge leftover managed files
	/// (e.g. side-by-side libs) when switching track/variant
	#[serde(default)]
	pub(super) files: Vec<String>,
	/// `ffmpeg -version` 首行 / First line of `ffmpeg -version`
	pub(super) version: String,
	/// 安装完成时间（RFC 3339）/ Install completion time (RFC 3339)
	pub(super) installed_at: String,
}

/// 读取安装标记；文件缺失或解析失败均返回 `None`（视为"非本程序安装"）。
/// Read the install marker; missing or unparseable both yield `None`
/// (treated as "not installed by us").
pub(super) fn read_marker() -> Option<BuildMarker> {
	let content = fs::read_to_string(tools_dir().join(MARKER_FILE_NAME)).ok()?;
	serde_json::from_str(&content).ok()
}

/// 写入安装标记。`files` 为本次换入 tools/ 的全部受管文件名。
/// Write the install marker. `files` lists every managed file name swapped
/// into tools/ this time.
pub(super) fn write_marker(
	tools: &Path,
	sha256: &str,
	platform: &str,
	variant: &str,
	files: &[String],
	version: &str,
) -> Result<()> {
	let marker = BuildMarker {
		archive_sha256: sha256.to_string(),
		platform: platform.to_string(),
		variant: variant.to_string(),
		files: files.to_vec(),
		version: version.to_string(),
		installed_at: chrono::Local::now().to_rfc3339(),
	};
	let json = serde_json::to_string_pretty(&marker)?;
	fs::write(tools.join(MARKER_FILE_NAME), json)?;
	Ok(())
}

// ─── 校验文件 / Checksum file ────────────────────────────────────────────────

/// 解析 sha256sum 格式的校验文件内容，返回小写的哈希值。
/// 取首个空白分隔的字段，必须是 64 位十六进制。
///
/// Parse sha256sum-format checksum content, returning the lowercased hash.
/// Takes the first whitespace-separated token, which must be 64 hex digits.
pub(super) fn parse_sha256_file(content: &str) -> Option<String> {
	let token = content.split_whitespace().next()?;
	if token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
		Some(token.to_ascii_lowercase())
	} else {
		None
	}
}
