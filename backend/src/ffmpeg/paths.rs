//! FFmpeg 安装路径工具 / FFmpeg Install Path Helpers
//!
//! - [`tools_dir`]：ffmpeg / ffprobe / ffplay 的自动安装目录（`<exe_dir>/tools/`）
//! - [`migrate_legacy_lib_dir`]：把旧版本 `lib/` 安装目录整体迁移为 `tools/`
//! - [`exe_name`]：当前平台的可执行文件名（Windows 带 `.exe` 后缀）
//!
//! - [`tools_dir`]: the auto-install directory for ffmpeg / ffprobe / ffplay (`<exe_dir>/tools/`)
//! - [`migrate_legacy_lib_dir`]: migrate the legacy `lib/` install dir to `tools/`
//! - [`exe_name`]: executable file name for the current platform (`.exe` on Windows)

use std::path::PathBuf;

/// ffmpeg / ffprobe 的自动安装目录（`<exe_dir>/tools/`）。
/// Auto-install directory for ffmpeg / ffprobe (`<exe_dir>/tools/`).
pub fn tools_dir() -> PathBuf {
	crate::config::app_state::exe_dir().join("tools")
}

/// 把旧版本使用的 `lib/` 安装目录整体迁移为 `tools/`。
/// 仅当 `tools` 不存在且 `lib` 存在时执行 `rename`；失败只记警告、不影响启动。
/// 供启动流程调用一次，避免已下载副本被孤儿化而重新下载约 200 MB。
///
/// Migrate the legacy `lib/` install dir to `tools/` in one rename. Only runs
/// when `tools` is absent and `lib` exists; failures are logged and ignored.
/// Called once by the startup flow so an already-downloaded copy is not
/// orphaned into a ~200 MB re-download.
pub(crate) fn migrate_legacy_lib_dir() {
	let exe = crate::config::app_state::exe_dir();
	let old = exe.join("lib");
	let new = tools_dir();
	if old.is_dir()
		&& !new.exists()
		&& let Err(e) = std::fs::rename(&old, &new)
	{
		tracing::warn!(
			"{}",
			crate::tl!(
				"ffmpeg.migrateLibDirFailed",
				from = old.display(),
				to = new.display(),
				error = e
			)
		);
	}
}

/// 当前平台的可执行文件名（Windows 带 `.exe` 后缀）。
/// Executable file name for the current platform (`.exe` suffix on Windows).
pub(crate) fn exe_name(base: &str) -> String {
	#[cfg(target_os = "windows")]
	{
		format!("{}.exe", base)
	}
	#[cfg(not(target_os = "windows"))]
	{
		base.to_string()
	}
}
