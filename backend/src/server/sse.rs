//! SSE 实时事件流 handler / SSE real-time event stream handler

use crate::server::router::ServerState;
use axum::{
    extract::State as AxumState,
    response::sse::{self, Sse},
};
use std::convert::Infallible;
use tokio::sync::broadcast;

pub async fn sse_handler(
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
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // 队列溢出，丢失了 n 条事件；断开连接让前端重连并恢复状态
                    // Queue overflow, lost n events; close connection so frontend reconnects and restores state
                    tracing::warn!("SSE broadcast lagged, {} events dropped", n);
                    let data = r#"{"event":"sse-lagged","payload":{}}"#;
                    yield Ok(sse::Event::default().data(data));
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(sse::KeepAlive::default())
}
