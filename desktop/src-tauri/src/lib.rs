//! Tauri 桌面应用库入口 / Tauri Desktop Application Library Entry
//!
//! 初始化所有后端组件（AppState、RecorderManager、StatusMonitor），
//! 注册 Tauri commands，启动后台任务（状态监控、Mouflon 同步、文件监控等）。
//!
//! Initializes all backend components (AppState, RecorderManager, StatusMonitor),
//! registers Tauri commands, and starts background tasks
//! (status monitoring, Mouflon sync, file watching, etc.).

mod commands;
mod emitter;
mod state;

use crate::emitter::TauriEmitter;
use crate::state::DesktopState;
use std::sync::Arc;
use tauri::Manager;
use stripchat_recorder_lib::{
    config::app_state::AppState,
    core::emitter::Emitter,
    platform::monitor::StatusMonitor,
    recording::recorder::RecorderManager,
    server::{scheduler, startup},
};

/// Tauri 应用的运行入口，由 `main.rs` 调用。
/// Tauri application run entry point, called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 创建专用 Tokio runtime，供 setup 和所有后台任务使用。
    // Tauri 的 setup() 回调不在 Tokio 上下文里，必须在这里建立 runtime。
    //
    // Create a dedicated Tokio runtime for setup and all background tasks.
    // Tauri's setup() callback does not run in a Tokio context, so we must
    // establish the runtime here.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    let rt = Arc::new(rt);
    let rt_for_setup = Arc::clone(&rt);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有实例在运行时，把主窗口拉到前台并聚焦
            // When another instance is launched, bring the existing window to front
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // 快速同步初始化，完成后立即显示窗口。
            // Fast synchronous initialization, then show the window immediately.
            rt_for_setup.block_on(async move {
                setup_app(app_handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Streamers
            commands::list_streamers,
            commands::add_streamer,
            commands::remove_streamer,
            commands::set_auto_record,
            commands::start_recording,
            commands::stop_recording,
            commands::verify_streamer,
            // Settings
            commands::get_settings,
            commands::save_settings_cmd,
            commands::get_disk_space,
            commands::get_system_info,
            // Mouflon Keys
            commands::list_mouflon_keys,
            commands::add_mouflon_key,
            commands::remove_mouflon_key,
            commands::sync_mouflon_keys,
            // Recordings
            commands::list_recordings,
            commands::get_merging_dirs,
            commands::delete_recording,
            commands::open_recording,
            commands::open_output_dir,
            commands::open_merged_dir,
            commands::open_output_file,
            commands::get_module_outputs,
            // Post-processing
            commands::run_postprocess_cmd,
            commands::run_postprocess_batch,
            commands::cancel_postprocess,
            commands::get_postprocess_tasks,
            // Pipeline
            commands::get_pipeline,
            commands::save_pipeline,
            commands::list_modules,
            // Community Modules
            commands::get_install_tasks,
            commands::install_community_module,
            commands::uninstall_community_module,
            // Locale
            commands::get_locale,
            commands::list_locales,
            // Notifications
            commands::get_notifications,
            commands::mark_notifications_read,
            // Startup warnings
            commands::get_startup_warnings,
            commands::remove_missing_pp_results,
            // File system
            commands::list_dir,
            commands::list_drives,
            commands::create_dir,
            // Update
            commands::check_for_updates_cmd,
            commands::apply_update_cmd,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    drop(rt);
}

/// 在 Tokio runtime 上下文中执行的应用初始化逻辑。
/// Application initialization logic executed within the Tokio runtime context.
async fn setup_app(app_handle: tauri::AppHandle) {
    // 数据根目录覆盖：改用 Tauri 的 app_data_dir()（操作系统标准的每用户数据目录），
    // 替代默认的"可执行文件同目录"约定。Desktop 端以系统安装包（NSIS/MSI、AppImage、
    // deb/rpm、dmg）分发，可执行文件所在目录可能只读（AppImage 的 FUSE 挂载点，每次
    // 启动路径还会变化）、无写权限（如 `/usr/bin`），或修改会破坏代码签名（macOS
    // `.app`），详见 `set_exe_dir_override` 文档。必须在下面任何调用 `exe_dir()`
    // 的代码（`AppState::log_dir`、`AppState::new` 等）之前完成；解析失败时静默回退
    // 到旧的 exe-relative 行为，不阻断启动。
    //
    // Data root directory override: use Tauri's app_data_dir() (the OS-standard
    // per-user data directory) instead of the default "next to the executable"
    // convention. See `set_exe_dir_override` docs for why. Must run before any code
    // below that calls `exe_dir()`. Falls back silently to the old exe-relative
    // behavior if resolution fails, so startup isn't blocked.
    match app_handle.path().app_data_dir() {
        Ok(dir) => stripchat_recorder_lib::config::app_state::set_exe_dir_override(dir),
        Err(e) => eprintln!(
            "Failed to resolve app data dir, falling back to exe-relative paths: {}",
            e
        ),
    }

    // 以下启动顺序与 Server（run_server）一致，详见 server::startup 模块文档
    // The startup order below matches Server (run_server); see the server::startup module docs

    // 在 AppState::new 之前初始化日志、locale 目录与日志翻译，让之后所有日志都有译文
    // Initialize logging, locale dirs and log translations before AppState::new so every later
    // log line is translated
    startup::init_logging_and_locale();

    // 初始化应用状态 / Initialize application state
    let app_state = AppState::new().expect("Failed to initialize app state");

    // 创建 TauriEmitter / Create TauriEmitter
    let emitter: Arc<dyn Emitter> = Arc::new(TauriEmitter::new(app_handle.clone()));

    // 创建录制管理器 / Create recorder manager
    let recorder = RecorderManager::new(Arc::clone(&app_state));

    // 创建状态监控器 / Create status monitor
    let monitor = StatusMonitor::new(Arc::clone(&app_state), Arc::clone(&recorder));

    // 尽早注册 DesktopState：只持有 Arc，没有初始化副作用，前端一加载就能调用命令
    // Register DesktopState as early as possible: it only holds Arcs with no init side effects,
    // so frontend commands work as soon as the webview loads
    app_handle.manage(DesktopState {
        app_state: Arc::clone(&app_state),
        recorder: Arc::clone(&recorder),
        monitor: Arc::clone(&monitor),
        emitter: Arc::clone(&emitter),
        pending_update: parking_lot::RwLock::new(None),
    });

    // 启动时一次性任务（与 Server 共用）：清空 tmp → meta 迁移 → ffmpeg 检查 →
    // 自定义语言文件校验 → 文件系统监控
    // One-shot startup tasks (shared with Server): clear tmp → meta migrations → ffmpeg check →
    // custom locale file validation → file system watchers
    startup::run_all(Arc::clone(&app_state), Arc::clone(&emitter));

    // 启动全部后台定时任务（与 Server 完全相同，含版本更新检查，输出目录维护启动 10 秒后首次执行）
    // Start every background scheduled task (identical to Server, including the version update
    // check; output-dir maintenance first runs 10 s after launch)
    scheduler::start_all(
        Arc::clone(&app_state),
        Arc::clone(&monitor),
        Arc::clone(&emitter),
        Arc::clone(&recorder),
    );

    // ── 阶段一结束：显示主窗口 / Phase 1 complete: show the main window ────────
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
