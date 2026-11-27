//! 资源占用端点 / Resource-usage endpoint
//!
//! `GET /api/process-usage`：返回本程序进程及其唤起的全部后代进程
//! （录制 ffmpeg、后处理模块、模块再唤起的 ffmpeg 等）的 CPU/内存
//! 快照。单次请求内部需跨过一个 CPU 差分采样窗口（约 450ms），前端
//! 以「请求 → 间隔 → 请求」的节奏轮询即可，无需高频并发调用。
//!
//! `GET /api/process-usage`: returns a CPU/memory snapshot of this process
//! plus every descendant it spawned (recording ffmpeg, post-process
//! modules, ffmpeg spawned by modules, ...). One request spans a CPU
//! differential sampling window (~450 ms) internally; the frontend simply
//! polls in a request → interval → request cadence — no need for
//! high-frequency concurrent calls.

use crate::server::error::ApiResult;
use crate::system::process::{ProcessUsageSnapshot, collect_process_usage};
use axum::Json;

/// `GET /api/process-usage` handler / `GET /api/process-usage` handler
pub async fn get_process_usage() -> ApiResult<ProcessUsageSnapshot> {
	Ok(Json(collect_process_usage().await))
}
