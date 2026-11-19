//! ffmpeg 使用中计数与空闲判定 / ffmpeg In-Use Counter and Idle Check
//!
//! 进程内 ffmpeg/ffprobe 使用计数 + 活跃录制计数组成"空闲"判断，
//! 自动更新只在空闲时替换二进制文件，避免影响正在运行的 ffmpeg 进程：
//! - [`in_use`] / [`InUseGuard`]：RAII 守卫，构造时计数 +1、Drop 时 -1
//! - [`in_use_count`]：当前使用中计数
//! - [`is_idle`]：没有进行中的 ffmpeg 调用且没有活跃录制即为空闲
//!
//! An in-process ffmpeg/ffprobe usage counter combined with the active
//! recording count forms the "idle" check; auto-update only swaps binaries
//! while idle so running ffmpeg processes are not disturbed:
//! - [`in_use`] / [`InUseGuard`]: RAII guard, +1 on construction, -1 on drop
//! - [`in_use_count`]: current in-use count
//! - [`is_idle`]: idle means no in-flight ffmpeg uses and no active recordings

use crate::config::app_state::AppState;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 进程内正在使用 ffmpeg/ffprobe 的调用计数（`in_use()` 守卫递增、Drop 递减）。
/// In-process count of active ffmpeg/ffprobe uses (`in_use()` guard increments, Drop decrements).
static IN_USE: AtomicUsize = AtomicUsize::new(0);

/// RAII 守卫：构造时 `IN_USE` +1，Drop 时 -1。
/// 在启动 ffmpeg/ffprobe 子进程（或会拉起 ffmpeg 的模块进程）的函数作用域内持有，
/// 供自动更新判断"当前是否有 ffmpeg 正在使用"。
///
/// RAII guard: `IN_USE` +1 on construction, -1 on drop.
/// Held for the scope of functions that spawn ffmpeg/ffprobe (or module processes
/// that may spawn ffmpeg), so auto-update can tell whether ffmpeg is in use.
pub struct InUseGuard {
    _private: (),
}

impl Drop for InUseGuard {
    fn drop(&mut self) {
        IN_USE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 获取一个 ffmpeg 使用中守卫（计数 +1，离开作用域自动 -1）。
/// Acquire an ffmpeg in-use guard (count +1, automatically -1 when dropped).
pub fn in_use() -> InUseGuard {
    IN_USE.fetch_add(1, Ordering::Relaxed);
    InUseGuard { _private: () }
}

/// 当前 ffmpeg 使用中计数。
/// Current ffmpeg in-use count.
pub fn in_use_count() -> usize {
    IN_USE.load(Ordering::Relaxed)
}

/// 当前是否空闲：没有进行中的 ffmpeg 调用且没有活跃录制。
/// 自动更新只在空闲时替换二进制文件，避免影响正在运行的 ffmpeg 进程。
///
/// Whether the process is idle: no in-flight ffmpeg uses and no active recordings.
/// Auto-update only swaps binaries while idle, avoiding disturbance to running
/// ffmpeg processes.
pub fn is_idle(app_state: &AppState) -> bool {
    in_use_count() == 0 && app_state.active_recording_count.load(Ordering::Relaxed) == 0
}
