//! Locale 读取与合并核心子模块：内嵌默认兜底、逐层合并与语言列表。
//! Locale read/merge core: embedded-default fallback, layered merging and
//! the available-locale listing.

use super::{
    APP_EN_US, APP_ZH_CN, LOG_EN_US, LOG_ZH_CN, MODULE_DEFAULTS, app_locale_dir, log_locale_dir,
    module_locale_dir, modules_locale_dir,
};
use crate::config::app_state::strip_utf8_bom;

/// 回退语言：某个语言缺少翻译时，用这个语言的翻译补上。
/// Fallback language: used to fill in entries a language has no translation for.
const FALLBACK_LOCALE: &str = "zh-CN";

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
                tracing::warn!(
                    "{}",
                    crate::tl!("locale.parseFileFailed", path = path.display(), error = e)
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                "{}",
                crate::tl!("locale.readFileFailed", path = path.display(), error = e)
            );
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
                .and_then(|v| {
                    v.get("languageName")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
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
                .and_then(|v| {
                    v.get("languageName")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| code.to_string());
            entries.push(LocaleEntry {
                code: code.to_string(),
                name,
            });
        }
    }

    entries
}
