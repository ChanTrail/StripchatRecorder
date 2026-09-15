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
    recording::{
        meta::schedule_meta_version_check,
        recorder::RecorderManager,
    },
    server::scheduler::{start_monitor, start_mouflon_sync, start_meta_cleanup, start_pp_load_monitor},
    watcher::fs_watch::{start_modules_dir_watcher, start_recordings_dir_watcher},
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

    // 初始化日志 / Initialize logging
    let log_dir = AppState::log_dir();
    if let Err(e) = stripchat_recorder_lib::core::logging::init_logging(&log_dir) {
        eprintln!("Failed to initialize logging: {}", e);
    }

    // 初始化应用状态 / Initialize application state
    let app_state = AppState::new().expect("Failed to initialize app state");

    // 初始化 locale 目录并加载日志翻译 / Initialize locale dirs and load log translations
    stripchat_recorder_lib::locale::manager::init_locale_dirs();
    {
        let locale_code = app_state.get_settings().language;
        stripchat_recorder_lib::locale::manager::load_log_translations(&locale_code);
    }

    // 创建 TauriEmitter / Create TauriEmitter
    let emitter: Arc<dyn Emitter> = Arc::new(TauriEmitter::new(app_handle.clone()));

    // 创建录制管理器 / Create recorder manager
    let recorder = RecorderManager::new(Arc::clone(&app_state));

    // 创建状态监控器 / Create status monitor
    let monitor = StatusMonitor::new(Arc::clone(&app_state), Arc::clone(&recorder));

    // 检测 ffmpeg 是否可用，不可用时推送通知
    // Check ffmpeg availability; push notification if unavailable
    if !stripchat_recorder_lib::recording::ffmpeg_util::ffmpeg_available() {
        app_state.notification_store.emit_i18n(            emitter.as_ref(),
            stripchat_recorder_lib::core::notifications::NotificationLevel::Error,
            "startup",
            "ffmpeg not found. Recording and post-processing will be unavailable.",
            "notifications.backend.ffmpegMissing",
            None,
        );
    }

    // 一次性启动迁移（旧 meta 文件扁平迁移到按主播子目录），迁移了文件时写入通知
    // One-shot migration of legacy flat meta files; push a notification if any were migrated
    {
        let count = stripchat_recorder_lib::recording::meta::migrate_flat_meta_files();
        if count > 0 {
            use std::collections::HashMap;
            let mut args = HashMap::new();
            args.insert("count".to_string(), serde_json::json!(count));
            app_state.notification_store.emit_i18n(
                emitter.as_ref(),
                stripchat_recorder_lib::core::notifications::NotificationLevel::Info,
                "startup",
                format!("Migrated {} legacy meta file(s) to per-streamer subdirectory layout.", count),
                "notifications.backend.metaMigrated",
                Some(args),
            );
        }
    }

    // 校验并推送自定义 locale 文件警告 / Validate and push custom locale warnings
    {
        let emitter_clone = Arc::clone(&emitter);
        tokio::task::spawn_blocking(move || {
            use stripchat_recorder_lib::core::emitter::EmitterExt;
            let warnings = stripchat_recorder_lib::locale::manager::check_custom_locale_files();
            if !warnings.is_empty() {
                let payload: Vec<serde_json::Value> = warnings
                    .into_iter()
                    .map(|(path, reason)| serde_json::json!({ "path": path, "reason": reason }))
                    .collect();
                emitter_clone.emit("locale-warnings", &payload);
            }
        });
    }

    // 将 DesktopState 注册为 Tauri 托管状态 / Register DesktopState as Tauri-managed state
    app_handle.manage(DesktopState {
        app_state: Arc::clone(&app_state),
        recorder: Arc::clone(&recorder),
        monitor: Arc::clone(&monitor),
        emitter: Arc::clone(&emitter),
        pending_update: parking_lot::RwLock::new(None),
    });

    // ── 启动后台异步任务 / Start background async tasks ──────────────────────

    // 状态监控轮询（start_monitor 内部注入 restart channel 到 app_state 和 monitor）
    // Status monitor polling (start_monitor injects restart channel into app_state and monitor)
    start_monitor(Arc::clone(&app_state), Arc::clone(&monitor), Arc::clone(&emitter));

    // Mouflon Keys 自动同步（start_mouflon_sync 内部注入 notify channel 到 app_state）
    // Mouflon Keys auto-sync (start_mouflon_sync injects notify channel into app_state)
    start_mouflon_sync(Arc::clone(&app_state), Arc::clone(&emitter));

    // 孤立 meta 文件清理（每小时）/ Orphaned meta file cleanup (every hour)
    start_meta_cleanup(Arc::clone(&app_state), Arc::clone(&emitter));

    // 后处理并发度负载自适应调节器（每 5 秒采样 CPU/内存动态调整并发许可）
    // Post-processing concurrency load-adaptive regulator (samples CPU/mem every 5s)
    start_pp_load_monitor(Arc::clone(&app_state));

    // 输出目录维护调度器：扫描/修复 meta，触发遗漏后处理，合并遗留 TS 分片
    // Output directory maintenance: scan/repair meta, trigger missed pp, merge leftover segments
    {
        let app_state_m = Arc::clone(&app_state);
        let emitter_m = Arc::clone(&emitter);
        let recorder_m = Arc::clone(&recorder);
        tokio::spawn(async move {
            schedule_meta_version_check(app_state_m, emitter_m, recorder_m, 300).await;
        });
    }

    // 文件系统监控 / File system watchers
    start_recordings_dir_watcher(Arc::clone(&app_state), Arc::clone(&emitter));
    start_modules_dir_watcher(Arc::clone(&emitter));
    stripchat_recorder_lib::watcher::fs_watch::start_locale_dir_watcher(Arc::clone(&emitter));

    // ── 阶段一结束：显示主窗口 / Phase 1 complete: show the main window ────────
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
