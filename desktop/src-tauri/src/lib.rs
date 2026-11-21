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
		let _ = window.show();
		let _ = window.set_focus();
	}
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
