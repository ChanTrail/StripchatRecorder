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
//!         └── notify_telegram/
//!             ├── zh-CN.json
//!             └── en-US.json
//! ```

use crate::config::settings::exe_dir;
use std::path::PathBuf;

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

/// 默认的主程序中文翻译 JSON。
/// Default app Chinese (zh-CN) translation JSON.
const APP_ZH_CN: &str = include_str!("defaults/app/zh-CN.json");

/// 默认的主程序英文翻译 JSON。
/// Default app English (en-US) translation JSON.
const APP_EN_US: &str = include_str!("defaults/app/en-US.json");

/// 内置模块的默认 locale 数据（模块 ID, 语言代码, JSON 内容）。
/// Default locale data for built-in modules (module_id, locale_code, json_content).
const MODULE_DEFAULTS: &[(&str, &str, &str)] = &[
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
];

/// 初始化 locale 目录：若文件不存在则创建，若内置文件校验失败则重建。
/// 此函数在程序启动时调用一次（emitter 就绪前）。
/// 用户自定义语言文件的校验警告通过 `emit_locale_warnings` 在 emitter 就绪后发送。
///
/// Initialize locale directories: create files if missing, rebuild built-in files if validation fails.
/// Called once at startup before the emitter is ready.
/// Custom locale file validation warnings are sent later via `emit_locale_warnings`.
pub fn init_locale_dirs() {
    // 创建目录结构 / Create directory structure
    let app_dir = app_locale_dir();
    let modules_dir = modules_locale_dir();

    for dir in [&app_dir, &modules_dir] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("Failed to create locale dir {:?}: {}", dir, e);
        }
    }

    // 主程序内置语言文件：不存在则创建，存在但校验失败则重建
    // Built-in app locale files: create if missing, rebuild if validation fails
    for (locale_code, default_content) in [("zh-CN", APP_ZH_CN), ("en-US", APP_EN_US)] {
        let path = app_dir.join(format!("{}.json", locale_code));
        write_or_rebuild_if_invalid(
            &path,
            default_content,
            validate_app_locale,
            locale_code,
        );
    }

    // 模块内置语言文件：不存在则创建，存在但校验失败则重建
    // Built-in module locale files: create if missing, rebuild if validation fails
    for (module_id, locale_code, content) in MODULE_DEFAULTS {
        let dir = module_locale_dir(module_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("Failed to create module locale dir {:?}: {}", dir, e);
            continue;
        }
        let file_path = dir.join(format!("{}.json", locale_code));
        write_or_rebuild_if_invalid(
            &file_path,
            content,
            validate_module_locale,
            &format!("{}/{}", module_id, locale_code),
        );
    }

    tracing::info!("Locale dirs initialized at {:?}", locale_dir());
}

/// 校验主程序语言文件：
/// 必须是 JSON object，包含 `languageName`（字符串），
/// 且包含与对应默认文件相同的全部顶层 key。
///
/// Validate an app locale file:
/// Must be a JSON object, contain `languageName` (string),
/// and contain all top-level keys present in the corresponding default file.
fn validate_app_locale(value: &serde_json::Value, default_content: &str) -> Result<(), String> {
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
        return Err(format!("missing required top-level keys: {}", missing.join(", ")));
    }

    Ok(())
}

/// 校验模块语言文件：
/// 必须是 JSON object，且包含 `name`、`description`、`params` 三个 key。
///
/// Validate a module locale file:
/// Must be a JSON object containing `name`, `description`, and `params`.
fn validate_module_locale(value: &serde_json::Value, _default_content: &str) -> Result<(), String> {
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

/// 写入或重建内置 locale 文件的统一逻辑：
/// - 文件不存在 → 写入默认内容
/// - 文件存在但解析失败 → 重建为默认内容，记录 warn 日志
/// - 文件存在且解析成功但校验失败 → 重建为默认内容，记录 warn 日志
/// - 文件存在且校验通过 → 不做任何操作
///
/// Unified logic for writing or rebuilding a built-in locale file:
/// - File missing → write default content
/// - File exists but JSON parse fails → rebuild from default, log warn
/// - File exists, parses OK, but validation fails → rebuild from default, log warn
/// - File exists and passes validation → do nothing
fn write_or_rebuild_if_invalid(
    path: &std::path::Path,
    default_content: &str,
    validator: fn(&serde_json::Value, &str) -> Result<(), String>,
    label: &str,
) {
    if !path.exists() {
        // 文件不存在，直接写入 / File missing, write it
        if let Err(e) = std::fs::write(path, default_content) {
            tracing::warn!("Failed to write locale file {:?}: {}", path, e);
        }
        return;
    }

    // 文件存在，尝试读取并校验 / File exists, try to read and validate
    let result = std::fs::read_to_string(path)
        .map_err(|e| format!("read error: {}", e))
        .and_then(|content| {
            serde_json::from_str::<serde_json::Value>(&content)
                .map_err(|e| format!("JSON parse error: {}", e))
        })
        .and_then(|value| validator(&value, default_content));

    match result {
        Ok(()) => {
            // 校验通过，无需操作 / Validation passed, nothing to do
        }
        Err(reason) => {
            // 校验失败，重建文件 / Validation failed, rebuild the file
            tracing::warn!(
                "Locale file {:?} failed validation ({}): \"{}\". Rebuilding from default.",
                path,
                label,
                reason
            );
            if let Err(e) = std::fs::write(path, default_content) {
                tracing::warn!("Failed to rebuild locale file {:?}: {}", path, e);
            } else {
                tracing::info!("Rebuilt locale file {:?}", path);
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
/// - `"module"` → 校验 module 文件（name + description + params）
fn validate_file_at_path(
    path: &std::path::Path,
    file_type: &str,
) -> Result<(), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read error: {}", e))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("JSON parse error: {}", e))?;

    match file_type {
        "module" => validate_module_locale(&value, ""),
        _ => {
            // app 文件：用对应代码的默认内容做 key 校验
            // App file: use the default content for the corresponding code as the key reference
            let code = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("zh-CN");
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
                    if let Err(reason) = validate_file_at_path(&file_path, "module") {
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

/// 读取主程序指定语言的 locale JSON。
/// 若文件不存在则返回内置默认内容（fallback to embedded defaults）。
///
/// Read the app locale JSON for the given locale code.
/// Falls back to embedded defaults if the file doesn't exist.
pub fn read_app_locale(locale_code: &str) -> serde_json::Value {
    let path = app_locale_dir().join(format!("{}.json", locale_code));
    read_locale_file(&path).unwrap_or_else(|| {
        // 内置 fallback / Embedded fallback
        let content = match locale_code {
            "en-US" => APP_EN_US,
            _ => APP_ZH_CN,
        };
        serde_json::from_str(content).unwrap_or(serde_json::Value::Object(Default::default()))
    })
}

/// 读取指定模块指定语言的 locale JSON。
/// 若目标语言文件不存在，自动回退到 en-US；
/// en-US 也不存在时返回 None（模块将使用自身 --describe 中的默认值）。
///
/// Read the locale JSON for a specific module and locale code.
/// Falls back to en-US if the target locale file doesn't exist.
/// Returns None if en-US is also absent (module uses its --describe defaults).
pub fn read_module_locale(module_id: &str, locale_code: &str) -> Option<serde_json::Value> {
    let dir = module_locale_dir(module_id);
    let path = dir.join(format!("{}.json", locale_code));

    // 目标语言文件存在则直接返回 / Return target locale if it exists
    if let Some(v) = read_locale_file(&path) {
        return Some(v);
    }

    // 目标语言不是 en-US 时 fallback 到 en-US / Fall back to en-US when target isn't already en-US
    if locale_code != "en-US" {
        let fallback = dir.join("en-US.json");
        if let Some(v) = read_locale_file(&fallback) {
            return Some(v);
        }
    }

    None
}

/// 从文件路径读取并解析 JSON；返回 None 表示文件不存在或解析失败。
/// Read and parse JSON from a file path; returns None if file doesn't exist or parse fails.
fn read_locale_file(path: &std::path::Path) -> Option<serde_json::Value> {
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("Failed to parse locale file {:?}: {}", path, e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to read locale file {:?}: {}", path, e);
            None
        }
    }
}

/// 获取完整的 locale 响应：主程序翻译 + 所有已发现模块的翻译覆盖。
///
/// Get the full locale response: app translations + module locale overrides for all discovered modules.
///
/// 返回结构 / Return structure:
/// ```json
/// {
///   "app": { ...app locale keys... },
///   "modules": {
///     "filter_short": { "name": "...", "description": "...", "params": {...} },
///     ...
///   }
/// }
/// ```
pub fn get_full_locale(locale_code: &str) -> serde_json::Value {
    let app = read_app_locale(locale_code);

    // 扫描 modules locale 目录，为每个有翻译文件的模块收集覆盖数据
    // Scan modules locale directory and collect overrides for each module with a locale file
    let mut modules_obj = serde_json::Map::new();

    if let Ok(entries) = std::fs::read_dir(modules_locale_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(module_id) = path.file_name().and_then(|n| n.to_str())
                && let Some(tr) = read_module_locale(module_id, locale_code)
            {
                modules_obj.insert(module_id.to_string(), tr);
            }
        }
    }

    serde_json::json!({
        "app": app,
        "modules": serde_json::Value::Object(modules_obj),
    })
}

/// 可用语言条目 / Available locale entry
#[derive(serde::Serialize)]
pub struct LocaleEntry {
    /// BCP 47 语言代码 / BCP 47 locale code
    pub code: String,
    /// 该语言的自身显示名称（从 JSON 的 languageName 字段读取）/ Native display name (from languageName field)
    pub name: String,
}

/// 扫描 locale/app/ 目录，返回所有可用语言列表。
/// 始终包含内置的 zh-CN 和 en-US（即使文件尚未创建）。
///
/// Scan the locale/app/ directory and return all available locales.
/// Always includes built-in zh-CN and en-US (even if files don't exist yet).
pub fn list_available_locales() -> Vec<LocaleEntry> {
    let mut entries: Vec<LocaleEntry> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 先扫描磁盘上的文件 / Scan files on disk first
    if let Ok(dir) = std::fs::read_dir(app_locale_dir()) {
        let mut paths: Vec<_> = dir.flatten().collect();
        paths.sort_by_key(|e| e.file_name());
        for entry in paths {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let code = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if seen.contains(&code) {
                continue;
            }
            let name = read_locale_file(&path)
                .and_then(|v| v.get("languageName").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_else(|| code.clone());
            seen.insert(code.clone());
            entries.push(LocaleEntry { code, name });
        }
    }

    // 补充内置语言（若磁盘上没有）/ Add built-in locales if not already present
    for (code, content) in [("zh-CN", APP_ZH_CN), ("en-US", APP_EN_US)] {
        if !seen.contains(code) {
            let name = serde_json::from_str::<serde_json::Value>(content)
                .ok()
                .and_then(|v| v.get("languageName").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .unwrap_or_else(|| code.to_string());
            entries.push(LocaleEntry { code: code.to_string(), name });
        }
    }

    entries
}
