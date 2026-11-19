//! FFmpeg 自动下载、安装与更新 / FFmpeg Auto-Download, Install and Update
//!
//! 统一的 tools 副本获取与维护入口，按职责拆分为以下子模块（父模块只保留
//! 共享常量、文档与统一再导出，外部 API 路径不变）：
//!
//! - [`release`]：发布源元数据——平台到压缩包资产的映射、`tools/ffmpeg-build.json`
//!   安装标记（只有带标记的副本才会被自动更新替换）、sha256 校验文件解析
//! - [`net`]：网络层——社区代理/镜像策略下的 reqwest 客户端、分段探测、
//!   多线程 `Range` 分段下载、整文件哈希
//! - [`progress`]：`ffmpeg-progress` SSE 事件广播与状态存储写入/重置
//! - [`pipeline`]：下载-安装流水线——校验、下载、空闲等待、解压、冒烟、
//!   `.old` 备份式换入与回滚；经 `INSTALL_MUTEX` 保证同时只有一个安装
//! - [`tasks`]：入口任务——遗留清理、启动自动安装、手动安装、定期自动更新
//!   循环（缺失补装 + 远端哈希比对换新版，仅空闲时替换）
//!
//! 只有带安装标记的 tools 副本会被自动更新：用户手动放入 tools 的 ffmpeg、
//! 或 PATH 上的 ffmpeg，永远不会被替换。
//!
//! The single entry point for fetching and maintaining the tools copy, split
//! into submodules by concern (the parent keeps only shared constants, the
//! doc and unified re-exports; external API paths are unchanged):
//!
//! - [`release`]: release metadata — host-platform to archive-asset mapping,
//!   the `tools/ffmpeg-build.json` install marker (only marked copies are
//!   auto-updated) and sha256 checksum-file parsing
//! - [`net`]: networking — the reqwest client under the community
//!   proxy/mirror policy, range probing, multi-threaded ranged download and
//!   whole-file hashing
//! - [`progress`]: `ffmpeg-progress` SSE broadcasting and progress-store
//!   writes/resets
//! - [`pipeline`]: the download-install pipeline — checksum, download, idle
//!   wait, extraction, smoke test, `.old`-backup swap with rollback;
//!   `INSTALL_MUTEX` keeps it to one install at a time
//! - [`tasks`]: entry tasks — leftover cleanup, startup auto-install, manual
//!   install and the periodic auto-update loop (refill missing components +
//!   remote-hash-driven upgrades, swapping only while idle)
//!
//! Only marker-bearing tools copies are auto-updated: a user-placed ffmpeg in
//! tools, or a PATH ffmpeg, is never replaced.

use std::time::Duration;

// ─── 子模块 / Submodules ─────────────────────────────────────────────────────

/// 网络层：代理、客户端、分段下载与哈希 / Networking: proxy, client, ranged download and hashing
mod net;
/// 下载-安装流水线 / The download-install pipeline
mod pipeline;
/// 进度事件广播 / Progress event broadcasting
mod progress;
/// 发布源元数据：平台映射、安装标记、校验文件 / Release metadata: platform map, marker, checksum
mod release;
/// 入口任务：启动/手动安装与自动更新循环 / Entry tasks: startup/manual installs and the update loop
mod tasks;

// ─── 统一再导出（外部 API 路径保持 crate::ffmpeg::download::* 不变）──────────
// ─── Unified re-exports (external API paths stay `crate::ffmpeg::download::*`) ──

pub use release::build_platform;
pub use tasks::{
    auto_update_loop, cleanup_leftovers, current_progress, spawn_manual_install,
    spawn_startup_install,
};

// ─── 共享常量 / Shared constants ─────────────────────────────────────────────

/// FFmpeg-Builds-Full 浮动 latest Release 的下载基址。
/// Base URL of the FFmpeg-Builds-Full floating `latest` release.
const RELEASE_BASE: &str =
    "https://github.com/ChanTrail/FFmpeg-Builds-Full/releases/download/latest";

/// 下载中的临时目录名（tools 之下）。
/// In-flight download temp dir name (under tools).
const DOWNLOAD_DIR_NAME: &str = ".download";

/// 解压暂存目录名（tools 之下）。
/// Extraction staging dir name (under tools).
const STAGING_DIR_NAME: &str = ".staging";

/// 安装标记文件名（tools 之下）。
/// Install marker file name (under tools).
const MARKER_FILE_NAME: &str = "ffmpeg-build.json";

/// 下载数据类请求的整体超时（秒）：压缩包体积约 200 MB，慢网络给足余量。
/// Overall timeout for download data requests (seconds): the archive is ~200 MB,
/// allow headroom for slow networks.
const DOWNLOAD_TIMEOUT_SECS: u64 = 3600;

/// 小型请求（更新检查拉取校验文件）的整体超时（秒）。
/// Overall timeout for small requests (update-check checksum fetch), seconds.
const SMALL_TIMEOUT_SECS: u64 = 30;

/// 首次自动更新检查前的延迟（启动后先让位给其他初始化）。
/// Delay before the first auto-update check (yields to other startup work).
const AUTO_UPDATE_FIRST_DELAY: Duration = Duration::from_secs(5 * 60);
/// 自动更新检查的固定间隔。
/// Fixed interval between auto-update checks.
const AUTO_UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// 等待空闲的轮询间隔。
/// Idle-wait poll interval.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// 并行下载的分段数。
/// Number of parallel download segments.
const PARALLEL_PARTS: u64 = 8;

/// 走并行分段下载的最小总字节数（小于则直接单流）。
/// Minimum total size for the parallel segmented path (smaller → single stream).
const PARALLEL_MIN_BYTES: u64 = 4 * 1024 * 1024;

/// 全局安装互斥锁：启动安装、手动安装与自动更新共用，保证同时只有一个
/// 安装在进行；慢网络下后来的任务在锁上排队而非并发。
///
/// Global install mutex shared by startup, manual and auto-update installs —
/// only one install runs at a time; later tasks queue on the lock instead of
/// racing on slow networks.
static INSTALL_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
