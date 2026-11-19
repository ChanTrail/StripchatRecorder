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
use crate::core::error::Result;
use crate::ffmpeg::tools_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// ─── 平台识别 / Platform detection ───────────────────────────────────────────

/// 返回当前编译目标对应的 `(平台标识, 压缩包扩展名)`；不支持的平台返回 `None`。
/// Returns `(platform id, archive extension)` for the current compile target;
/// `None` on unsupported platforms.
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

/// 远端压缩包文件名：`ffmpeg-latest-{platform}.{ext}`
/// （同名 `.sha256` 文件由调用方拼接）。
///
/// Remote archive file name: `ffmpeg-latest-{platform}.{ext}`
/// (callers append `.sha256` for the checksum file).
pub(super) fn archive_file_name(platform: &str, ext: &str) -> String {
    format!("ffmpeg-latest-{}.{}", platform, ext)
}

// ─── 安装标记 / Install marker ───────────────────────────────────────────────

/// `tools/ffmpeg-build.json` 的内容：标记该 tools 副本由本程序下载维护。
/// Contents of `tools/ffmpeg-build.json`: marks the tools copy as managed by us.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BuildMarker {
    /// 已安装压缩包的 SHA-256（与远端 `.sha256` 比对判断是否有更新）
    /// SHA-256 of the installed archive (compared with the remote `.sha256` for updates)
    pub(super) archive_sha256: String,
    /// 下载时使用的平台标识 / Platform id used for the download
    pub(super) platform: String,
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

/// 写入安装标记。
/// Write the install marker.
pub(super) fn write_marker(
    tools: &Path,
    sha256: &str,
    platform: &str,
    version: &str,
) -> Result<()> {
    let marker = BuildMarker {
        archive_sha256: sha256.to_string(),
        platform: platform.to_string(),
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
