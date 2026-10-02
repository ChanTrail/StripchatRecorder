//! 转发路由处理器 / Relay Route Handlers
//!
//! 端点：
//! - GET  /stream/{modelname}         → 持续输出 HTTP-FLV 流（按需启动 worker）
//! - GET  /api/relay/sessions         → 查询所有活跃转发会话状态
//! - POST /api/relay/{modelname}/stop → 强制停止指定主播的转发 worker
//!
//! 转发流永远可访问，无需手动启动：
//! - 有请求时自动启动 worker
//! - 上游在线时转发直播流（HTTP-FLV 格式，兼容 flv.js / mpegts.js 等播放器）
//! - 上游离线时 worker 退出，HTTP 连接自然关闭

use super::state::RelayManager;
use super::worker::start_streamer;
use crate::config::app_state::AppState;
use axum::{
    Json,
    body::Body,
    extract::{Path, State as AxumState},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Axum 路由共享状态（转发专用）/ Axum shared state for relay routes
#[derive(Clone)]
pub struct RelayState {
    pub app_state: Arc<AppState>,
    pub relay_manager: Arc<RelayManager>,
}

// ─────────────────────────────────────────────────────────────────────────────

/// GET /stream/{modelname}
///
/// 按需启动转发 worker，持续输出 HTTP-FLV 字节流。
/// 兼容所有支持 HTTP-FLV 的播放器（flv.js、mpegts.js、VLC、PotPlayer 等）。
/// 上游离线时 worker 自动退出，HTTP 响应正常结束，客户端连接关闭。
///
/// Starts relay worker on demand, continuously outputs HTTP-FLV byte stream.
/// Compatible with any player that supports HTTP-FLV (flv.js, mpegts.js, VLC, PotPlayer, etc.).
/// When upstream goes offline the worker exits, the HTTP response ends, and the client disconnects.
pub async fn stream_handler(
    AxumState(s): AxumState<RelayState>,
    Path(modelname): Path<String>,
) -> Response {
    // 在单次写锁内完成"已存在则订阅，否则创建会话并订阅"，并发请求只有一个会拿到
    // new_worker，因此每个会话只会启动一个 worker。
    // Check-and-create plus subscribe happen within a single write lock; among concurrent
    // requests only one receives new_worker, so exactly one worker is started per session.
    let sub = s.relay_manager.subscribe_or_create(&modelname);
    if let Some(worker) = sub.new_worker {
        start_streamer(
            modelname.clone(),
            sub.session_id,
            Arc::clone(&s.app_state),
            Arc::clone(&s.relay_manager),
            worker,
        );
    }
    let rx = sub.rx;
    let prebuf = sub.prebuffer;

    let relay_manager = Arc::clone(&s.relay_manager);
    let modelname_clone = modelname.clone();

    // RAII guard：无论 stream 正常结束还是客户端强制断开，都能保证 unsubscribe 被调用。
    // 携带 session_id，旧会话的连接断开不会扣减新会话的计数。
    // RAII guard: ensures unsubscribe is called whether the stream ends normally or the client disconnects abruptly.
    // Carries session_id so a connection of an old session never decrements a newer session's count.
    struct UnsubscribeGuard {
        relay_manager: Arc<RelayManager>,
        username: String,
        session_id: u64,
    }
    impl Drop for UnsubscribeGuard {
        fn drop(&mut self) {
            self.relay_manager.unsubscribe(&self.username, self.session_id);
        }
    }
    let _guard = UnsubscribeGuard {
        relay_manager: Arc::clone(&relay_manager),
        username: modelname_clone.clone(),
        session_id: sub.session_id,
    };

    // 连接断开时减少计数 / Decrement connection count on disconnect
    let stream = async_stream::stream! {
        // 将 guard 移入 stream 闭包，确保 stream 被 drop 时触发 unsubscribe
        // Move guard into stream closure so unsubscribe fires when stream is dropped
        let _guard = _guard;
        let mut rx = rx;

        // 先将预缓冲数据推给播放器，使其立即有数据可解码，无需等待 worker 下一批产出。
        // Push prebuffered data first so the player can start decoding immediately
        // without waiting for the worker's next output cycle.
        for chunk in prebuf {
            yield Ok::<axum::body::Bytes, std::convert::Infallible>(
                axum::body::Bytes::from(chunk.as_ref().clone())
            );
        }

        loop {
            match rx.recv().await {
                Ok(chunk) => {
                    yield Ok::<axum::body::Bytes, std::convert::Infallible>(
                        axum::body::Bytes::from(chunk.as_ref().clone())
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    (
        [
            (header::CONTENT_TYPE, "video/x-flv"),
            (header::CACHE_CONTROL, "no-cache, no-store"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (header::TRANSFER_ENCODING, "chunked"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// GET /api/relay/sessions
///
/// 返回所有活跃转发会话的状态列表。
/// Returns the status list of all active relay sessions.
pub async fn relay_sessions(
    AxumState(s): AxumState<RelayState>,
) -> impl IntoResponse {
    let sessions = s.relay_manager.get_all_status();
    Json(sessions)
}

/// POST /api/relay/{modelname}/stop
///
/// 强制停止指定主播的转发 worker，无论当前是否有播放器连接。
/// 适用于 PotPlayer 等在关闭时会短暂重连、导致空闲超时无法触发的播放器。
///
/// Forcefully stops the relay worker for the given streamer, regardless of active connections.
/// Useful for players like PotPlayer that briefly reconnect on close, preventing idle timeout.
pub async fn stop_relay_handler(
    AxumState(s): AxumState<RelayState>,
    Path(modelname): Path<String>,
) -> impl IntoResponse {
    s.relay_manager.remove(&modelname);
    (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
}
