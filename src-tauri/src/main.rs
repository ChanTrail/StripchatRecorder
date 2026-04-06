//! 应用程序可执行文件入口 / Application Executable Entry Point
//!
//! 在 Release 构建中隐藏 Windows 控制台窗口，并委托给库 crate 的启动逻辑。
//! Hides the Windows console window in release builds and delegates to the library crate's startup logic.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    stripchat_recorder_lib::run_with_mode_select()
}
