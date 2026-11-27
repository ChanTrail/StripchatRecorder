//! FFmpeg 状态 handler / FFmpeg status handler
//!
//! GET /api/ffmpeg/status — 查询当前 FFmpeg 下载/安装进度
//! （SSE `ffmpeg-progress` 重连后前端用于恢复状态）
//! GET /api/ffmpeg/locations — 查询 system（PATH）与 tools 目录中
//! ffmpeg/ffprobe/ffplay 的存在情况与版本（"FFmpeg 首选项"向导用）

use crate::ffmpeg::download::FFmpegDownloadChoice;
use crate::ffmpeg::{FFmpegLocations, FFmpegProgress};
use crate::server::router::ServerState;
use axum::{Json, extract::State as AxumState, http::StatusCode};
use std::sync::Arc;

/// GET /api/ffmpeg/status — 返回当前 FFmpeg 下载/安装进度。
/// 进度在没有进行中任务时为 `{"state":"idle"}`（终态 Done/Error 已即时重置）。
///
/// Returns the current FFmpeg download/install progress. With nothing in flight
/// it is `{"state":"idle"}` (terminal Done/Error states are reset immediately).
pub async fn get_ffmpeg_status(AxumState(s): AxumState<ServerState>) -> Json<FFmpegProgress> {
	Json(crate::ffmpeg::download::current_progress(&s.app_state))
}

/// GET /api/ffmpeg/locations — 返回 system（PATH）与 tools 目录中
/// ffmpeg/ffprobe/ffplay 的存在情况与 ffmpeg 版本，供"FFmpeg 首选项"向导展示。
/// 探测会对找到的 ffmpeg 运行 `-version`（拉起子进程），放到 blocking 线程执行。
///
/// Returns ffmpeg/ffprobe/ffplay presence plus the ffmpeg version on the system
/// PATH and in the tools directory, for the FFmpeg preference wizard. The probe
/// runs `-version` on each found ffmpeg (spawns child processes), so it runs on
/// a blocking thread.
pub async fn get_ffmpeg_locations() -> Result<Json<FFmpegLocations>, StatusCode> {
	tokio::task::spawn_blocking(crate::ffmpeg::ffmpeg_locations)
		.await
		.map(Json)
		.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// POST /api/ffmpeg/download — 手动触发 tools 目录的 FFmpeg 下载安装
/// （"FFmpeg 首选项"向导的"下载"按钮）。可选 JSON 体
/// `{ "platform": "linux-x86_64-legacy", "variant": "-shared" }` 指定构建
/// 选择（平台轨 × 变体后缀，取值见 `GET /api/ffmpeg/locations` 下发的
/// `download_options`；字段缺省 → 保留已安装标记的轨/形态，无标记回退
/// 宿主默认轨 + 静态）。立即返回 204：实际下载在后台进行，进度经
/// `ffmpeg-progress` SSE 广播。首选项为系统环境变量或平台不支持下载时
/// 为 no-op。
///
/// Manually trigger a tools-directory FFmpeg download/install (the FFmpeg
/// preference wizard's Download button). An optional JSON body
/// `{ "platform": "linux-x86_64-legacy", "variant": "-shared" }` selects the
/// build (platform track × variant suffix; allowed values come from the
/// `download_options` of `GET /api/ffmpeg/locations`; omitted fields keep
/// the installed marker's track/variant, falling back to host default +
/// static). Returns 204 immediately: the download runs in the background
/// with progress over the `ffmpeg-progress` SSE event. No-op when the
/// preference is the system PATH or the platform can't download.
pub async fn download_ffmpeg(
	AxumState(s): AxumState<ServerState>,
	body: Option<Json<FFmpegDownloadChoice>>,
) -> StatusCode {
	let choice = body.map(|b| b.0).unwrap_or_default();
	crate::ffmpeg::download::spawn_manual_install(
		Arc::clone(&s.app_state),
		Arc::clone(&s.emitter),
		choice,
	);
	StatusCode::NO_CONTENT
}
