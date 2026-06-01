//! 应用程序可执行文件入口 / Application Executable Entry Point
//!
//! 从命令行参数或环境变量读取监听端口，启动 HTTP Server 模式。
//! Reads the listen port from CLI args or environment variable, then starts HTTP Server mode.

fn main() {
    stripchat_recorder_lib::run()
}
