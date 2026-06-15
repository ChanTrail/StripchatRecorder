//! Stripchat Recorder 库 crate 根模块 / Stripchat Recorder Library Crate Root
//!
//! 仅支持 Server 模式（HTTP API + SSE），通过命令行参数或环境变量指定监听端口。
//! Only supports Server mode (HTTP API + SSE); listen port is specified via CLI arg or env var.

pub mod commands;
pub mod config;
pub mod core;
pub mod locale;
pub mod postprocess;
pub mod recording;
pub mod relay;
pub mod server_mod;
pub mod streaming;
pub mod watcher;

/// 应用程序主入口：从命令行参数或环境变量读取端口，启动 HTTP Server 模式。
///
/// Port resolution order:
/// 1. First CLI argument (e.g. `./stripchat-recorder 3030`)
/// 2. `PORT` environment variable
/// 3. `server_port` field in `config/settings.json`
/// 4. Default: 3030
pub fn run() {
    // 解析端口：CLI 参数 > 环境变量 > 配置文件 > 默认值
    // Resolve port: CLI arg > env var > config file > default
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .or_else(|| std::env::var("PORT").ok().and_then(|s| s.parse().ok()))
        .or_else(|| {
            config::settings::AppState::new()
                .ok()
                .map(|s| s.get_settings().server_port)
        })
        .unwrap_or(3030);

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(server_mod::server::run_server(port));
}
