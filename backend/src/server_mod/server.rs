//! HTTP 服务器模式 / HTTP Server Mode
//!
//! 基于 Axum 构建的 HTTP API 服务器，提供与 Tauri 命令等价的 REST 接口和 SSE 实时事件流。
//! 同时内嵌前端静态资源（通过 rust-embed 编译进二进制）。
//!
//! Axum-based HTTP API server providing REST endpoints equivalent to Tauri commands,
//! plus an SSE real-time event stream.
//! Also embeds frontend static assets (compiled into the binary via rust-embed).

use crate::config::settings::AppState;
use crate::core::emitter::{BroadcastEmitter, Event};
use crate::recording::recorder::RecorderManager;
use crate::relay::handler::{RelayState, relay_sessions, stop_relay_handler, stream_handler};
use crate::relay::state::RelayManager;
use crate::server_mod::auth::TokenStore;
use crate::server_mod::routes::{
    auth::{auth_status, change_password, init_password, login, logout, renew},
    locale::{get_locale_handler, list_locales_handler},
    postprocess::{
        cancel_postprocess, get_module_outputs, get_pipeline, get_postprocess_tasks, list_modules,
        run_postprocess, run_postprocess_batch, save_pipeline,
    },
    recording::{
        delete_recording, get_merging_dirs_handler, list_recordings, open_output_dir,
        open_recording, serve_output_file,
    },
    settings::{
        add_mouflon_key, create_dir_handler, get_disk_space_handler, get_settings,
        get_startup_warnings_handler, list_dir_handler, list_drives_handler, list_mouflon_keys,
        remove_mouflon_key, save_settings, sync_mouflon_keys,
    },
    streamer::{
        add_streamer, list_streamers, remove_streamer, set_auto_record, start_recording,
        stop_recording, verify_streamer,
    },
};
use crate::server_mod::sse::sse_handler;
use crate::server_mod::static_files::static_handler;
use crate::streaming::monitor::StatusMonitor;
use axum::{
    Router,
    middleware,
    routing::{delete, get, post},
};
use std::sync::Arc;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};

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
    /// 转发管理器 / Relay manager
    pub relay_manager: Arc<RelayManager>,
    /// Session token 存储（登录认证）/ Session token store (login auth)
    pub token_store: TokenStore,
}

/// 构建 Axum 路由器，注册所有 API 路由和静态资源回退处理器。
/// Build the Axum router, registering all API routes and the static asset fallback handler.
pub fn build_router(state: ServerState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let relay_state = RelayState {
        app_state: Arc::clone(&state.app_state),
        relay_manager: Arc::clone(&state.relay_manager),
    };
    // /stream/{modelname} 路由（独立 state）/ /stream/{modelname} route (independent state)
    let stream_router: Router<()> = Router::new()
        .route("/{modelname}", get(stream_handler))
        .with_state(relay_state.clone());
    // /api/relay/sessions 路由 / /api/relay/sessions route
    let relay_api_router: Router<()> = Router::new()
        .route("/sessions", get(relay_sessions))
        .route("/{modelname}/stop", post(stop_relay_handler))
        .with_state(relay_state);

    // 主路由器先固化 state，再合并转发路由
    // Finalize main router state first, then merge relay router

    // 认证路由 + 公开路由（豁免 auth 中间件）
    // Auth routes + public routes (exempt from auth middleware)
    let auth_routes = Router::new()
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/init-password", post(init_password))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/renew", post(renew))
        // locale 路由公开：未登录时前端也需要加载语言包（否则登录/setup页文字乱码）
        // Locale routes are public: frontend needs locale data before login (login/setup pages)
        .route("/api/locale/{locale_code}", get(get_locale_handler))
        .route("/api/locales", get(list_locales_handler));

    // 需要鉴权的 API 路由
    // API routes that require authentication
    let protected_routes = Router::new()
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
        .route("/api/mouflon-keys/sync", post(sync_mouflon_keys))
        .route("/api/startup-warnings", get(get_startup_warnings_handler))
        .route("/api/disk-space", get(get_disk_space_handler))
        .route("/api/fs/list-dir", get(list_dir_handler))
        .route("/api/fs/list-drives", get(list_drives_handler))
        .route("/api/fs/create-dir", post(create_dir_handler))
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
        .route("/api/auth/change-password", post(change_password))
        .route("/api/events", get(sse_handler))
        .route_layer(middleware::from_fn_with_state(
            state.token_store.clone(),
            crate::server_mod::auth::auth_middleware,
        ));

    let main_router: Router<()> = Router::new()
        .merge(auth_routes)
        .merge(protected_routes)
        .with_state(state)
        .fallback(static_handler);

    // 合并转发路由（两者都是 Router<()>，可以直接 merge）
    // Merge relay routes (both are Router<()>, can merge directly)
    main_router
        .nest("/stream", stream_router)
        .nest("/api/relay", relay_api_router)
        .layer(cors)
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
    let (tx, _) = broadcast::channel::<Event>(4096);
    let emitter: Arc<dyn crate::core::emitter::Emitter> = Arc::new(BroadcastEmitter(tx.clone()));
    let monitor = StatusMonitor::new(Arc::clone(&app_state), Arc::clone(&recorder));

    // 执行所有启动时一次性初始化任务（locale 初始化、ffmpeg 检查、FS 监控）。
    // 输出目录维护（合并遗留分片、重建 meta 等）由下方的定时任务首次立即执行覆盖，
    // 不在此处单独重复。
    //
    // Run all one-shot startup tasks (locale init, ffmpeg check, FS watchers).
    // Output-directory maintenance (merging leftover segments, rebuilding meta, etc.) is
    // covered by the scheduled task's immediate first run below, not duplicated here.
    crate::server_mod::startup::run_all(Arc::clone(&app_state), Arc::clone(&emitter));

    // 启动所有后台定时任务（状态轮询、配置检查、密钥同步、输出目录维护）
    // Launch all background scheduled tasks (status polling, config checks, key sync, output dir maintenance)
    crate::server_mod::scheduler::start_all(
        Arc::clone(&app_state),
        Arc::clone(&monitor),
        Arc::clone(&emitter),
        Arc::clone(&recorder),
    );

    let password_configured = app_state.has_admin_password();
    let server_state = ServerState {
        app_state,
        recorder,
        monitor,
        emitter,
        broadcast_tx: tx,
        relay_manager: RelayManager::new(),
        token_store: TokenStore::new(password_configured),
    };

    let app = build_router(server_state);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {} — {}", addr, e));

    println!("Server mode: listening on http://{}", addr);
    println!("API docs: GET /api/events → SSE stream");
    axum::serve(listener, app).await.expect("server error");
}
