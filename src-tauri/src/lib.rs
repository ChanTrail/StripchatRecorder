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

/// 从 settings.json 读取上次保存的运行模式，未配置时返回 `None`。
/// Reads the previously saved run mode from settings.json; returns `None` if not yet configured.
fn load_saved_mode() -> Option<RunMode> {
    let state = config::settings::AppState::new().ok()?;
    let settings = state.get_settings();
    // run_mode 为空字符串表示尚未首次配置 / empty string means not yet configured
    if settings.run_mode.is_empty() {
        return None;
    }
    match settings.run_mode.as_str() {
        "desktop" => Some(RunMode::Desktop),
        "server" => Some(RunMode::Server(settings.server_port)),
        _ => None,
    }
}

/// 将运行模式和端口持久化到 settings.json。
/// Persists the run mode and port to settings.json.
fn save_mode(mode: &RunMode, lang: &str) {
    if let Ok(state) = config::settings::AppState::new() {
        let mut settings = state.get_settings();
        settings.language = lang.to_string();
        match mode {
            RunMode::Desktop => {
                settings.run_mode = "desktop".to_string();
            }
            RunMode::Server(port) => {
                settings.run_mode = "server".to_string();
                settings.server_port = *port;
            }
        }
        let _ = state.update_settings(settings);
    }
}

/// 通过 crossterm TUI 让用户选择运行模式（仅首次启动时调用）。
/// 先选语言，再选运行模式，返回 (RunMode, 语言码)。
/// Uses crossterm TUI to let the user select run mode (called only on first launch).
/// Language is selected first, then run mode; returns (RunMode, language_code).
fn ask_mode_interactive() -> (RunMode, String) {
    use crossterm::{
        cursor,
        event::{self, Event, KeyCode},
        execute,
        style::{Color, Print, ResetColor, SetForegroundColor},
        terminal::{self, ClearType},
    };
    use std::io::{self, Write};

    // ── 通用方向键菜单 / Generic arrow-key menu ──────────────────────────────
    fn run_menu<W: Write>(
        stdout: &mut W,
        title: &str,
        items: &[&str],
        highlight: Color,
    ) -> usize {
        use crossterm::event::KeyEventKind;
        let mut selected = 0usize;
        loop {
            execute!(stdout, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            execute!(
                stdout,
                SetForegroundColor(Color::Cyan),
                Print(format!("  {}\n\n", title)),
                ResetColor
            )
            .unwrap();
            for (i, item) in items.iter().enumerate() {
                if i == selected {
                    execute!(
                        stdout,
                        SetForegroundColor(highlight),
                        Print(format!("  ▶  {}\n", item)),
                        ResetColor
                    )
                    .unwrap();
                } else {
                    execute!(stdout, Print(format!("     {}\n", item))).unwrap();
                }
            }
            execute!(
                stdout,
                Print("\n"),
                SetForegroundColor(Color::DarkGrey),
                Print("  ↑/↓ 移动  Enter 确认 / ↑/↓ move  Enter confirm"),
                ResetColor
            )
            .unwrap();
            stdout.flush().unwrap();

            if let Ok(Event::Key(key)) = event::read() {
                // 只处理按下事件，忽略释放和重复 / Only handle Press, ignore Release/Repeat
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Up => {
                        if selected > 0 {
                            selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if selected + 1 < items.len() {
                            selected += 1;
                        }
                    }
                    KeyCode::Enter => break,
                    _ => {}
                }
            }
        }
        selected
    }

    // ── 输入端口号（crossterm raw mode）/ Input port number via crossterm ────
    fn ask_port<W: Write>(stdout: &mut W, lang_en: bool) -> u16 {
        use crossterm::event::KeyEventKind;
        let (title, hint) = if lang_en {
            ("  Listen port (↵ = 3030):", "  Backspace to delete, Enter to confirm")
        } else {
            ("  监听端口（回车默认 3030）:", "  Backspace 删除，Enter 确认")
        };

        let mut input = String::new();

        loop {
            execute!(stdout, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
            execute!(
                stdout,
                SetForegroundColor(Color::Cyan),
                Print(format!("{}\n\n", title)),
                ResetColor,
                Print(format!("  > {}_\n\n", input)),
                SetForegroundColor(Color::DarkGrey),
                Print(hint),
                ResetColor,
            )
            .unwrap();
            stdout.flush().unwrap();

            if let Ok(Event::Key(key)) = event::read() {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Enter => break,
                    KeyCode::Backspace => { input.pop(); }
                    KeyCode::Char(c) if c.is_ascii_digit() && input.len() < 5 => {
                        input.push(c);
                    }
                    _ => {}
                }
            }
        }
        input.trim().parse::<u16>().unwrap_or(3030)
    }

    let mut stdout = io::stdout();
    terminal::enable_raw_mode().expect("Failed to enable raw mode");
    execute!(stdout, cursor::Hide).unwrap();

    // 清空启动时残留的输入事件（如回车键）/ Drain leftover input events from startup
    std::thread::sleep(std::time::Duration::from_millis(50));
    while event::poll(std::time::Duration::from_millis(0)).unwrap_or(false) {
        let _ = event::read();
    }

    // ── Step 1: 选语言 / Select language ─────────────────────────────────────
    let lang_items = ["中文 (zh-CN)", "English (en-US)"];
    let lang_idx = run_menu(
        &mut stdout,
        "Stripchat Recorder — Select Language / 选择语言",
        &lang_items,
        Color::Green,
    );
    let (lang_code, lang_en) = if lang_idx == 1 {
        ("en-US", true)
    } else {
        ("zh-CN", false)
    };

    // 将语言写入配置 / Persist language to config
    // （由调用方 run_with_mode_select 统一处理 / handled by run_with_mode_select）

    // ── Step 2: 选运行模式 / Select run mode ─────────────────────────────────
    let (title, mode_items) = if lang_en {
        (
            "Stripchat Recorder — First Launch Setup",
            ["Desktop mode  (Tauri GUI)", "Server mode   (HTTP API + SSE)"],
        )
    } else {
        (
            "Stripchat Recorder — 首次启动配置",
            ["Desktop 模式  (Tauri 图形界面)", "Server  模式  (HTTP API + SSE)"],
        )
    };
    let mode_idx = run_menu(&mut stdout, title, &mode_items, Color::Yellow);

    let mode = if mode_idx == 1 {
        let port = ask_port(&mut stdout, lang_en);
        RunMode::Server(port)
    } else {
        RunMode::Desktop
    };

    execute!(stdout, cursor::Show, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0)).unwrap();
    terminal::disable_raw_mode().unwrap();
    (mode, lang_code.to_string())
}

/// 应用程序主入口：读取或交互式选择运行模式，然后启动对应的运行时。
/// Application main entry: reads or interactively selects the run mode, then starts the corresponding runtime.
pub fn run_with_mode_select() {
    let mode = match load_saved_mode() {
        Some(m) => m,
        None => {
            let (m, lang) = ask_mode_interactive();
            save_mode(&m, &lang);
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
            commands::streamer_cmd::verify_streamer,
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
