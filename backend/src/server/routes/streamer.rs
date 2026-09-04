//! 主播相关路由 handler / Streamer-related route handlers

use crate::core::emitter::EmitterExt;
use crate::server::error::{ApiError, ApiResult};
use crate::server::router::ServerState;
use axum::{
    Json,
    extract::{Path, State as AxumState},
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn list_streamers(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let streamers = s.app_state.get_streamers();

    let has_any_status = streamers
        .iter()
        .any(|st| s.monitor.get_status(&st.username).is_some());
    if !has_any_status && !streamers.is_empty() {
        let monitor = Arc::clone(&s.monitor);
        let emitter = Arc::clone(&s.emitter);
        tokio::spawn(async move {
            monitor.poll_all_with_emitter(&emitter).await;
        });
    }

    let result: Vec<serde_json::Value> = streamers
        .into_iter()
        .map(|st| {
            let status = s.monitor.get_status(&st.username);
            serde_json::json!({
                "username": st.username,
                "auto_record": st.auto_record,
                "added_at": st.added_at,
                "is_online": status.as_ref().map(|s| s.is_online).unwrap_or(false),
                "is_recording": s.recorder.is_recording(&st.username),
                "is_recordable": status.as_ref().map(|s| s.is_recordable).unwrap_or(false),
                "status": status.as_ref().map(|s| s.status.clone()).unwrap_or_default(),
                "thumbnail_url": status.and_then(|s| s.thumbnail_url),
            })
        })
        .collect();
    Ok(Json(serde_json::Value::Array(result)))
}

#[derive(Deserialize)]
pub struct AddStreamerBody {
    /// 支持单个用户名（兼容旧调用方）或多个用户名列表。
    /// Accepts a single username (for backward compatibility) or a list.
    pub usernames: Vec<String>,
}

pub async fn add_streamer(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<AddStreamerBody>,
) -> ApiResult<serde_json::Value> {
    // 去重、清理、过滤空值 / Deduplicate, trim, filter blanks
    let mut seen = std::collections::HashSet::new();
    let usernames: Vec<String> = body
        .usernames
        .into_iter()
        .map(|u| u.trim().to_lowercase())
        .filter(|u| !u.is_empty() && seen.insert(u.clone()))
        .collect();

    if usernames.is_empty() {
        return Err(ApiError("用户名不能为空".into()));
    }

    let total = usernames.len();
    let settings = s.app_state.get_settings();
    let api = Arc::new(
        crate::platform::stripchat::StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
        )
        .map_err(ApiError::from)?,
    );

    // 全量并发：所有请求同时发出，边完成边处理，最快速度
    // Full concurrency: all requests sent at once, processed as they complete for maximum speed
    let mut tasks = tokio::task::JoinSet::new();
    for username in &usernames {
        let api = Arc::clone(&api);
        let username = username.clone();
        tasks.spawn(async move {
            let result = api.verify_user_exists(&username).await;
            (username, result)
        });
    }

    let mut success = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut done = 0usize;

    // 每完成一个验证立即处理并推送进度事件，无需等待其他请求
    // Process and emit progress for each result as soon as it completes
    while let Some(res) = tasks.join_next().await {
        let Ok((username, verify_result)) = res else { continue };

        let (ok, skipped_entry, error_msg) = match verify_result {
            Ok(model_id) => match s.app_state.add_streamer(&username, model_id) {
                Ok(()) => {
                    s.emitter.emit(
                        "streamer-added",
                        &serde_json::json!({ "username": username }),
                    );
                    success += 1;
                    (true, false, None)
                }
                Err(e) if e.to_string().contains("已存在") => {
                    // 已存在视为跳过，不计入失败 / Already exists: treat as skipped, not failed
                    skipped += 1;
                    (true, true, None)
                }
                Err(e) => {
                    failed += 1;
                    (false, false, Some(e.to_string()))
                }
            },
            Err(e) => {
                failed += 1;
                (false, false, Some(e.to_string()))
            }
        };

        done += 1;
        // 推送进度事件 / Emit progress event
        s.emitter.emit(
            "streamer-batch-progress",
            &serde_json::json!({
                "done": done,
                "total": total,
                "username": username,
                "ok": ok,
                "skipped": skipped_entry,
                "error": error_msg,
            }),
        );
    }

    // 所有验证和添加完成后，统一触发一次全量状态轮询（而非每个主播单独 spawn），
    // 避免与验证请求并发过多导致 API 限流。
    // After all verifications and additions, trigger a single full poll (instead of per-streamer spawns)
    // to avoid concurrent API rate limiting with the verify requests.
    if success > 0 {
        let emitter = Arc::clone(&s.emitter);
        let monitor = Arc::clone(&s.monitor);
        tokio::spawn(async move {
            monitor.poll_all_with_emitter(&emitter).await;
        });
    }

    Ok(Json(serde_json::json!({
        "total": total,
        "success": success,
        "skipped": skipped,
        "failed": failed,
    })))
}

pub async fn remove_streamer(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    if s.recorder.is_recording(&name) {
        s.recorder
            .stop_recording(&name)
            .await
            .map_err(ApiError::from)?;
    }
    let settings = s.app_state.get_settings();
    let dir = std::path::PathBuf::from(&settings.output_dir).join(&name);
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    s.app_state.remove_streamer(&name).map_err(ApiError::from)?;
    // 清除 monitor 内存 dead 集合，避免重新添加同名主播后仍被跳过
    // Clear monitor in-memory dead set so re-adding the same username isn't skipped
    s.monitor.dead_streamers.write().remove(&name);
    s.emitter
        .emit("streamer-removed", &serde_json::json!({ "username": name }));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AutoRecordBody {
    pub enabled: bool,
}

pub async fn set_auto_record(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
    Json(body): Json<AutoRecordBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .set_auto_record(&name, body.enabled)
        .map_err(ApiError::from)?;
    s.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": name, "enabled": body.enabled }),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn start_recording(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let playlist_url = if let Some(url) = s.monitor.get_cached_playlist_url(&name) {
        url
    } else {
        let settings = s.app_state.get_settings();
        let api = crate::platform::stripchat::StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
        )
        .map_err(ApiError::from)?
        .with_mouflon_keys(s.app_state.get_mouflon_keys());
        let info = api
            .get_stream_info(&name, true, None)
            .await
            .map_err(ApiError::from)?;
        info.playlist_url
            .ok_or_else(|| ApiError(format!("Stream offline: {}", name)))?
    };
    let path = s
        .recorder
        .start_recording_with_emitter(&name, &playlist_url, Arc::clone(&s.emitter))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "path": path })))
}

pub async fn stop_recording(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let _ = s.app_state.set_auto_record(&name, false);
    s.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": name, "enabled": false }),
    );
    s.recorder
        .stop_recording(&name)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn verify_streamer(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    let api = crate::platform::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
        Some(settings.sc_mirror_scheme.as_str()),
    )
    .map_err(ApiError::from)?;
    match api.verify_user_exists(&name).await {
        Ok(_) => Ok(Json(serde_json::json!({ "exists": true }))),
        Err(crate::core::error::AppError::UserNotFound(_)) => {
            Ok(Json(serde_json::json!({ "exists": false })))
        }
        Err(e) => Err(ApiError(e.to_string())),
    }
}
