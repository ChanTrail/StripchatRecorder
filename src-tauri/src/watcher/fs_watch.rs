//! 文件系统监控 / File System Watchers
//!
//! 提供两个文件监控器：
//! 1. 录制输出目录监控：检测文件变化并向前端发送 `recordings-dir-changed` 事件（防抖 400ms）
//! 2. 模块目录监控：检测模块可执行文件的增删并发送 `modules-changed` 事件（防抖 500ms）
//!
//! Provides two file system watchers:
//! 1. Recording output directory watcher: detects file changes and emits `recordings-dir-changed` events (400ms debounce)
//! 2. Modules directory watcher: detects module executable additions/removals and emits `modules-changed` events (500ms debounce)

use crate::core::emitter::{Emitter, EmitterExt};
use crate::config::settings::AppState;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 判断路径是否为"噪声"路径（.ts/.tmp/.part/.partial 文件），这些文件变化频繁，不需要触发刷新。
/// Check if a path is a "noisy" path (.ts/.tmp/.part/.partial files) that change frequently
/// and should not trigger a refresh.
fn is_noisy_path(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
    {
        Some(ext) if ext == "ts" || ext == "tmp" || ext == "part" || ext == "partial" => true,
        _ => false,
    }
}

/// 判断文件系统事件是否应该触发前端刷新。
/// 过滤掉纯访问事件和噪声路径的变化。
///
/// Determine if a file system event should trigger a frontend refresh.
/// Filters out pure access events and changes to noisy paths.
fn should_emit(event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().any(|p| !is_noisy_path(p))
}

/// 启动录制输出目录监控器（在独立线程中运行）。
/// 当输出目录设置变更时自动切换监控目标。
///
/// Start the recording output directory watcher (runs in a dedicated thread).
/// Automatically switches the watch target when the output directory setting changes.
pub fn start_recordings_dir_watcher(state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();

        let mut watched_dir = PathBuf::new();
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut _watcher: Option<RecommendedWatcher> = None;

        loop {
            let current_dir = PathBuf::from(state.get_settings().output_dir);
            // 若输出目录发生变化，重新创建监控器 / If output dir changed, recreate the watcher
            if current_dir != watched_dir {
                if let Err(e) = std::fs::create_dir_all(&current_dir) {
                    tracing::error!("Failed to create watch dir {:?}: {}", current_dir, e);
                }

                match RecommendedWatcher::new(tx.clone(), Config::default()) {
                    Ok(mut w) => match w.watch(&current_dir, RecursiveMode::Recursive) {
                        Ok(()) => {
                            tracing::info!("Watching recordings dir: {:?}", current_dir);
                            watched_dir = current_dir;
                            _watcher = Some(w);
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to watch recordings dir {:?}: {}",
                                current_dir,
                                e
                            );
                        }
                    },
                    Err(e) => {
                        tracing::error!("Failed to create watcher: {}", e);
                    }
                }
            }

            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Ok(event)) => {
                    if watched_dir.as_os_str().is_empty() || !should_emit(&event) {
                        continue;
                    }
                    // 防抖：400ms 内的多次事件只触发一次 / Debounce: only emit once within 400ms
                    if last_emit.elapsed() < Duration::from_millis(400) {
                        continue;
                    }
                    last_emit = Instant::now();
                    emitter.emit(
                        "recordings-dir-changed",
                        &serde_json::json!({
                            "outputDir": watched_dir,
                            "kind": format!("{:?}", event.kind),
                            "paths": event.paths,
                        }),
                    );
                }
                Ok(Err(e)) => tracing::error!("recordings watcher event error: {}", e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    tracing::error!("recordings watcher channel disconnected");
                    break;
                }
            }
        }
    });
}

/// 启动模块目录监控器（在独立线程中运行）。
/// 检测 modules/ 目录中可执行文件的增删，发送 `modules-changed` 事件。
///
/// Start the modules directory watcher (runs in a dedicated thread).
/// Detects additions/removals of executables in the modules/ directory and emits `modules-changed` events.
pub fn start_modules_dir_watcher(emitter: Arc<dyn Emitter>) {
    std::thread::spawn(move || {
        let modules_dir = crate::postprocess::pipeline::modules_dir();
        let _ = std::fs::create_dir_all(&modules_dir);

        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut last_emit = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(mut w) => {
                // 只监控模块目录本身，不递归 / Watch only the modules dir itself, non-recursive
                if let Err(e) = w.watch(&modules_dir, RecursiveMode::NonRecursive) {
                    tracing::error!("Failed to watch modules dir: {}", e);
                } else {
                    tracing::info!("Watching modules dir: {:?}", modules_dir);
                }
                w
            }
            Err(e) => {
                tracing::error!("Failed to create modules watcher: {}", e);
                return;
            }
        };
        let _watcher = &mut watcher;

        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(event)) => {
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    // 防抖：500ms 内的多次事件只触发一次 / Debounce: only emit once within 500ms
                    if last_emit.elapsed() < Duration::from_millis(500) {
                        continue;
                    }
                    last_emit = Instant::now();
                    emitter.emit("modules-changed", &serde_json::json!({}));
                }
                Ok(Err(e)) => tracing::error!("modules watcher error: {}", e),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
}
