//! 流转发 Worker / Stream Relay Worker
//!
//! 架构：主播在线时转发 HLS fMP4 分片，离线时 worker 直接退出（HTTP 连接断开）。
//! - 上游在线：HLS fMP4 分片 → converter ffmpeg → HTTP-FLV 广播
//! - 上游离线/出错：worker 退出，broadcast channel 关闭，客户端连接自然断开
//!
//! 优化：若 AppState 中已缓存该主播的 model_id，直接用它构造 CDN playlist URL，
//! 跳过 /api/front/v1/broadcasts/{username} 的 API 查询，减少不必要的 HTTP 请求。
//!
//! Optimisation: when the streamer's model_id is already cached in AppState, the CDN
//! playlist URL is constructed directly from it, bypassing the
//! /api/front/v1/broadcasts/{username} API call.

use super::state::{RelayManager, RelayStreamState};
use crate::core::no_window::NoWindowExt;
use crate::config::app_state::AppState;
use crate::recording::hls::{get_url_prefix, parse_playlist};
use crate::platform::stripchat::StripchatApi;
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

async fn worker_loop(
    username: String,
    app_state: Arc<AppState>,
    relay_manager: Arc<RelayManager>,
    mut stop_rx: mpsc::Receiver<()>,
    ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
) {
    tracing::info!("{}", crate::tl!("relay.workerStarted", username = username));

    if relay_manager.is_idle(&username, IDLE_STOP_SECS) {
        tracing::info!("{}", crate::tl!("relay.workerIdle", username = username));
        relay_manager.remove(&username);
        return;
    }

    let settings = app_state.get_settings();
    let api = match StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
        Some(settings.sc_mirror_scheme.as_str()),
    ) {
        Ok(a) => a
            .with_mouflon_keys(app_state.get_mouflon_keys())
            .with_resolution_selection(
                settings.preferred_resolution,
                &settings.resolution_preference,
            ),
        Err(e) => {
            tracing::error!("{}", crate::tl!("relay.apiClientError", username = username, error = e));
            relay_manager.set_state(&username, RelayStreamState::Error { message: e.to_string() });
            relay_manager.remove(&username);
            return;
        }
    };

    relay_manager.set_state(&username, RelayStreamState::Connecting);

    // 从 AppState 取缓存的 model_id，优先用它直接构造 playlist URL
    // Try the cached model_id first to build the playlist URL without an API call
    let cached_model_id: Option<i64> = app_state
        .get_streamers()
        .into_iter()
        .find(|s| s.username == username)
        .and_then(|s| s.model_id);

    let (playlist_url, model_id) = if let Some(mid) = cached_model_id {
        // 有缓存 model_id：直接竞速 CDN 构造 playlist URL
        // Cached model_id available: build playlist URL via CDN race, no API call
        match api.get_playlist_url(&username, mid).await {
            Ok(url) => {
                tracing::info!("{}", crate::tl!("relay.upstreamLive", username = username));
                relay_manager.set_state(&username, RelayStreamState::Live);
                relay_manager.clear_prebuffer(&username);
                (url, mid)
            }
            Err(_) => {
                // CDN 失败（可能真的离线），回退到完整 API 查询做确认
                // CDN failed (possibly really offline); fall back to full API query
                match api.get_stream_info(&username, true, Some(mid)).await {
                    Ok(info) if info.playlist_url.is_some() => {
                        // 同步 backfill model_id（改名后新 model_id 可能不同）
                        if let Some(new_mid) = info.model_id {
                            app_state.backfill_model_id(&username, new_mid);
                        }
                        relay_manager.set_state(&username, RelayStreamState::Live);
                        relay_manager.set_streamer_status(&username, info.is_online, info.status);
                        relay_manager.clear_prebuffer(&username);
                        let new_mid = info.model_id.unwrap_or(mid);
                        (info.playlist_url.unwrap(), new_mid)
                    }
                    Ok(info) => {
                        tracing::info!("{}", crate::tl!("relay.upstreamOffline", username = username, status = info.status));
                        relay_manager.set_state(&username, RelayStreamState::Offline { status: info.status.clone() });
                        relay_manager.set_streamer_status(&username, info.is_online, info.status);
                        relay_manager.remove(&username);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("{}", crate::tl!("relay.streamInfoFailed", username = username, error = e));
                        relay_manager.set_state(&username, RelayStreamState::Error { message: e.to_string() });
                        relay_manager.remove(&username);
                        return;
                    }
                }
            }
        }
    } else {
        // 没有缓存 model_id：必须走完整 API 查询
        // No cached model_id: must use full API query
        match api.get_stream_info(&username, true, None).await {
            Ok(info) if info.playlist_url.is_some() => {
                // 回填 model_id 供下次直接使用
                // Backfill model_id for next time
                if let Some(mid) = info.model_id {
                    app_state.backfill_model_id(&username, mid);
                }
                relay_manager.set_state(&username, RelayStreamState::Live);
                relay_manager.set_streamer_status(&username, info.is_online, info.status);
                relay_manager.clear_prebuffer(&username);
                let mid = info.model_id.unwrap_or(0);
                (info.playlist_url.unwrap(), mid)
            }
            Ok(info) => {
                tracing::info!("{}", crate::tl!("relay.upstreamOffline", username = username, status = info.status));
                relay_manager.set_state(&username, RelayStreamState::Offline { status: info.status.clone() });
                relay_manager.set_streamer_status(&username, info.is_online, info.status);
                relay_manager.remove(&username);
                return;
            }
            Err(e) => {
                tracing::warn!("{}", crate::tl!("relay.streamInfoFailed", username = username, error = e));
                relay_manager.set_state(&username, RelayStreamState::Error { message: e.to_string() });
                relay_manager.remove(&username);
                return;
            }
        }
    };

    relay_manager.set_playlist_url(&username, Some(playlist_url.clone()));

    feed_live(
        &username, &playlist_url, model_id,
        &app_state, Arc::clone(&relay_manager), &ts_tx, &mut stop_rx,
    ).await;

    relay_manager.set_playlist_url(&username, None);
    relay_manager.remove(&username);
    tracing::info!("{}", crate::tl!("relay.workerStopped", username = username));
}

/// 在线阶段：fMP4 分片 → converter ffmpeg → HTTP-FLV 广播。
/// 上游离线或出错时直接返回（worker 随后退出，broadcast channel 关闭，客户端自然断连）。
///
/// `model_id`：主播内部 ID，用于失败时直接重建 playlist URL，跳过 API 查询。
/// `0` 表示未知（由无缓存 model_id 的首次 API 查询路径产生），此时退化为完整 API 刷新。
///
/// Live phase: fMP4 segments → converter ffmpeg → HTTP-FLV broadcast.
/// `model_id`: internal streamer ID for rebuilding the playlist URL on failure without
/// an API call. `0` means unknown (first-time path with no cached id), falling back to
/// full API refresh.
async fn feed_live(
    username: &str,
    initial_playlist_url: &str,
    model_id: i64,
    app_state: &AppState,
    relay_manager: Arc<RelayManager>,
    ts_tx: &broadcast::Sender<Arc<Vec<u8>>>,
    stop_rx: &mut mpsc::Receiver<()>,
) {
    // converter: fMP4 pipe:0 → HTTP-FLV pipe:1
    // -probesize / -analyzeduration 最小化，减少首帧延迟
    let mut converter = match tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-probesize", "32",
            "-analyzeduration", "0",
            "-f", "mp4",
            "-i", "pipe:0",
            "-c", "copy",
            "-f", "flv",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_window()
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("{}", crate::tl!("relay.liveConverterFailed", username = username, error = e));
            return;
        }
    };

    let converter_stdin = converter.stdin.take().unwrap();
    let mut converter_stdout = converter.stdout.take().unwrap();

    let (conv_in_tx, mut conv_in_rx) = mpsc::channel::<Vec<u8>>(64);

    let conv_stdin_task = tokio::spawn(async move {
        let mut stdin = converter_stdin;
        while let Some(data) = conv_in_rx.recv().await {
            if stdin.write_all(&data).await.is_err() { break; }
        }
        let _ = stdin.shutdown().await;
    });

    let ts_tx_clone = ts_tx.clone();
    let relay_manager_clone = Arc::clone(&relay_manager);
    let username_conv = username.to_string();
    let conv_stdout_task = tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match converter_stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = Arc::new(buf[..n].to_vec());
                    relay_manager_clone.push_prebuffer(&username_conv, Arc::clone(&chunk));
                    let _ = ts_tx_clone.send(chunk);
                }
            }
        }
        tracing::info!("{}", crate::tl!("relay.liveConverterClosed", username = username_conv));
    });

    let mut last_settings = app_state.get_settings();
    let mut api = match StripchatApi::new_api_only(
        last_settings.api_proxy_url.as_deref(),
        last_settings.cdn_proxy_url.as_deref(),
        last_settings.sc_mirror_url.as_deref(),
        Some(last_settings.sc_mirror_scheme.as_str()),
    ) {
        Ok(a) => a
            .with_mouflon_keys(app_state.get_mouflon_keys())
            .with_resolution_selection(
                last_settings.preferred_resolution,
                &last_settings.resolution_preference,
            ),
        Err(_) => {
            drop(conv_in_tx);
            let _ = conv_stdin_task.await;
            let _ = conv_stdout_task.await;
            let _ = converter.wait().await;
            return;
        }
    };
    let mut last_mouflon_keys = app_state.get_mouflon_keys();

    let mut current_url = initial_playlist_url.to_string();
    let mut url_prefix = get_url_prefix(&current_url);
    let mut downloaded: HashSet<u32> = HashSet::new();
    let mut init_data: Option<Vec<u8>> = None;
    let mut cached_init_url: Option<String> = None;
    let mut consecutive_failures: u32 = 0;
    const MAX_FAILURES: u32 = 3;

    loop {
        if stop_rx.try_recv().is_ok() { break; }
        if relay_manager.is_idle(username, IDLE_STOP_SECS) {
            tracing::info!("{}", crate::tl!("relay.liveIdle", username = username));
            break;
        }

        // 按需重建 API 客户端（设置变更时）
        let current_settings = app_state.get_settings();
        let current_mouflon_keys = app_state.get_mouflon_keys();
        let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
            || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
            || current_settings.sc_mirror_url != last_settings.sc_mirror_url
            || current_settings.sc_mirror_scheme != last_settings.sc_mirror_scheme;
        let resolution_changed = current_settings.preferred_resolution != last_settings.preferred_resolution
            || current_settings.resolution_preference != last_settings.resolution_preference;
        if proxy_changed || resolution_changed || current_mouflon_keys != last_mouflon_keys {
            if let Ok(new_api) = StripchatApi::new_api_only(
                current_settings.api_proxy_url.as_deref(),
                current_settings.cdn_proxy_url.as_deref(),
                current_settings.sc_mirror_url.as_deref(),
                Some(current_settings.sc_mirror_scheme.as_str()),
            ) {
                api = new_api
                    .with_mouflon_keys(current_mouflon_keys.clone())
                    .with_resolution_selection(
                        current_settings.preferred_resolution,
                        &current_settings.resolution_preference,
                    );
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
                        _ = stop_rx.recv() => break,
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(1000)) => {
                            if relay_manager.is_idle(username, IDLE_STOP_SECS) { break; }
                        }
                    }
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("{}", crate::tl!("relay.livePollFailed", username = username, cur = consecutive_failures, max = MAX_FAILURES, error = e));

                if consecutive_failures >= MAX_FAILURES {
                    tracing::info!("{}", crate::tl!("relay.upstreamOffline", username = username, status = "offline"));
                    break;
                }

                // 优先用缓存 model_id 直接重建 playlist URL，跳过 API 查询
                // Prefer rebuilding playlist URL from cached model_id, skipping the API call
                let refreshed = if model_id != 0 {
                    match api.get_playlist_url(username, model_id).await {
                        Ok(new_url) => {
                            url_prefix = get_url_prefix(&new_url);
                            current_url = new_url;
                            consecutive_failures = 0;
                            true
                        }
                        // CDN 竞速全失败，走完整 API 查询确认是否真的离线
                        // CDN race failed entirely; fall back to full API query to confirm offline
                        Err(_) => false,
                    }
                } else {
                    false
                };

                if !refreshed {
                    // 回退到完整 API 查询（含改名回退）
                    // Fall back to full API query (with rename fallback)
                    let known_mid = if model_id != 0 { Some(model_id) } else { None };
                    match api.get_stream_info(username, true, known_mid).await {
                        Ok(info) if info.playlist_url.is_some() => {
                            if let Some(mid) = info.model_id {
                                app_state.backfill_model_id(username, mid);
                            }
                            let new_url = info.playlist_url.unwrap();
                            url_prefix = get_url_prefix(&new_url);
                            current_url = new_url;
                            consecutive_failures = 0;
                        }
                        Ok(info) => {
                            tracing::info!("{}", crate::tl!("relay.upstreamOffline", username = username, status = info.status));
                            relay_manager.set_state(username, RelayStreamState::Offline { status: info.status.clone() });
                            relay_manager.set_streamer_status(username, info.is_online, info.status);
                            break;
                        }
                        Err(_) => { /* 查询失败，靠 MAX_FAILURES 自然退出 */ }
                    }
                }

                tokio::select! {
                    _ = stop_rx.recv() => break,
                    _ = tokio::time::sleep(tokio::time::Duration::from_millis(500)) => {
                        if relay_manager.is_idle(username, IDLE_STOP_SECS) { break; }
                    }
                }
            }
        }
    }

    drop(conv_in_tx);
    let _ = conv_stdin_task.await;
    let _ = conv_stdout_task.await;
    let _ = converter.kill().await;
    let _ = converter.wait().await;
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

    let segments = parse_playlist(&playlist_text, url_prefix, mouflon_keys)
        .map_err(|e| e.to_string())?;

    let new_init_url: Option<String> = segments.iter().find_map(|s| s.init_url.clone());

    let init_url_path = |u: &str| u.split('?').next().unwrap_or(u).to_string();
    let new_init_path = new_init_url.as_deref().map(init_url_path);
    let cached_path = cached_init_url.as_deref().map(init_url_path);

    if new_init_path.is_some() && new_init_path != cached_path
        && let Some(ref url) = new_init_url
    {
        match api.download_segment(url).await {
            Ok(data) => {
                *init_data = Some(data);
                *cached_init_url = Some(url.to_string());
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
                tracing::warn!("{}", crate::tl!("relay.liveSegmentFailed", seq = seg.sequence, username = username, error = e));
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
