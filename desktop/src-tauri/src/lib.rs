//! Tauri 桌面应用库入口 / Tauri Desktop Application Library Entry
//!
//! 初始化所有后端组件（AppState、RecorderManager、StatusMonitor），
//! 注册 Tauri commands，启动后台任务（状态监控、Mouflon 同步、文件监控等）。
//!
//! Initializes all backend components (AppState, RecorderManager, StatusMonitor),
//! registers Tauri commands, and starts background tasks
//! (status monitoring, Mouflon sync, file watching, etc.).

mod commands;
mod emitter;
mod state;

use crate::emitter::TauriEmitter;
use crate::state::DesktopState;
use std::sync::Arc;
use stripchat_recorder_lib::{
	config::app_state::AppState,
	core::emitter::Emitter,
	platform::monitor::StatusMonitor,
	recording::recorder::RecorderManager,
	server::{scheduler, startup},
};
use tauri::Manager;

/// Tauri 应用的运行入口，由 `main.rs` 调用。
/// Tauri application run entry point, called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
	// 创建专用 Tokio runtime，供 setup 和所有后台任务使用。
	// Tauri 的 setup() 回调不在 Tokio 上下文里，必须在这里建立 runtime。
	//
	// Create a dedicated Tokio runtime for setup and all background tasks.
	// Tauri's setup() callback does not run in a Tokio context, so we must
	// establish the runtime here.
	let rt = tokio::runtime::Builder::new_multi_thread()
		.worker_threads(4)
		.enable_all()
		.build()
		.expect("Failed to create Tokio runtime");

	let rt = Arc::new(rt);
	let rt_for_setup = Arc::clone(&rt);

	tauri::Builder::default()
		.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
			// 已有实例在运行时，把主窗口拉到前台并聚焦
			// When another instance is launched, bring the existing window to front
			if let Some(window) = app.get_webview_window("main") {
				let _ = window.unminimize();
				let _ = window.show();
				let _ = window.set_focus();
			}
		}))
		.plugin(tauri_plugin_opener::init())
		.plugin(tauri_plugin_updater::Builder::new().build())
		.plugin(tauri_plugin_process::init())
		.setup(move |app| {
			let app_handle = app.handle().clone();

			// 快速同步初始化，完成后立即显示窗口。
			// Fast synchronous initialization, then show the window immediately.
			rt_for_setup.block_on(async move {
				setup_app(app_handle).await;
			});

			Ok(())
		})
		.invoke_handler(tauri::generate_handler![
			// Streamers
			commands::list_streamers,
			commands::add_streamer,
			commands::remove_streamer,
			commands::set_auto_record,
			commands::start_recording,
			commands::stop_recording,
			commands::verify_streamer,
			// Settings
			commands::get_settings,
			commands::save_settings_cmd,
			commands::get_disk_space,
			commands::get_system_info,
			// Mouflon Keys
			commands::list_mouflon_keys,
			commands::add_mouflon_key,
			commands::remove_mouflon_key,
			commands::sync_mouflon_keys,
			// Recordings
			commands::list_recordings,
			commands::get_merging_dirs,
			commands::delete_recording,
			commands::open_recording,
			commands::open_output_dir,
			commands::open_merged_dir,
			commands::open_output_file,
			commands::get_module_outputs,
			// Post-processing
			commands::run_postprocess_cmd,
			commands::run_postprocess_batch,
			commands::cancel_postprocess,
			commands::get_postprocess_tasks,
			// Pipeline
			commands::get_pipeline,
			commands::save_pipeline,
			commands::list_modules,
			// Community Modules
			commands::get_install_tasks,
			commands::install_community_module,
			commands::uninstall_community_module,
			// Locale
			commands::get_locale,
			commands::list_locales,
			// Notifications
			commands::get_notifications,
			commands::mark_notifications_read,
			// Startup warnings
			commands::get_startup_warnings,
			commands::remove_missing_pp_results,
			// File system
			commands::list_dir,
			commands::list_drives,
			commands::create_dir,
			// Update
			commands::check_for_updates_cmd,
			commands::apply_update_cmd,
			// SC 官方分类浏览列表（StripChat 页）/ Official SC category list (StripChat page)
			commands::list_sc_models,
			// 转发流：系统播放器打开 / Relay stream: open in the system player
			commands::open_relay_stream,
			// FFmpeg
			commands::get_ffmpeg_status,
			commands::get_ffmpeg_locations,
			commands::download_ffmpeg,
			// 资源占用 / Resource usage
			commands::process_usage,
			commands::service_status,
		])
		.run(tauri::generate_context!())
		.expect("error while running tauri application");

	drop(rt);
}

/// 在 Tokio runtime 上下文中执行的应用初始化逻辑。
/// Application initialization logic executed within the Tokio runtime context.
async fn setup_app(app_handle: tauri::AppHandle) {
	// 数据根目录定位，必须在下面任何调用 `exe_dir()` 的代码
	// （`AppState::log_dir`、`AppState::new` 等）之前完成。
	// Resolve the data root directory; must run before any code below that
	// calls `exe_dir()` (`AppState::log_dir`, `AppState::new`, etc.).
	#[cfg(feature = "portable")]
	setup_portable_data_root(&app_handle);
	#[cfg(not(feature = "portable"))]
	{
		// 安装版：改用 Tauri 的 app_data_dir()（操作系统标准的每用户数据目录），
		// 替代默认的"可执行文件同目录"约定。Desktop 端以系统安装包（NSIS/MSI、
		// AppImage、deb/rpm、dmg）分发，可执行文件所在目录可能只读（AppImage 的
		// FUSE 挂载点，每次启动路径还会变化）、无写权限（如 `/usr/bin`），或修改
		// 会破坏代码签名（macOS `.app`），详见 `set_exe_dir_override` 文档。
		// 解析失败时静默回退到旧的 exe-relative 行为，不阻断启动。
		//
		// Installed build: use Tauri's app_data_dir() (the OS-standard per-user
		// data directory) instead of the default "next to the executable"
		// convention. See `set_exe_dir_override` docs for why. Falls back
		// silently to the old exe-relative behavior if resolution fails, so
		// startup isn't blocked.
		match app_handle.path().app_data_dir() {
			Ok(dir) => stripchat_recorder_lib::config::app_state::set_exe_dir_override(dir),
			Err(e) => eprintln!(
				"Failed to resolve app data dir, falling back to exe-relative paths: {}",
				e
			),
		}
	}

	// 以下启动顺序与 Server（run_server）一致，详见 server::startup 模块文档
	// The startup order below matches Server (run_server); see the server::startup module docs

	// 在 AppState::new 之前初始化日志、locale 目录与日志翻译，让之后所有日志都有译文
	// Initialize logging, locale dirs and log translations before AppState::new so every later
	// log line is translated
	startup::init_logging_and_locale();

	// 初始化应用状态 / Initialize application state
	let app_state = AppState::new().expect("Failed to initialize app state");

	// 创建 TauriEmitter / Create TauriEmitter
	let emitter: Arc<dyn Emitter> = Arc::new(TauriEmitter::new(app_handle.clone()));

	// 创建录制管理器 / Create recorder manager
	let recorder = RecorderManager::new(Arc::clone(&app_state));

	// 创建状态监控器 / Create status monitor
	let monitor = StatusMonitor::new(Arc::clone(&app_state), Arc::clone(&recorder));

	// 尽早注册 DesktopState：只持有 Arc，没有初始化副作用，前端一加载就能调用命令
	// Register DesktopState as early as possible: it only holds Arcs with no init side effects,
	// so frontend commands work as soon as the webview loads
	app_handle.manage(DesktopState {
		app_state: Arc::clone(&app_state),
		recorder: Arc::clone(&recorder),
		monitor: Arc::clone(&monitor),
		emitter: Arc::clone(&emitter),
		pending_update: parking_lot::RwLock::new(None),
		relay_manager: stripchat_recorder_lib::relay::state::RelayManager::new(),
		relay_port: tokio::sync::OnceCell::new(),
	});

	// 启动时一次性任务（与 Server 共用）：清空 tmp → meta 迁移 → ffmpeg 检查
	// （缺失时后台自动下载到 lib）→ 自定义语言文件校验 → 文件系统监控
	// One-shot startup tasks (shared with Server): clear tmp → meta migrations → ffmpeg check
	// (auto-downloads into lib in the background when missing) → custom locale file
	// validation → file system watchers
	startup::run_all(Arc::clone(&app_state), Arc::clone(&emitter));

	// 启动全部后台定时任务（与 Server 完全相同，各任务首轮均启动即执行）
	// Start every background scheduled task (identical to Server — every
	// task's first pass runs right at launch)
	scheduler::start_all(
		Arc::clone(&app_state),
		Arc::clone(&monitor),
		Arc::clone(&emitter),
		Arc::clone(&recorder),
	);

	// ── 阶段一结束：显示主窗口 / Phase 1 complete: show the main window ────────
	if let Some(window) = app_handle.get_webview_window("main") {
		// 期望的最小逻辑尺寸取自 tauri.conf.json 里 main 窗口的
		// minWidth/minHeight（缺省回退 1600×720）；
		// config() 是内存读取，不经过事件循环，任何线程都安全。
		// The desired minimum logical size comes from the main window's
		// minWidth/minHeight in tauri.conf.json (falling back to
		// 1600×720); config() is an in-memory read with no event-loop
		// round trip, so it is safe on any thread.
		let desired_min = app_handle
			.config()
			.app
			.windows
			.iter()
			.find(|w| w.label == "main")
			.map(|w| (w.min_width.unwrap_or(1600.0), w.min_height.unwrap_or(720.0)))
			.unwrap_or((1600.0, 720.0));

		// 运行中的 DPI 变化（修改系统缩放、把窗口拖到另一台显示器）会使
		// 工作区与窗口的实际尺寸再次失配，此时按新工作区重新收紧。
		// 事件回调在事件循环线程上同步执行，而 fit 内部 inner_size 等
		// getter 需要事件循环线程回包，回调里直接调用会死锁，因此转到
		// 独立线程执行。
		//
		// 但不能在收到 ScaleFactorChanged 就立刻适配：鼠标拖拽时窗口
		// 矩形由 WM_MOVING 模态循环接管，期间外部 set_size 会被下一次
		// 拖动事件整体回滚（实测拖拽中收缩被立即撤销，松手后窗口保持
		// 越界）。改为几何事件去抖驱动——Resized/ScaleFactorChanged 喂
		// 给专用 worker，事件静默 ~120ms 且左键已松开（拖拽结束）才执
		// 行适配；拖拽中的任何回滚都会再产生事件重新喂饱去抖，松手后
		// 的最后一次适配必然生效。
		// A runtime DPI change (system scaling changed, window dragged to
		// another monitor) re-breaks the fit between work area and window
		// size, so the fit is re-applied to the new work area. The event
		// callback runs on the event-loop thread while fit's getters
		// (inner_size, ...) round-trip through that same loop — calling
		// them inline would deadlock, so the work goes to a separate
		// thread.
		//
		// The fit must NOT run right when ScaleFactorChanged arrives:
		// during a mouse drag the window rect is owned by the WM_MOVING
		// modal loop, which reverts any external set_size on the next
		// move event (observed: a mid-drag shrink was undone instantly,
		// leaving the window overflowing after the drop). Instead the
		// fit is debounce-driven — Resized/ScaleFactorChanged feed a
		// dedicated worker that runs the fit once events have been quiet
		// for ~120ms and the left button is up (drag finished). Any
		// mid-drag revert produces more events that re-arm the debounce,
		// so the post-drop fit always lands and sticks.
		// 收缩/恢复的跨调用尺寸记忆：worker 与首次适配共用一份，
		// 显示器往返才能恢复出同一目标尺寸
		// Shared cross-call size memory for shrink/restore — the worker
		// and the first fit share one instance so monitor round-trips
		// restore to the same remembered size
		let fit_memory = std::sync::Arc::new(parking_lot::Mutex::new(FitMemory::default()));
		let (fit_tx, fit_rx) = std::sync::mpsc::channel::<()>();
		let window_for_fit = window.clone();
		let memory_for_worker = fit_memory.clone();
		std::thread::spawn(move || {
			use std::sync::mpsc::RecvTimeoutError;
			while fit_rx.recv().is_ok() {
				// 静默 ~120ms 且无拖拽进行（左键松开）后适配一次；
				// 按住左键说明拖拽模态循环仍在接管窗口矩形，继续等待
				// 上限 ~10s 兜底异常挂住的场景。
				// Fit once events are quiet for ~120ms and no drag is in
				// progress (left button released); while the button is
				// held the drag's modal loop still owns the window rect,
				// so keep waiting — capped at ~10s for stuck-input
				// edge cases.
				let mut waited_ms = 0u64;
				loop {
					match fit_rx.recv_timeout(std::time::Duration::from_millis(120)) {
						Ok(()) => continue,
						Err(RecvTimeoutError::Timeout) => {
							if drag_in_progress() && waited_ms < 10_000 {
								waited_ms += 120;
								continue;
							}
							break;
						}
						Err(RecvTimeoutError::Disconnected) => return,
					}
				}
				fit_window_to_work_area(&window_for_fit, desired_min, &memory_for_worker);
			}
		});

		window.on_window_event(move |event| {
			if matches!(
				event,
				tauri::WindowEvent::ScaleFactorChanged { .. } | tauri::WindowEvent::Resized(_)
			) {
				let _ = fit_tx.send(());
			}
		});

		// 首次适配同样不能在当前线程做：本函数经 block_on 跑在事件循环
		// 主线程上，getter 回包要等循环空闲才送达，直接调用会永久阻塞；
		// set_min_size / set_size / set_position / show / set_focus 只是
		// 投递消息，任何线程都安全。把 适配→显示→聚焦 整段交给 runtime
		// worker 线程：setup 返回、事件循环开始泵送后其 getter 才解
		// 阻塞，天然保证“先适配、后显示”——窗口创建时是隐藏的，
		// 用户看不到适配前的越界瞬间。
		// The first fit cannot run on the current thread either: this
		// function is driven by block_on on the event-loop main thread,
		// and getter replies only arrive once the loop is free — calling
		// them here would block forever. set_min_size / set_size /
		// set_position / show / set_focus only enqueue messages and are
		// safe on any thread. Hand the whole fit → show → focus sequence
		// to a runtime worker thread: its getters unblock once setup()
		// returns and the loop starts pumping, which naturally orders
		// "fit first, show after" — the window is created hidden, so
		// users never see the pre-fit overflow.
		tokio::task::spawn_blocking(move || {
			fit_window_to_work_area(&window, desired_min, &fit_memory);
			let _ = window.show();
			let _ = window.set_focus();
		});
	}
}

/// 把主窗口收紧到所在显示器的可用工作区内。
///
/// `tauri.conf.json` 中的 `width`/`height`/`minWidth`/`minHeight` 都是逻辑
/// 像素；Windows 开启系统缩放（如 150%）后同样的逻辑尺寸映射成更多
/// 物理像素——1920×1080 屏幕在 150% 下工作区仅约 1280×693 逻辑像素，
/// 1600 逻辑像素宽的最小尺寸（2400 物理像素）必然把窗口推出屏幕外。
/// 本函数把最小尺寸收紧到 `min(期望值, 工作区可容纳)`，并把超出工作区
/// 的窗口收缩后贴回工作区边缘，保证任何缩放比例下窗口都不越出屏幕；
/// 收缩前的尺寸记入 `memory`，回到容得下的显示器时自动恢复（用户
/// 手动改过的尺寸不会被回扭）。显示器解析失败时静默返回（沿用配置
/// 文件的 minWidth/minHeight），不阻断启动。
///
/// 注意：只能在非事件循环线程上调用——内部的 `inner_size` /
/// `outer_size` / `current_monitor` 等 getter 需要事件循环线程回包，
/// 在事件循环线程（setup 回调、window 事件回调）上调用会死锁。
///
/// Fit the main window inside its monitor's available work area.
///
/// `width`/`height`/`minWidth`/`minHeight` in `tauri.conf.json` are logical
/// pixels; with Windows display scaling (e.g. 150%) the same logical size
/// maps to more physical pixels — a 1920×1080 screen at 150% only has a
/// ~1280×693 logical work area, so a 1600-logical-pixel minimum width
/// (2400 physical pixels) inevitably pushes the window off screen. This
/// function caps the minimum at `min(desired, work-area-fittable)` and
/// shrinks an overflowing window back onto the work-area edge, keeping
/// the window on-screen at any scale factor. The pre-shrink size is kept
/// in `memory` and restored automatically when a monitor can hold it
/// again (a user-chosen size is never wrestled back). It returns silently
/// (leaving the config-file minWidth/minHeight in effect) when the
/// monitor cannot be resolved, so startup is never blocked.
///
/// Note: must only be called off the event-loop thread — the blocking
/// getters inside (`inner_size`, `outer_size`, `current_monitor`, ...)
/// require the event-loop thread to answer, so calling this on it
/// (setup callback, window event callback) deadlocks.
///
/// @param window 目标主窗口 / The main window to fit.
/// @param desired_min 期望的最小逻辑尺寸 (宽, 高)，通常来自
///   `tauri.conf.json` / The desired minimum logical size (width,
///   height), usually from `tauri.conf.json`.
/// @param memory 跨调用的尺寸记忆（收缩前的应有尺寸与收缩落地值）；
///   多显示器往返恢复依赖它 / Cross-call size memory (the intended
///   pre-shrink size and the landed shrink size) — needed to restore
///   when the window returns to a monitor that can hold it again.
fn fit_window_to_work_area(
	window: &tauri::WebviewWindow,
	desired_min: (f64, f64),
	memory: &parking_lot::Mutex<FitMemory>,
) {
	// 找不到窗口所在显示器时不做调整，配置文件的最小尺寸兜底
	// Keep the config-file minimum when no monitor resolves
	let Some(monitor) = window.current_monitor().ok().flatten() else {
		return;
	};
	let scale = monitor.scale_factor();
	let work_area = monitor.work_area();

	// inner 尺寸不含标题栏与边框：工作区换算逻辑像素前先把这部分
	// 物理厚度扣除，否则 inner 填满工作区时 outer 仍会略微越界
	// （例如盖住任务栏）。
	// The inner size excludes the title bar and borders: subtract that
	// physical thickness before converting the work area, otherwise the
	// outer frame would still slightly overflow (e.g. covering the
	// taskbar) when the inner area fills the work area.
	let inner = window.inner_size().ok();
	let frame = match (inner, window.outer_size()) {
		(Some(inner), Ok(outer)) => (
			f64::from(outer.width.saturating_sub(inner.width)),
			f64::from(outer.height.saturating_sub(inner.height)),
		),
		_ => (0.0, 0.0),
	};

	// 工作区扣除边框厚度后换算成逻辑像素，得到 inner 可容纳的最大尺寸
	// Subtract the frame thickness and convert to logical pixels — the
	// maximum inner size the work area can hold
	let fit_w = (f64::from(work_area.size.width) - frame.0) / scale;
	let fit_h = (f64::from(work_area.size.height) - frame.1) / scale;

	// 最小尺寸取期望与可容纳的较小者：缩放使工作区小于期望时以可容纳
	// 为准，否则最小尺寸本身就把窗口推出屏幕
	// Minimum = min(desired, fittable): when scaling shrinks the work
	// area below the desired size, the fittable size wins — otherwise
	// the minimum alone would push the window off screen
	let min_w = desired_min.0.min(fit_w).max(0.0);
	let min_h = desired_min.1.min(fit_h).max(0.0);
	let _ = window.set_min_size(Some(tauri::LogicalSize::new(min_w, min_h)));

	// 尺寸记忆决定本轮动作：
	// - 当前尺寸越界 → 收缩；收缩前的尺寸若不是上一次收缩的产物，
	//   记为"应有尺寸"（用户自持意图）
	// - 当前尺寸仍是收缩产物（≈上次收缩落地值）→ 说明小窗口是适配
	//   小显示器的权宜结果；新显示器容得下记忆尺寸时恢复
	// - 其余情况（用户手动改过的尺寸）→ 直接采纳为新的应有尺寸，
	//   绝不回扭用户的选择
	// The size memory decides this round's action:
	// - Current size overflows → shrink; if the pre-shrink size isn't
	//   the last landed shrink value, adopt it as the "intended" size
	// - Current size is still our shrink artifact (≈ last landed
	//   shrink) → the small size is just the small-monitor compromise;
	//   restore the remembered size when this monitor can hold it
	// - Anything else (a user-chosen size) → adopt it as the new
	//   intended size; never wrestle the user's choice back
	let Some(inner) = inner else { return };
	let cur_w = f64::from(inner.width) / scale;
	let cur_h = f64::from(inner.height) / scale;
	let cur = (cur_w, cur_h);
	// 逻辑↔物理换算有 ±几 px 取整漂移，比较尺寸需带容差
	// Logical↔physical conversion drifts by a few px — compare sizes
	// with tolerance
	const TOL: f64 = 4.0;
	let approx = |b: Option<(f64, f64)>| {
		b.is_some_and(|(w, h)| (cur_w - w).abs() <= TOL && (cur_h - h).abs() <= TOL)
	};

	let mut st = memory.lock();
	if cur_w > fit_w || cur_h > fit_h {
		if !approx(st.clamped) {
			st.unclamped = Some(cur);
		}
		let target = (cur_w.min(fit_w).max(0.0), cur_h.min(fit_h).max(0.0));
		st.clamped = Some(target);
		drop(st); // 先放锁再走消息往返 / release the lock before the message round-trips
		let _ = window.set_size(tauri::LogicalSize::new(target.0, target.1));
		if let Ok(pos) = window.outer_position() {
			let x = if cur_w > fit_w { work_area.position.x } else { pos.x };
			let y = if cur_h > fit_h { work_area.position.y } else { pos.y };
			let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
		}
	} else if approx(st.clamped) {
		// 恢复目标夹在 [min, fit]；只在两轴都不缩的前提下放大
		// Restore target clamped to [min, fit]; grow only when neither
		// axis shrinks
		let Some(mem) = st.unclamped else { return };
		let target = (mem.0.clamp(min_w, fit_w), mem.1.clamp(min_h, fit_h));
		let grows = target.0 >= cur_w - TOL
			&& target.1 >= cur_h - TOL
			&& (target.0 > cur_w + TOL || target.1 > cur_h + TOL);
		if !grows {
			return;
		}
		// 恢复落地值同样标记为"我们的产物"：若恢复的是被 fit 截断的
		// 中间尺寸（记忆尺寸仍放不下），不能让它在下一次 fit 里被
		// 采纳为新的应有尺寸——否则链式跨屏会丢更大的原始记忆
		// The landed restore is also our artifact: when it is a
		// fit-truncated mid size (the remembered size still doesn't
		// fit), it must not be adopted as the new intended size on the
		// next fit — chained monitor hops would lose the larger memory
		st.clamped = Some(target);
		drop(st);
		let _ = window.set_size(tauri::LogicalSize::new(target.0, target.1));
		// 放大后的 outer 若越过工作区右/下边缘则回拉，保持整体在屏内
		// Pull the enlarged outer rect back inside when it would spill
		// past the work area's right/bottom edge
		if let Ok(pos) = window.outer_position() {
			let outer_w = target.0 * scale + frame.0;
			let outer_h = target.1 * scale + frame.1;
			let pos_x = f64::from(pos.x);
			let pos_y = f64::from(pos.y);
			let right = f64::from(work_area.position.x) + f64::from(work_area.size.width);
			let bottom = f64::from(work_area.position.y) + f64::from(work_area.size.height);
			let x = if pos_x + outer_w > right {
				(right - outer_w).max(f64::from(work_area.position.x))
			} else {
				pos_x
			};
			let y = if pos_y + outer_h > bottom {
				(bottom - outer_h).max(f64::from(work_area.position.y))
			} else {
				pos_y
			};
			if (x - pos_x).abs() > 0.5 || (y - pos_y).abs() > 0.5 {
				let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
			}
		}
	} else {
		st.unclamped = Some(cur);
		st.clamped = None;
	}
}

/// 工作区适配的跨调用尺寸记忆。
/// Cross-call size memory for the work-area fit.
#[derive(Default)]
struct FitMemory {
	/// 最近一次未被工作区挤压的尺寸（逻辑像素）——回到容得下的显示
	/// 器时恢复的目标 / The last size not squashed by the work area —
	/// the restore target once a monitor can hold it again
	unclamped: Option<(f64, f64)>,
	/// 我们上一次改尺寸落地值（逻辑像素，含收缩与恢复）——当前尺寸
	/// 仍等于它时，该尺寸是我们的产物而非用户选择 / The size our last
	/// correction landed (shrink or restore) — while the current size
	/// still equals it, that size is our artifact, not user intent
	clamped: Option<(f64, f64)>,
}

/// 判断是否可能有拖拽/缩放的模态循环正在接管窗口矩形。
///
/// Windows 上鼠标按下标题栏/边框后，系统在 WM_ENTERSIZEMOVE 与
/// WM_EXITSIZEMOVE 之间运行嵌套模态循环并跟踪拖动矩形，期间外部
/// `set_size`/`set_position` 会被下一次 WM_MOVING 回滚。以左键按住
/// 作为启发式判定（鼠标拖动必按住左键）；键盘移动（Alt+Space→M）
/// 等无按键场景返回 false——此时适配可直接执行。
/// 非 Windows 平台无此模态循环语义，恒为 false。
///
/// Whether a drag/resize modal loop may currently own the window rect.
///
/// On Windows, grabbing the title bar or a border starts a nested
/// modal loop between WM_ENTERSIZEMOVE/WM_EXITSIZEMOVE that tracks its
/// own drag rectangle and reverts external set_size/set_position on
/// the next WM_MOVING. Holding the left button is used as the
/// heuristic (a mouse drag necessarily holds it); button-free moves
/// (Alt+Space → M) return false so the fit can apply right away.
/// Always false off Windows, where this modal-loop semantics doesn't
/// apply.
///
/// @returns 可能处于拖拽模态循环中时为 true / true when a drag modal
///   loop may be active.
#[cfg(target_os = "windows")]
fn drag_in_progress() -> bool {
	// GetAsyncKeyState 返回值的最高位表示按键当前处于按下状态
	// The high bit of GetAsyncKeyState's return marks the key as down
	unsafe {
		windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
			windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_LBUTTON as i32,
		) < 0
	}
}

/// 非 Windows 平台的拖拽判定占位，恒为 false。
/// Non-Windows placeholder for the drag check — always false.
#[cfg(not(target_os = "windows"))]
fn drag_in_progress() -> bool {
	false
}

/// 便携版的数据根目录定位（`portable` feature，配合 `tauri build --no-bundle` /
/// `--bundles appimage` 产出的分发形态使用）。
///
/// - AppImage：`current_exe()` 指向的是 FUSE 挂载点内的解包路径——只读且每次
///   启动都会变化，必须用 `$APPIMAGE`（AppImage runtime 注入的环境变量，指向
///   `.AppImage` 文件自身）定位其所在目录作为数据根目录。
/// - 其他形态（Windows `--no-bundle` 裸 exe、手动解压运行的二进制）：不设覆盖，
///   `exe_dir()` 默认值已经是可执行文件所在目录。
///
/// 确定数据根目录后做一次"安装版 → 便携版"的用户数据迁移：便携目录下还没有
/// `config/`、`locale/`、`modules/` 而旧的每用户数据目录里有，就复制过来——
/// 否则从安装版/旧 AppImage 换到便携版的用户会以为自己丢了全部配置。
/// 各目录仅当目标不存在时复制（已存在的便携配置优先），失败只打 stderr 不阻断启动；
/// `tools/` 不迁移（FFmpeg 可自动重新下载），`recordings/` 等输出目录由用户配置
/// 指定，与数据根目录解耦，同样不涉及。
///
/// Data-root resolution for portable builds (the `portable` feature, used by the
/// `--no-bundle` / `--bundles appimage` distribution artifacts).
///
/// - AppImage: `current_exe()` points inside the FUSE mount — read-only and a
///   different path on every launch, so the data root is resolved from
///   `$APPIMAGE` (injected by the AppImage runtime; the `.AppImage` file itself).
/// - Other shapes (Windows `--no-bundle` exe, a manually extracted binary): no
///   override needed — the default `exe_dir()` is already the exe's directory.
///
/// Once the root is known, a one-shot "installed → portable" migration runs: any
/// of `config/`, `locale/`, `modules/` that is missing under the portable root
/// but present in the old per-user data dir gets copied over — otherwise users
/// switching from an installed build / older AppImage would appear to lose all
/// settings. Each directory is only copied when the target does not exist
/// (existing portable data wins); failures log to stderr without blocking
/// startup. `tools/` is not migrated (FFmpeg re-downloads automatically), and
/// `recordings/` etc. are user-configured output paths independent of the data
/// root, so they are unaffected.
#[cfg(feature = "portable")]
fn setup_portable_data_root(app_handle: &tauri::AppHandle) {
	use std::path::PathBuf;

	// AppImage 运行时注入 $APPIMAGE；普通裸二进制走 exe_dir() 默认路径。
	// $APPIMAGE is injected by the AppImage runtime; plain binaries fall back
	// to the default exe_dir() resolution.
	let portable_root: PathBuf = std::env::var_os("APPIMAGE")
		.map(PathBuf::from)
		.and_then(|p| p.parent().map(|d| d.to_path_buf()))
		.unwrap_or_else(stripchat_recorder_lib::config::app_state::exe_dir);
	if portable_root != stripchat_recorder_lib::config::app_state::exe_dir() {
		stripchat_recorder_lib::config::app_state::set_exe_dir_override(portable_root.clone());
	}

	// 安装版遗留数据的迁移源 / Migration source: the old per-user data dir
	let Ok(old_root) = app_handle.path().app_data_dir() else {
		return;
	};
	for name in ["config", "locale", "modules"] {
		let dst = portable_root.join(name);
		let src = old_root.join(name);
		if !dst.exists()
			&& src.is_dir()
			&& let Err(e) = copy_dir_recursive(&src, &dst)
		{
			eprintln!(
				"Failed to migrate {name} to portable dir ({} -> {}): {e}",
				src.display(),
				dst.display()
			);
		}
	}
}

/// 递归复制目录（便携版迁移辅助）：目标目录按需创建，逐文件复制，保留层级。
/// Recursive directory copy (portable migration helper): creates the target as
/// needed and copies files one by one, preserving the hierarchy.
#[cfg(feature = "portable")]
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
	std::fs::create_dir_all(dst)?;
	for entry in std::fs::read_dir(src)? {
		let entry = entry?;
		let ty = entry.file_type()?;
		let (s, d) = (entry.path(), dst.join(entry.file_name()));
		if ty.is_dir() {
			copy_dir_recursive(&s, &d)?;
		} else if ty.is_file() {
			std::fs::copy(&s, &d)?;
		}
	}
	Ok(())
}
