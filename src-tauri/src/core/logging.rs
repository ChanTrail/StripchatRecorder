//! 日志系统初始化 / Logging System Initialization
//!
//! 配置 tracing 订阅者，同时输出到控制台（带颜色）和滚动日志文件（最多保留 10 个）。
//! Configures the tracing subscriber to output to both the console (with colors)
//! and a rolling log file (keeping at most 10 files).

use std::fs::{self, File};
use std::path::PathBuf;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// 初始化日志系统：创建日志目录、清理旧日志、配置控制台和文件输出层。
/// Initialize the logging system: create log directory, clean up old logs,
/// configure console and file output layers.
///
/// # 参数 / Parameters
/// - `log_dir`: 日志文件存放目录 / Directory where log files are stored
pub fn init_logging(log_dir: &PathBuf) -> std::io::Result<()> {
    fs::create_dir_all(log_dir)?;
    cleanup_old_logs(log_dir, 10)?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let log_path = log_dir.join(format!("stripchat-recorder_{}.log", timestamp));
    let log_file = File::create(&log_path)?;

    // 控制台层：从 RUST_LOG 环境变量读取过滤级别，默认 info
    // Console layer: reads filter level from RUST_LOG env var, defaults to info
    let console_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let console_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_target(true)
        .with_filter(console_filter);

    // 文件层：固定 INFO 级别，包含文件名和行号，不含 ANSI 颜色码
    // File layer: fixed INFO level, includes filename and line number, no ANSI color codes
    let file_layer = fmt::layer()
        .with_writer(log_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true)
        .with_filter(LevelFilter::INFO);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    tracing::info!("Logging initialized → {:?}", log_path);

    Ok(())
}

/// 清理日志目录中的旧日志文件，按修改时间降序排列，保留最新的 `keep_count` 个。
/// Clean up old log files in the log directory, sorted by modification time descending,
/// keeping the newest `keep_count` files.
///
/// # 参数 / Parameters
/// - `log_dir`: 日志目录 / Log directory
/// - `keep_count`: 保留的最大日志文件数 / Maximum number of log files to keep
fn cleanup_old_logs(log_dir: &PathBuf, keep_count: usize) -> std::io::Result<()> {
    let mut log_files: Vec<_> = fs::read_dir(log_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("stripchat-recorder_") && name.ends_with(".log")
        })
        .collect();

    if log_files.len() < keep_count {
        return Ok(());
    }

    // 按修改时间降序排列（最新的在前）/ Sort by modification time descending (newest first)
    log_files.sort_by(|a, b| {
        let ta = a.metadata().and_then(|m| m.modified()).ok();
        let tb = b.metadata().and_then(|m| m.modified()).ok();
        tb.cmp(&ta)
    });

    // 删除超出保留数量的旧日志 / Delete old logs beyond the keep count
    for entry in log_files.into_iter().skip(keep_count - 1) {
        if let Err(e) = fs::remove_file(entry.path()) {
            eprintln!("Failed to remove old log {:?}: {}", entry.path(), e);
        }
    }

    Ok(())
}
