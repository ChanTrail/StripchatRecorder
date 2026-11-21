//! FFmpeg 可用性管理与自动下载 / FFmpeg Availability Management and Auto-Download
//!
//! 本模块统一管理 ffmpeg / ffprobe / ffplay 的查找、可用性检测与自动安装，
//! 按职责拆分为以下子模块（父模块只做文档与统一再导出，外部 API 路径不变）：
//!
//! - [`paths`]：安装目录（`tools_dir`）、旧 `lib/` 目录迁移与平台可执行文件名
//! - [`preference`]：`ffmpeg_preference` 设置的进程内镜像、生效值（含所选
//!   位置的组件存在性门控）、按首选项的程序路径解析、tools 前置的 PATH
//!   环境变量与 ffmpeg 可用性检测；解析逐次读取镜像，改完首选项下一次
//!   调用立即生效、无需重启
//! - [`guard`]：进程内 ffmpeg 使用计数 + 活跃录制计数组成的"空闲"判断，
//!   自动更新只在空闲时替换二进制文件
//! - [`state`]：下载/安装进度事件模型（`FFmpegKind` / `FFmpegProgress`）
//!   与进度状态存储（SSE 广播 + 重连恢复）
//! - [`probe`]：system（PATH）与 tools 两个位置中 ffmpeg/ffprobe/ffplay 的
//!   存在性与 `-version` 版本探测，供"FFmpeg 首选项"向导与设置页展示
//! - [`download`]：缺失时的启动自动下载、手动下载、多线程分段下载、解压
//!   安装、下载副本的定期自动更新，以及 `tools/ffmpeg-build.json` 安装标记
//!
//! This module centrally manages ffmpeg / ffprobe / ffplay lookup, availability
//! checks and automatic installation, split into submodules by concern (the
//! parent only documents and re-exports; external API paths are unchanged):
//!
//! - [`paths`]: install directory (`tools_dir`), legacy `lib/` migration and
//!   the platform executable name
//! - [`preference`]: in-process mirror of the `ffmpeg_preference` setting, the
//!   effective value (with an existence gate on the chosen location),
//!   preference-aware program resolution, a tools-prepended PATH env and the
//!   ffmpeg availability check; resolution reads the mirror per call so a
//!   changed preference takes effect on the next invocation without restart
//! - [`guard`]: in-process ffmpeg usage counter combined with the active
//!   recording count; auto-updates only swap binaries while idle
//! - [`state`]: download/install progress event model (`FFmpegKind` /
//!   `FFmpegProgress`) and the progress state store (SSE broadcast +
//!   reconnect restore)
//! - [`probe`]: presence + `-version` probing for ffmpeg/ffprobe/ffplay on the
//!   system PATH and in the tools directory, shown by the FFmpeg preference
//!   wizard and the settings page
//! - [`download`]: startup auto-download when missing, manual download,
//!   multi-threaded ranged download, extraction, periodic auto-update of the
//!   managed copy and the `tools/ffmpeg-build.json` install marker

// ─── 子模块 / Submodules ─────────────────────────────────────────────────────

/// 下载、安装与自动更新 / Download, install and auto-update
pub mod download;
/// 使用中计数与空闲判定 / In-use counter and idle check
mod guard;
/// 安装路径与可执行文件名 / Install paths and executable names
mod paths;
/// 首选项镜像、程序解析与可用性检测 / Preference mirror, program resolution and availability
mod preference;
/// 两位置组件探测 / Two-location component probing
mod probe;
/// 进度事件模型与状态存储 / Progress event model and state store
mod state;

// ─── 统一再导出（外部 API 路径保持 crate::ffmpeg::* 不变）───────────────────
// ─── Unified re-exports (external API paths stay `crate::ffmpeg::*`) ────────

pub use guard::{InUseGuard, in_use, in_use_count, is_idle};
pub use paths::tools_dir;
pub use preference::{
	ffmpeg_available, ffmpeg_program, ffplay_program, ffprobe_program, path_env_with_tools,
	preference, set_preference,
};
pub use probe::{FFmpegLocationProbe, FFmpegLocations, ffmpeg_locations};
pub use state::{FFmpegKind, FFmpegProgress, FFmpegStateStore, new_ffmpeg_state};

pub(crate) use paths::{exe_name, migrate_legacy_lib_dir};
pub(crate) use preference::{location_complete, raw_preference};
