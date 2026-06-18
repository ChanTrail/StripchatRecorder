//! 流转发 Worker / Stream Relay Worker
//!
//! 架构：无中间层，各状态直接输出 MPEG-TS 广播给播放器。
//! - 上游在线：HLS fMP4 分片 → converter ffmpeg → MPEG-TS 广播
//! - 上游离线：离线源 ffmpeg (lavfi) → MPEG-TS 广播
//! - 状态切换时共用同一个 broadcast channel，播放器不断连。
//!   切换瞬间时间戳会跳变，播放器通常能自动适应（重新缓冲约 1 秒）。

use super::state::{RelayManager, RelayStreamState};
use crate::config::settings::AppState;
use crate::recording::hls::{get_url_prefix, parse_playlist};
use crate::streaming::stripchat::StripchatApi;
use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};

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

const IDLE_STOP_SECS: u64 = 30;

fn find_cjk_font() -> Option<String> {
    let candidates: &[&str] = &[
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyhbd.ttc",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/STZHONGS.TTF",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
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

fn escape_drawtext(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('\'', "\\'")
     .replace(':', "\\:")
     .replace('[', "\\[")
     .replace(']', "\\]")
}

fn build_drawtext(username: &str, status_text: &str) -> String {
    let username_esc = escape_drawtext(username);
    match find_cjk_font() {
        Some(font_path) => {
            let font_esc = font_path.replace('\\', "/").replace(':', "\\:");
            let status_esc = escape_drawtext(status_text);
            format!(
                "drawtext=fontfile='{}':text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=fontfile='{}':text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                font_esc, username_esc, font_esc, status_esc
            )
        }
        None => {
            let status_ascii = escape_drawtext(&to_ascii_status(status_text));
            format!(
                "drawtext=text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                username_esc, status_ascii,
            )
        }
    }
}

async fn worker_loop(
    username: String,
    app_state: Arc<AppState>,
    relay_manager: Arc<RelayManager>,
    mut stop_rx: mpsc::Receiver<()>,
    ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
) {
    tracing::info!("Relay worker started for {}", username);

    loop {
        if stop_rx.try_recv().is_ok() { break; }
        if relay_manager.is_idle(&username, IDLE_STOP_SECS) {
            tracing::info!("Relay worker: idle, stopping for {}", username);
            break;
        }

        let settings = app_state.get_settings();
        let api = match StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
        ) {
            Ok(a) => Arc::new(a.with_mouflon_keys(app_state.get_mouflon_keys())),
            Err(e) => {
                tracing::error!("Relay: API client error for {}: {}", username, e);
                relay_manager.set_state(&username, RelayStreamState::Error { message: e.to_string() });
                tokio::select! {
                    _ = stop_rx.recv() => break,
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
                }
                continue;
            }
        };

        relay_manager.set_state(&username, RelayStreamState::Connecting);

        match api.get_stream_info(&username, true).await {
            Ok(info) if info.playlist_url.is_some() => {
                let playlist_url = info.playlist_url.unwrap();
                tracing::info!("Relay worker [{}]: upstream live", username);
                relay_manager.set_playlist_url(&username, Some(playlist_url.clone()));
                relay_manager.set_state(&username, RelayStreamState::Live);
                relay_manager.set_streamer_status(&username, info.is_online, info.status);

                let cont = feed_live(
                    &username, &playlist_url, &app_state, &relay_manager, &ts_tx, &mut stop_rx,
                ).await;

                relay_manager.set_playlist_url(&username, None);
                if !cont { break; }
            }
            Ok(info) => {
                let status_text = info.status.clone();
                tracing::info!("Relay worker [{}]: upstream offline ({})", username, status_text);
                relay_manager.set_state(&username, RelayStreamState::Offline { status: status_text.clone() });
                relay_manager.set_streamer_status(&username, info.is_online, info.status);

                let cont = feed_offline(
                    &username, &status_text, &relay_manager, &ts_tx, &mut stop_rx,
                    Some(&app_state), 30,
                ).await;
                if !cont { break; }
            }
            Err(e) => {
                tracing::warn!("Relay worker [{}]: get_stream_info failed: {}", username, e);
                relay_manager.set_state(&username, RelayStreamState::Error { message: e.to_string() });

                let cont = feed_offline(
                    &username, "Status Unavailable", &relay_manager, &ts_tx, &mut stop_rx,
                    Some(&app_state), 30,
                ).await;
                if !cont { break; }
            }
        }

        tokio::select! {
            _ = stop_rx.recv() => break,
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                if relay_manager.is_idle(&username, IDLE_STOP_SECS) { break; }
            }
        }
    }

    relay_manager.remove(&username);
    tracing::info!("Relay worker stopped for {}", username);
}

/// 离线阶段：直接用 lavfi ffmpeg 输出 MPEG-TS，无中间层。
/// 每 5 秒检测上游状态，上线时立即返回。
async fn feed_offline(
    username: &str,
    status_text: &str,
    relay_manager: &RelayManager,
    ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
    stop_rx: &mut mpsc::Receiver<()>,
    app_state: Option<&AppState>,
    max_secs: u64,
) -> bool {
    let drawtext = build_drawtext(username, status_text);

    let mut src = match tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-re",
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
            "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Relay offline [{}]: failed to spawn ffmpeg: {}", username, e);
            tokio::select! {
                _ = stop_rx.recv() => return false,
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {}
            }
            return true;
        }
    };

    let mut stdout = src.stdout.take().unwrap();
    let mut buf = vec![0u8; 65536];
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(max_secs);

    let mut upstream_interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
    upstream_interval.tick().await;

    let should_continue = loop {
        if stop_rx.try_recv().is_ok() { break false; }
        if relay_manager.is_idle(username, IDLE_STOP_SECS) { break false; }
        if tokio::time::Instant::now() >= deadline { break true; }

        tokio::select! {
            biased;
            _ = stop_rx.recv() => break false,

            _ = upstream_interval.tick(), if app_state.is_some() => {
                if let Some(state) = app_state {
                    let settings = state.get_settings();
                    if let Ok(api) = StripchatApi::new_api_only(
                        settings.api_proxy_url.as_deref(),
                        settings.cdn_proxy_url.as_deref(),
                        settings.sc_mirror_url.as_deref(),
                    ) {
                        let api = api.with_mouflon_keys(state.get_mouflon_keys());
                        match api.get_stream_info(username, true).await {
                            Ok(info) => {
                                tracing::info!("Relay offline [{}]: status={} playlist={}", username, info.status, info.playlist_url.is_some());
                                if info.playlist_url.is_some() {
                                    tracing::info!("Relay offline [{}]: upstream live → switching", username);
                                    relay_manager.set_streamer_status(username, info.is_online, info.status);
                                    break true;
                                }
                                relay_manager.set_streamer_status(username, info.is_online, info.status.clone());
                                relay_manager.set_state(username, RelayStreamState::Offline { status: info.status });
                            }
                            Err(e) => tracing::warn!("Relay offline [{}]: check failed: {}", username, e),
                        }
                    }
                }
            }

            n = stdout.read(&mut buf) => {
                match n {
                    Ok(0) | Err(_) => break true,
                    Ok(n) => { let _ = ts_tx.send(Arc::new(buf[..n].to_vec())); }
                }
            }

            _ = tokio::time::sleep(tokio::time::Duration::from_millis(200)) => {
                if relay_manager.is_idle(username, IDLE_STOP_SECS) { break false; }
                if tokio::time::Instant::now() >= deadline { break true; }
            }
        }
    };

    let _ = src.kill().await;
    let _ = src.wait().await;
    should_continue
}

/// 在线阶段：fMP4 分片 → converter ffmpeg → MPEG-TS 直接广播，无主 ffmpeg 中间层。
async fn feed_live(
    username: &str,
    initial_playlist_url: &str,
    app_state: &AppState,
    relay_manager: &RelayManager,
    ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
    stop_rx: &mut mpsc::Receiver<()>,
) -> bool {
    // converter: fMP4 pipe:0 → MPEG-TS pipe:1，直接广播给播放器
    // -probesize / -analyzeduration 最小化，减少首帧延迟
    let mut converter = match tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-probesize", "32",
            "-analyzeduration", "0",
            "-copyts",
            "-f", "mp4",
            "-i", "pipe:0",
            "-c", "copy",
            "-f", "mpegts",
            "-mpegts_flags", "pat_pmt_at_frames",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Relay live [{}]: failed to spawn converter: {}", username, e);
            return true;
        }
    };

    let converter_stdin = converter.stdin.take().unwrap();
    let mut converter_stdout = converter.stdout.take().unwrap();

    let (conv_in_tx, mut conv_in_rx) = mpsc::channel::<Vec<u8>>(64);

    // converter stdin 写入任务
    let conv_stdin_task = tokio::spawn(async move {
        let mut stdin = converter_stdin;
        while let Some(data) = conv_in_rx.recv().await {
            if stdin.write_all(&data).await.is_err() { break; }
        }
        let _ = stdin.shutdown().await;
    });

    // converter stdout → broadcast
    let ts_tx_clone = ts_tx.clone();
    let username_conv = username.to_string();
    let conv_stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match converter_stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => { let _ = ts_tx_clone.send(Arc::new(buf[..n].to_vec())); }
            }
        }
        tracing::info!("Relay live [{}]: converter stdout closed", username_conv);
    });

    let mut last_settings = app_state.get_settings();
    let mut api = match StripchatApi::new_api_only(
        last_settings.api_proxy_url.as_deref(),
        last_settings.cdn_proxy_url.as_deref(),
        last_settings.sc_mirror_url.as_deref(),
    ) {
        Ok(a) => a.with_mouflon_keys(app_state.get_mouflon_keys()),
        Err(_) => {
            drop(conv_in_tx);
            let _ = conv_stdin_task.await;
            let _ = conv_stdout_task.await;
            let _ = converter.wait().await;
            return true;
        }
    };
    let mut last_mouflon_keys = app_state.get_mouflon_keys();

    let mut current_url = initial_playlist_url.to_string();
    let mut url_prefix = get_url_prefix(&current_url);
    let mut downloaded: HashSet<u32> = HashSet::new();
    let mut init_data: Option<Vec<u8>> = None;
    let mut cached_init_url: Option<String> = None;
    let mut consecutive_failures: u32 = 0;
    const MAX_FAILURES: u32 = 10;

    let should_continue = loop {
        if stop_rx.try_recv().is_ok() { break false; }
        if relay_manager.is_idle(username, IDLE_STOP_SECS) {
            tracing::info!("Relay live [{}]: idle, stopping", username);
            break false;
        }

        // 检测设置变更
        let current_settings = app_state.get_settings();
        let current_mouflon_keys = app_state.get_mouflon_keys();
        let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
            || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
            || current_settings.sc_mirror_url != last_settings.sc_mirror_url;
        if proxy_changed || current_mouflon_keys != last_mouflon_keys {
            if let Ok(new_api) = StripchatApi::new_api_only(
                current_settings.api_proxy_url.as_deref(),
                current_settings.cdn_proxy_url.as_deref(),
                current_settings.sc_mirror_url.as_deref(),
            ) {
                api = new_api.with_mouflon_keys(current_mouflon_keys.clone());
            }
            last_settings = current_settings;
            last_mouflon_keys = current_mouflon_keys;
        }

        match poll_and_feed(&api, username, &current_url, &url_prefix, &conv_in_tx,
                            &mut downloaded, &mut init_data, &mut cached_init_url).await {
            Ok(had_new) => {
                consecutive_failures = 0;
                if !had_new {
                    tokio::select! {
                        _ = stop_rx.recv() => break false,
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(1000)) => {
                            if relay_manager.is_idle(username, IDLE_STOP_SECS) { break false; }
                        }
                    }
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("Relay live [{}]: poll failed ({}/{}): {}", username, consecutive_failures, MAX_FAILURES, e);

                if consecutive_failures >= MAX_FAILURES { break true; }

                if let Ok(info) = api.get_stream_info(username, true).await {
                    if let Some(new_url) = info.playlist_url {
                        url_prefix = get_url_prefix(&new_url);
                        current_url = new_url;
                        consecutive_failures = 0;
                    } else if !info.is_recordable {
                        break true;
                    }
                }

                tokio::select! {
                    _ = stop_rx.recv() => break false,
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(2000)) => {
                        if relay_manager.is_idle(username, IDLE_STOP_SECS) { break false; }
                    }
                }
            }
        }
    };

    drop(conv_in_tx);
    let _ = conv_stdin_task.await;
    let _ = conv_stdout_task.await;
    let _ = converter.kill().await;
    let _ = converter.wait().await;

    should_continue
}

#[allow(clippy::too_many_arguments)]
async fn poll_and_feed(
    api: &StripchatApi,
    username: &str,
    playlist_url: &str,
    url_prefix: &str,
    fmp4_tx: &mpsc::Sender<Vec<u8>>,
    downloaded: &mut HashSet<u32>,
    init_data: &mut Option<Vec<u8>>,
    cached_init_url: &mut Option<String>,
) -> Result<bool, String> {
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
        if downloaded.contains(&seg.sequence) { continue; }

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
            return Err("converter stdin channel closed".to_string());
        }

        downloaded.insert(seg.sequence);
        had_new = true;
    }

    Ok(had_new)
}
