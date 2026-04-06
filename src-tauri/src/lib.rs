//! Stripchat Recorder 库 crate 根模块 / Stripchat Recorder Library Crate Root
//!
//! 负责运行模式选择（Desktop / Server）、Tauri 桌面应用初始化以及各子模块的声明。
//! Handles run mode selection (Desktop / Server), Tauri desktop app initialization,
//! and sub-module declarations.

mod commands;
mod config;
mod core;
mod postprocess;
mod recording;
mod server_mod;
mod streaming;
mod watcher;

use config::settings::AppState;
use recording::recorder::RecorderManager;
use streaming::monitor::StatusMonitor;
use std::sync::Arc;
use tauri::Emitter as TauriEmitterTrait;

/// 应用运行模式 / Application run mode
#[derive(Debug, Clone, PartialEq)]
enum RunMode {
    /// Tauri 图形界面模式 / Tauri GUI desktop mode
    Desktop,
    /// HTTP API + SSE 服务器模式，监听指定端口 / HTTP API + SSE server mode on the given port
    Server(u16),
}

/// 返回运行模式持久化文件的路径。
/// Returns the path to the run mode persistence file.
fn mode_file_path() -> std::path::PathBuf {
    config::settings::AppState::config_dir().join("run_mode.txt")
}

/// 从磁盘读取上次保存的运行模式，文件不存在或格式无效时返回 `None`。
/// Reads the previously saved run mode from disk; returns `None` if the file is missing or invalid.
fn load_saved_mode() -> Option<RunMode> {
    let content = std::fs::read_to_string(mode_file_path()).ok()?;
    let content = content.trim();
    if content == "desktop" {
        return Some(RunMode::Desktop);
    }
    if let Some(port_str) = content.strip_prefix("server:") {
        if let Ok(port) = port_str.parse::<u16>() {
            return Some(RunMode::Server(port));
        }
    }
    None
}

/// 将运行模式持久化到磁盘，供下次启动时直接使用。
/// Persists the run mode to disk so it can be reused on the next launch.
fn save_mode(mode: &RunMode) {
    let path = mode_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = match mode {
        RunMode::Desktop => "desktop".to_string(),
        RunMode::Server(port) => format!("server:{}", port),
    };
    let _ = std::fs::write(path, content);
}

/// 通过交互式命令行提示用户选择运行模式（仅首次启动时调用）。
/// Prompts the user to select a run mode via interactive CLI (called only on first launch).
fn ask_mode_interactive() -> RunMode {
    use std::io::{self, BufRead, Write};

    println!("┌─────────────────────────────────────────┐");
    println!("│   Stripchat Recorder — 首次启动配置      │");
    println!("├─────────────────────────────────────────┤");
    println!("│  [1] Desktop 模式  (Tauri 图形界面)      │");
    println!("│  [2] Server  模式  (HTTP API + SSE)      │");
    println!("└─────────────────────────────────────────┘");
    print!("请选择运行模式 [1/2]: ");
    let _ = io::stdout().flush();

    let stdin = io::stdin();
    let choice = stdin
        .lock()
        .lines()
        .next()
        .and_then(|l| l.ok())
        .unwrap_or_default();

    if choice.trim() == "2" {
        print!("请输入监听端口 [默认 3030]: ");
        let _ = io::stdout().flush();
        let port_str = stdin
            .lock()
            .lines()
            .next()
            .and_then(|l| l.ok())
            .unwrap_or_default();
        let port: u16 = port_str.trim().parse().unwrap_or(3030);
        RunMode::Server(port)
    } else {
        RunMode::Desktop
    }
}

/// 应用程序主入口：读取或交互式选择运行模式，然后启动对应的运行时。
/// Application main entry: reads or interactively selects the run mode, then starts the corresponding runtime.
pub fn run_with_mode_select() {
    let mode = match load_saved_mode() {
        Some(m) => m,
        None => {
            let m = ask_mode_interactive();
            save_mode(&m);
            m
        }
    };

    match mode {
        RunMode::Desktop => run_desktop(),
        RunMode::Server(port) => {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            rt.block_on(server_mod::server::run_server(port));
        }
    }
}

/// Tauri 移动端入口点（由 `tauri::mobile_entry_point` 宏使用）。
/// Tauri mobile entry point (used by the `tauri::mobile_entry_point` macro).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_desktop();
}

/// 初始化并启动 Tauri 桌面应用。
/// 包括：日志初始化、状态管理、录制器、状态监控、插件注册、命令处理器注册。
///
/// Initializes and starts the Tauri desktop application.
/// Includes: logging init, state management, recorder, status monitor, plugin registration, command handler registration.
fn run_desktop() {
    let log_dir = AppState::log_dir();
    if let Err(e) = core::logging::init_logging(&log_dir) {
        eprintln!("Failed to initialize logging: {}", e);
    }

    let state = AppState::new().expect("Failed to initialize app state");
    let recorder = RecorderManager::new(Arc::clone(&state));
    let monitor = StatusMonitor::new(Arc::clone(&state), Arc::clone(&recorder));

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Arc::clone(&state))
        .manage(Arc::clone(&recorder))
        .manage(Arc::clone(&monitor))
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // 检查 ffmpeg 是否可用，不可用时向前端发送警告事件
            // Check if ffmpeg is available; emit a warning event to the frontend if not
            if !recording::recorder::ffmpeg_available() {
                tracing::error!("ffmpeg not found on PATH");
                let _ = app_handle.emit(
                    "ffmpeg-missing",
                    serde_json::json!({
                        "message": "未找到 ffmpeg，录制功能不可用。请下载 ffmpeg 并将其加入系统环境变量后重启应用。\nhttps://ffmpeg.org/download.html"
                    }),
                );
            }

            // 启动时合并遗留的未完成录制片段，并清理空目录
            // Merge leftover recording segments on startup and clean up empty directories
            {
                let settings = state.get_settings();
                let output_dir = std::path::PathBuf::from(&settings.output_dir);
                let merge_format = settings.merge_format.clone();
                let recorder_clone = Arc::clone(&recorder);
                let app_handle_clone = app_handle.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let emitter: Arc<dyn crate::core::emitter::Emitter> =
                        Arc::new(crate::core::emitter::TauriEmitter(app_handle_clone.clone()));
                    recording::recorder::startup_merge_leftover_segments(&output_dir, &merge_format, &emitter, &recorder_clone);
                    recording::recorder::startup_remove_empty_dirs(&output_dir);
                });
            }

            // 启动主播状态轮询监控循环
            // Start the streamer status polling monitor loop
            let monitor_clone = Arc::clone(&monitor);
            let app_handle_clone = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                monitor_clone.start(app_handle_clone);
            });

            // 启动每日配置检查（验证主播账号是否仍然存在）
            // Start daily config checks (verify streamer accounts still exist)
            {
                let state_clone = Arc::clone(&state);
                let emitter: Arc<dyn crate::core::emitter::Emitter> =
                    Arc::new(crate::core::emitter::TauriEmitter(app_handle.clone()));
                tauri::async_runtime::spawn(async move {
                    config::settings::schedule_config_checks(state_clone, emitter).await;
                });
            }

            // 启动模块目录文件监控（检测模块可执行文件的增删）
            // Start modules directory file watcher (detects module executable additions/removals)
            let emitter_for_modules: Arc<dyn crate::core::emitter::Emitter> =
                Arc::new(crate::core::emitter::TauriEmitter(app_handle.clone()));
            crate::watcher::fs_watch::start_modules_dir_watcher(emitter_for_modules);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::streamer_cmd::list_streamers,
            commands::streamer_cmd::add_streamer,
            commands::streamer_cmd::remove_streamer,
            commands::streamer_cmd::set_auto_record,
            commands::streamer_cmd::start_recording,
            commands::streamer_cmd::stop_recording,
            commands::settings_cmd::get_settings,
            commands::settings_cmd::save_settings_cmd,
            commands::settings_cmd::pick_output_dir,
            commands::settings_cmd::list_mouflon_keys,
            commands::settings_cmd::add_mouflon_key,
            commands::settings_cmd::remove_mouflon_key,
            commands::settings_cmd::get_startup_warnings,
            commands::settings_cmd::remove_missing_pp_results,
            commands::settings_cmd::get_disk_space,
            commands::recording_cmd::list_recordings,
            commands::recording_cmd::get_merging_dirs,
            commands::recording_cmd::open_recording,
            commands::recording_cmd::delete_recording,
            commands::recording_cmd::open_output_dir,
            commands::postprocess_cmd::list_modules,
            commands::postprocess_cmd::get_pipeline,
            commands::postprocess_cmd::save_pipeline,
            commands::postprocess_cmd::run_postprocess_cmd,
            commands::postprocess_cmd::get_postprocess_tasks,
            commands::postprocess_cmd::get_module_outputs,
            commands::postprocess_cmd::cancel_postprocess,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
