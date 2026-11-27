//! FFmpeg 两位置组件探测 / FFmpeg Two-Location Component Probing
//!
//! system（PATH）与 tools 两个位置中 ffmpeg/ffprobe/ffplay 的存在性与
//! `-version` 版本探测，供"FFmpeg 首选项"向导与设置页展示：
//! - [`FFmpegLocationProbe`] / [`FFmpegLocations`]：探测结果结构
//! - [`ffmpeg_locations`]：一次探测两个位置（对每个找到的组件跑一次
//!   `-version`）；会拉起子进程，调用方应使用 `spawn_blocking` 包裹
//! - [`find_on_path`]：沿 PATH 查找可执行文件（首选项的存在性门控也用它）
//!
//! Presence and `-version` probing of ffmpeg/ffprobe/ffplay on the system PATH
//! and in the tools directory, shown by the FFmpeg preference wizard and the
//! settings page:
//! - [`FFmpegLocationProbe`] / [`FFmpegLocations`]: probe result structures
//! - [`ffmpeg_locations`]: probe both locations at once (running `-version`
//!   on each found component); spawns child processes — callers should wrap
//!   it in `spawn_blocking`
//! - [`find_on_path`]: PATH lookup for an executable (also used by the
//!   preference existence gate)

use super::download::{FFmpegBuildInfo, FFmpegDownloadOptions};
use super::paths::{exe_name, tools_dir};
use crate::core::no_window::NoWindowExt;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 单个位置的 ffmpeg / ffprobe / ffplay 探测结果。
/// Probe result for one location.
#[derive(Debug, Clone, Serialize)]
pub struct FFmpegLocationProbe {
	/// 是否找到 ffmpeg / Whether ffmpeg was found
	pub ffmpeg: bool,
	/// 是否找到 ffprobe / Whether ffprobe was found
	pub ffprobe: bool,
	/// 是否找到 ffplay / Whether ffplay was found
	pub ffplay: bool,
	/// ffmpeg 的完整路径（未找到时为 `null`）/ Full path to ffmpeg (`null` when not found)
	pub ffmpeg_path: Option<String>,
	/// ffprobe 的完整路径（未找到时为 `null`）/ Full path to ffprobe (`null` when not found)
	pub ffprobe_path: Option<String>,
	/// ffplay 的完整路径（未找到时为 `null`）/ Full path to ffplay (`null` when not found)
	pub ffplay_path: Option<String>,
	/// `ffmpeg -version` 首行的版本号字段（未找到或运行失败时为 `null`）
	/// Version token from the first `ffmpeg -version` line (`null` when absent or won't run)
	pub ffmpeg_version: Option<String>,
	/// `ffprobe -version` 首行的版本号字段（未找到或运行失败时为 `null`）
	/// Version token from the first `ffprobe -version` line (`null` when absent or won't run)
	pub ffprobe_version: Option<String>,
	/// `ffplay -version` 首行的版本号字段（未找到或运行失败时为 `null`）
	/// Version token from the first `ffplay -version` line (`null` when absent or won't run)
	pub ffplay_version: Option<String>,
}

/// system（PATH）与 tools 两个位置的探测结果汇总。
/// Combined probe results for the system PATH and the tools directory.
#[derive(Debug, Clone, Serialize)]
pub struct FFmpegLocations {
	/// tools 目录的绝对路径（供前端展示）/ Absolute tools directory path (for display)
	pub tools_dir: String,
	/// 系统 PATH 中的探测结果 / Probe result on the system PATH
	pub system: FFmpegLocationProbe,
	/// tools 目录中的探测结果 / Probe result in the tools directory
	pub tools: FFmpegLocationProbe,
	/// 当前主机可选的下载构建（平台轨 × 变体）；不支持下载的平台为 `null`——
	/// 前端据此决定要不要渲染构建选择器
	/// Downloadable build choices for this host (platform tracks × variants);
	/// `null` on platforms without downloads — the frontend uses it to decide
	/// whether to render the build selectors
	pub download_options: Option<FFmpegDownloadOptions>,
	/// tools 副本的已安装构建信息（平台轨 × 变体后缀）；仅本程序下载的
	/// 副本有标记，手动放置/标记损坏时为 `null`——前端据此在版本号旁
	/// 追加构建形态展示
	/// Build info of the installed tools copy (platform track × variant
	/// suffix); `null` for manually placed copies or a corrupt marker —
	/// the frontend uses it to annotate the version with the build shape
	pub tools_build: Option<FFmpegBuildInfo>,
}

/// 沿 PATH 查找指定可执行文件，返回第一个命中的完整路径。
/// Search PATH for an executable, returning the first matching full path.
pub(crate) fn find_on_path(base: &str) -> Option<PathBuf> {
	let name = exe_name(base);
	let paths = std::env::var_os("PATH")?;
	std::env::split_paths(&paths)
		.map(|dir| dir.join(&name))
		.find(|p| p.is_file())
}

/// 探测 system（PATH）与 tools 目录中的 ffmpeg/ffprobe/ffplay 存在情况，
/// 并对每个位置找到的组件各运行一次 `-version` 取版本号，供前端
/// "FFmpeg 首选项"向导展示。会拉起子进程（最多六次 `-version`），调用方应
/// 用 `tokio::task::spawn_blocking` 包裹，避免阻塞 async 执行线程。
///
/// Probe ffmpeg/ffprobe/ffplay presence on the system PATH and in the tools
/// directory, running `-version` once on each found binary for its version,
/// for the frontend FFmpeg preference wizard. Spawns child processes
/// (up to six `-version` runs) — callers should wrap it in
/// `tokio::task::spawn_blocking` so async worker threads are not blocked.
pub fn ffmpeg_locations() -> FFmpegLocations {
	let tools = tools_dir();
	FFmpegLocations {
		tools_dir: tools.display().to_string(),
		system: probe_system(),
		tools: probe_dir(&tools),
		download_options: super::download::download_options(),
		tools_build: super::download::tools_build_info(),
	}
}

/// 沿 PATH 探测 ffmpeg/ffprobe/ffplay。/ Probe ffmpeg/ffprobe/ffplay along PATH.
fn probe_system() -> FFmpegLocationProbe {
	location_probe(
		find_on_path("ffmpeg"),
		find_on_path("ffprobe"),
		find_on_path("ffplay"),
	)
}

/// 探测固定目录下的 ffmpeg/ffprobe/ffplay。/ Probe the binaries inside a fixed directory.
fn probe_dir(dir: &Path) -> FFmpegLocationProbe {
	let pick = |base: &str| {
		let p = dir.join(exe_name(base));
		p.is_file().then_some(p)
	};
	location_probe(pick("ffmpeg"), pick("ffprobe"), pick("ffplay"))
}

/// 由三个可选路径组装探测结果；对存在的组件各跑一次 `-version` 取版本号。
/// Assemble a probe result from the optional paths; runs `-version` once on
/// each present binary for its version token.
fn location_probe(
	ffmpeg: Option<PathBuf>,
	ffprobe: Option<PathBuf>,
	ffplay: Option<PathBuf>,
) -> FFmpegLocationProbe {
	FFmpegLocationProbe {
		ffmpeg: ffmpeg.is_some(),
		ffprobe: ffprobe.is_some(),
		ffplay: ffplay.is_some(),
		ffmpeg_version: ffmpeg.as_deref().and_then(probe_version),
		ffprobe_version: ffprobe.as_deref().and_then(probe_version),
		ffplay_version: ffplay.as_deref().and_then(probe_version),
		ffmpeg_path: ffmpeg.map(|p| p.display().to_string()),
		ffprobe_path: ffprobe.map(|p| p.display().to_string()),
		ffplay_path: ffplay.map(|p| p.display().to_string()),
	}
}

/// 对指定可执行文件运行 `-version`，返回首行的版本号字段
/// （`<name> version <x>` 的第 3 个空白分隔字段；ffmpeg / ffprobe / ffplay
/// 的输出首行格式一致）。运行失败、退出码非零或输出为空时返回 `None`。
///
/// Run `-version` on the given binary and return the version token (3rd
/// whitespace-separated field of `<name> version <x>`; ffmpeg / ffprobe /
/// ffplay share the same first-line format). `None` when it won't launch,
/// exits non-zero, or produces no output.
fn probe_version(binary: &Path) -> Option<String> {
	let out = Command::new(binary)
		.arg("-version")
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.no_window()
		.output()
		.ok()?;
	if !out.status.success() {
		return None;
	}
	let stdout = String::from_utf8_lossy(&out.stdout);
	stdout
		.lines()
		.next()?
		.split_whitespace()
		.nth(2)
		.map(str::to_string)
}
