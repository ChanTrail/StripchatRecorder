//! 用系统媒体播放器打开 HTTP-FLV 转发流 / Open the HTTP-FLV relay stream in a
//! system media player
//!
//! 桌面端「StripChat 浏览页点卡片 → 播放」的实现：转发流是
//! `http://127.0.0.1:{port}/stream/{username}` 的 HTTP-FLV，浏览器无法直接
//! 解码，必须交给本地播放器。按平台探测常见播放器（PATH → 常见安装目录 →
//! macOS `.app`），找到即以流地址为参数拉起；全部未命中返回
//! [`NO_PLAYER`]，让前端提示用户安装 VLC/mpv/PotPlayer——不回退系统默认
//! 打开方式（http URL 会落到浏览器，只会触发下载）。
//!
//! Implementation of the desktop "click a browse card → play" flow: the
//! relay stream is HTTP-FLV at `http://127.0.0.1:{port}/stream/{username}`,
//! which browsers can't decode — a local player is required. Common players
//! are probed per platform (PATH → well-known install dirs → macOS `.app`
//! bundles); the first hit is spawned with the stream URL. When nothing
//! matches, [`NO_PLAYER`] is returned so the frontend can ask the user to
//! install VLC/mpv/PotPlayer — there is deliberately no fallback to the OS
//! default handler (an http URL would land in the browser and just start a
//! download).

use crate::core::error::{AppError, Result};
use crate::core::no_window::NoWindowExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// 未找到任何可用播放器时的错误标记（前端据此给出本地化提示）/
/// Error marker when no usable player is found (the frontend maps it to a
/// localized hint)
pub const NO_PLAYER: &str = "no_player";

/// 用系统媒体播放器打开转发流地址，返回所启动播放器的展示名。
/// Open the relay stream URL in a system media player; returns the launched
/// player's display name.
///
/// ## 参数 / Parameters
/// - `url`：HTTP-FLV 转发流地址 / HTTP-FLV relay stream URL
///
/// ## 错误 / Errors
/// - 全部候选未命中 → [`AppError::Other`]`(NO_PLAYER)`
///   All candidates missed → [`AppError::Other`]`(NO_PLAYER)`
/// - 拉起播放器进程失败 → IO 错误 / spawning the player failed → IO error
pub fn open_in_media_player(url: &str) -> Result<String> {
    for candidate in candidates() {
        if let Some((prog, mut argv)) = candidate.resolve() {
            let label = candidate.label.to_string();
            tracing::info!(
                "{}",
                crate::tl!("relay.openInPlayer", player = label, url = url)
            );
            argv.push(url.to_string());
            Command::new(prog)
                .args(&argv)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .no_window()
                .spawn()
                .map_err(|e| AppError::Other(format!("failed to launch player: {e}")))?;
            return Ok(label);
        }
    }
    Err(AppError::Other(NO_PLAYER.to_string()))
}

/// 播放器定位方式 / How a player is located
enum Resolve {
    /// 沿 PATH 查找可执行文件 / Look the executable up on PATH
    OnPath(&'static str),
    /// 固定安装路径存在即可 / A fixed install path that only needs to exist
    ///
    /// 仅 Windows 候选使用（按 ProgramFiles 拼安装路径）；其他平台构造
    /// 不出来，故变体随平台裁剪以免触发 dead_code /
    /// Only used by Windows candidates (install dirs under ProgramFiles);
    /// other platforms never construct it, so the variant is cfg-gated to
    /// keep dead_code quiet
    #[cfg(target_os = "windows")]
    AtPath(PathBuf),
    /// macOS `.app` 包：存在 `/Applications/{name}.app` 即以 `open -a` 唤起 /
    /// macOS `.app` bundle: when `/Applications/{name}.app` exists, launch via `open -a`
    #[cfg(target_os = "macos")]
    MacApp(&'static str),
}

/// 一个播放器候选：展示名 + 定位方式 /
/// One player candidate: display name + location strategy
struct Candidate {
    /// 播放器展示名 / Player display name
    label: &'static str,
    /// 定位方式 / Location strategy
    how: Resolve,
}

impl Candidate {
    /// 命中返回 `(可执行程序, 前置参数)`；未命中返回 `None` /
    /// On a hit returns `(program, leading args)`; `None` when absent
    fn resolve(&self) -> Option<(PathBuf, Vec<String>)> {
        match &self.how {
            Resolve::OnPath(exe) => find_on_path(exe).map(|p| (p, Vec::new())),
            #[cfg(target_os = "windows")]
            Resolve::AtPath(p) => p.is_file().then(|| (p.clone(), Vec::new())),
            #[cfg(target_os = "macos")]
            Resolve::MacApp(app) => PathBuf::from(format!("/Applications/{app}.app"))
                .is_dir()
                .then(|| {
                    (
                        PathBuf::from("open"),
                        vec!["-a".to_string(), app.to_string()],
                    )
                }),
        }
    }
}

/// 按平台优先级排列的候选列表——mpv/VLC/PotPlayer 对 HTTP-FLV 的兼容性
/// 最好，优先于系统默认程序。/ Platform-ordered candidate list — mpv, VLC
/// and PotPlayer handle HTTP-FLV best and come before any OS default.
fn candidates() -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        // PATH 命中（免安装/加入 PATH 的播放器）/ PATH hits (portable or PATH-installed players)
        for (exe, label) in [
            ("mpv", "mpv"),
            ("vlc", "VLC"),
            ("mpvnet", "mpv.net"),
            ("PotPlayerMini64", "PotPlayer"),
            ("mpc-hc64", "MPC-HC"),
            ("mpc-be64", "MPC-BE"),
        ] {
            out.push(Candidate {
                label,
                how: Resolve::OnPath(exe),
            });
        }
        // 常见安装目录（安装包不写 PATH 的播放器）/ Well-known install dirs
        // (installers that don't write PATH)
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                for (rel, label) in [
                    ("VideoLAN\\VLC\\vlc.exe", "VLC"),
                    ("DAUM\\PotPlayer\\PotPlayerMini64.exe", "PotPlayer"),
                    ("Kakao\\PotPlayer\\PotPlayerMini64.exe", "PotPlayer"),
                ] {
                    out.push(Candidate {
                        label,
                        how: Resolve::AtPath(base.join(rel)),
                    });
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // .app 包（open -a 传 URL）/ .app bundles (URL passed via `open -a`)
        for (app, label) in [("IINA", "IINA"), ("VLC", "VLC")] {
            out.push(Candidate {
                label,
                how: Resolve::MacApp(app),
            });
        }
        // brew 安装的 CLI 播放器 / CLI players installed via brew
        for (exe, label) in [("mpv", "mpv"), ("vlc", "VLC")] {
            out.push(Candidate {
                label,
                how: Resolve::OnPath(exe),
            });
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for (exe, label) in [
            ("mpv", "mpv"),
            ("vlc", "VLC"),
            ("celluloid", "Celluloid"),
            ("smplayer", "SMPlayer"),
            ("totem", "Totem"),
        ] {
            out.push(Candidate {
                label,
                how: Resolve::OnPath(exe),
            });
        }
    }

    out
}

/// 沿 PATH 查找可执行文件（Windows 下自动补 `.exe` 后缀）。
/// Search PATH for an executable (appends `.exe` on Windows).
fn find_on_path(base: &str) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    let name = format!("{base}.exe");
    #[cfg(not(target_os = "windows"))]
    let name = base.to_string();
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file())
}
