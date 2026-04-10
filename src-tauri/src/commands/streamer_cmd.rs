//! 主播管理 Tauri 命令 / Streamer Management Tauri Commands
//!
//! 提供主播列表查询、添加/移除主播、设置自动录制、手动开始/停止录制等命令。
//! 所有命令均通过 `#[tauri::command]` 宏注册，可从前端 JS 调用。
//!
//! Provides commands for querying the streamer list, adding/removing streamers,
//! setting auto-record, and manually starting/stopping recordings.
//! All commands are registered via the `#[tauri::command]` macro and callable from the frontend.

use crate::core::error::Result;
use crate::streaming::monitor::StatusMonitor;
use crate::recording::recorder::RecorderManager;
use crate::config::settings::AppState;
use crate::streaming::stripchat::StripchatApi;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// 主播条目（序列化后返回给前端）/ Streamer entry (serialized and returned to the frontend)
#[derive(serde::Serialize)]
pub struct StreamerEntry {
    pub username: String,
    pub auto_record: bool,
    pub added_at: String,
    /// 是否在线 / Whether online
    pub is_online: bool,
    /// 是否正在录制 / Whether currently recording
    pub is_recording: bool,
    /// 是否可录制（直播间公开可访问）/ Whether recordable (stream publicly accessible)
    pub is_recordable: bool,
    pub viewers: i64,
    /// 直播间状态文字 / Stream status text
    pub status: String,
    pub thumbnail_url: Option<String>,
}

/// 列出所有追踪主播及其当前状态。
/// List all tracked streamers with their current status.
#[tauri::command]
pub async fn list_streamers(
    state: State<'_, Arc<AppState>>,
    monitor: State<'_, Arc<StatusMonitor>>,
    recorder: State<'_, Arc<RecorderManager>>,
) -> Result<Vec<StreamerEntry>> {
    let streamers = state.get_streamers();

    Ok(streamers
        .into_iter()
        .map(|s| {
            let status = monitor.get_status(&s.username);
            StreamerEntry {
                username: s.username.clone(),
                auto_record: s.auto_record,
                added_at: s.added_at,
                is_online: status.as_ref().map(|s| s.is_online).unwrap_or(false),
                is_recording: recorder.is_recording(&s.username),
                is_recordable: status.as_ref().map(|s| s.is_recordable).unwrap_or(false),
                viewers: status.as_ref().map(|s| s.viewers).unwrap_or(0),
                status: status
                    .as_ref()
                    .map(|s| s.status.clone())
                    .unwrap_or_else(|| "未知".to_string()),
                thumbnail_url: status.and_then(|s| s.thumbnail_url),
            }
        })
        .collect())
}

/// 添加新主播到追踪列表。
/// 先通过 API 验证用户名存在，再保存并触发一次状态轮询。
///
/// Add a new streamer to the tracking list.
/// Verifies the username exists via API before saving and triggering a status poll.
#[tauri::command]
pub async fn add_streamer(
    username: String,
    state: State<'_, Arc<AppState>>,
    monitor: State<'_, Arc<StatusMonitor>>,
    app_handle: AppHandle,
) -> Result<()> {
    let username = username.trim().to_lowercase();
    if username.is_empty() {
        return Err("用户名不能为空".into());
    }

    let settings = state.get_settings();
    let api = StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )?;
    // 验证用户名是否存在（不获取播放列表）/ Verify username exists (without fetching playlist)
    api.get_stream_info(&username, false)
        .await
        .map_err(|e| crate::core::error::AppError::Other(format!("{}", e)))?;

    state.add_streamer(&username)?;

    let _ = app_handle.emit(
        "streamer-added",
        serde_json::json!({ "username": username }),
    );

    // 异步触发一次状态轮询，更新前端显示 / Async trigger a status poll to update the frontend
    let monitor = Arc::clone(&monitor);
    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        monitor.poll_one(&username, &app_handle_clone).await;
    });

    Ok(())
}

/// 从追踪列表中移除主播，同时停止录制并删除录制文件目录。
/// Remove a streamer from the tracking list, stopping any recording and deleting the recording directory.
#[tauri::command]
pub async fn remove_streamer(
    username: String,
    state: State<'_, Arc<AppState>>,
    recorder: State<'_, Arc<RecorderManager>>,
    app_handle: AppHandle,
) -> Result<()> {
    if recorder.is_recording(&username) {
        recorder.stop_recording(&username).await?;
    }
    let settings = state.get_settings();
    let streamer_dir = std::path::PathBuf::from(&settings.output_dir).join(&username);
    if streamer_dir.exists() {
        std::fs::remove_dir_all(&streamer_dir)?;
    }
    state.remove_streamer(&username)?;
    let _ = app_handle.emit(
        "streamer-removed",
        serde_json::json!({ "username": username }),
    );
    Ok(())
}

/// 设置指定主播的自动录制开关，并广播变更事件。
/// Set the auto-record toggle for a specific streamer and broadcast the change event.
#[tauri::command]
pub async fn set_auto_record(
    username: String,
    enabled: bool,
    state: State<'_, Arc<AppState>>,
    app_handle: AppHandle,
) -> Result<()> {
    state.set_auto_record(&username, enabled)?;
    let _ = app_handle.emit(
        "auto-record-changed",
        serde_json::json!({ "username": username, "enabled": enabled }),
    );
    Ok(())
}

/// 手动开始录制指定主播。
/// 优先使用监控器缓存的播放列表 URL，否则重新从 API 获取。
///
/// Manually start recording a specific streamer.
/// Prefers the cached playlist URL from the monitor; otherwise re-fetches from the API.
///
/// # 返回值 / Returns
/// 录制会话目录路径 / Recording session directory path
#[tauri::command]
pub async fn start_recording(
    username: String,
    state: State<'_, Arc<AppState>>,
    recorder: State<'_, Arc<RecorderManager>>,
    monitor: State<'_, Arc<StatusMonitor>>,
    app_handle: AppHandle,
) -> Result<String> {
    let playlist_url = if let Some(url) = monitor.get_cached_playlist_url(&username) {
        url
    } else {
        let settings = state.get_settings();
        let api = StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
        )?;
        let info = api.get_stream_info(&username, true).await?;
        info.playlist_url
            .ok_or_else(|| crate::core::error::AppError::StreamOffline(username.clone()))?
    };

    recorder
        .start_recording(&username, &playlist_url, app_handle)
        .await
}

/// 手动停止录制指定主播。
/// 同时关闭自动录制，并在停止后触发一次状态轮询更新前端。
///
/// Manually stop recording a specific streamer.
/// Also disables auto-record and triggers a status poll after stopping to update the frontend.
#[tauri::command]
pub async fn stop_recording(
    username: String,
    state: State<'_, Arc<AppState>>,
    recorder: State<'_, Arc<RecorderManager>>,
    monitor: State<'_, Arc<StatusMonitor>>,
    app_handle: AppHandle,
) -> Result<()> {
    // 关闭自动录制，防止停止后立即重新开始 / Disable auto-record to prevent immediate restart after stop
    let _ = state.set_auto_record(&username, false);
    let _ = app_handle.emit(
        "auto-record-changed",
        serde_json::json!({ "username": username, "enabled": false }),
    );
    recorder.stop_recording(&username).await?;
    // 等待录制实际停止后再轮询状态（最多等待 10 秒）
    // Wait for recording to actually stop before polling status (up to 10 seconds)
    let monitor = Arc::clone(&monitor);
    let recorder = Arc::clone(&recorder);
    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        for _ in 0..40 {
            if !recorder.is_recording(&username) {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
        monitor.poll_one(&username, &app_handle_clone).await;
    });
    Ok(())
}

/// 验证主播用户名是否存在于 Stripchat（通过后端代理/镜像）。
/// Verify whether a streamer username exists on Stripchat (via backend proxy/mirror).
#[tauri::command]
pub async fn verify_streamer(
    username: String,
    state: State<'_, Arc<AppState>>,
) -> Result<serde_json::Value> {
    let settings = state.get_settings();
    let api = StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )?;
    match api.get_stream_info(&username, false).await {
        Ok(_) => Ok(serde_json::json!({ "exists": true })),
        Err(crate::core::error::AppError::UserNotFound(_)) => {
            Ok(serde_json::json!({ "exists": false }))
        }
        Err(e) => Err(e),
    }
}
