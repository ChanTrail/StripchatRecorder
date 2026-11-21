//! FFmpeg 进度事件广播 / FFmpeg Progress Event Broadcasting
//!
//! - [`emit_progress`]：写入 [`FFmpegStateStore`] 并广播 SSE `ffmpeg-progress`
//! - [`emit_terminal`]：广播终态（Done / Error）后把存储重置为 `Idle`——存储
//!   只用于晚连接客户端恢复"进行中"的显示，不保留过期终态
//!
//! - [`emit_progress`]: write [`FFmpegStateStore`] and broadcast the SSE
//!   `ffmpeg-progress` event
//! - [`emit_terminal`]: broadcast a terminal state (Done / Error), then reset
//!   the store to `Idle` — the store only restores in-progress displays for
//!   late-connecting clients and must not retain stale terminal states

use crate::core::emitter::{Emitter, EmitterExt};
use crate::ffmpeg::{FFmpegProgress, FFmpegStateStore};
use std::sync::Arc;

// ─── 进度事件 / Progress events ──────────────────────────────────────────────

/// 写入状态存储并广播 `ffmpeg-progress` 事件。
/// Store the progress and broadcast the `ffmpeg-progress` event.
pub(super) fn emit_progress(
	store: &FFmpegStateStore,
	emitter: &Arc<dyn Emitter>,
	progress: FFmpegProgress,
) {
	*store.write() = progress.clone();
	emitter.emit("ffmpeg-progress", &progress);
}

/// 广播终态（Done / Error）后把存储重置为 `Idle`：存储只用于让晚连接的客户端
/// 恢复"进行中"的下载显示，不应保留已过期的终态。
///
/// Broadcast a terminal state (Done / Error), then reset the store to `Idle`:
/// the store only exists so late-connecting clients can restore an in-progress
/// download — they must never see a stale terminal state.
pub(super) fn emit_terminal(
	store: &FFmpegStateStore,
	emitter: &Arc<dyn Emitter>,
	progress: FFmpegProgress,
) {
	emit_progress(store, emitter, progress);
	*store.write() = FFmpegProgress::Idle;
}
