//! Locale 文件管理器 / Locale File Manager
//!
//! 负责在程序首次运行时创建语言文件夹结构并写入默认语言 JSON 文件，
//! 以及在运行时读取 locale JSON 以支持覆盖内置翻译。
//!
//! Responsible for creating the locale folder structure on first run and writing
//! default language JSON files, as well as reading locale JSON at runtime to
//! support overriding built-in translations.
//!
//! # 目录结构 / Directory Structure
//! ```
//! <exe_dir>/
//! └── locale/
//!     ├── app/
//!     │   ├── zh-CN.json      # 主程序中文翻译
//!     │   └── en-US.json      # 主程序英文翻译
//!     └── modules/
//!         ├── filter_short/
//!         │   ├── zh-CN.json
//!         │   └── en-US.json
//!         ├── contact_sheet/
//!         │   ├── zh-CN.json
//!         │   └── en-US.json
//!         ├── notify_discord/
//!         │   ├── zh-CN.json
//!         │   └── en-US.json
//!         ├── notify_telegram/
//!         │   ├── zh-CN.json
//!         │   └── en-US.json
//!         ├── cleanup/
//!         │   ├── zh-CN.json
//!         │   └── en-US.json
//!         └── __builtin__/
//!             ├── zh-CN.json  # 所有内置节点翻译（recording_input、unpack）
//!             └── en-US.json
//! ```
//!
//! 子模块划分 / Submodule layout:
//! - `reader`：locale 读取与合并（内嵌默认兜底）/ locale reading and merging (embedded fallback)
//! - `validate`：locale 文件校验与重建 / locale-file validation and rebuilding
//! - `log_rt`：后端日志翻译运行时 / backend log-translation runtime

mod log_rt;
mod reader;
mod validate;

use crate::config::app_state::exe_dir;
use std::path::PathBuf;

// 保持 `locale::manager::*` 的公开路径不变 / Keep the `locale::manager::*` public paths unchanged
pub use log_rt::{load_log_translations, tl_log};
pub use reader::{
	LocaleEntry, get_full_locale, list_available_locales, read_app_locale, read_log_locale,
	read_module_locale,
};
pub use validate::{check_custom_locale_files, validate_locale_file};

use validate::{
	load_builtin_state, save_builtin_state, validate_app_locale, validate_builtin_locale,
	validate_log_locale, validate_module_locale, write_or_rebuild_if_invalid,
};

/// 返回 locale 根目录路径（`<exe_dir>/locale`）。
/// Returns the locale root directory path (`<exe_dir>/locale`).
pub fn locale_dir() -> PathBuf {
	exe_dir().join("locale")
}

/// 返回主程序 locale 目录（`<exe_dir>/locale/app`）。
/// Returns the app locale directory (`<exe_dir>/locale/app`).
pub fn app_locale_dir() -> PathBuf {
	locale_dir().join("app")
}

/// 返回模块 locale 根目录（`<exe_dir>/locale/modules`）。
/// Returns the modules locale root directory (`<exe_dir>/locale/modules`).
pub fn modules_locale_dir() -> PathBuf {
	locale_dir().join("modules")
}

/// 返回指定模块的 locale 目录（`<exe_dir>/locale/modules/<module_id>`）。
/// Returns the locale directory for a specific module.
pub fn module_locale_dir(module_id: &str) -> PathBuf {
	modules_locale_dir().join(module_id)
}

/// 返回后端日志翻译目录（`<exe_dir>/locale/log`）。
/// Returns the backend log locale directory (`<exe_dir>/locale/log`).
pub fn log_locale_dir() -> PathBuf {
	locale_dir().join("log")
}

/// 默认的主程序中文翻译 JSON。
/// Default app Chinese (zh-CN) translation JSON.
pub(super) const APP_ZH_CN: &str = include_str!("defaults/app/zh-CN.json");

/// 默认的主程序英文翻译 JSON。
/// Default app English (en-US) translation JSON.
pub(super) const APP_EN_US: &str = include_str!("defaults/app/en-US.json");

/// 默认的后端日志中文翻译 JSON。
/// Default backend log Chinese (zh-CN) translation JSON.
pub(super) const LOG_ZH_CN: &str = include_str!("defaults/log/zh-CN.json");

/// 默认的后端日志英文翻译 JSON。
/// Default backend log English (en-US) translation JSON.
pub(super) const LOG_EN_US: &str = include_str!("defaults/log/en-US.json");

/// 内置模块的默认 locale 数据（模块 ID, 语言代码, JSON 内容）。
/// 包含外部可执行模块和内置节点（`__builtin__*`）。
/// Default locale data for built-in modules (module_id, locale_code, json_content).
/// Covers both external executable modules and built-in nodes (`__builtin__*`).
pub(super) const MODULE_DEFAULTS: &[(&str, &str, &str)] = &[
	(
		"filter_short",
		"zh-CN",
		include_str!("defaults/modules/filter_short/zh-CN.json"),
	),
	(
		"filter_short",
		"en-US",
		include_str!("defaults/modules/filter_short/en-US.json"),
	),
	(
		"cleanup",
		"zh-CN",
		include_str!("defaults/modules/cleanup/zh-CN.json"),
	),
	(
		"cleanup",
		"en-US",
		include_str!("defaults/modules/cleanup/en-US.json"),
	),
	(
		"contact_sheet",
		"zh-CN",
		include_str!("defaults/modules/contact_sheet/zh-CN.json"),
	),
	(
		"contact_sheet",
		"en-US",
		include_str!("defaults/modules/contact_sheet/en-US.json"),
	),
	(
		"notify_discord",
		"zh-CN",
		include_str!("defaults/modules/notify_discord/zh-CN.json"),
	),
	(
		"notify_discord",
		"en-US",
		include_str!("defaults/modules/notify_discord/en-US.json"),
	),
	(
		"notify_telegram",
		"zh-CN",
		include_str!("defaults/modules/notify_telegram/zh-CN.json"),
	),
	(
		"notify_telegram",
		"en-US",
		include_str!("defaults/modules/notify_telegram/en-US.json"),
	),
	// 内置节点（两个节点的翻译合并在单一文件中）
	// Built-in nodes (both nodes' translations merged into a single file per locale)
	(
		"__builtin__",
		"zh-CN",
		include_str!("defaults/modules/__builtin__/zh-CN.json"),
	),
	(
		"__builtin__",
		"en-US",
		include_str!("defaults/modules/__builtin__/en-US.json"),
	),
];

/// 初始化 locale 目录：若文件不存在则创建，若内置文件校验失败则重建。
/// 此函数在程序启动时调用一次（emitter 就绪前）。
/// 用户自定义语言文件的校验警告通过 `emit_locale_warnings` 在 emitter 就绪后发送。
///
/// 内置文件还带「随版本刷新」语义：以 `<locale>/.builtin-state.json` 记录我们
/// 上次写出的内容指纹——指纹一致说明文件未被用户动过，内容落后于新版本的
/// 内嵌默认时安全重写；指纹不一致说明用户自定义过，标记 "custom" 永久保留。
/// 无记录的旧文件（旧版本安装首次运行此机制）与默认一致则认领，不一致则
/// 备份 `.bak` 后刷新（无法区分旧默认与自定义时按升级语义处理）。
///
/// Initialize locale directories: create files if missing, rebuild built-in files
/// if validation fails. Called once at startup before the emitter is ready.
/// Custom locale file validation warnings are sent later via `emit_locale_warnings`.
///
/// Built-in files additionally carry refresh-on-version-bump semantics:
/// `<locale>/.builtin-state.json` records fingerprints of what we last wrote —
/// a match means the file is untouched and may be safely rewritten when it lags
/// the new embedded default; a mismatch marks it "custom" and keeps it forever.
/// Unrecorded legacy files (first boot on a pre-mechanism install) are claimed
/// when identical to the default, otherwise backed up to `.bak` and refreshed.
pub fn init_locale_dirs() {
	// 创建目录结构 / Create directory structure
	let app_dir = app_locale_dir();
	let modules_dir = modules_locale_dir();
	let log_dir = log_locale_dir();

	for dir in [&app_dir, &modules_dir, &log_dir] {
		if let Err(e) = std::fs::create_dir_all(dir) {
			tracing::warn!(
				"{}",
				crate::tl!("locale.createDirFailed", dir = dir.display(), error = e)
			);
		}
	}

	// 内置文件托管状态：跨所有文件共享，循环结束后统一写盘
	// Managed state for built-in files: shared across all files, persisted once below
	let mut state = load_builtin_state();

	// 主程序内置语言文件：不存在则创建，存在但校验失败则重建，有效则按指纹随版本刷新
	// Built-in app locale files: create if missing, rebuild if invalid,
	// refresh on version drift when still ours
	for (locale_code, default_content) in [("zh-CN", APP_ZH_CN), ("en-US", APP_EN_US)] {
		let path = app_dir.join(format!("{}.json", locale_code));
		write_or_rebuild_if_invalid(
			&path,
			default_content,
			validate_app_locale,
			locale_code,
			&format!("app/{locale_code}"),
			&mut state,
		);
	}

	// 后端日志翻译文件：不存在则创建，存在但 JSON 解析失败则重建，有效则按指纹随版本刷新
	// Backend log locale files: create if missing, rebuild if JSON parse fails,
	// refresh on version drift when still ours
	for (locale_code, default_content) in [("zh-CN", LOG_ZH_CN), ("en-US", LOG_EN_US)] {
		let path = log_dir.join(format!("{}.json", locale_code));
		write_or_rebuild_if_invalid(
			&path,
			default_content,
			validate_log_locale,
			&format!("log/{}", locale_code),
			&format!("log/{locale_code}"),
			&mut state,
		);
	}

	// 模块内置语言文件：不存在则创建，存在但校验失败则重建
	// `__builtin__` 是按节点分组的嵌套结构，需要用专门的校验函数（见
	// validate_builtin_locale 的文档），其余常规模块用扁平结构的校验函数。
	//
	// Built-in module locale files: create if missing, rebuild if validation fails.
	// `__builtin__` has a nested per-node structure and needs its own validator (see
	// validate_builtin_locale's docs); other regular modules use the flat-structure validator.
	for (module_id, locale_code, content) in MODULE_DEFAULTS {
		let dir = module_locale_dir(module_id);
		if let Err(e) = std::fs::create_dir_all(&dir) {
			tracing::warn!(
				"{}",
				crate::tl!("locale.createDirFailed", dir = dir.display(), error = e)
			);
			continue;
		}
		let file_path = dir.join(format!("{}.json", locale_code));
		let validator: fn(&serde_json::Value, &str) -> Result<(), String> =
			if *module_id == "__builtin__" {
				validate_builtin_locale
			} else {
				validate_module_locale
			};
		write_or_rebuild_if_invalid(
			&file_path,
			content,
			validator,
			&format!("{}/{}", module_id, locale_code),
			&format!("modules/{module_id}/{locale_code}"),
			&mut state,
		);
	}

	// 统一写回托管状态 / Persist the managed state once
	save_builtin_state(&state);

	tracing::info!(
		"{}",
		crate::tl!("locale.dirsInitialized", dir = locale_dir().display())
	);
}
