//! 主窗口尺寸管理 / Main window sizing
//!
//! 组装主窗口显示前的尺寸处理：[`fit`] 在启动时把超出所在显示器工作区的
//! 窗口收缩一次。高分辨率缩放遵循 Per-Monitor V2 的系统语义：Windows/tao
//! 在缩放倍率变化时保持窗口逻辑尺寸并按真实 DPI 重映射物理像素，WebView2
//! 随之以真实 DPI 锐利渲染；这里不再反向缩放窗口或 WebView，避免高倍率
//! 显示器上的界面被人为锁定在小尺寸。
//!
//! 运行期不监听 `Resized`，最大化、贴边分屏等系统窗口操作不会被干预。
//!
//! Assembles the main window's sizing before it is shown: [`fit`] shrinks the
//! window once at startup when it overflows its monitor's work area.
//! High-resolution scaling follows the native Per-Monitor V2 semantics:
//! Windows/tao keeps the logical window size while remapping it to physical
//! pixels at the real DPI, and WebView2 renders crisply at that DPI. Neither
//! the window nor the WebView is inverse-scaled here, so the interface is not
//! artificially locked to a small size on high-scale monitors.
//!
//! Nothing listens to `Resized` at runtime, so maximize, snap and other system
//! window operations are left alone.

mod fit;

/// 处理主窗口尺寸并显示、聚焦窗口。
///
/// 内部的 `inner_size` / `scale_factor` / `hwnd` 等 getter 需要事件循环线程
/// 回包，必须在非事件循环线程上调用（setup 期间事件循环尚未开始泵送，
/// 在主线程调用会永久阻塞）；本函数在 getter 解阻塞后才显示窗口，天然
/// 保证"先处理尺寸、后显示"——窗口创建时是隐藏的，用户看不到越界瞬间。
///
/// The `inner_size` / `scale_factor` / `hwnd` getters inside round-trip
/// through the event-loop thread, so this must run off it (during setup the
/// loop isn't pumping yet; calling on the main thread would block forever).
/// The window is only shown after those getters return, which naturally
/// orders "size first, show after" — the window is created hidden, so users
/// never see an overflowing frame.
///
/// Params：
/// - window: tauri::WebviewWindow —— 主窗口 / The main window
pub fn prepare_and_show(window: tauri::WebviewWindow) {
	fit::shrink_to_work_area(&window);
	let _ = window.show();
	let _ = window.set_focus();
}
