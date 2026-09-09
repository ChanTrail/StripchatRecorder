//! 应用程序可执行文件入口 / Application Executable Entry Point
//!
//! 从命令行参数或环境变量读取监听端口，启动 HTTP Server 模式。
//! Reads the listen port from CLI args or environment variable, then starts HTTP Server mode.

// Windows 链接器创建 .lib/.exp 文件时会输出信息到 stdout，
// linker_messages lint 只能在 bin crate 根部控制。
// Suppress Windows linker stdout noise; this lint must be set at the bin crate root.
#![cfg_attr(target_os = "windows", allow(linker_messages))]

fn main() {
    stripchat_recorder_lib::run()
}
