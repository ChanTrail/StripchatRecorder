//! 更新检查模块 / Update Check Module
//!
//! 提供版本检查、Docker 环境检测、平台识别功能，以及后台下载并安装更新的逻辑。
//!
//! 下载安装流程（`install::download_and_install`）：
//! 1. 用 reqwest 流式下载 zip，每 1%（或 200ms、512KB）广播 SSE `update-progress` 进度
//! 2. 用 self_update::Extract 解压到临时目录
//! 3. 用 self_update::MoveAll 事务性多文件替换（任一步失败全部回滚）
//! 4. 用 duct::cmd! 在独立进程中启动新版本，立即退出当前进程
//!
//! Update check module: version checks, Docker/platform detection, plus the
//! background download-and-install flow (`install::download_and_install`):
//! 1. Stream-download the zip via reqwest, broadcasting SSE `update-progress` every 1% / 200ms / 512KB
//! 2. Extract with self_update::Extract to a temp dir
//! 3. Atomically replace files with self_update::MoveAll (all-or-nothing, rolls back on failure)
//! 4. Spawn the new version in a detached process via duct::cmd!, then exit immediately
//!
//! ## 子模块 / Submodules
//! - [`platform`]：编译目标平台识别 + Docker 容器检测 /
//!   compile-target platform string + Docker container detection
//! - [`version`]：beta 判定 + 语义化版本比较 /
//!   beta-build check + semantic version comparison
//! - [`github`]：GitHub Release 查询与 ReleaseInfo/UpdateInfo 类型 /
//!   GitHub release queries plus the ReleaseInfo/UpdateInfo types
//! - [`install`]：UpdateProgress 状态 + 流式下载安装主流程 /
//!   UpdateProgress state plus the streaming download/install driver
//! - [`extract`]：zip 解压 + MoveAll 事务性替换（install 的私有子步骤）/
//!   zip extraction + MoveAll transactional replace (private step of install)

mod extract;
mod github;
mod install;
mod platform;
mod version;

// 全部条目在模块根部重导出，调用方保持 `crate::update::X` 路径不变 /
// Everything is re-exported at the module root so callers keep their
// `crate::update::X` paths unchanged
pub use github::{ReleaseInfo, UpdateInfo, fetch_latest_release, fetch_latest_release_with_assets};
pub use install::{UpdateProgress, UpdateStateStore, download_and_install, new_update_state};
pub use platform::{current_platform, is_docker};
pub use version::{is_beta_version, semver_gt};

/// 当前应用版本（与 Cargo.toml 一致）
/// Current application version (matches Cargo.toml)
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// GitHub 仓库 owner/repo（Release 查询与下载 URL 均以此为根）/
/// GitHub repository owner/repo (root of all release queries and download URLs)
pub(crate) const OWNER: &str = "ChanTrail";
pub(crate) const REPO: &str = "StripchatRecorder";
