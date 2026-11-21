//! Locale 文件校验子模块：内置/自定义语言文件的格式校验与重建。
//! Locale-file validation submodule: format checks and rebuilding of
//! built-in and custom locale files.

use super::{APP_EN_US, APP_ZH_CN, app_locale_dir, locale_dir, modules_locale_dir};
use crate::config::app_state::strip_utf8_bom;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 校验后端日志翻译文件：必须是 JSON object，且包含至少一个子 object（对应一个日志模块分组）。
/// 只验证顶层结构，不做深层 key 检查——用户可以按需定制各日志分组的翻译。
///
/// Validate a backend log locale file: must be a JSON object with at least one sub-object
/// (corresponding to a log module group). Only checks top-level structure.
pub(super) fn validate_log_locale(
	value: &serde_json::Value,
	_default_content: &str,
) -> Result<(), String> {
	let obj = value
		.as_object()
		.ok_or_else(|| "not a JSON object".to_string())?;
	if obj.is_empty() {
		return Err("must contain at least one log group entry".to_string());
	}
	Ok(())
}

/// 校验主程序语言文件：
/// 必须是 JSON object，包含 `languageName`（字符串），
/// 且包含与对应默认文件相同的全部顶层 key。
///
/// Validate an app locale file:
/// Must be a JSON object, contain `languageName` (string),
/// and contain all top-level keys present in the corresponding default file.
pub(super) fn validate_app_locale(
	value: &serde_json::Value,
	default_content: &str,
) -> Result<(), String> {
	let obj = value
		.as_object()
		.ok_or_else(|| "not a JSON object".to_string())?;

	// 必须有 languageName 字符串 / Must have languageName string
	match obj.get("languageName") {
		Some(serde_json::Value::String(s)) if !s.is_empty() => {}
		Some(_) => return Err("languageName must be a non-empty string".to_string()),
		None => return Err("missing required key: languageName".to_string()),
	}

	// 必须包含默认文件中的所有顶层 key / Must contain all top-level keys from the default
	let default_val: serde_json::Value = serde_json::from_str(default_content)
		.map_err(|e| format!("failed to parse default: {}", e))?;
	let default_obj = default_val
		.as_object()
		.ok_or_else(|| "default is not a JSON object".to_string())?;

	let missing: Vec<&str> = default_obj
		.keys()
		.filter(|k| !obj.contains_key(k.as_str()))
		.map(|k| k.as_str())
		.collect();

	if !missing.is_empty() {
		return Err(format!(
			"missing required top-level keys: {}",
			missing.join(", ")
		));
	}

	Ok(())
}

/// 校验模块语言文件：必须是 JSON object，且包含 `name`、`description`、`params`
/// 三个 key。只校验键的完整性，不检查值的具体内容——翻译文本本身允许用户自定义
/// 替换为任意内容，值的取值范围不属于"格式是否有效"的判断标准。
///
/// 只适用于常规的单节点模块（每个可执行模块对应一个 `ModuleInfo`）。`__builtin__`
/// 对应多个内置节点，顶层结构不同，见 [`validate_builtin_locale`]。
///
/// Validate a module locale file: must be a JSON object containing `name`,
/// `description`, and `params`. Only checks key completeness, not value content —
/// translation text itself is meant to be freely customizable by the user, so the
/// value's content is not part of what determines "format validity".
///
/// Only applies to regular single-node modules (each executable module maps to one
/// `ModuleInfo`). `__builtin__` covers multiple built-in nodes and has a different
/// top-level shape — see [`validate_builtin_locale`].
pub(super) fn validate_module_locale(
	value: &serde_json::Value,
	_default_content: &str,
) -> Result<(), String> {
	let obj = value
		.as_object()
		.ok_or_else(|| "not a JSON object".to_string())?;

	for required in ["name", "description", "params"] {
		if !obj.contains_key(required) {
			return Err(format!("missing required key: {}", required));
		}
	}

	Ok(())
}

/// 校验 `__builtin__` locale 文件：与常规模块 locale 文件（单节点，顶层直接是
/// `name`/`description`/`params`）不同，`__builtin__` 对应多个内置节点（目前是
/// `recording_input` 和 `unpack`，见 `postprocess::builtin_nodes`），顶层结构是
/// "按节点 key 分组"的嵌套对象——每个节点 key 下才是常规的
/// `name`/`description`/`params` 结构。
///
/// 不硬编码要求具体哪些节点 key 必须存在——`builtin_nodes.rs` 的读取逻辑本身就
/// 容忍某个节点 key 缺失（缺失时回退到内嵌的英文默认值），校验只需确保**已存在**
/// 的每个节点条目结构正确即可，这样未来新增内置节点时旧的 locale 文件不会因
/// "缺少新节点的 key"而被误判为无效。
///
/// 只校验键的完整性，不检查值的具体内容，与 [`validate_module_locale`] 保持一致
/// 的校验哲学。
///
/// Validate a `__builtin__` locale file: unlike a regular module locale file (single
/// node, `name`/`description`/`params` directly at the top level), `__builtin__` covers
/// multiple built-in nodes (currently `recording_input` and `unpack`, see
/// `postprocess::builtin_nodes`) — its top level is a nested object grouped by node
/// key, with the usual `name`/`description`/`params` shape one level down, under each
/// node key.
///
/// Does not hardcode which specific node keys must be present — the reading logic in
/// `builtin_nodes.rs` already tolerates a missing node key (falling back to an embedded
/// English default), so validation only needs to ensure that whichever entries **are**
/// present have the correct shape. This way, an older locale file won't be wrongly
/// flagged as invalid just for "missing the key for a newly added built-in node".
///
/// Only checks key completeness, not value content, consistent with
/// [`validate_module_locale`]'s validation philosophy.
pub(super) fn validate_builtin_locale(
	value: &serde_json::Value,
	_default_content: &str,
) -> Result<(), String> {
	let obj = value
		.as_object()
		.ok_or_else(|| "not a JSON object".to_string())?;

	if obj.is_empty() {
		return Err("must contain at least one built-in node entry".to_string());
	}

	for (node_key, node_value) in obj {
		let node_obj = node_value
			.as_object()
			.ok_or_else(|| format!("node \"{}\" must be a JSON object", node_key))?;
		for required in ["name", "description", "params"] {
			if !node_obj.contains_key(required) {
				return Err(format!(
					"node \"{}\" missing required key: {}",
					node_key, required
				));
			}
		}
	}

	Ok(())
}

/// 内置 locale 文件的托管状态：记录我们上次写出的内容指纹（SHA-256），
/// 用来区分「升级后过时的旧默认内容」和「用户自定义修改」——
/// 指纹对得上 → 文件是我们写的，版本更新后可安全重写；
/// 指纹对不上 → 用户改过，标记 "custom" 后永久保留。
/// 持久化在 `<locale>/.builtin-state.json`。
///
/// Managed state for built-in locale files: fingerprints (SHA-256) of the
/// content we last wrote, used to tell "stale default after an upgrade"
/// apart from "user customization" — a matching fingerprint means the
/// file is ours and may be safely rewritten on version bumps; a mismatch
/// marks the file "custom" and it is kept forever.
/// Persisted at `<locale>/.builtin-state.json`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct BuiltinState {
	/// `<相对键> -> "sha256:<hex>" | "custom"` / `<rel key> -> "sha256:<hex>" | "custom"`
	#[serde(default)]
	files: BTreeMap<String, String>,
}

/// 托管状态文件路径（locale 根目录下的点文件，不参与语言文件扫描）。
/// Managed-state file path (a dotfile at the locale root, excluded from
/// locale-file scans).
fn builtin_state_path() -> PathBuf {
	locale_dir().join(".builtin-state.json")
}

/// 内容指纹：SHA-256。特意不用 `DefaultHasher`——其算法不保证跨版本稳定，
/// 指纹漂移会让托管文件被误判成「用户自定义」而永久停止刷新。
///
/// Content fingerprint: SHA-256. `DefaultHasher` is deliberately avoided —
/// its algorithm isn't guaranteed stable across versions, and a drifting
/// fingerprint would misclassify our managed files as "custom" forever.
fn fingerprint(content: &str) -> String {
	let mut s = String::with_capacity(7 + 64);
	s.push_str("sha256:");
	for b in Sha256::digest(content.as_bytes()) {
		let _ = write!(s, "{b:02x}");
	}
	s
}

/// 读取托管状态；文件不存在（新装 / 旧版本首次运行此机制）返回空状态，
/// 损坏时告警并按空状态继续（最坏后果是文件走一遍「无记录」路径）。
///
/// Load the managed state; a missing file (fresh install / first boot of
/// this mechanism on an older install) yields an empty state, a corrupt
/// one logs a warning and does the same (worst case: files take the
/// "no record" claim/refresh path once more).
pub(super) fn load_builtin_state() -> BuiltinState {
	let path = builtin_state_path();
	match std::fs::read_to_string(&path) {
		Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
			tracing::warn!(
				"{}",
				crate::tl!("locale.parseFileFailed", path = path.display(), error = e)
			);
			BuiltinState::default()
		}),
		// 不存在或不可读 → 空状态 / Missing or unreadable → empty state
		Err(_) => BuiltinState::default(),
	}
}

/// 把托管状态写回磁盘（init 末尾调用一次）。
/// Persist the managed state (called once at the end of init).
pub(super) fn save_builtin_state(state: &BuiltinState) {
	let path = builtin_state_path();
	let result = serde_json::to_string_pretty(state)
		.map_err(|e| e.to_string())
		.and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
	if let Err(e) = result {
		tracing::warn!(
			"{}",
			crate::tl!("locale.writeFileFailed", path = path.display(), error = e)
		);
	}
}

/// 覆盖前备份：`xxx.json.bak`；该名字已被占用时改用带时间戳的
/// `xxx.json.bak-YYYYMMDD_HHMMSS`。返回备份文件路径，失败返回 None 并记录 warn。
///
/// Pre-overwrite backup: `xxx.json.bak`, or a timestamped
/// `xxx.json.bak-YYYYMMDD_HHMMSS` when that name is already taken.
/// Returns the backup path; None on failure (a warn is logged).
fn backup_file(path: &Path) -> Option<PathBuf> {
	let file_name = path.file_name()?.to_str()?;
	let mut bak = path.with_file_name(format!("{file_name}.bak"));
	if bak.exists() {
		let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
		bak = path.with_file_name(format!("{file_name}.bak-{stamp}"));
	}
	match std::fs::copy(path, &bak) {
		Ok(_) => Some(bak),
		Err(e) => {
			tracing::warn!(
				"{}",
				crate::tl!("locale.backupFailed", path = path.display(), error = e)
			);
			None
		}
	}
}

/// 写入或重建内置 locale 文件，文件有效时再执行「随版本刷新」判定：
/// - 文件不存在 → 写入默认内容并记录指纹
/// - 读取/解析/校验失败 → 先备份再重建为默认内容，记录 warn
/// - 有效且指纹与记录一致 → 是我们写的：内容与内嵌默认不同（版本漂移）则重写
/// - 有效但指纹与记录不一致 → 用户改过：标记 `"custom"`，永久保留
/// - 有效但无记录（旧版本安装首次运行此机制）→ 与默认一致则认领指纹；
///   不一致则无法区分「旧默认 / 用户自定义」：备份后按版本升级语义刷新为新默认
///
/// Unified write-or-rebuild logic for built-in locale files, plus a
/// version-refresh decision for valid existing files:
/// - Missing → write the default and record its fingerprint
/// - Read/parse/validation failure → back up, rebuild from default, log warn
/// - Valid with a matching recorded fingerprint → we wrote it: rewrite when
///   the content drifted from the embedded default (version bump)
/// - Valid with a mismatched fingerprint → user-edited: mark `"custom"`,
///   keep forever
/// - Valid with no record (first boot of this mechanism on an older
///   install) → claim the fingerprint when identical to the default;
///   otherwise stale-default vs user-edited is ambiguous: back it up and
///   refresh to the new default anyway
pub(super) fn write_or_rebuild_if_invalid(
	path: &Path,
	default_content: &str,
	validator: fn(&serde_json::Value, &str) -> Result<(), String>,
	label: &str,
	state_key: &str,
	state: &mut BuiltinState,
) {
	if !path.exists() {
		// 文件不存在，直接写入并记录指纹 / File missing, write it and record the fingerprint
		if let Err(e) = std::fs::write(path, default_content) {
			tracing::warn!(
				"{}",
				crate::tl!("locale.writeFileFailed", path = path.display(), error = e)
			);
		} else {
			state
				.files
				.insert(state_key.to_string(), fingerprint(default_content));
		}
		return;
	}

	// 文件存在，读取并校验（保留原文供指纹计算）
	// File exists, read and validate (keep the raw text for fingerprinting)
	let raw = std::fs::read_to_string(path)
		.map_err(|e| format!("read error: {}", e))
		.and_then(|content| {
			// 去掉 BOM，避免用户用记事本编辑过的文件被误判为损坏并重建
			// Strip the BOM so a file edited in Notepad isn't misjudged as corrupt and rebuilt
			serde_json::from_str::<serde_json::Value>(strip_utf8_bom(&content))
				.map_err(|e| format!("JSON parse error: {}", e))
				.map(|value| (content, value))
		})
		.and_then(|(content, value)| validator(&value, default_content).map(|_| content));

	let raw = match raw {
		Ok(raw) => raw,
		Err(reason) => {
			// 校验失败：先备份再重建，避免静默吞掉文件原内容
			// Validation failed: back up before rebuilding so the original
			// content isn't silently swallowed
			tracing::warn!(
				"{}",
				crate::tl!(
					"locale.validationFailed",
					path = path.display(),
					label = label,
					reason = reason
				)
			);
			backup_file(path);
			if let Err(e) = std::fs::write(path, default_content) {
				tracing::warn!(
					"{}",
					crate::tl!("locale.rebuildFailed", path = path.display(), error = e)
				);
			} else {
				tracing::info!(
					"{}",
					crate::tl!("locale.rebuiltFile", path = path.display())
				);
				state
					.files
					.insert(state_key.to_string(), fingerprint(default_content));
			}
			return;
		}
	};

	// 校验通过：按指纹判断文件归属，决定能否随版本刷新
	// Validation passed: the fingerprint decides file ownership and
	// whether a version refresh is allowed
	let disk_fp = fingerprint(&raw);
	match state.files.get(state_key).map(String::as_str) {
		// 已标记为用户自定义，保持现状 / Already marked user-modified, keep as-is
		Some("custom") => {}
		// 指纹一致：是我们上次写出的内容，与默认不同即可安全重写
		// Fingerprint match: we wrote this file — safe to rewrite on drift
		Some(recorded) if recorded == disk_fp => {
			if raw != default_content {
				if let Err(e) = std::fs::write(path, default_content) {
					tracing::warn!(
						"{}",
						crate::tl!("locale.writeFileFailed", path = path.display(), error = e)
					);
				} else {
					tracing::info!(
						"{}",
						crate::tl!("locale.refreshedFile", path = path.display())
					);
					state
						.files
						.insert(state_key.to_string(), fingerprint(default_content));
				}
			}
		}
		// 指纹不一致：写出后被用户改过，标记自定义并永久保留
		// Fingerprint mismatch: edited after our write — mark custom, keep forever
		Some(_) => {
			state
				.files
				.insert(state_key.to_string(), "custom".to_string());
			tracing::info!(
				"{}",
				crate::tl!("locale.customFileKept", path = path.display())
			);
		}
		// 无记录：旧版本安装首次运行此机制。与默认一致 → 认领；
		// 不一致则无法区分旧默认与自定义 —— 备份后按「随版本刷新」重写，
		// 自定义可从 .bak 找回
		// No record: first boot of this mechanism on a pre-mechanism
		// install. Identical to default → claim it; otherwise stale vs
		// customized is ambiguous — back it up and refresh like a version
		// bump (customizations remain recoverable from the .bak)
		None => {
			if raw == default_content {
				state.files.insert(state_key.to_string(), disk_fp);
			} else {
				if let Some(bak) = backup_file(path) {
					tracing::info!(
						"{}",
						crate::tl!("locale.backupCreated", path = bak.display())
					);
				}
				if let Err(e) = std::fs::write(path, default_content) {
					tracing::warn!(
						"{}",
						crate::tl!("locale.writeFileFailed", path = path.display(), error = e)
					);
				} else {
					tracing::info!(
						"{}",
						crate::tl!("locale.refreshedFile", path = path.display())
					);
					state
						.files
						.insert(state_key.to_string(), fingerprint(default_content));
				}
			}
		}
	}
}

/// 校验单个 locale 文件并返回错误原因列表（每项对应一个问题）。
/// 对 app 文件和 module 文件使用不同的校验规则。
///
/// Validate a single locale file and return a list of error reasons (one per issue).
/// Uses different validation rules for app vs module files.
///
/// `file_type`:
/// - `"app"` → 校验 app 文件（languageName + 所有顶层 key）
/// - `"module"` → 校验常规单节点 module 文件（name + description + params）
/// - `"builtin"` → 校验 `__builtin__` 的按节点分组嵌套结构（见 [`validate_builtin_locale`]）
fn validate_file_at_path(path: &std::path::Path, file_type: &str) -> Result<(), String> {
	let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
	let value: serde_json::Value = serde_json::from_str(strip_utf8_bom(&content))
		.map_err(|e| format!("JSON parse error: {}", e))?;

	match file_type {
		"module" => validate_module_locale(&value, ""),
		"builtin" => validate_builtin_locale(&value, ""),
		_ => {
			// app 文件：用对应代码的默认内容做 key 校验
			// App file: use the default content for the corresponding code as the key reference
			let code = path.file_stem().and_then(|s| s.to_str()).unwrap_or("zh-CN");
			let default_content = match code {
				"en-US" => APP_EN_US,
				_ => APP_ZH_CN,
			};
			validate_app_locale(&value, default_content)
		}
	}
}

/// 扫描所有用户自定义语言文件（不在内置列表中的文件）并返回校验失败的项。
/// 内置文件（zh-CN / en-US 及四个内置模块的语言文件）由 `init_locale_dirs` 在启动时处理。
///
/// Scan all user-defined locale files (not in the built-in list) and return validation failures.
/// Built-in files are handled by `init_locale_dirs` at startup.
///
/// 返回：`Vec<(文件路径字符串, 错误原因)>` / Returns: `Vec<(file path string, error reason)>`
pub fn check_custom_locale_files() -> Vec<(String, String)> {
	let mut warnings: Vec<(String, String)> = Vec::new();

	// 检查 app 自定义语言文件 / Check custom app locale files
	let builtin_app_codes: &[&str] = &["zh-CN", "en-US"];
	if let Ok(dir) = std::fs::read_dir(app_locale_dir()) {
		for entry in dir.flatten() {
			let path = entry.path();
			if path.extension().and_then(|e| e.to_str()) != Some("json") {
				continue;
			}
			let code = match path.file_stem().and_then(|s| s.to_str()) {
				Some(s) => s.to_string(),
				None => continue,
			};
			if builtin_app_codes.contains(&code.as_str()) {
				continue; // 内置文件由 init_locale_dirs 负责 / Built-in handled by init_locale_dirs
			}
			if let Err(reason) = validate_file_at_path(&path, "app") {
				warnings.push((path.to_string_lossy().to_string(), reason));
			}
		}
	}

	// 检查模块自定义语言文件 / Check custom module locale files
	let builtin_module_ids: &[&str] = &[
		"filter_short",
		"contact_sheet",
		"notify_discord",
		"notify_telegram",
		"cleanup",
		"__builtin__",
	];
	let builtin_locale_codes: &[&str] = &["zh-CN", "en-US"];
	if let Ok(mod_dir) = std::fs::read_dir(modules_locale_dir()) {
		for module_entry in mod_dir.flatten() {
			let module_path = module_entry.path();
			if !module_path.is_dir() {
				continue;
			}
			let module_id = match module_path.file_name().and_then(|n| n.to_str()) {
				Some(s) => s.to_string(),
				None => continue,
			};
			let is_builtin_module = builtin_module_ids.contains(&module_id.as_str());
			if let Ok(locale_files) = std::fs::read_dir(&module_path) {
				for locale_entry in locale_files.flatten() {
					let file_path = locale_entry.path();
					if file_path.extension().and_then(|e| e.to_str()) != Some("json") {
						continue;
					}
					let locale_code = match file_path.file_stem().and_then(|s| s.to_str()) {
						Some(s) => s.to_string(),
						None => continue,
					};
					// 内置模块的内置语言由 init_locale_dirs 负责
					// Built-in module + built-in locale handled by init_locale_dirs
					if is_builtin_module && builtin_locale_codes.contains(&locale_code.as_str()) {
						continue;
					}
					// __builtin__ 的自定义语言文件（如用户新增了 __builtin__ 目录下
					// 未内置的语言代码）沿用按节点分组的嵌套结构校验；其余模块用常规
					// 单节点结构校验。
					//
					// Custom locale files for __builtin__ (e.g. the user added a locale
					// code not covered by the built-in defaults, still under the
					// __builtin__ directory) use the nested per-node structure
					// validator; all other modules use the regular single-node validator.
					let file_type = if module_id == "__builtin__" {
						"builtin"
					} else {
						"module"
					};
					if let Err(reason) = validate_file_at_path(&file_path, file_type) {
						warnings.push((file_path.to_string_lossy().to_string(), reason));
					}
				}
			}
		}
	}

	warnings
}

/// 校验指定语言文件，返回错误原因（供切换语言时使用）。
/// 对 app locale 文件检查 languageName + 顶层 key；对模块 locale 只检查结构。
///
/// Validate the specified locale file and return an error reason if invalid (used on language switch).
pub fn validate_locale_file(locale_code: &str) -> Option<String> {
	let path = app_locale_dir().join(format!("{}.json", locale_code));
	if !path.exists() {
		return None; // 不存在则用内置 fallback，无需警告 / Missing = use built-in fallback, no warning
	}
	validate_file_at_path(&path, "app").err()
}
