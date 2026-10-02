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

use crate::config::app_state::{exe_dir, strip_utf8_bom};
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

/// 返回后端日志翻译目录（`<exe_dir>/locale/log`）。
/// Returns the backend log locale directory (`<exe_dir>/locale/log`).
pub fn log_locale_dir() -> PathBuf {
    locale_dir().join("log")
}

/// 默认的主程序中文翻译 JSON。
/// Default app Chinese (zh-CN) translation JSON.
const APP_ZH_CN: &str = include_str!("defaults/app/zh-CN.json");

/// 默认的主程序英文翻译 JSON。
/// Default app English (en-US) translation JSON.
const APP_EN_US: &str = include_str!("defaults/app/en-US.json");

/// 默认的后端日志中文翻译 JSON。
/// Default backend log Chinese (zh-CN) translation JSON.
const LOG_ZH_CN: &str = include_str!("defaults/log/zh-CN.json");

/// 默认的后端日志英文翻译 JSON。
/// Default backend log English (en-US) translation JSON.
const LOG_EN_US: &str = include_str!("defaults/log/en-US.json");

/// 内置模块的默认 locale 数据（模块 ID, 语言代码, JSON 内容）。
/// 包含外部可执行模块和内置节点（`__builtin__*`）。
/// Default locale data for built-in modules (module_id, locale_code, json_content).
/// Covers both external executable modules and built-in nodes (`__builtin__*`).
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
/// Initialize locale directories: create files if missing, rebuild built-in files if validation fails.
/// Called once at startup before the emitter is ready.
/// Custom locale file validation warnings are sent later via `emit_locale_warnings`.
pub fn init_locale_dirs() {
    // 创建目录结构 / Create directory structure
    let app_dir = app_locale_dir();
    let modules_dir = modules_locale_dir();
    let log_dir = log_locale_dir();

    for dir in [&app_dir, &modules_dir, &log_dir] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("{}", crate::tl!("locale.createDirFailed", dir = dir.display(), error = e));
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

    // 后端日志翻译文件：不存在则创建，存在但 JSON 解析失败则重建
    // Backend log locale files: create if missing, rebuild if JSON parse fails
    for (locale_code, default_content) in [("zh-CN", LOG_ZH_CN), ("en-US", LOG_EN_US)] {
        let path = log_dir.join(format!("{}.json", locale_code));
        write_or_rebuild_if_invalid(
            &path,
            default_content,
            validate_log_locale,
            &format!("log/{}", locale_code),
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
            tracing::warn!("{}", crate::tl!("locale.createDirFailed", dir = dir.display(), error = e));
            continue;
        }
        let file_path = dir.join(format!("{}.json", locale_code));
        let validator: fn(&serde_json::Value, &str) -> Result<(), String> = if *module_id == "__builtin__" {
            validate_builtin_locale
        } else {
            validate_module_locale
        };
        write_or_rebuild_if_invalid(
            &file_path,
            content,
            validator,
            &format!("{}/{}", module_id, locale_code),
        );
    }

    tracing::info!("{}", crate::tl!("locale.dirsInitialized", dir = locale_dir().display()));
}

/// 校验后端日志翻译文件：必须是 JSON object，且包含至少一个子 object（对应一个日志模块分组）。
/// 只验证顶层结构，不做深层 key 检查——用户可以按需定制各日志分组的翻译。
///
/// Validate a backend log locale file: must be a JSON object with at least one sub-object
/// (corresponding to a log module group). Only checks top-level structure.
fn validate_log_locale(value: &serde_json::Value, _default_content: &str) -> Result<(), String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "not a JSON object".to_string())?;
    if obj.is_empty() {
        return Err("must contain at least one log group entry".to_string());
    }
    Ok(())
}

/// 读取后端日志翻译（`locale/log/<locale_code>.json`），缺失的条目用内置默认文案兜底，
/// 回退语言为简体中文（规则见 [`read_with_embedded_fallback`]）：磁盘上有的条目优先
/// （保留用户的自定义），磁盘缺失的条目（如升级后新增的日志文案）使用内置文案，不会显示成
/// key 名。不修改磁盘文件。
///
/// Read the backend log translations (`locale/log/<locale_code>.json`), with missing entries
/// filled in from the embedded defaults; the fallback language is Simplified Chinese (rules in
/// [`read_with_embedded_fallback`]): entries present on disk win (keeping user customizations),
/// and entries missing on disk (e.g. log messages added in an upgrade) use the embedded text
/// instead of showing the raw key. Disk files are never modified.
pub fn read_log_locale(locale_code: &str) -> serde_json::Value {
    read_log_locale_in(&log_locale_dir(), locale_code)
}

/// [`read_log_locale`] 的实现，作用于指定的日志翻译目录。
/// Implementation of [`read_log_locale`] for the given log locale directory.
fn read_log_locale_in(dir: &std::path::Path, locale_code: &str) -> serde_json::Value {
    read_with_embedded_fallback(dir, locale_code, Some(LOG_ZH_CN), Some(LOG_EN_US))
        .unwrap_or(serde_json::Value::Object(Default::default()))
}

/// 回退语言：某个语言缺少翻译时，用这个语言的翻译补上。
/// Fallback language: used to fill in entries a language has no translation for.
const FALLBACK_LOCALE: &str = "zh-CN";

/// 读取某个翻译目录下指定语言的文件，并以内置默认翻译兜底缺失的条目，回退语言为
/// 简体中文（[`FALLBACK_LOCALE`]）。日志翻译、界面翻译与内置模块翻译共用这一规则，
/// 自下而上逐层合并（上层条目覆盖下层）：
///
/// 1. **内置简体中文**：所有语言的底；没有内置中文时（只有英文默认值）用内置英文
/// 2. **该语言自己的内置翻译**（如 en-US）：有就叠加上去，所以英文界面缺的条目显示中文，
///    而不是 key 名；自定义语言（如 ja-JP）没有内置翻译，缺的条目直接显示中文
/// 3. **磁盘文件**：该语言自己的文件（磁盘条目优先，保留用户自定义）；该语言没有内置翻译
///    且磁盘上也没有它的文件时，改用磁盘上的 zh-CN 文件
/// 4. 以上都没有时返回 `None`（如未提供翻译文件的社区模块）
///
/// 结果只在内存中合并，不修改磁盘文件。
///
/// Read a locale file for the given language from a translation directory, filling in missing
/// entries from the embedded defaults; the fallback language is Simplified Chinese
/// ([`FALLBACK_LOCALE`]). Log, UI and built-in module translations share this rule, merged layer
/// by layer from the bottom up (upper entries override lower ones):
///
/// 1. **embedded Simplified Chinese**: the base for every language; the embedded English is used
///    when there is no embedded Chinese (only an English default exists)
/// 2. **the language's own embedded translation** (e.g. en-US): layered on when present, so an
///    English UI shows Chinese rather than the raw key for missing entries; custom languages
///    (e.g. ja-JP) have none, so their missing entries show Chinese directly
/// 3. **disk file**: the language's own file (disk entries win, keeping user customizations);
///    when the language has no embedded translation and no file of its own on disk, the zh-CN
///    file on disk is used instead
/// 4. returns `None` when there is nothing at all (e.g. a community module that ships no locale
///    files)
///
/// The result is merged in memory only; disk files are never modified.
fn read_with_embedded_fallback(
    dir: &std::path::Path,
    locale_code: &str,
    zh_default: Option<&str>,
    en_default: Option<&str>,
) -> Option<serde_json::Value> {
    let parse = |c: &str| serde_json::from_str::<serde_json::Value>(c).ok();
    let own_default = match locale_code {
        "zh-CN" => zh_default,
        "en-US" => en_default,
        _ => None,
    };
    let mut merged: Option<serde_json::Value> = None;
    // 1. 回退语言（简体中文）的内置翻译打底，没有时用内置英文
    // 1. the fallback language's (Simplified Chinese) embedded translation as the base, or the
    //    embedded English when there is none
    layer_json(&mut merged, zh_default.or(en_default).and_then(parse));
    // 2. 叠加该语言自己的内置翻译；第 1 层已经是它时（zh-CN，或没有内置中文时的 en-US）跳过
    // 2. layer the language's own embedded translation; skipped when layer 1 already is it
    //    (zh-CN, or en-US when there is no embedded Chinese)
    if locale_code != FALLBACK_LOCALE && zh_default.is_some() {
        layer_json(&mut merged, own_default.and_then(parse));
    }
    // 3. 磁盘文件 / disk file
    let disk = read_locale_file(&dir.join(format!("{}.json", locale_code))).or_else(|| {
        if own_default.is_none() && locale_code != FALLBACK_LOCALE {
            read_locale_file(&dir.join(format!("{}.json", FALLBACK_LOCALE)))
        } else {
            None
        }
    });
    layer_json(&mut merged, disk);
    merged
}

/// 把 `overlay` 合并到 `base` 上（见 [`merge_json`]）；`base` 为空时直接取 `overlay`。
/// Merge `overlay` onto `base` (see [`merge_json`]); takes `overlay` as is when `base` is empty.
fn layer_json(base: &mut Option<serde_json::Value>, overlay: Option<serde_json::Value>) {
    let Some(overlay) = overlay else {
        return;
    };
    match base {
        Some(b) => merge_json(b, overlay),
        None => *base = Some(overlay),
    }
}

/// 查找内置模块的默认翻译 / Look up a built-in module's embedded default translation
fn module_default(module_id: &str, locale_code: &str) -> Option<&'static str> {
    MODULE_DEFAULTS
        .iter()
        .find(|(id, code, _)| *id == module_id && *code == locale_code)
        .map(|(_, _, content)| *content)
}

/// 把 `overlay` 逐层合并进 `base`：两边都是对象时按 key 递归合并，否则 `overlay` 覆盖 `base`。
/// Merge `overlay` into `base` level by level: objects are merged key by key recursively;
/// otherwise `overlay` replaces `base`.
fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(bv) => merge_json(bv, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
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
fn validate_builtin_locale(value: &serde_json::Value, _default_content: &str) -> Result<(), String> {
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
                return Err(format!("node \"{}\" missing required key: {}", node_key, required));
            }
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
            tracing::warn!("{}", crate::tl!("locale.writeFileFailed", path = path.display(), error = e));
        }
        return;
    }

    // 文件存在，尝试读取并校验 / File exists, try to read and validate
    let result = std::fs::read_to_string(path)
        .map_err(|e| format!("read error: {}", e))
        .and_then(|content| {
            // 去掉 BOM，避免用户用记事本编辑过的文件被误判为损坏并重建
            // Strip the BOM so a file edited in Notepad isn't misjudged as corrupt and rebuilt
            serde_json::from_str::<serde_json::Value>(strip_utf8_bom(&content))
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
                "{}",
                crate::tl!("locale.validationFailed", path = path.display(), label = label, reason = reason)
            );
            if let Err(e) = std::fs::write(path, default_content) {
                tracing::warn!("{}", crate::tl!("locale.rebuildFailed", path = path.display(), error = e));
            } else {
                tracing::info!("{}", crate::tl!("locale.rebuiltFile", path = path.display()));
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
fn validate_file_at_path(
    path: &std::path::Path,
    file_type: &str,
) -> Result<(), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("read error: {}", e))?;
    let value: serde_json::Value = serde_json::from_str(strip_utf8_bom(&content))
        .map_err(|e| format!("JSON parse error: {}", e))?;

    match file_type {
        "module" => validate_module_locale(&value, ""),
        "builtin" => validate_builtin_locale(&value, ""),
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
                    let file_type = if module_id == "__builtin__" { "builtin" } else { "module" };
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

/// 读取主程序界面的 locale JSON，缺失的条目用内置默认文案兜底
/// （规则见 [`read_with_embedded_fallback`]）：已有安装升级后，`locale/app/` 里的旧文件缺少
/// 新增的界面文案时，界面显示内置文案而不是 key 名。磁盘文件里的条目优先。
///
/// Read the app UI locale JSON, filling in missing entries from the embedded defaults (rules in
/// [`read_with_embedded_fallback`]): after an upgrade, when the old file in `locale/app/` lacks
/// newly added UI text, the UI shows the embedded text instead of the raw key. Disk entries win.
pub fn read_app_locale(locale_code: &str) -> serde_json::Value {
    read_app_locale_in(&app_locale_dir(), locale_code)
}

/// [`read_app_locale`] 的实现，作用于指定目录 / Implementation of [`read_app_locale`] for the given dir
fn read_app_locale_in(dir: &std::path::Path, locale_code: &str) -> serde_json::Value {
    read_with_embedded_fallback(dir, locale_code, Some(APP_ZH_CN), Some(APP_EN_US))
        .unwrap_or(serde_json::Value::Object(Default::default()))
}

/// 读取指定模块指定语言的 locale JSON。内置模块缺失的条目用内置默认翻译兜底；
/// 没有内置翻译的模块（如社区模块）沿用磁盘文件，目标语言文件不存在时回退 zh-CN 文件
/// （规则见 [`read_with_embedded_fallback`]）。都没有时返回 None（模块将使用自身
/// --describe 中的默认值）。
///
/// Read the locale JSON for a specific module and locale code. Missing entries of built-in
/// modules fall back to the embedded defaults; modules without embedded translations (e.g.
/// community modules) use their disk files, falling back to the zh-CN file when the target
/// language file doesn't exist (rules in [`read_with_embedded_fallback`]). Returns None when
/// there's nothing (the module uses its --describe defaults).
pub fn read_module_locale(module_id: &str, locale_code: &str) -> Option<serde_json::Value> {
    read_module_locale_in(&module_locale_dir(module_id), module_id, locale_code)
}

/// [`read_module_locale`] 的实现，作用于指定目录 / Implementation of [`read_module_locale`] for the given dir
fn read_module_locale_in(
    dir: &std::path::Path,
    module_id: &str,
    locale_code: &str,
) -> Option<serde_json::Value> {
    read_with_embedded_fallback(
        dir,
        locale_code,
        module_default(module_id, "zh-CN"),
        module_default(module_id, "en-US"),
    )
}

/// 从文件路径读取并解析 JSON；返回 None 表示文件不存在或解析失败。
/// Read and parse JSON from a file path; returns None if file doesn't exist or parse fails.
fn read_locale_file(path: &std::path::Path) -> Option<serde_json::Value> {
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str(strip_utf8_bom(&content)) {
            Ok(v) => Some(v),
            // 加载日志翻译时也会调用本函数：tl! 只读取当前已加载的翻译，且 load_log_translations
            // 先读完文件、再获取写锁，不会与这里的读锁冲突
            // Also called while loading log translations: tl! only reads the translations already
            // loaded, and load_log_translations finishes reading files before taking the write lock,
            // so it never conflicts with the read lock taken here
            Err(e) => {
                tracing::warn!("{}", crate::tl!("locale.parseFileFailed", path = path.display(), error = e));
                None
            }
        },
        Err(e) => {
            tracing::warn!("{}", crate::tl!("locale.readFileFailed", path = path.display(), error = e));
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
        "log": read_log_locale(locale_code),
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

// ─── 后端日志翻译运行时 / Backend log translation runtime ──────────────────

use std::sync::OnceLock;
use parking_lot::RwLock as ParkingRwLock;

/// 当前生效的日志翻译 JSON（从 `locale/log/<lang>.json` 加载）。
/// Current active log translation JSON (loaded from `locale/log/<lang>.json`).
static LOG_TRANSLATIONS: OnceLock<ParkingRwLock<serde_json::Value>> = OnceLock::new();

fn log_translations() -> &'static ParkingRwLock<serde_json::Value> {
    LOG_TRANSLATIONS.get_or_init(|| {
        ParkingRwLock::new(serde_json::Value::Object(Default::default()))
    })
}

/// 加载指定语言的日志翻译到全局缓存（缺失条目用内置文案兜底，见 [`read_log_locale`]）。
/// 启动时（`startup::init_logging_and_locale`）和两端保存设置时语言发生变化时调用。
/// Load the log translations for the given locale into the global cache (missing entries fall
/// back to the embedded text, see [`read_log_locale`]). Called at startup
/// (`startup::init_logging_and_locale`) and on both ends when a settings save changes the language.
pub fn load_log_translations(locale_code: &str) {
    let value = read_log_locale(locale_code);
    *log_translations().write() = value;
}

/// 通过点分 key 在翻译 JSON 中查找字符串，进行 `{varName}` 参数插值后返回。
/// 若 key 不存在则返回 key 本身（透明 fallback）。
///
/// Look up a string in the log translation JSON by dot-separated key,
/// perform `{varName}` interpolation, and return the result.
/// Returns the key itself if not found (transparent fallback).
///
/// `key` 示例 / Example key: `"recorder.started"`
/// `params` 示例 / Example params: `&[("username", "alice"), ("dir", "/tmp/rec")]`
pub fn tl_log(key: &str, params: &[(&str, &str)]) -> String {
    let guard = log_translations().read();
    // 按 '.' 逐层下钻 / Descend level by level on '.'
    let mut node: &serde_json::Value = &guard;
    for part in key.split('.') {
        match node.get(part) {
            Some(v) => node = v,
            None => return key.to_string(),
        }
    }
    let template = match node.as_str() {
        Some(s) => s.to_string(),
        None => return key.to_string(),
    };
    // 参数插值：把 {varName} 替换为对应值 / Interpolate {varName} → value
    if params.is_empty() {
        return template;
    }
    let mut result = template;
    for (name, value) in params {
        result = result.replace(&format!("{{{}}}", name), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded(content: &str) -> serde_json::Value {
        serde_json::from_str(content).expect("embedded locale")
    }

    /// 磁盘文件（带 BOM）只含部分条目时：磁盘条目优先，缺失条目用内置中文兜底。
    /// With a partial disk file (with BOM), disk entries win and missing ones fall back to the
    /// embedded Chinese text.
    #[test]
    fn zh_missing_keys_fall_back_to_embedded() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("zh-CN.json"),
            "\u{FEFF}{\"locale\":{\"rebuiltFile\":\"自定义\"}}",
        )
        .expect("write zh");
        let zh = embedded(LOG_ZH_CN);
        let merged = read_log_locale_in(dir.path(), "zh-CN");
        assert_eq!(merged["locale"]["rebuiltFile"], "自定义");
        assert_eq!(merged["locale"]["dirsInitialized"], zh["locale"]["dirsInitialized"]);
        assert_eq!(merged["recorder"], zh["recorder"]);
    }

    /// zh-CN 文件缺失时用内置中文，不会被磁盘上的 en-US 文件替换成英文。
    /// When the zh-CN file is missing the embedded Chinese is used, not the en-US file on disk.
    #[test]
    fn zh_missing_file_uses_embedded_not_en_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("en-US.json"), "{\"locale\":{\"rebuiltFile\":\"custom en\"}}")
            .expect("write en");
        let zh = embedded(LOG_ZH_CN);
        let merged = read_log_locale_in(dir.path(), "zh-CN");
        assert_eq!(merged["locale"]["rebuiltFile"], zh["locale"]["rebuiltFile"]);
    }

    /// 自定义语言：自身文件的条目优先，缺失条目用内置中文；自身文件缺失时沿用磁盘 zh-CN 文件，
    /// 不会改用磁盘上的 en-US 文件。
    /// Custom language: its own entries win and missing ones use the embedded Chinese; when its
    /// file is missing, the zh-CN file on disk is used, never the en-US file on disk.
    #[test]
    fn custom_language_falls_back_to_chinese() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zh = embedded(LOG_ZH_CN);
        std::fs::write(dir.path().join("ja-JP.json"), "{\"locale\":{\"dirsInitialized\":\"ja\"}}")
            .expect("write ja");
        std::fs::write(dir.path().join("en-US.json"), "{\"locale\":{\"rebuiltFile\":\"custom en\"}}")
            .expect("write en");
        let merged = read_log_locale_in(dir.path(), "ja-JP");
        assert_eq!(merged["locale"]["dirsInitialized"], "ja");
        assert_eq!(merged["locale"]["rebuiltFile"], zh["locale"]["rebuiltFile"]);

        std::fs::write(dir.path().join("zh-CN.json"), "{\"locale\":{\"rebuiltFile\":\"自定义中文\"}}")
            .expect("write zh");
        let merged = read_log_locale_in(dir.path(), "ko-KR");
        assert_eq!(merged["locale"]["rebuiltFile"], "自定义中文");
        assert_eq!(merged["locale"]["dirsInitialized"], zh["locale"]["dirsInitialized"]);
    }

    /// en-US：缺的条目先用内置英文，内置英文也没有的才用内置中文；磁盘 en-US 条目优先。
    /// en-US: missing entries use the embedded English first and the embedded Chinese only when
    /// the English lacks them too; disk en-US entries win.
    #[test]
    fn english_falls_back_to_english_then_chinese() {
        let dir = tempfile::tempdir().expect("tempdir");
        let zh = r#"{"g":{"a":"甲","b":"乙","c":"丙"}}"#;
        let en = r#"{"g":{"a":"A","b":"B"}}"#;
        std::fs::write(dir.path().join("en-US.json"), r#"{"g":{"a":"disk A"}}"#).expect("write en");
        let merged =
            read_with_embedded_fallback(dir.path(), "en-US", Some(zh), Some(en)).expect("locale");
        assert_eq!(merged["g"]["a"], "disk A");
        assert_eq!(merged["g"]["b"], "B");
        assert_eq!(merged["g"]["c"], "丙");
    }

    /// 界面翻译：旧文件只含部分条目时，磁盘条目优先，缺失的分组与嵌套条目用内置文案兜底；
    /// 文件不存在时直接用内置翻译。
    /// UI translations: with an old partial file, disk entries win and missing groups and nested
    /// entries fall back to the embedded text; a missing file uses the embedded translation.
    #[test]
    fn app_missing_keys_fall_back_to_embedded() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("zh-CN.json"),
            "{\"languageName\":\"自定义中文\",\"notifications\":{\"action\":{\"view_update\":\"自定义\"}}}",
        )
        .expect("write zh");
        let zh = embedded(APP_ZH_CN);
        let merged = read_app_locale_in(dir.path(), "zh-CN");
        assert_eq!(merged["languageName"], "自定义中文");
        assert_eq!(merged["notifications"]["action"]["view_update"], "自定义");
        assert_eq!(
            merged["notifications"]["action"]["remove_streamers"],
            zh["notifications"]["action"]["remove_streamers"]
        );
        assert_eq!(merged["notifications"]["backend"], zh["notifications"]["backend"]);
        assert_eq!(merged["settings"], zh["settings"]);

        assert_eq!(read_app_locale_in(dir.path(), "en-US"), embedded(APP_EN_US));
    }

    /// 内置模块缺失的条目用内置翻译兜底；没有内置翻译的模块沿用磁盘文件（含 zh-CN 回退），
    /// 什么都没有时返回 None。
    /// Built-in modules fall back to embedded translations for missing entries; modules without
    /// embedded translations use disk files (including the zh-CN fallback), and None when
    /// nothing exists.
    #[test]
    fn module_locale_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("zh-CN.json"), "{\"name\":\"自定义\"}").expect("write zh");
        let zh: serde_json::Value =
            serde_json::from_str(module_default("filter_short", "zh-CN").expect("default"))
                .expect("parse default");
        let merged = read_module_locale_in(dir.path(), "filter_short", "zh-CN").expect("locale");
        assert_eq!(merged["name"], "自定义");
        assert_eq!(merged["description"], zh["description"]);
        assert_eq!(merged["params"], zh["params"]);

        let community = tempfile::tempdir().expect("tempdir");
        assert!(read_module_locale_in(community.path(), "some_module", "en-US").is_none());
        std::fs::write(community.path().join("zh-CN.json"), "{\"name\":\"某模块\"}").expect("write zh");
        let merged =
            read_module_locale_in(community.path(), "some_module", "en-US").expect("zh fallback");
        assert_eq!(merged["name"], "某模块");
        std::fs::write(community.path().join("en-US.json"), "{\"name\":\"Some\"}").expect("write en");
        let merged =
            read_module_locale_in(community.path(), "some_module", "en-US").expect("own file");
        assert_eq!(merged["name"], "Some");
        // 只有 en-US 文件时，zh-CN 请求不会用英文文件 / zh-CN requests never use the English file
        std::fs::remove_file(community.path().join("zh-CN.json")).expect("remove zh");
        assert!(read_module_locale_in(community.path(), "some_module", "zh-CN").is_none());
    }
}
