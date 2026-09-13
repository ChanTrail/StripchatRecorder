//! Tauri 命令层 / Tauri Command Layer
//!
//! 将后端业务函数包装为 `#[tauri::command]`，供前端通过 `invoke()` 调用。
//! 命令名与 server 模式下 HTTP 路由的语义一一对应。
//!
//! Wraps backend functions as `#[tauri::command]` for frontend invocation via `invoke()`.
//! Command names correspond to HTTP route semantics in server mode.

use crate::state::DesktopState;
use std::sync::Arc;
use tauri::State;

use stripchat_recorder_lib::{
    config::app_state::Settings,
    core::{emitter::EmitterExt, error::AppError},
    postprocess::{RegistryModule, community, pipeline::PipelineConfig},
    platform::stripchat::StripchatApi,
    system::disk::get_disk_space_entries,
};

type CmdResult<T> = std::result::Result<T, String>;

fn map_err(e: AppError) -> String {
    e.to_string()
}

// ─── Streamers ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_streamers(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let streamers = state.app_state.get_streamers();
    let has_any_status = streamers
        .iter()
        .any(|s| state.monitor.get_status(&s.username).is_some());

    if !has_any_status && !streamers.is_empty() {
        let monitor = Arc::clone(&state.monitor);
        let emitter = Arc::clone(&state.emitter);
        tokio::spawn(async move {
            monitor.poll_all_with_emitter(&emitter).await;
        });
    }

    let result: Vec<serde_json::Value> = streamers
        .into_iter()
        .map(|s| {
            let status = state.monitor.get_status(&s.username);
            serde_json::json!({
                "username": s.username,
                "auto_record": s.auto_record,
                "added_at": s.added_at,
                "is_online": status.as_ref().map(|st| st.is_online).unwrap_or(false),
                "is_recording": state.recorder.is_recording(&s.username),
                "is_recordable": status.as_ref().map(|st| st.is_recordable).unwrap_or(false),
                "status": status.as_ref().map(|st| st.status.clone()).unwrap_or_default(),
                "thumbnail_url": status.and_then(|st| st.thumbnail_url),
            })
        })
        .collect();
    Ok(serde_json::Value::Array(result))
}

#[tauri::command]
pub async fn add_streamer(
    usernames: Vec<String>,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let mut total = 0usize;
    let mut success = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for raw in usernames {
        let username = raw.trim().to_lowercase();
        if username.is_empty() { failed += 1; continue; }
        total += 1;

        // 已存在则跳过 / Skip if already tracked
        if state.app_state.get_streamers().iter().any(|s| s.username == username) {
            skipped += 1;
            state.emitter.emit("streamer-batch-progress", &serde_json::json!({
                "done": success + skipped + failed,
                "username": username,
            }));
            continue;
        }

        let api_res = StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
        );
        match api_res {
            Err(e) => { failed += 1; tracing::warn!("add_streamer: api init failed for {username}: {e}"); }
            Ok(api) => {
                match api.get_stream_info(&username, false, None).await {
                    Err(_) => { failed += 1; }
                    Ok(info) => {
                        let model_id = info.model_id;
                        match state.app_state.add_streamer(&username, model_id) {
                            Err(_) => { failed += 1; }
                            Ok(_) => {
                                success += 1;
                                state.emitter.emit("streamer-added", &serde_json::json!({ "username": username }));
                                let monitor = Arc::clone(&state.monitor);
                                let emitter = Arc::clone(&state.emitter);
                                let u = username.clone();
                                tokio::spawn(async move {
                                    monitor.poll_all_with_emitter(&emitter).await;
                                    // 轮询完成后单独推一次该主播的状态（近似 poll_one 的效果）
                                    // After polling, the streamer's status will be emitted by the monitor
                                    let _ = u; // suppress unused warning
                                });
                            }
                        }
                    }
                }
            }
        }
        state.emitter.emit("streamer-batch-progress", &serde_json::json!({
            "done": success + skipped + failed,
            "username": username,
        }));
    }
    Ok(serde_json::json!({ "total": total, "success": success, "skipped": skipped, "failed": failed }))
}

#[tauri::command]
pub async fn remove_streamer(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    if state.recorder.is_recording(&username) {
        state.recorder.stop_recording(&username).await.map_err(map_err)?;
    }
    state.app_state.remove_streamer(&username).map_err(map_err)?;
    state.emitter.emit("streamer-removed", &serde_json::json!({ "username": username }));
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn set_auto_record(
    username: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.set_auto_record(&username, enabled).map_err(map_err)?;
    state.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": username, "enabled": enabled }),
    );
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn start_recording(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let playlist_url = if let Some(url) = state.monitor.get_cached_playlist_url(&username) {
        url
    } else {
        let settings = state.app_state.get_settings();
        let api = StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
        )
        .map_err(map_err)?
        .with_mouflon_keys(state.app_state.get_mouflon_keys());
        let info = api.get_stream_info(&username, true, None).await.map_err(map_err)?;
        info.playlist_url
            .ok_or_else(|| format!("Stream offline: {}", username))?
    };
    let path = state
        .recorder
        .start_recording_with_emitter(&username, &playlist_url, Arc::clone(&state.emitter))
        .await
        .map_err(map_err)?;
    Ok(serde_json::json!({ "path": path }))
}

#[tauri::command]
pub async fn stop_recording(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let _ = state.app_state.set_auto_record(&username, false);
    state.emitter.emit(
        "auto-record-changed",
        &serde_json::json!({ "username": username, "enabled": false }),
    );
    state.recorder.stop_recording(&username).await.map_err(map_err)?;
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn verify_streamer(
    username: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let api = StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
        Some(settings.sc_mirror_scheme.as_str()),
    )
    .map_err(map_err)?;
    match api.get_stream_info(&username, false, None).await {
        Ok(_) => Ok(serde_json::json!({ "exists": true })),
        Err(AppError::UserNotFound(_)) => Ok(serde_json::json!({ "exists": false })),
        Err(e) => Err(e.to_string()),
    }
}

// ─── Settings ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_settings(state: State<'_, DesktopState>) -> CmdResult<Settings> {
    Ok(state.app_state.get_settings())
}

#[tauri::command]
pub async fn save_settings_cmd(
    new_settings: Settings,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.update_settings(new_settings).map_err(map_err)?;
    state.emitter.emit("settings-updated", &state.app_state.get_settings());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn get_disk_space(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let output_dir = settings.output_dir.clone();

    // 取 ts_merge 节点的自定义输出目录（若已配置）
    let ts_merge_dir = {
        let pipeline = state.app_state.get_pipeline();
        pipeline.nodes.iter()
            .find(|n| n.module_id == "ts_merge" && n.enabled)
            .and_then(|n| n.params.get("output_dir"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    let result = tokio::task::spawn_blocking(move || {
        get_disk_space_entries(&output_dir, ts_merge_dir.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(serde_json::to_value(result).unwrap())
}

#[tauri::command]
pub async fn get_system_info() -> CmdResult<serde_json::Value> {
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cap = (cpu_count * 2).max(1);
    Ok(serde_json::json!({
        "cpuCount": cpu_count,
        "maxPpConcurrentCap": cap,
        "maxConcurrentCap": cap,
    }))
}

// ─── Mouflon Keys ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_mouflon_keys(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    Ok(serde_json::to_value(state.app_state.get_mouflon_keys_store()).unwrap())
}

#[tauri::command]
pub async fn add_mouflon_key(
    pkey: String,
    pdkey: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.add_mouflon_key(&pkey, &pdkey).map_err(map_err)?;
    state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn remove_mouflon_key(
    pkey: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.remove_mouflon_key(&pkey).map_err(map_err)?;
    state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn sync_mouflon_keys(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let url = settings
        .mouflon_sync_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or("未配置 mouflon_sync_url")?
        .to_string();
    let token = settings.mouflon_sync_token.clone();
    let updated = state
        .app_state
        .sync_mouflon_keys_from_worker(&url, token.as_deref())
        .await
        .map_err(map_err)?;
    if updated {
        state.emitter.emit("mouflon-keys-updated", &state.app_state.get_mouflon_keys_store());
    }
    Ok(serde_json::json!({ "updated": updated }))
}

// ─── Recordings ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_recordings(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let app_state = Arc::clone(&state.app_state);
    let recorder = Arc::clone(&state.recorder);
    let files = tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::recording::service::list_recordings_inner(&app_state, &recorder)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(files).unwrap())
}

#[tauri::command]
pub async fn get_merging_dirs(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    // merge_format 字段现在通过 ts_merge pipeline 节点控制，默认 mp4
    let merge_format = {
        let pipeline = state.app_state.get_pipeline();
        pipeline.nodes.iter()
            .find(|n| n.module_id == "ts_merge")
            .and_then(|n| n.params.get("format"))
            .and_then(|v| v.as_str())
            .unwrap_or("mp4")
            .to_string()
    };

    let make_entry = |path: &std::path::PathBuf, status: &str| {
        let path_str = path.to_string_lossy().to_string();
        let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let parent = path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let sep = if path_str.contains('\\') { "\\" } else { "/" };
        let merged_path = format!("{}{}{}.{}", parent, sep, stem, merge_format);
        serde_json::json!({
            "session_dir": path_str,
            "merged_path": merged_path,
            "merge_format": merge_format,
            "username": username,
            "status": status,
        })
    };

    let mut result: Vec<serde_json::Value> = state
        .recorder
        .merging_dirs
        .read()
        .iter()
        .map(|p| make_entry(p, "merging"))
        .collect();
    result.extend(
        state.recorder.waiting_merge_dirs.read().iter().map(|p| make_entry(p, "waiting")),
    );
    let _ = settings;
    Ok(serde_json::json!(result))
}

#[tauri::command]
pub async fn delete_recording(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    // 先请求取消正在进行的后处理 / Request cancellation of any running pp task first
    state.app_state.pp_queue.cancel(&path);

    let recorder = Arc::clone(&state.recorder);
    let app_state = Arc::clone(&state.app_state);
    let path_clone = path.clone();

    tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::recording::service::delete_recording_inner(
            &path_clone,
            &recorder,
            &app_state,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(map_err)?;

    state.emitter.emit("recording-deleted", &serde_json::json!({ "path": path }));
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Post-processing ──────────────────────────────────────────────────────────

#[tauri::command]
pub async fn run_postprocess_cmd(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let pipeline = state.app_state.get_pipeline();
    if !pipeline.nodes.iter().any(|n| n.enabled) {
        return Err("后处理流水线为空".to_string());
    }
    let video_path = std::path::PathBuf::from(&path);
    let initial_path = video_path.clone();
    let emitter = Arc::clone(&state.emitter);
    let app_state = Arc::clone(&state.app_state);
    tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::postprocess::service::run_postprocess_for_path(
            &initial_path,
            &video_path,
            &pipeline,
            &emitter,
            &app_state,
        );
    });
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn run_postprocess_batch(
    paths: Vec<String>,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let pipeline = state.app_state.get_pipeline();
    if !pipeline.nodes.iter().any(|n| n.enabled) {
        return Err("后处理流水线为空".to_string());
    }
    for path in paths {
        let video_path = std::path::PathBuf::from(&path);
        let initial_path = video_path.clone();
        let emitter = Arc::clone(&state.emitter);
        let app_state = Arc::clone(&state.app_state);
        let pipeline = pipeline.clone();
        tokio::task::spawn_blocking(move || {
            stripchat_recorder_lib::postprocess::service::run_postprocess_for_path(
                &initial_path,
                &video_path,
                &pipeline,
                &emitter,
                &app_state,
            );
        });
    }
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn cancel_postprocess(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.pp_queue.cancel(&path);
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn get_postprocess_tasks(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    Ok(serde_json::to_value(state.app_state.pp_queue.get_all_tasks()).unwrap())
}

// ─── Pipeline ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_pipeline(state: State<'_, DesktopState>) -> CmdResult<PipelineConfig> {
    Ok(state.app_state.get_pipeline())
}

#[tauri::command]
pub async fn save_pipeline(
    pipeline: PipelineConfig,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    state.app_state.update_pipeline(pipeline).map_err(map_err)?;
    state.emitter.emit("pipeline-updated", &state.app_state.get_pipeline());
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub async fn list_modules() -> CmdResult<serde_json::Value> {
    let modules = tokio::task::spawn_blocking(
        stripchat_recorder_lib::postprocess::pipeline::discover_modules,
    )
    .await
    .unwrap_or_default();
    Ok(serde_json::to_value(modules).unwrap())
}

// ─── Community Modules ────────────────────────────────────────────────────────

/// 获取当前正在安装的社区模块列表（模块 ID → 已下载字节数）。
/// Get the list of currently in-progress community module installs (module ID → downloaded bytes).
#[tauri::command]
pub async fn get_install_tasks(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let tasks: std::collections::HashMap<String, u64> = state.app_state.install_tasks.read().clone();
    Ok(serde_json::to_value(tasks).unwrap())
}

/// 安装指定社区模块（下载 + sha256 校验 + 写入 modules/ 目录）。
/// 安装完成或失败后推送事件 `community-module-install-done`。
///
/// Install a community module (download + sha256 verify + write to modules/).
/// Pushes event `community-module-install-done` on completion or failure.
#[tauri::command]
pub async fn install_community_module(
    module: RegistryModule,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let settings = state.app_state.get_settings();
    let proxy_url = settings.community_proxy_url.clone();
    let mirror_url = settings.community_mirror_url.clone();
    let emitter = Arc::clone(&state.emitter);
    let module_id = module.id.clone();
    let app_state = Arc::clone(&state.app_state);
    let app_state2 = Arc::clone(&state.app_state);

    // 写入安装任务表，记录初始已下载字节数为 0 / Record install task with 0 downloaded bytes
    app_state.install_tasks.write().insert(module_id.clone(), 0);

    let result = community::install_module(&module, proxy_url, mirror_url, |downloaded, total| {
        // 更新内存中的已下载字节数 / Update in-memory downloaded bytes
        app_state2.install_tasks.write().insert(module_id.clone(), downloaded);

        let pct = if total > 0 {
            (downloaded as f64 / total as f64 * 100.0).min(100.0)
        } else {
            -1.0
        };
        emitter.emit(
            "community-module-download-progress",
            &serde_json::json!({
                "moduleId": module_id,
                "downloaded": downloaded,
                "total": total,
                "pct": pct,
            }),
        );
    })
    .await;

    // 无论成功失败都从任务表中删除 / Remove from task map regardless of outcome
    app_state.install_tasks.write().remove(&module.id);

    match result {
        Ok(()) => {
            state.emitter.emit(
                "community-module-install-done",
                &serde_json::json!({ "moduleId": module.id, "success": true }),
            );
            Ok(serde_json::json!({ "ok": true }))
        }
        Err(e) => {
            tracing::error!("community module install failed: id={} error={}", module.id, e);
            state.emitter.emit(
                "community-module-install-done",
                &serde_json::json!({ "moduleId": module.id, "success": false, "error": e.to_string() }),
            );
            Err(e.to_string())
        }
    }
}

/// 卸载指定社区模块（从 modules/ 目录删除对应可执行文件）。
/// 幂等操作：若模块未安装，静默成功。
///
/// Uninstall a community module (remove the executable from modules/ directory).
/// Idempotent: silently succeeds if the module is not installed.
#[tauri::command]
pub async fn uninstall_community_module(module_id: String) -> CmdResult<serde_json::Value> {
    tokio::task::spawn_blocking(move || community::uninstall_module(&module_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(map_err)?;
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Locale ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_locale(locale_code: String) -> CmdResult<serde_json::Value> {
    let lc = locale_code.clone();
    let (locale, warning) = tokio::task::spawn_blocking(move || {
        let data = stripchat_recorder_lib::locale::manager::get_full_locale(&lc);
        let warning = stripchat_recorder_lib::locale::manager::validate_locale_file(&lc);
        (data, warning)
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut result = locale;
    if let Some(w) = warning {
        result["warning"] = serde_json::Value::String(w);
    }
    Ok(result)
}

#[tauri::command]
pub async fn list_locales() -> CmdResult<serde_json::Value> {
    let locales = tokio::task::spawn_blocking(
        stripchat_recorder_lib::locale::manager::list_available_locales,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(locales).unwrap())
}

// ─── Notifications ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_notifications(state: State<'_, DesktopState>) -> CmdResult<serde_json::Value> {
    let notifications = state.app_state.notification_store.list();
    let unread = notifications.len();
    Ok(serde_json::json!({
        "notifications": serde_json::to_value(notifications).unwrap(),
        "unread_count": unread,
    }))
}

/// 标记指定通知（或全部）为已读。空 ids = 全部清除，与 backend
/// `routes::notifications::mark_notifications_read` 的分支逻辑保持一致——
/// `NotificationStore::mark_read(&[])` 对空切片是空操作（保留所有元素），
/// 必须显式调用 `clear_all()` 才能真正清空。
///
/// Mark specified (or all) notifications as read. Empty ids = clear all, matching
/// the branch logic in backend's `routes::notifications::mark_notifications_read` —
/// `NotificationStore::mark_read(&[])` is a no-op on an empty slice (keeps everything),
/// so `clear_all()` must be called explicitly to actually clear.
#[tauri::command]
pub async fn mark_notifications_read(
    ids: Vec<u64>,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    if ids.is_empty() {
        state.app_state.notification_store.clear_all();
    } else {
        state.app_state.notification_store.mark_read(&ids);
    }
    Ok(serde_json::json!({ "ok": true }))
}

// ─── Startup Warnings ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_startup_warnings(
    _state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    // 新架构中启动警告已通过 maintain_output_dir 和 notification_store 处理
    // In the new architecture, startup warnings are handled via maintain_output_dir and notification_store
    Ok(serde_json::json!({
        "missing_streamers": [],
        "missing_pp_results": [],
    }))
}

#[tauri::command]
pub async fn remove_missing_pp_results(
    _paths: Vec<String>,
    _state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    Ok(serde_json::json!({ "ok": true }))
}

// ─── File Ops ─────────────────────────────────────────────────────────────────

/// 打开录制文件（用系统默认播放器）/ Open a recording file with the system default player
#[tauri::command]
pub async fn open_recording(
    path: String,
    app: tauri::AppHandle,
) -> CmdResult<serde_json::Value> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(&path, None::<&str>).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 在文件管理器中打开输出目录 / Open the output directory in the file manager
#[tauri::command]
pub async fn open_output_dir(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    use tauri_plugin_opener::OpenerExt;
    let output_dir = state.app_state.get_settings().output_dir;
    app.opener().open_path(&output_dir, None::<&str>).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 在文件管理器中打开合并后视频文件夹。
/// - ts_merge 节点配置了 output_dir → 打开该目录
/// - 未配置 → 打开 settings.output_dir（分片目录，合并文件存放于此）
///
/// Open the merged video directory in the file manager.
/// - If ts_merge has an output_dir param → open that directory
/// - Otherwise → open settings.output_dir (the fragment dir, where merged files also live)
#[tauri::command]
pub async fn open_merged_dir(
    app: tauri::AppHandle,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    use tauri_plugin_opener::OpenerExt;

    let dir = {
        let pipeline = state.app_state.get_pipeline();
        pipeline.nodes.iter()
            .find(|n| n.module_id == "ts_merge" && n.enabled)
            .and_then(|n| n.params.get("output_dir"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| state.app_state.get_settings().output_dir)
    };

    // 目录不存在时提前报错，比让 opener 静默失败更友好
    // Fail early if the directory doesn't exist — better than opener silently doing nothing
    if !std::path::Path::new(&dir).exists() {
        return Err(format!("目录不存在: {}", dir));
    }

    app.opener().open_path(&dir, None::<&str>).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 读取输出目录中的图片文件（base64 编码）
/// 用系统默认程序打开模块输出文件（如 contact_sheet 预览图）。
/// 通过 video_path + module_id 经 meta 解析出真实路径后调用 opener，
/// 与 open_recording 保持一致，无需 base64 转换。
///
/// Open a module output file (e.g. contact_sheet preview image) with the system
/// default application. Resolves the real path via meta from video_path + module_id,
/// then calls opener — consistent with open_recording, no base64 conversion needed.
#[tauri::command]
pub async fn open_output_file(
    video_path: String,
    module_id: String,
    app: tauri::AppHandle,
) -> CmdResult<serde_json::Value> {
    let vp = std::path::PathBuf::from(&video_path);
    let mid = module_id;

    let real_path_str = tokio::task::spawn_blocking(move || {
        let pp_execution = stripchat_recorder_lib::recording::meta::read_meta(&vp)
            .and_then(|m| m.pp_execution);
        let outputs =
            stripchat_recorder_lib::recording::meta::extract_verified_module_outputs(
                pp_execution.as_deref(),
            );
        outputs.get(&mid).cloned()
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "模块输出不存在或文件已被删除".to_string())?;

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(&real_path_str, None::<&str>)
        .map_err(|e| e.to_string())?;

    Ok(serde_json::json!({ "ok": true }))
}

/// 获取模块输出路径 / Get module output paths
#[tauri::command]
pub async fn get_module_outputs(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<serde_json::Value> {
    let video_path = std::path::PathBuf::from(&path);
    let app_state = Arc::clone(&state.app_state);
    let outputs = tokio::task::spawn_blocking(move || {
        // extract_verified_module_outputs(pp_execution: Option<&[PpExecutionEntry]>)
        // read_meta 直接返回 Option<VideoMeta> / read_meta directly returns Option<VideoMeta>
        let pp_execution = stripchat_recorder_lib::recording::meta::read_meta(&video_path)
            .and_then(|m| m.pp_execution);
        let entries_ref = pp_execution.as_deref();
        stripchat_recorder_lib::recording::meta::extract_verified_module_outputs(entries_ref)
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app_state;
    Ok(serde_json::to_value(outputs).unwrap())
}

// ─── File System (Directory Browser) ─────────────────────────────────────────

/// 列出目录内容 / List directory contents
#[tauri::command]
pub async fn list_dir(path: String) -> CmdResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::system::fs_browser::list_dir_inner(&path)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(result).unwrap())
}

/// 列出所有驱动器 / List all drives
#[tauri::command]
pub async fn list_drives() -> CmdResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(
        stripchat_recorder_lib::system::fs_browser::list_drives_inner,
    )
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(serde_json::to_value(result).unwrap())
}

/// 创建目录 / Create a directory
#[tauri::command]
pub async fn create_dir(parent: String, name: String) -> CmdResult<serde_json::Value> {
    let result = tokio::task::spawn_blocking(move || {
        stripchat_recorder_lib::system::fs_browser::create_dir_inner(&parent, &name)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "path": result }))
}
