//! 后台定时任务调度器 / Background Scheduled Task Launchers
//!
//! 该模块汇总了所有需要**周期性运行**或**常驻后台**的任务启动逻辑与其具体实现，
//! 包括主播状态轮询、Mouflon 密钥同步、孤立 meta 文件清理和输出目录维护。
//! `config::app_state` 只负责配置数据结构与持久化，不包含任何定时/周期性逻辑。
//!
//! This module collects all **periodic** or **always-on background** task launchers
//! and their implementations, including streamer status polling, Mouflon key sync,
//! orphaned meta file cleanup, and output directory maintenance.
//! `config::app_state` is limited to config data structures and persistence; it contains
//! no scheduled/periodic logic.

use crate::config::app_state::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::notifications::NotificationLevel;
use crate::platform::monitor::StatusMonitor;
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

/// 计算从现在到本地时区次日 00:00:00 的剩余时长。
/// 若时钟异常导致结果为零，回退为 24 小时。
///
/// Compute the duration from now to the next local midnight (00:00:00).
/// Falls back to 24 hours if the result is zero due to clock anomalies.
fn secs_until_midnight() -> tokio::time::Duration {
    use chrono::{Local, Timelike};
    let now = Local::now();
    let tomorrow_midnight = (now + chrono::Duration::days(1))
        .with_hour(0).unwrap()
        .with_minute(0).unwrap()
        .with_second(0).unwrap()
        .with_nanosecond(0).unwrap();
    let secs = (tomorrow_midnight - now).num_seconds().max(0) as u64;
    tokio::time::Duration::from_secs(if secs == 0 { 86400 } else { secs })
}

/// 启动 Mouflon Keys 自动同步调度器：启动时立即同步一次，之后每天 UTC 00:00 同步一次。
/// 若 Settings 中未配置 mouflon_sync_url，则静默跳过。
/// 同步失败超出重试上限时写入错误通知。
///
/// Start the Mouflon Keys auto-sync scheduler: sync once on startup, then daily at UTC 00:00.
/// Silently skips if mouflon_sync_url is not configured in Settings.
/// Pushes an error notification when sync fails after all retries.
async fn schedule_mouflon_sync_inner(
    state: Arc<AppState>,
    emitter: Arc<dyn Emitter>,
    mut notify_rx: tokio::sync::mpsc::Receiver<()>,
) {
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
                            tracing::warn!(
                                "Mouflon keys sync failed after {} attempts: {:?}",
                                attempt, e
                            );
                            // 超出重试上限 → 写入错误通知
                            // Exceeded retries → push error notification
                            state.notification_store.emit(
                                &emitter,
                                NotificationLevel::Error,
                                "mouflon_sync",
                                format!(
                                    "Mouflon 密钥同步失败（已重试 {} 次）：{}",
                                    MAX_RETRIES, e
                                ),
                            );
                            break;
                        }
                        tracing::warn!(
                            "Mouflon keys sync failed (attempt {}/{}): {:?}, retrying in {}s",
                            attempt,
                            MAX_RETRIES,
                            e,
                            RETRY_INTERVAL.as_secs()
                        );
                        tokio::time::sleep(RETRY_INTERVAL).await;
                    }
                }
            }
        }
        // 等待到次日 UTC 0 点，或收到立即同步通知
        // Wait until next UTC midnight, or until an immediate sync notification arrives
        tokio::select! {
            _ = tokio::time::sleep(secs_until_midnight()) => {}
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

/// 启动 Mouflon 密钥自动同步调度器（启动时立即执行一次，之后每天 UTC 0 点执行）。
///
/// 将通知发送端注入 `AppState`，使手动触发同步（`/api/mouflon-keys/sync`）
/// 同样能够重置调度器计时器。
///
/// Start the Mouflon key auto-sync scheduler (runs once immediately, then daily at UTC midnight).
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
/// 若清理到孤立文件，写入信息通知。
///
/// Start the orphaned meta file cleanup scheduler (runs once immediately, then every hour).
/// Pushes an info notification if orphaned files were cleaned up.
pub fn start_meta_cleanup(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    tokio::spawn(async move {
        loop {
            let count = tokio::task::spawn_blocking(
                crate::recording::meta::cleanup_orphaned_meta_files
            )
            .await
            .unwrap_or(0);

            if count > 0 {
                app_state.notification_store.emit(
                    &emitter,
                    NotificationLevel::Info,
                    "meta_cleanup",
                    format!("已清理 {} 个孤立的 meta 文件（对应视频已不存在）", count),
                );
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
        }
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
pub fn start_output_dir_maintenance(
    app_state: Arc<AppState>,
    emitter: Arc<dyn Emitter>,
    recorder: Arc<crate::recording::recorder::RecorderManager>,
) {
    tokio::spawn(async move {
        crate::recording::meta::schedule_meta_version_check(app_state, emitter, recorder, 300)
            .await;
    });
}

/// 启动版本更新检查调度器（启动后延迟 60 秒执行第一次，之后每 24 小时检查一次）。
///
/// 发现新版本时推送 Info 通知，携带 `view_update` action 供前端跳转关于页。
/// 防重复逻辑：记录上次已通知的版本号，同一版本在进程生命周期内只通知一次。
/// Docker 环境下仍然通知（提示用户更新镜像），非 Docker 环境同样通知（提示下载）。
/// 检查失败时静默处理，不写入错误通知，等待下次周期自动重试。
///
/// Start the version update check scheduler (runs 60 s after launch, then every 24 h).
///
/// Pushes an Info notification with a `view_update` action for the frontend to navigate
/// to the About page when a new version is found.
/// Dedup: only notifies once per version per process lifetime.
/// Silently ignores check failures and retries on the next cycle.
pub fn start_update_check(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
    tokio::spawn(async move {
        // 启动时延迟 60 秒，避免在服务器刚启动、网络尚未就绪时立即查询
        // Delay 60 s at startup to avoid querying before the network is ready
        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

        // 记录本次进程已通知过的版本，避免每 24 小时重复弹同一版本的通知
        // Track the last notified version to avoid repeated notifications for the same version
        let mut last_notified_version: Option<String> = None;

        loop {
            let proxy_url = app_state.get_settings().api_proxy_url;

            match crate::update::fetch_latest_release(proxy_url.as_deref()).await {
                Ok(release) => {
                    let current = crate::update::APP_VERSION;
                    let latest = &release.latest_version;

                    // 语义化版本比较：latest > current 才推通知
                    // Semantic version comparison: only notify when latest > current
                    if crate::update::semver_gt(latest, current)
                        && last_notified_version.as_deref() != Some(latest.as_str())
                    {
                        tracing::info!(
                            "New version available: {} (current: {})",
                            latest, current
                        );

                        let is_docker = crate::update::is_docker();
                        let message = if is_docker {
                            format!(
                                "发现新版本 v{}，请更新 Docker 镜像以升级",
                                latest
                            )
                        } else {
                            format!(
                                "发现新版本 v{}，前往关于页面下载更新",
                                latest
                            )
                        };

                        app_state.notification_store.emit_with_action(
                            &emitter,
                            NotificationLevel::Info,
                            "update_check",
                            message,
                            Some(crate::core::notifications::NotificationAction {
                                action_type: "view_update".to_string(),
                                targets: vec![latest.clone()],
                            }),
                        );

                        last_notified_version = Some(latest.clone());
                    } else {
                        tracing::debug!(
                            "Update check: already on latest version {}",
                            current
                        );
                    }
                }
                Err(e) => {
                    // 静默处理，等下次周期重试
                    // Silently ignore; retry next cycle
                    tracing::debug!("Update check failed (will retry in 24 h): {:?}", e);
                }
            }

            // 每 24 小时检查一次 / Check every 24 hours
            tokio::time::sleep(tokio::time::Duration::from_secs(24 * 3600)).await;
        }
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
        Arc::clone(&monitor),
        Arc::clone(&emitter),
    );
    start_mouflon_sync(Arc::clone(&app_state), Arc::clone(&emitter));
    start_meta_cleanup(Arc::clone(&app_state), Arc::clone(&emitter));
    start_output_dir_maintenance(Arc::clone(&app_state), Arc::clone(&emitter), recorder);
    start_update_check(app_state, emitter);
}
