//! 启动时一次性初始化任务 / Startup One-Shot Initialization
//!
//! 该模块汇总了程序启动时需要**执行一次**的所有检查、修复和预热逻辑。
//! Server（`server::router::run_server`）与 Desktop（`desktop-tauri` 的 `setup_app`）
//! 共用同一套函数和同一顺序，保证两端启动行为一致：
//!
//! 1. （仅 Desktop）设置数据根目录覆盖
//! 2. [`init_logging_and_locale`]：预加载日志翻译 → 初始化日志 → 初始化 locale 目录
//!    （在 `AppState::new` 之前，让之后所有日志都有译文）
//! 3. `AppState::new`：创建配置/meta/输出目录、首次运行写入 settings.json、初始化并发上限
//! 4. 构造 RecorderManager、Emitter、StatusMonitor（Desktop 随即注册托管状态）
//! 5. [`run_all`]：清空 tmp → meta 迁移 → ffmpeg 检查 → 自定义语言文件校验 → 文件系统监控
//! 6. `scheduler::start_all`：启动全部后台定时任务（两端完全相同）
//! 7. Server 开始监听 HTTP / Desktop 显示主窗口
//!
//! This module collects all **one-shot** checks, repairs, and warm-up tasks that must run at
//! startup. Server (`server::router::run_server`) and Desktop (`setup_app` in `desktop-tauri`)
//! share the same functions in the same order, so both ends start up identically:
//!
//! 1. (Desktop only) set the data root override
//! 2. [`init_logging_and_locale`]: preload log translations → initialize logging → initialize
//!    locale dirs (before `AppState::new`, so every later log line is translated)
//! 3. `AppState::new`: create config/meta/output dirs, write settings.json on first run,
//!    initialize concurrency limits
//! 4. construct RecorderManager, Emitter, StatusMonitor (Desktop then registers managed state)
//! 5. [`run_all`]: clear tmp → meta migrations → ffmpeg check → custom locale file validation →
//!    file system watchers
//! 6. `scheduler::start_all`: start every background scheduled task (identical on both ends)
//! 7. Server starts listening on HTTP / Desktop shows the main window

use crate::config::app_state::AppState;
use crate::core::emitter::Emitter;
use std::sync::Arc;

/// 初始化日志与 locale：两端在设置好数据根目录后、`AppState::new` 之前调用。
///
/// 1. 按 settings.json 中的语言预加载日志翻译（语言文件不存在时用内置默认值），
///    让日志初始化本身的提示也显示译文
/// 2. 初始化日志（`logs/`）
/// 3. 初始化 locale 目录（首次运行时写入内置默认语言文件，损坏时重建），之后重新加载一次
///    日志翻译，以便用上刚写入/重建的文件
///
/// 只读磁盘上的 settings.json（不存在时用默认语言，与 `AppState::new` 稍后写入的默认值
/// 一致），不依赖 AppState，因此 `AppState::new` 内的并发初始化日志和之后的启动迁移日志
/// 都能显示译文，而不是 key。
///
/// Initialize logging and locale; both ends call this after the data root is set and before
/// `AppState::new`.
///
/// 1. preload log translations for the language in settings.json (falling back to the
///    embedded defaults when locale files don't exist yet), so logging's own startup message
///    is translated too
/// 2. initialize logging (`logs/`)
/// 3. initialize locale dirs (writing built-in default files on first run, rebuilding corrupt
///    ones), then reload the log translations so the files just written/rebuilt are used
///
/// Only reads settings.json from disk (falling back to the default language, which matches
/// what `AppState::new` writes later) and doesn't depend on AppState, so the concurrency-init
/// logs inside `AppState::new` and the later startup migration logs are translated instead of
/// raw keys.
pub fn init_logging_and_locale() {
    let locale_code = AppState::load_settings_from_disk().language;
    crate::locale::manager::load_log_translations(&locale_code);
    if let Err(e) = crate::core::logging::init_logging(&AppState::log_dir()) {
        eprintln!("Failed to initialize logging: {}", e);
    }
    crate::locale::manager::init_locale_dirs();
    crate::locale::manager::load_log_translations(&locale_code);
}

/// 检查 ffmpeg 是否在 PATH 中可用，若不可用则记录警告并写入通知。
///
/// Check if ffmpeg is available on PATH; log a warning and push a notification if not found.
pub fn check_ffmpeg(app_state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
    if !crate::recording::ffmpeg_util::ffmpeg_available() {
        tracing::warn!("{}", crate::tl!("startup.ffmpegMissing"));
        app_state.notification_store.emit_i18n(
            emitter.as_ref(),
            crate::core::notifications::NotificationLevel::Error,
            "startup",
            "ffmpeg not found. Recording and post-processing will be unavailable.",
            "notifications.backend.ffmpegMissing",
            None,
        );
    }
}

/// 扫描用户自定义语言文件，将校验警告通过 SSE 推送给前端。
/// 在 emitter 就绪后于 `spawn_blocking` 中执行，避免阻塞 async 运行时。
///
/// Scan user-defined locale files and push validation warnings to the frontend via SSE.
/// Runs inside `spawn_blocking` after the emitter is ready to avoid blocking the async runtime.
pub fn check_locale_files(emitter: Arc<dyn Emitter>) {
    tokio::task::spawn_blocking(move || {
        use crate::core::emitter::EmitterExt;
        let warnings = crate::locale::manager::check_custom_locale_files();
        if warnings.is_empty() {
            return;
        }
        let payload: Vec<serde_json::Value> = warnings
            .into_iter()
            .map(|(path, reason)| serde_json::json!({ "path": path, "reason": reason }))
            .collect();
        tracing::warn!("{}", crate::tl!("startup.localeFileWarning", payload = format!("{:?}", payload)));
        emitter.emit("locale-warnings", &payload);
    });
}

/// 启动所有文件系统监控器（录制目录、模块目录、locale 目录）。
///
/// Start all file system watchers (recordings dir, modules dir, locale dir).
pub fn start_fs_watchers(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    crate::watcher::fs_watch::start_recordings_dir_watcher(
        Arc::clone(&app_state),
        Arc::clone(&emitter),
    );
    crate::watcher::fs_watch::start_modules_dir_watcher(Arc::clone(&emitter));
    crate::watcher::fs_watch::start_locale_dir_watcher(emitter);
}

/// 一次性迁移旧版扁平 meta 文件（升级前生成、直接平铺于 meta 根目录下）到按主播
/// 分子目录的新结构。迁移了文件时写入 Info 通知。
///
/// One-shot migration of legacy flat meta files into the new per-streamer subdirectory layout.
/// Pushes an Info notification if any files were migrated.
pub fn migrate_flat_meta_files(app_state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
    let count = crate::recording::meta::migrate_flat_meta_files();
    if count > 0 {
        use std::collections::HashMap;
        let mut args = HashMap::new();
        args.insert("count".to_string(), serde_json::json!(count));
        app_state.notification_store.emit_i18n(
            emitter.as_ref(),
            crate::core::notifications::NotificationLevel::Info,
            "startup",
            format!("Migrated {} legacy meta file(s) to per-streamer subdirectory layout.", count),
            "notifications.backend.metaMigrated",
            Some(args),
        );
    }
    // 修正旧 stem 规则截断文件名的 meta（含 '.' 用户名，B16），只记日志不发通知；
    // 必须在首次扫描前执行
    // Fix meta files whose names were truncated by the old stem rule (usernames with '.',
    // B16); logs only, no notification; must run before the first scan
    crate::recording::meta::migrate_truncated_stem_meta_files();
}

/// 统一执行所有启动时一次性任务，Server 与 Desktop 都调用本函数（在
/// [`init_logging_and_locale`] 与 `AppState::new` 之后、`scheduler::start_all` 之前）。
///
/// 执行顺序及原因：
/// 1. 清空 tmp：上一个进程的后处理都已结束，tmp 里全是残留；必须在任何可能运行模块的
///    组件（状态监控、维护调度）启动之前执行
/// 2. meta 迁移（扁平 meta → 按主播子目录；修正截断文件名）：必须在首次扫描之前
/// 3. ffmpeg 检查：缺失时写入通知
/// 4. 自定义语言文件校验：在后台线程执行，有问题时推送 `locale-warnings`
/// 5. 文件系统监控：只在文件变化时推送事件
///
/// 注意：输出目录维护（重建 meta、触发遗漏后处理、清理空目录、tmp 定时清理）不在此处执行，
/// 而是交给 `scheduler::start_output_dir_maintenance`——它启动 10 秒后执行第一遍，
/// 之后每 5 分钟重复，启动检查和周期性维护共用完全相同的逻辑。
///
/// Run all one-shot startup tasks; both Server and Desktop call this (after
/// [`init_logging_and_locale`] and `AppState::new`, before `scheduler::start_all`).
///
/// Order and rationale:
/// 1. clear tmp: all post-processing of the previous process has ended, so everything in tmp
///    is a leftover; must run before any component that may run modules (status monitor,
///    maintenance scheduler) starts
/// 2. meta migrations (flat meta → per-streamer subdirs; truncated file names): must precede
///    the first scan
/// 3. ffmpeg check: pushes a notification when missing
/// 4. custom locale file validation: runs on a blocking thread, pushes `locale-warnings` on issues
/// 5. file system watchers: only push events when files change
///
/// Note: output-directory maintenance (rebuilding meta, triggering missed post-processing,
/// removing empty dirs, the scheduled tmp cleanup) is intentionally NOT run here. It's handled
/// by `scheduler::start_output_dir_maintenance`, which first runs 10 s after launch and then
/// every 5 minutes, so the startup pass and periodic maintenance share identical logic.
pub fn run_all(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    crate::recording::meta::cleanup_tmp_on_startup();
    migrate_flat_meta_files(&app_state, &emitter);
    check_ffmpeg(&app_state, &emitter);
    check_locale_files(Arc::clone(&emitter));
    start_fs_watchers(app_state, emitter);
}
