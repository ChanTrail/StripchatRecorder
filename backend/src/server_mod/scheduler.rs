//! 后台定时任务调度器 / Background Scheduled Task Launchers
//!
//! 该模块汇总了所有需要**周期性运行**或**常驻后台**的任务启动逻辑与其具体实现，
//! 包括主播状态轮询、配置健全性检查、Mouflon 密钥同步和 meta 文件维护。
//! `config::settings` 只负责配置数据结构与持久化，不包含任何定时/周期性逻辑。
//!
//! This module collects all **periodic** or **always-on background** task launchers
//! and their implementations, including streamer status polling, config sanity checks,
//! Mouflon key sync, and meta file maintenance.
//! `config::settings` is limited to config data structures and persistence; it contains
//! no scheduled/periodic logic.

use crate::config::settings::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::AppError;
use crate::streaming::monitor::StatusMonitor;
use std::sync::Arc;

/// 启动主播状态监控轮询循环。
///
/// 提前创建 restart channel 并将发送端注入 `AppState` 和 `StatusMonitor`，
/// 保证 `poll_interval_secs` 变更时能够立即通知监控循环重置计时器。
///
/// Start the streamer status monitoring poll loop.
///
/// A restart channel is created upfront and its sender is injected into both
/// `AppState` and `StatusMonitor`, so that changes to `poll_interval_secs`
/// can immediately notify the loop to reset its timer.
pub fn start_monitor(
    app_state: Arc<AppState>,
    monitor: Arc<StatusMonitor>,
    emitter: Arc<dyn Emitter>,
) {
    let (restart_tx, restart_rx) = tokio::sync::mpsc::channel::<()>(1);
    *app_state.poll_interval_notify_tx.write() = Some(restart_tx.clone());
    *monitor.restart_tx.write() = Some(restart_tx);

    tokio::spawn(async move {
        monitor.start_with_emitter_inner(emitter, restart_rx).await;
    });
}

/// 执行一次配置检查：验证所有追踪主播是否仍然存在，并检查孤立的后处理记录。
/// 若发现问题，通过 emitter 向前端发送 `startup-warnings` 事件。
///
/// Perform a single config check: verify all tracked streamers still exist,
/// and check for orphaned post-processing records.
/// If issues are found, emit a `startup-warnings` event to the frontend via the emitter.
async fn run_config_check(state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
    let settings = state.get_settings();
    let streamers = state.get_streamers();

    let api = match crate::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
        Some(settings.sc_mirror_scheme.as_str()),
    ) {
        Ok(a) => a,
        Err(_) => return,
    };

    // 每个主播最多重试 3 次，间隔 10 秒，确认不存在后才加入缺失列表
    // Retry up to 3 times per streamer with 10s delay; only add to missing list after confirmed
    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(10);

    let mut missing_streamers = Vec::new();
    for s in &streamers {
        let mut confirmed_missing = false;
        for attempt in 1..=MAX_ATTEMPTS {
            match api.get_stream_info(&s.username, false, s.model_id).await {
                Ok(_) => {
                    confirmed_missing = false;
                    break;
                }
                Err(AppError::UserNotFound(_)) => {
                    confirmed_missing = true;
                    break;
                }
                Err(_) => {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(RETRY_DELAY).await;
                    } else {
                        confirmed_missing = true;
                    }
                }
            }
        }
        if confirmed_missing {
            missing_streamers.push(s.username.clone());
        }
    }

    if !missing_streamers.is_empty() {
        emitter.emit(
            "startup-warnings",
            &serde_json::json!({
                "missing_streamers": missing_streamers,
            }),
        );
    }
}

/// 启动配置检查调度器：立即执行一次检查，之后每天午夜执行一次。
/// Start the config check scheduler: run once immediately, then once every day at midnight.
async fn schedule_config_checks_inner(state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    run_config_check(&state, &emitter).await;

    loop {
        // 计算到下一个午夜的等待秒数 / Calculate seconds until next midnight
        let now = chrono::Local::now();
        let secs_until = {
            let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
            let midnight = tomorrow.and_hms_opt(0, 0, 0).unwrap();
            let midnight_local = midnight
                .and_local_timezone(chrono::Local)
                .single()
                .unwrap_or_else(|| now + chrono::Duration::hours(24));
            (midnight_local - now).num_seconds().max(0) as u64
        };
        tokio::time::sleep(tokio::time::Duration::from_secs(secs_until)).await;
        run_config_check(&state, &emitter).await;
    }
}

/// 启动配置健全性检查调度器（立即执行一次，之后每天午夜执行）。
///
/// Start the config sanity-check scheduler (runs once immediately, then every midnight).
pub fn start_config_checks(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    tokio::spawn(async move {
        schedule_config_checks_inner(app_state, emitter).await;
    });
}

/// 启动 Mouflon Keys 自动同步调度器：启动时立即同步一次，之后每小时同步一次。
/// 若 Settings 中未配置 mouflon_sync_url，则静默跳过。
///
/// Start the Mouflon Keys auto-sync scheduler: sync once on startup, then every hour.
/// Silently skips if mouflon_sync_url is not configured in Settings.
async fn schedule_mouflon_sync_inner(
    state: Arc<AppState>,
    emitter: Arc<dyn Emitter>,
    mut notify_rx: tokio::sync::mpsc::Receiver<()>,
) {
    const INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(3600);
    // 失败后的重试间隔（5 分钟），最多重试 3 次
    // Retry interval after failure (5 minutes), up to 3 retries
    const RETRY_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(300);
    const MAX_RETRIES: u32 = 3;

    loop {
        let settings = state.get_settings();
        if let Some(url) = settings.mouflon_sync_url.as_deref().filter(|u| !u.is_empty()) {
            let token = settings.mouflon_sync_token.clone();
            let url = url.to_string();
            let mut attempt = 0u32;
            loop {
                match state.sync_mouflon_keys_from_worker(&url, token.as_deref()).await {
                    Ok(true) => {
                        tracing::info!("Mouflon keys synced from {}", url);
                        emitter.emit(
                            "mouflon-keys-updated",
                            &state.get_mouflon_keys_store(),
                        );
                        break;
                    }
                    Ok(false) => {
                        tracing::debug!("Mouflon keys up-to-date, skipped");
                        break;
                    }
                    Err(e) => {
                        attempt += 1;
                        if attempt >= MAX_RETRIES {
                            tracing::warn!("Mouflon keys sync failed after {} attempts: {:?}", attempt, e);
                            break;
                        }
                        tracing::warn!("Mouflon keys sync failed (attempt {}/{}): {:?}, retrying in {}s",
                            attempt, MAX_RETRIES, e, RETRY_INTERVAL.as_secs());
                        tokio::time::sleep(RETRY_INTERVAL).await;
                    }
                }
            }
        }
        // 等待 1 小时，或收到立即同步通知
        // Wait 1 hour, or until an immediate sync notification arrives
        tokio::select! {
            _ = tokio::time::sleep(INTERVAL) => {}
            v = notify_rx.recv() => {
                if v.is_none() {
                    // 发送端已关闭，退出调度器 / Sender dropped, exit scheduler
                    break;
                }
                tracing::info!("Mouflon sync: settings changed, triggering immediate sync");
            }
        }
    }
}

/// 启动 Mouflon 密钥自动同步调度器（立即执行一次，之后每小时执行）。
///
/// 将通知发送端注入 `AppState`，使手动触发同步（`/api/mouflon-keys/sync`）
/// 同样能够重置调度器计时器。
///
/// Start the Mouflon key auto-sync scheduler (runs once immediately, then every hour).
///
/// The notifier sender is injected into `AppState` so that a manual sync trigger
/// (`/api/mouflon-keys/sync`) can also reset the scheduler timer.
pub fn start_mouflon_sync(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    let (notify_tx, notify_rx) = tokio::sync::mpsc::channel::<()>(1);
    *app_state.mouflon_sync_notify_tx.write() = Some(notify_tx);

    tokio::spawn(async move {
        schedule_mouflon_sync_inner(app_state, emitter, notify_rx).await;
    });
}

/// 启动孤立 meta 文件清理调度器（立即执行一次，之后每小时执行）。
///
/// Start the orphaned meta file cleanup scheduler (runs once immediately, then every hour).
pub fn start_meta_cleanup() {
    tokio::spawn(async move {
        crate::recording::meta::schedule_meta_cleanup().await;
    });
}

/// 启动输出目录维护调度器（立即执行一次，之后每 5 分钟执行）。
///
/// 这是程序启动时和周期性维护共用的唯一入口：合并遗留分片、清理空目录、
/// 重建缺失/损坏的 meta（含 ts_merge 自定义输出目录）、对因进程重启而遗留的
/// 陈旧 pp_waiting/pp_running 视频以及遗漏的后处理任务重新触发。
/// 程序启动时不再需要单独执行一遍——首次立即执行已覆盖启动时检查的需求。
///
/// Start the output-directory maintenance scheduler (runs once immediately, then every 5 minutes).
///
/// This is the single shared entry point for both the startup pass and periodic maintenance:
/// merges leftover segments, removes empty directories, rebuilds missing/corrupt meta
/// (including ts_merge's custom output directory), and re-triggers post-processing for
/// videos left in a stale pp_waiting/pp_running state from a previous restart, as well as
/// any missed tasks. The startup path no longer needs a separate pass — the immediate first
/// run already covers it.
pub fn start_output_dir_maintenance(
    app_state: Arc<AppState>,
    emitter: Arc<dyn Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
) {
    tokio::spawn(async move {
        crate::recording::meta::schedule_meta_version_check(app_state, emitter, recorder, 300).await;
    });
}

/// 在 `run_server()` 中统一启动所有后台定时任务。
///
/// Launch all background scheduled tasks from `run_server()`.
pub fn start_all(
    app_state: Arc<AppState>,
    monitor: Arc<StatusMonitor>,
    emitter: Arc<dyn Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
) {
    start_monitor(
        Arc::clone(&app_state),
        monitor,
        Arc::clone(&emitter),
    );
    start_config_checks(Arc::clone(&app_state), Arc::clone(&emitter));
    start_mouflon_sync(Arc::clone(&app_state), Arc::clone(&emitter));
    start_meta_cleanup();
    start_output_dir_maintenance(app_state, emitter, recorder);
}
