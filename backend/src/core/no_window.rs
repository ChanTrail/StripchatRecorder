//! Windows CREATE_NO_WINDOW 工具 trait / Windows CREATE_NO_WINDOW utility trait
//!
//! 在 Windows 上调用外部程序时（ffmpeg、模块可执行文件等），默认会弹出黑色控制台窗口。
//! 通过设置 CREATE_NO_WINDOW (0x08000000) 标志可以抑制该窗口。
//!
//! On Windows, spawning external processes (ffmpeg, module executables, etc.) shows a
//! black console window by default. Setting the CREATE_NO_WINDOW (0x08000000) flag
//! suppresses it.
//!
//! 用法 / Usage:
//! ```rust
//! use crate::core::no_window::NoWindowExt;
//! Command::new("ffmpeg").no_window().spawn()?;
//! ```

/// 抑制 Windows 控制台黑窗的 CREATE_NO_WINDOW 标志值。
/// CREATE_NO_WINDOW flag value to suppress the Windows console window.
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 为 `std::process::Command` 和 `tokio::process::Command` 提供 `.no_window()` 方法。
/// Provides `.no_window()` method for `std::process::Command` and `tokio::process::Command`.
pub trait NoWindowExt {
    /// 在 Windows 上设置 CREATE_NO_WINDOW 标志，其他平台为空操作。
    /// On Windows, sets CREATE_NO_WINDOW flag; no-op on other platforms.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindowExt for std::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

impl NoWindowExt for tokio::process::Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        self.creation_flags(CREATE_NO_WINDOW);
        self
    }
}
