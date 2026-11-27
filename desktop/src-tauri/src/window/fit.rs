//! 启动时的工作区适配 / Startup work-area fit
//!
//! `tauri.conf.json` 的 `width`/`height` 是逻辑像素；Windows 开启系统缩放后
//! 同样的逻辑尺寸映射成更多物理像素（1600 逻辑宽在 175% 下是 2800 物理
//! 像素），窗口可能越出屏幕。启动时检查一次，超出则收缩并贴回工作区边缘。
//! 只在启动时执行，运行期不再干预窗口尺寸。
//!
//! `width`/`height` in `tauri.conf.json` are logical pixels; with Windows
//! display scaling the same logical size maps to more physical pixels (1600
//! logical px is 2800 physical px at 175%), so the window may overflow the
//! screen. This checks once at startup and shrinks the window back onto the
//! work-area edge when it overflows. It only runs at startup and never touches
//! the window size afterwards.

/// 窗口超出所在显示器工作区时收缩并贴回工作区边缘。
///
/// 显示器解析失败时静默返回，不阻断启动。只能在非事件循环线程上调用。
///
/// Shrink the window back onto the work-area edge when it overflows its
/// monitor's work area. Returns silently when the monitor can't be resolved,
/// so startup is never blocked. Must be called off the event-loop thread.
///
/// Params：
/// - window: &tauri::WebviewWindow —— 主窗口 / The main window
pub fn shrink_to_work_area(window: &tauri::WebviewWindow) {
	let Some(monitor) = window.current_monitor().ok().flatten() else {
		return;
	};
	let (Ok(inner), Ok(outer)) = (window.inner_size(), window.outer_size()) else {
		return;
	};
	let scale = monitor.scale_factor();
	let work_area = monitor.work_area();

	// inner 不含标题栏与边框：先从工作区扣除这部分物理厚度再换算逻辑
	// 像素，下限 0 防止异常工作区得出负值
	// The inner size excludes the title bar and borders: subtract that
	// physical thickness from the work area before converting to logical
	// pixels, floored at 0 for abnormal work areas
	let frame_w = f64::from(outer.width.saturating_sub(inner.width));
	let frame_h = f64::from(outer.height.saturating_sub(inner.height));
	let fit_w = ((f64::from(work_area.size.width) - frame_w) / scale).max(0.0);
	let fit_h = ((f64::from(work_area.size.height) - frame_h) / scale).max(0.0);

	let cur_w = f64::from(inner.width) / scale;
	let cur_h = f64::from(inner.height) / scale;
	let clamp_w = cur_w > fit_w;
	let clamp_h = cur_h > fit_h;
	if !clamp_w && !clamp_h {
		return;
	}

	let _ = window.set_size(tauri::LogicalSize::new(cur_w.min(fit_w), cur_h.min(fit_h)));
	// 被收缩的轴贴到工作区起始边，修正居中定位留下的越界偏移；
	// 未超出的轴保持原位置
	// Clamped axes pin to the work area's leading edge to undo the overflow
	// left by centered placement; axes that fit keep their position
	if let Ok(pos) = window.outer_position() {
		let x = if clamp_w { work_area.position.x } else { pos.x };
		let y = if clamp_h { work_area.position.y } else { pos.y };
		let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
	}
}
