//! FFmpeg 下载/安装进度模型 / FFmpeg Download/Install Progress Model
//!
//! 进度通过 SSE `ffmpeg-progress` 事件广播给前端，同时写入
//! [`FfmpegStateStore`]（`AppState.ffmpeg_state`）供重连的客户端通过状态
//! 接口恢复显示：
//! - [`FfmpegKind`]：安装 / 更新的种类，决定进度事件与日志措辞
//! - [`FfmpegProgress`]：进度状态机（idle → downloading → installing →
//!   done / error）
//! - [`new_ffmpeg_state`]：创建初始为 `Idle` 的进度状态存储
//!
//! Progress is broadcast to the frontend via the SSE `ffmpeg-progress` event
//! and stored in [`FfmpegStateStore`] (`AppState.ffmpeg_state`) so reconnecting
//! clients can restore an in-progress display via the status endpoint:
//! - [`FfmpegKind`]: install vs update, driving event and log wording
//! - [`FfmpegProgress`]: the progress state machine (idle → downloading →
//!   installing → done / error)
//! - [`new_ffmpeg_state`]: create a progress store initialized to `Idle`

use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;

/// FFmpeg 安装 / 更新的种类，决定进度事件与日志措辞。
/// Kind of FFmpeg install/update, driving progress-event and log wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FfmpegKind {
    /// 首次安装（本地没有可用的 ffmpeg）/ First install (no usable ffmpeg present)
    Install,
    /// 对已安装副本的自动更新 / Auto-update of a managed copy
    Update,
}

/// FFmpeg 下载/安装进度，通过 SSE `ffmpeg-progress` 事件广播给前端，
/// 同时写入 [`FfmpegStateStore`] 供重连的客户端通过状态接口恢复显示。
///
/// FFmpeg download/install progress, broadcast via the SSE `ffmpeg-progress`
/// event and stored in [`FfmpegStateStore`] so reconnecting clients can restore
/// an in-progress display via the status endpoint.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FfmpegProgress {
    /// 空闲（无下载/安装进行中）/ Idle (nothing in progress)
    Idle,
    /// 正在下载压缩包 / Downloading the archive
    Downloading {
        kind: FfmpegKind,
        downloaded: u64,
        total: u64,
        pct: Option<u8>,
    },
    /// 正在校验并安装；`pct` 为解压进度百分比（`None` = 解压尚未开始/
    /// 暂不可计算，前端显示空轨道）/ Verifying and installing; `pct` is the
    /// extraction percentage (`None` = extraction hasn't started or isn't
    /// computable — the frontend shows an empty track)
    Installing { kind: FfmpegKind, pct: Option<u8> },
    /// 完成 / Done
    Done { kind: FfmpegKind },
    /// 失败 / Failed
    Error { kind: FfmpegKind, message: String },
}

/// FFmpeg 进度状态存储（`AppState.ffmpeg_state`）。
/// FFmpeg progress state store (`AppState.ffmpeg_state`).
pub type FfmpegStateStore = Arc<RwLock<FfmpegProgress>>;

/// 创建初始为 `Idle` 的进度状态存储。
/// Create a progress state store initialized to `Idle`.
pub fn new_ffmpeg_state() -> FfmpegStateStore {
    Arc::new(RwLock::new(FfmpegProgress::Idle))
}
