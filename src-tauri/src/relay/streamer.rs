//! 流转发 Worker / Stream Relay Worker
//!
//! 每个主播对应一个持久 worker，行为如下：
//! - 上游在线：拉取 fMP4 分片 → 喂给持久 ffmpeg 进程 → 输出 MPEG-TS
//! - 上游离线：用 ffmpeg lavfi 生成黑屏+状态文字画面持续输出
//! - 状态切换时无缝衔接（同一个 broadcast channel，播放器不断流）

use super::state::{RelayManager, RelayStreamState};
use crate::config::settings::AppState;
use crate::recording::hls::{get_url_prefix, parse_playlist};
use crate::streaming::stripchat::StripchatApi;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};

/// 启动流转发 worker。
pub fn start_streamer(
    username: String,
    app_state: Arc<AppState>,
    relay_manager: Arc<RelayManager>,
) -> (mpsc::Sender<()>, broadcast::Sender<Arc<Vec<u8>>>) {
    let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
    let (ts_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(256);
    let ts_tx_clone = ts_tx.clone();

    tokio::spawn(worker_loop(
        username,
        app_state,
        relay_manager,
        stop_rx,
        ts_tx_clone,
    ));

    (stop_tx, ts_tx)
}

/// 无播放器连接超过此时长（秒）后自动停止转发 worker。
/// Auto-stop relay worker after this many seconds with no connected players.
const IDLE_STOP_SECS: u64 = 5; // 5 秒 / 5 seconds

/// 子函数退出原因 / Reason a sub-function exited
enum WorkerExit {
    /// 流自然结束（上游断流/离线轮询超时），应继续外层循环重新检查状态
    /// Stream ended naturally; outer loop should continue and recheck state
    StreamEnded,
    /// 收到停止信号，应退出整个 worker
    /// Stop signal received; exit the whole worker
    Stopped,
    /// 空闲超时，应退出整个 worker
    /// Idle timeout; exit the whole worker
    Idle,
}

/// Worker 主循环 / Worker main loop
async fn worker_loop(
    username: String,
    app_state: Arc<AppState>,
    relay_manager: Arc<RelayManager>,
    mut stop_rx: mpsc::Receiver<()>,
    ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
) {
    tracing::info!("Relay worker started for {}", username);

    loop {
        // 检查停止信号 / Check stop signal
        if stop_rx.try_recv().is_ok() {
            break;
        }

        // 检查空闲超时：无连接超过 IDLE_STOP_SECS 秒则自动停止
        // Check idle timeout: auto-stop if no connections for IDLE_STOP_SECS seconds
        if relay_manager.is_idle(&username, IDLE_STOP_SECS) {
            tracing::info!(
                "Relay worker: no connections for {}s, auto-stopping for {}",
                IDLE_STOP_SECS,
                username
            );
            break;
        }

        // 尝试获取上游播放列表 URL / Try to get upstream playlist URL
        let settings = app_state.get_settings();
        let api = match StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
        ) {
            Ok(a) => Arc::new(a.with_mouflon_keys(app_state.get_mouflon_keys())),
            Err(e) => {
                tracing::error!("Relay worker: API client error for {}: {}", username, e);
                relay_manager.set_state(&username, RelayStreamState::Error {
                    message: format!("API client error: {}", e),
                });
                tokio::select! {
                    _ = stop_rx.recv() => break,
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {}
                }
                continue;
            }
        };

        relay_manager.set_state(&username, RelayStreamState::Connecting);

        match api.get_stream_info(&username, true).await {
            Ok(info) => {
                if let Some(playlist_url) = info.playlist_url {
                    // 上游在线，启动直播转发 / Upstream live, start live relay
                    relay_manager.set_playlist_url(&username, Some(playlist_url.clone()));
                    relay_manager.set_state(&username, RelayStreamState::Live);

                    let exit = run_live_relay(
                        &username,
                        &playlist_url,
                        &app_state,
                        &relay_manager,
                        &ts_tx,
                        &mut stop_rx,
                    ).await;

                    relay_manager.set_playlist_url(&username, None);

                    match exit {
                        WorkerExit::StreamEnded => {} // 继续外层循环 / Continue outer loop
                        WorkerExit::Stopped | WorkerExit::Idle => break,
                    }
                } else {
                    // 上游离线，输出状态画面 / Upstream offline, output status frame
                    let status_text = info.status.clone();
                    relay_manager.set_state(&username, RelayStreamState::Offline {
                        status: status_text.clone(),
                    });

                    let exit = run_offline_relay(
                        &username,
                        &status_text,
                        &relay_manager,
                        &ts_tx,
                        &mut stop_rx,
                    ).await;

                    match exit {
                        WorkerExit::StreamEnded => {}
                        WorkerExit::Stopped | WorkerExit::Idle => break,
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Relay worker: get_stream_info failed for {}: {}", username, e);
                relay_manager.set_state(&username, RelayStreamState::Error {
                    message: e.to_string(),
                });

                let exit = run_offline_relay(
                    &username,
                    "Status Unavailable",
                    &relay_manager,
                    &ts_tx,
                    &mut stop_rx,
                ).await;

                match exit {
                    WorkerExit::StreamEnded => {}
                    WorkerExit::Stopped | WorkerExit::Idle => break,
                }
            }
        }

        // 短暂等待后重新检查状态，同时响应停止信号和空闲超时
        // Brief wait before rechecking state, also respond to stop signal and idle timeout
        tokio::select! {
            _ = stop_rx.recv() => break,
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                if relay_manager.is_idle(&username, IDLE_STOP_SECS) {
                    tracing::info!("Relay worker: idle after recheck wait, stopping for {}", username);
                    break;
                }
            }
        }
    }

    relay_manager.remove(&username);
    tracing::info!("Relay worker stopped for {}", username);
}

/// 运行直播转发，返回退出原因。
/// Run live relay. Returns the reason for exiting.
async fn run_live_relay(
    username: &str,
    initial_playlist_url: &str,
    app_state: &AppState,
    relay_manager: &RelayManager,
    ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> WorkerExit {
    let mut last_settings = app_state.get_settings();
    let mut api = match StripchatApi::new_api_only(
        last_settings.api_proxy_url.as_deref(),
        last_settings.cdn_proxy_url.as_deref(),
        last_settings.sc_mirror_url.as_deref(),
    ) {
        Ok(a) => a.with_mouflon_keys(app_state.get_mouflon_keys()),
        Err(_) => return WorkerExit::StreamEnded,
    };
    let mut last_mouflon_keys = app_state.get_mouflon_keys();

    // 启动持久 ffmpeg 进程 / Start persistent ffmpeg process
    let mut child = match tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-copyts",
            "-f", "mp4",
            "-i", "pipe:0",
            "-c", "copy",
            "-f", "mpegts",
            "-mpegts_flags", "pat_pmt_at_frames",
            "-metadata", &format!("service_name={}", username),
            "-metadata", "service_provider=StripchatRecorder",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Relay: failed to spawn ffmpeg for {}: {}", username, e);
            return WorkerExit::StreamEnded;
        }
    };

    let mut ffmpeg_stdin = child.stdin.take().unwrap();
    let mut ffmpeg_stdout = child.stdout.take().unwrap();

    let (fmp4_tx, mut fmp4_rx) = mpsc::channel::<Vec<u8>>(32);

    // stdin 写入任务 / stdin writer task
    let stdin_task = tokio::spawn(async move {
        while let Some(data) = fmp4_rx.recv().await {
            if ffmpeg_stdin.write_all(&data).await.is_err() {
                break;
            }
        }
        let _ = ffmpeg_stdin.shutdown().await;
    });

    // stdout 读取任务 / stdout reader task
    let ts_tx_clone = ts_tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match ffmpeg_stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => { let _ = ts_tx_clone.send(Arc::new(buf[..n].to_vec())); }
            }
        }
    });

    let mut current_url = initial_playlist_url.to_string();
    let mut url_prefix = get_url_prefix(&current_url);
    let mut downloaded: HashSet<u32> = HashSet::new();
    let mut init_data: Option<Vec<u8>> = None;
    let mut cached_init_url: Option<String> = None;
    let mut consecutive_failures: u32 = 0;
    const MAX_FAILURES: u32 = 10;

    let result = loop {
        if stop_rx.try_recv().is_ok() {
            break WorkerExit::Stopped;
        }

        // 空闲检查：无连接超过阈值则退出 / Idle check: exit if no connections for threshold
        if relay_manager.is_idle(username, IDLE_STOP_SECS) {
            tracing::info!("Relay live: idle timeout, stopping for {}", username);
            break WorkerExit::Idle;
        }

        // 检测代理/密钥设置变更，变更时重建 api 实例使其立即生效
        // Detect proxy/key setting changes and rebuild api instance for immediate effect
        let current_settings = app_state.get_settings();
        let current_mouflon_keys = app_state.get_mouflon_keys();
        let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
            || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
            || current_settings.sc_mirror_url != last_settings.sc_mirror_url;
        let keys_changed = current_mouflon_keys != last_mouflon_keys;
        if proxy_changed || keys_changed {
            match StripchatApi::new_api_only(
                current_settings.api_proxy_url.as_deref(),
                current_settings.cdn_proxy_url.as_deref(),
                current_settings.sc_mirror_url.as_deref(),
            ) {
                Ok(new_api) => {
                    api = new_api.with_mouflon_keys(current_mouflon_keys.clone());
                    tracing::info!("Relay live {}: api client rebuilt due to settings change", username);
                }
                Err(e) => {
                    tracing::warn!("Relay live {}: failed to rebuild api client: {}", username, e);
                }
            }
            last_settings = current_settings;
            last_mouflon_keys = current_mouflon_keys;
        }

        match poll_and_feed(
            &api, username, &current_url, &url_prefix,
            app_state, &fmp4_tx,
            &mut downloaded, &mut init_data, &mut cached_init_url,
        ).await {
            Ok(had_new) => {
                consecutive_failures = 0;
                if !had_new {
                    tokio::select! {
                        _ = stop_rx.recv() => break WorkerExit::Stopped,
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(1000)) => {
                            if relay_manager.is_idle(username, IDLE_STOP_SECS) {
                                tracing::info!("Relay live: idle timeout during poll wait, stopping for {}", username);
                                break WorkerExit::Idle;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("Relay live: poll failed for {} ({}/{}): {}", username, consecutive_failures, MAX_FAILURES, e);

                if consecutive_failures >= MAX_FAILURES {
                    break WorkerExit::StreamEnded;
                }

                if let Ok(info) = api.get_stream_info(username, true).await {
                    if let Some(new_url) = info.playlist_url {
                        url_prefix = get_url_prefix(&new_url);
                        current_url = new_url;
                        consecutive_failures = 0;
                    } else if !info.is_recordable {
                        break WorkerExit::StreamEnded;
                    }
                }

                tokio::select! {
                    _ = stop_rx.recv() => break WorkerExit::Stopped,
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(2000)) => {
                        if relay_manager.is_idle(username, IDLE_STOP_SECS) {
                            tracing::info!("Relay live: idle timeout during error wait, stopping for {}", username);
                            break WorkerExit::Idle;
                        }
                    }
                }
            }
        }
    };

    drop(fmp4_tx);
    let _ = stdin_task.await;
    let _ = stdout_task.await;
    let _ = child.wait().await;

    result
}

/// 检测系统中可用的中文字体路径。
/// Detect available CJK font path on the system.
fn find_cjk_font() -> Option<String> {
    // 按优先级尝试常见字体路径 / Try common font paths in priority order
    let candidates: &[&str] = &[
        // Windows
        "C:/Windows/Fonts/msyh.ttc",      // 微软雅黑
        "C:/Windows/Fonts/msyhbd.ttc",
        "C:/Windows/Fonts/simsun.ttc",    // 宋体
        "C:/Windows/Fonts/simhei.ttf",    // 黑体
        "C:/Windows/Fonts/STZHONGS.TTF",  // 华文中宋
        // Linux
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];

    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}
/// 将中文状态文字映射为英文 ASCII 替代（无中文字体时使用）。
/// Map Chinese status text to English ASCII fallback (used when no CJK font available).
fn to_ascii_status(s: &str) -> String {
    match s {
        "公开秀" => "Public Show".to_string(),
        "私密秀" => "Private Show".to_string(),
        "票务秀" => "Ticket Show".to_string(),
        "计时秀" => "Per-Minute Show".to_string(),
        "群组秀" => "Group Show".to_string(),
        "虚拟私密" => "Virtual Private".to_string(),
        "等待" => "Waiting".to_string(),
        "离线" => "Offline".to_string(),
        "获取状态失败" => "Status Unavailable".to_string(),
        _ => s.to_string(),
    }
}

async fn run_offline_relay(
    username: &str,
    status_text: &str,
    relay_manager: &RelayManager,
    ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> WorkerExit {
    // 转义 drawtext 特殊字符 / Escape drawtext special characters
    let escape = |s: &str| {
        s.replace('\\', "\\\\")
         .replace('\'', "\\'")
         .replace(':', "\\:")
         .replace('[', "\\[")
         .replace(']', "\\]")
    };

    let username_esc = escape(username);

    // 构建 drawtext 滤镜，若有中文字体则指定 fontfile
    // Build drawtext filter, specify fontfile if CJK font is available
    let drawtext = match find_cjk_font() {
        Some(font_path) => {
            // Windows 路径需要转义反斜杠 / Windows paths need backslash escaping
            let font_esc = font_path.replace('\\', "/").replace(':', "\\:");
            let status_esc = escape(status_text);
            format!(
                "drawtext=fontfile='{}':text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=fontfile='{}':text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                font_esc, username_esc, font_esc, status_esc
            )
        }
        None => {
            // 无中文字体，状态文字转为英文 / No CJK font, convert status to English
            let status_ascii = escape(&to_ascii_status(status_text));
            format!(
                "drawtext=text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                username_esc, status_ascii,
            )
        }
    };

    let mut child = match tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi",
            "-i", &format!("color=c=black:s=1280x720:r=5,{}", drawtext),
            "-f", "lavfi",
            "-i", "anullsrc=r=48000:cl=stereo",
            "-c:v", "libx264",
            "-preset", "ultrafast",
            "-tune", "stillimage",
            "-c:a", "aac",
            "-b:a", "32k",
            "-f", "mpegts",
            "-metadata", &format!("service_name={}", username),
            "-metadata", "service_provider=StripchatRecorder",
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Relay offline: failed to spawn ffmpeg for {}: {}", username, e);
            // ffmpeg 不可用时等待后重试 / Wait and retry if ffmpeg unavailable
            tokio::select! {
                _ = stop_rx.recv() => return WorkerExit::Stopped,
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {}
            }
            return WorkerExit::StreamEnded;
        }
    };

    let mut stdout = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 65536];

    // 输出状态画面约 30 秒后重新检查上游状态
    // Output status frame for ~30s then recheck upstream status
    let check_interval = tokio::time::Duration::from_secs(30);
    let deadline = tokio::time::Instant::now() + check_interval;

    let result = loop {
        if stop_rx.try_recv().is_ok() {
            break WorkerExit::Stopped;
        }

        // 空闲检查 / Idle check
        if relay_manager.is_idle(username, IDLE_STOP_SECS) {
            tracing::info!("Relay offline: idle timeout, stopping for {}", username);
            break WorkerExit::Idle;
        }

        if tokio::time::Instant::now() >= deadline {
            break WorkerExit::StreamEnded; // 超时，重新检查 / Timeout, recheck
        }

        tokio::select! {
            _ = stop_rx.recv() => break WorkerExit::Stopped,
            n = stdout.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => break WorkerExit::StreamEnded,
                    Ok(n) => { let _ = ts_tx.send(Arc::new(buf[..n].to_vec())); }
                }
            }
            // 每 100ms 检查一次 idle 和 deadline
            // Check idle and deadline every 100ms
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                if relay_manager.is_idle(username, IDLE_STOP_SECS) {
                    tracing::info!("Relay offline: idle timeout during output, stopping for {}", username);
                    break WorkerExit::Idle;
                }
                if tokio::time::Instant::now() >= deadline {
                    break WorkerExit::StreamEnded;
                }
            }
        }
    };

    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

/// 拉取一次播放列表，下载新分片，将 fMP4 数据发送给 ffmpeg stdin。
#[allow(clippy::too_many_arguments)]
async fn poll_and_feed(
    api: &StripchatApi,
    username: &str,
    playlist_url: &str,
    url_prefix: &str,
    _app_state: &AppState,
    fmp4_tx: &mpsc::Sender<Vec<u8>>,
    downloaded: &mut HashSet<u32>,
    init_data: &mut Option<Vec<u8>>,
    cached_init_url: &mut Option<String>,
) -> Result<bool, String> {
    // 直接使用 api 实例中已更新的 mouflon_keys（由外层循环动态重建保证最新）
    // Use mouflon_keys from the api instance (kept up-to-date by the outer loop's dynamic rebuild)
    let mouflon_keys = api.mouflon_keys();

    let playlist_text = api.fetch_playlist(playlist_url).await
        .map_err(|e| e.to_string())?;

    let (segments, new_init_url) = parse_playlist(&playlist_text, url_prefix, mouflon_keys)
        .map_err(|e| e.to_string())?;

    let init_url_path = |u: &str| u.split('?').next().unwrap_or(u).to_string();
    let new_init_path = new_init_url.as_deref().map(init_url_path);
    let cached_path = cached_init_url.as_deref().map(init_url_path);

    if new_init_path.is_some() && new_init_path != cached_path
        && let Some(ref url) = new_init_url
    {
        match api.download_segment(url).await {
            Ok(data) => {
                *init_data = Some(data);
                *cached_init_url = Some(url.clone());
            }
            Err(e) => return Err(format!("Failed to download init segment: {}", e)),
        }
    }

    let mut had_new = false;
    for seg in segments {
        if downloaded.contains(&seg.sequence) {
            continue;
        }

        let seg_bytes = match api.download_segment(&seg.url).await {
            Ok(d) if d.len() > 1000 => d,
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!("Relay: failed to download segment {} for {}: {}", seg.sequence, username, e);
                continue;
            }
        };

        let fmp4 = match init_data.as_deref() {
            Some(init) => {
                let mut v = Vec::with_capacity(init.len() + seg_bytes.len());
                v.extend_from_slice(init);
                v.extend_from_slice(&seg_bytes);
                v
            }
            None => seg_bytes,
        };

        if fmp4_tx.send(fmp4).await.is_err() {
            return Err("ffmpeg stdin channel closed".to_string());
        }

        downloaded.insert(seg.sequence);
        had_new = true;
    }

    Ok(had_new)
}
