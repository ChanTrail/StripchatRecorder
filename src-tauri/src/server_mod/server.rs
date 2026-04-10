//! HTTP 服务器模式 / HTTP Server Mode
//!
//! 基于 Axum 构建的 HTTP API 服务器，提供与 Tauri 命令等价的 REST 接口和 SSE 实时事件流。
//! 同时内嵌前端静态资源（通过 rust-embed 编译进二进制）。
//!
//! Axum-based HTTP API server providing REST endpoints equivalent to Tauri commands,
//! plus an SSE real-time event stream.
//! Also embeds frontend static assets (compiled into the binary via rust-embed).

use crate::core::emitter::{BroadcastEmitter, EmitterExt, Event};
use crate::streaming::monitor::StatusMonitor;
use crate::recording::recorder::RecorderManager;
use crate::config::settings::AppState;
use axum::extract::Query;
use axum::{
    extract::{Path, State as AxumState},
    http::{header, StatusCode, Uri},
    response::{
        sse::{self, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};

/// 嵌入的前端静态资源（从 ../dist/ 目录编译进二进制）。
/// Embedded frontend static assets (compiled from ../dist/ into the binary).
#[derive(RustEmbed)]
#[folder = "../dist/"]
struct FrontendAssets;

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match FrontendAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => match FrontendAssets::get("index.html") {
            Some(content) => ([(header::CONTENT_TYPE, "text/html")], content.data).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

/// Axum 路由共享状态 / Axum router shared state
#[derive(Clone)]
pub struct ServerState {
    /// 应用全局状态 / Application global state
    pub app_state: Arc<AppState>,
    /// 录制管理器 / Recorder manager
    pub recorder: Arc<RecorderManager>,
    /// 状态监控器 / Status monitor
    pub monitor: Arc<StatusMonitor>,
    /// 事件发射器 / Event emitter
    pub emitter: Arc<dyn crate::core::emitter::Emitter>,
    /// SSE 广播发送端 / SSE broadcast sender
    pub broadcast_tx: broadcast::Sender<Event>,
}

struct ApiError(String);

impl From<crate::core::error::AppError> for ApiError {
    fn from(e: crate::core::error::AppError) -> Self {
        ApiError(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, self.0).into_response()
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

/// 构建 Axum 路由器，注册所有 API 路由和静态资源回退处理器。
/// Build the Axum router, registering all API routes and the static asset fallback handler.
pub fn build_router(state: ServerState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/streamers", get(list_streamers).post(add_streamer))
        .route("/api/streamers/{name}", delete(remove_streamer))
        .route("/api/streamers/{name}/auto-record", post(set_auto_record))
        .route("/api/streamers/{name}/start", post(start_recording))
        .route("/api/streamers/{name}/stop", post(stop_recording))
        .route("/api/streamers/{name}/verify", get(verify_streamer))
        .route("/api/settings", get(get_settings).post(save_settings))
        .route(
            "/api/mouflon-keys",
            get(list_mouflon_keys).post(add_mouflon_key),
        )
        .route("/api/mouflon-keys/{pkey}", delete(remove_mouflon_key))
        .route("/api/startup-warnings", get(get_startup_warnings_handler))
        .route(
            "/api/startup-warnings/pp-results",
            post(remove_missing_pp_results_handler),
        )
        .route("/api/disk-space", get(get_disk_space_handler))
        .route("/api/recordings", get(list_recordings))
        .route("/api/recordings/merging", get(get_merging_dirs_handler))
        .route("/api/recordings/delete", post(delete_recording))
        .route("/api/recordings/open", post(open_recording))
        .route("/api/recordings/open-dir", post(open_output_dir))
        .route("/api/recordings/postprocess", post(run_postprocess))
        .route(
            "/api/recordings/postprocess-batch",
            post(run_postprocess_batch),
        )
        .route(
            "/api/recordings/postprocess-cancel",
            post(cancel_postprocess),
        )
        .route("/api/pipeline", get(get_pipeline).post(save_pipeline))
        .route("/api/modules", get(list_modules))
        .route("/api/postprocess-tasks", get(get_postprocess_tasks))
        .route("/api/recordings/module-outputs", post(get_module_outputs))
        .route("/api/files", get(serve_output_file))
        .route("/api/events", get(sse_handler))
        .layer(cors)
        .with_state(state)
        .fallback(static_handler)
}

async fn sse_handler(
    AxumState(s): AxumState<ServerState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<sse::Event, Infallible>>> {
    let mut rx = s.broadcast_tx.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(e) => {
                    let data = format!(r#"{{"event":"{}","payload":{}}}"#, e.name, e.payload);
                    yield Ok(sse::Event::default().data(data));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(sse::KeepAlive::default())
}

async fn list_streamers(AxumState(s): AxumState<ServerState>) -> ApiResult<serde_json::Value> {
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
                "viewers": status.as_ref().map(|s| s.viewers).unwrap_or(0),
                "status": status.as_ref().map(|s| s.status.clone()).unwrap_or_default(),
                "thumbnail_url": status.and_then(|s| s.thumbnail_url),
            })
        })
        .collect();
    Ok(Json(serde_json::Value::Array(result)))
}

#[derive(Deserialize)]
struct AddStreamerBody {
    username: String,
}

async fn add_streamer(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<AddStreamerBody>,
) -> ApiResult<serde_json::Value> {
    let username = body.username.trim().to_lowercase();
    if username.is_empty() {
        return Err(ApiError("用户名不能为空".into()));
    }
    let settings = s.app_state.get_settings();
    let api = crate::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )
    .map_err(ApiError::from)?;
    api.get_stream_info(&username, false)
        .await
        .map_err(ApiError::from)?;
    s.app_state
        .add_streamer(&username)
        .map_err(ApiError::from)?;
    s.emitter.emit(
        "streamer-added",
        &serde_json::json!({ "username": username }),
    );
    let emitter = Arc::clone(&s.emitter);
    let monitor = Arc::clone(&s.monitor);
    tokio::spawn(async move {
        monitor.poll_one_with_emitter(&username, &emitter).await;
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn remove_streamer(
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
    s.emitter
        .emit("streamer-removed", &serde_json::json!({ "username": name }));
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct AutoRecordBody {
    enabled: bool,
}

async fn set_auto_record(
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

async fn start_recording(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let playlist_url = if let Some(url) = s.monitor.get_cached_playlist_url(&name) {
        url
    } else {
        let settings = s.app_state.get_settings();
        let api = crate::streaming::stripchat::StripchatApi::new_api_only(
            settings.api_proxy_url.as_deref(),
            settings.cdn_proxy_url.as_deref(),
            settings.sc_mirror_url.as_deref(),
        )
        .map_err(ApiError::from)?;
        let info = api
            .get_stream_info(&name, true)
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

async fn stop_recording(
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

async fn verify_streamer(
    AxumState(s): AxumState<ServerState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    let api = crate::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    )
    .map_err(ApiError::from)?;
    match api.get_stream_info(&name, false).await {
        Ok(_) => Ok(Json(serde_json::json!({ "exists": true }))),
        Err(crate::core::error::AppError::UserNotFound(_)) => {
            Ok(Json(serde_json::json!({ "exists": false })))
        }
        Err(e) => Err(ApiError(e.to_string())),
    }
}

async fn get_settings(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::config::settings::Settings> {
    Ok(Json(s.app_state.get_settings()))
}

async fn save_settings(
    AxumState(s): AxumState<ServerState>,
    Json(new_settings): Json<crate::config::settings::Settings>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .update_settings(new_settings)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("settings-updated", &s.app_state.get_settings());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_mouflon_keys(AxumState(s): AxumState<ServerState>) -> ApiResult<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(s.app_state.get_mouflon_keys()).unwrap(),
    ))
}

#[derive(Deserialize)]
struct MouflonKeyBody {
    pkey: String,
    pdkey: String,
}

async fn add_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<MouflonKeyBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .add_mouflon_key(&body.pkey, &body.pdkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn remove_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Path(pkey): Path<String>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .remove_mouflon_key(&pkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_startup_warnings_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let state = Arc::clone(&s.app_state);
    let warnings = tokio::task::spawn_blocking(move || {
        let data = state.data.read();
        let missing_pp_results: Vec<String> = data
            .pp_results
            .keys()
            .filter(|path| !std::path::Path::new(path).exists())
            .cloned()
            .collect();
        serde_json::json!({
            "missing_streamers": [],
            "missing_pp_results": missing_pp_results,
        })
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?;
    Ok(Json(warnings))
}

#[derive(Deserialize)]
struct RemovePpResultsBody {
    paths: Vec<String>,
}

async fn remove_missing_pp_results_handler(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<RemovePpResultsBody>,
) -> ApiResult<serde_json::Value> {
    let mut data = s.app_state.data.write();
    for path in &body.paths {
        data.pp_results.remove(path);
    }
    drop(data);
    s.app_state.save().map_err(ApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_disk_space_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let state = Arc::clone(&s.app_state);
    let result = tokio::task::spawn_blocking(move || {
        crate::commands::settings_cmd::get_disk_space_inner(&state.get_settings().output_dir)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))??;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn list_recordings(AxumState(s): AxumState<ServerState>) -> ApiResult<serde_json::Value> {
    let state = Arc::clone(&s.app_state);
    let recorder = Arc::clone(&s.recorder);
    let files = tokio::task::spawn_blocking(move || {
        crate::commands::recording_cmd::list_recordings_inner(&state, &recorder)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(|e| ApiError(e.to_string()))?;
    Ok(Json(serde_json::to_value(files).unwrap()))
}

async fn get_merging_dirs_handler(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    let merge_format = settings.merge_format.clone();

    let make_entry = |path: &std::path::PathBuf, status: &str| {
        let path_str = path.to_string_lossy().to_string();
        let stem = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let username = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let parent = path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
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

    let mut result: Vec<serde_json::Value> = s
        .recorder
        .merging_dirs
        .read()
        .iter()
        .map(|p| make_entry(p, "merging"))
        .collect();
    result.extend(
        s.recorder
            .waiting_merge_dirs
            .read()
            .iter()
            .map(|p| make_entry(p, "waiting")),
    );
    Ok(Json(serde_json::json!(result)))
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

async fn delete_recording(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let recorder = Arc::clone(&s.recorder);
    let state = Arc::clone(&s.app_state);
    let path = body.path.clone();
    tokio::task::spawn_blocking(move || {
        crate::commands::recording_cmd::delete_recording_inner(&path, &recorder, &state)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?
    .map_err(ApiError::from)?;
    s.emitter.emit(
        "recording-deleted",
        &serde_json::json!({ "path": body.path }),
    );
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn open_recording(Json(body): Json<PathBody>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "path": body.path })))
}

async fn open_output_dir(AxumState(s): AxumState<ServerState>) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    Ok(Json(serde_json::json!({ "path": settings.output_dir })))
}

async fn run_postprocess(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let pipeline = s.app_state.get_pipeline();
    if pipeline.nodes.is_empty() {
        return Err(ApiError("后处理流水线为空".into()));
    }
    let video_path = std::path::PathBuf::from(&body.path);
    let emitter = Arc::clone(&s.emitter);
    let state = Arc::clone(&s.app_state);
    state.pp_task_enqueue(&body.path);
    emitter.emit(
        "postprocess-waiting",
        &serde_json::json!({ "path": body.path }),
    );
    tokio::task::spawn_blocking(move || {
        crate::commands::postprocess_cmd::run_postprocess_for_path_inner(
            &video_path,
            &pipeline,
            &emitter,
            &state,
        );
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
struct BatchPathBody {
    paths: Vec<String>,
}

async fn run_postprocess_batch(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<BatchPathBody>,
) -> ApiResult<serde_json::Value> {
    let pipeline = s.app_state.get_pipeline();
    if pipeline.nodes.is_empty() {
        return Err(ApiError("后处理流水线为空".into()));
    }
    for path in body.paths {
        let video_path = std::path::PathBuf::from(&path);
        let emitter = Arc::clone(&s.emitter);
        let state = Arc::clone(&s.app_state);
        let pipeline = pipeline.clone();
        state.pp_task_enqueue(&path);
        emitter.emit("postprocess-waiting", &serde_json::json!({ "path": path }));
        tokio::task::spawn_blocking(move || {
            crate::commands::postprocess_cmd::run_postprocess_for_path_inner(
                &video_path,
                &pipeline,
                &emitter,
                &state,
            );
        });
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn cancel_postprocess(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state.pp_task_cancel(&body.path);
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_pipeline(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::postprocess::pipeline::PipelineConfig> {
    Ok(Json(s.app_state.get_pipeline()))
}

async fn save_pipeline(
    AxumState(s): AxumState<ServerState>,
    Json(pipeline): Json<crate::postprocess::pipeline::PipelineConfig>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .update_pipeline(pipeline)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("pipeline-updated", &s.app_state.get_pipeline());
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn list_modules() -> ApiResult<serde_json::Value> {
    let modules: Vec<crate::postprocess::pipeline::ModuleInfo> = tokio::task::spawn_blocking(crate::postprocess::pipeline::discover_modules)
        .await
        .unwrap_or_default();
    Ok(Json(serde_json::to_value(modules).unwrap()))
}

async fn get_postprocess_tasks(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(s.app_state.get_pp_tasks()).unwrap(),
    ))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

async fn serve_output_file(
    AxumState(s): AxumState<ServerState>,
    Query(q): Query<FileQuery>,
) -> impl IntoResponse {
    let settings = s.app_state.get_settings();
    let output_dir = std::path::Path::new(&settings.output_dir);
    let requested = std::path::Path::new(&q.path);

    let canonical_output = match output_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "output dir error").into_response(),
    };
    let canonical_requested = match requested.canonicalize() {
        Ok(p) => p,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };
    if !canonical_requested.starts_with(&canonical_output) {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let data = match std::fs::read(&canonical_requested) {
        Ok(d) => d,
        Err(_) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };

    let ext = canonical_requested
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mime = match ext {
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => "application/octet-stream",
    };

    ([(header::CONTENT_TYPE, mime)], data).into_response()
}

async fn get_module_outputs(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PathBody>,
) -> ApiResult<serde_json::Value> {
    let video_path = std::path::Path::new(&body.path);
    let pipeline = s.app_state.get_pipeline();
    let mut outputs: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    for node in &pipeline.nodes {
        if !node.enabled {
            continue;
        }
        if node.module_id == "contact_sheet" {
            let format = node
                .params
                .get("format")
                .and_then(|v: &serde_json::Value| v.as_str())
                .unwrap_or("webp");
            if let (Some(parent), Some(stem)) = (
                video_path.parent(),
                video_path.file_stem().and_then(|s| s.to_str()),
            ) {
                let img_path = parent.join(format!("{}.{}", stem, format));
                if img_path.exists() {
                    outputs.insert(
                        node.module_id.clone(),
                        img_path.to_string_lossy().to_string(),
                    );
                }
            }
        }
    }

    Ok(Json(serde_json::to_value(outputs).unwrap()))
}

/// 初始化并启动 HTTP 服务器模式。
/// Initialize and start the HTTP server mode.
pub async fn run_server(port: u16) {
    let log_dir = AppState::log_dir();
    if let Err(e) = crate::core::logging::init_logging(&log_dir) {
        eprintln!("Failed to initialize logging: {}", e);
    }

    let app_state = AppState::new().expect("Failed to initialize app state");
    let recorder = RecorderManager::new(Arc::clone(&app_state));
    let (tx, _) = broadcast::channel::<Event>(256);
    let emitter: Arc<dyn crate::core::emitter::Emitter> = Arc::new(BroadcastEmitter(tx.clone()));
    let monitor = StatusMonitor::new(Arc::clone(&app_state), Arc::clone(&recorder));
    crate::watcher::fs_watch::start_recordings_dir_watcher(Arc::clone(&app_state), Arc::clone(&emitter));
    crate::watcher::fs_watch::start_modules_dir_watcher(Arc::clone(&emitter));

    if !crate::recording::recorder::ffmpeg_available() {
        tracing::warn!("ffmpeg not found on PATH");
    }
    {
        let settings = app_state.get_settings();
        let output_dir = std::path::PathBuf::from(&settings.output_dir);
        let merge_format = settings.merge_format.clone();
        let emitter_clone = Arc::clone(&emitter);
        let recorder_clone = Arc::clone(&recorder);
        tokio::task::spawn_blocking(move || {
            crate::recording::recorder::startup_merge_leftover_segments(
                &output_dir,
                &merge_format,
                &emitter_clone,
                &recorder_clone,
            );
            crate::recording::recorder::startup_remove_empty_dirs(&output_dir);
        });
    }

    let monitor_clone = Arc::clone(&monitor);
    let emitter_clone = Arc::clone(&emitter);
    tokio::spawn(async move {
        monitor_clone.start_with_emitter(emitter_clone).await;
    });

    let app_state_clone = Arc::clone(&app_state);
    let emitter_clone2 = Arc::clone(&emitter);
    tokio::spawn(async move {
        crate::config::settings::schedule_config_checks(app_state_clone, emitter_clone2).await;
    });

    let server_state = ServerState {
        app_state,
        recorder,
        monitor,
        emitter,
        broadcast_tx: tx,
    };

    let app = build_router(server_state);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {}:{} — {}", addr, port, e));

    println!("Server mode: listening on http://{}", addr);
    println!("API docs: GET /api/events → SSE stream");
    axum::serve(listener, app).await.expect("server error");
}
