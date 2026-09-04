//! 启动时一次性初始化任务 / Startup One-Shot Initialization
//!
//! 该模块汇总了程序启动时需要**执行一次**的所有检查、修复和预热逻辑，
//! 使 `run_server()` 的主流程保持简洁。
//!
//! This module collects all **one-shot** checks, repairs, and warm-up tasks
//! that must run at startup, keeping the main `run_server()` flow clean.

use crate::config::app_state::AppState;
use crate::core::emitter::Emitter;
use std::sync::Arc;

/// 初始化 locale 目录，首次运行时写入内置默认语言文件。
///
/// Initialize locale directories, writing built-in default locale files on first run.
pub fn init_locale_dirs() {
    crate::locale::manager::init_locale_dirs();
}

/// 检查 ffmpeg 是否在 PATH 中可用，若不可用则记录警告并写入通知。
///
/// Check if ffmpeg is available on PATH; log a warning and push a notification if not found.
pub fn check_ffmpeg(app_state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
    if !crate::recording::ffmpeg_util::ffmpeg_available() {
        tracing::warn!("ffmpeg not found on PATH");
        app_state.notification_store.emit(
            emitter.as_ref(),
            crate::core::notifications::NotificationLevel::Error,
            "startup",
            "未检测到 ffmpeg，录制和后处理功能将无法使用。请安装 ffmpeg 并确保其在 PATH 中。",
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
        tracing::warn!("Custom locale file validation warnings: {:?}", payload);
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
        app_state.notification_store.emit(
            emitter.as_ref(),
            crate::core::notifications::NotificationLevel::Info,
            "startup",
            format!(
                "已将 {} 个旧版 meta 文件迁移到按主播子目录的新结构。",
                count
            ),
        );
    }
}

/// 在 `run_server()` 中统一执行所有启动时一次性任务。
///
/// 注意：输出目录维护（合并遗留分片、重建 meta、触发遗漏后处理）不在此处执行，
/// 而是交给 `scheduler::start_output_dir_maintenance`——它首次立即执行一遍，
/// 之后每 5 分钟重复，因此启动检查和周期性维护共用完全相同的逻辑，无需在此重复。
///
/// Run all one-shot startup tasks from `run_server()`.
///
/// Note: output-directory maintenance (merging leftover segments, rebuilding meta,
/// triggering missed post-processing) is intentionally NOT run here. It's handled by
/// `scheduler::start_output_dir_maintenance`, whose first immediate run covers the
/// startup check — so the startup pass and periodic maintenance share identical logic
/// without duplicating it here.
pub fn run_all(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    migrate_flat_meta_files(&app_state, &emitter);
    init_locale_dirs();
    check_ffmpeg(&app_state, &emitter);
    check_locale_files(Arc::clone(&emitter));
    start_fs_watchers(app_state, emitter);
}
