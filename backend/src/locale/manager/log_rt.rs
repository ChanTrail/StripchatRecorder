//! 后端日志翻译运行时子模块：全局翻译缓存与 `tl_log` 查询。
//! Backend log-translation runtime: the global translation cache and the
//! `tl_log` lookup.

use super::read_log_locale;
use parking_lot::RwLock as ParkingRwLock;
use std::sync::OnceLock;

/// 当前生效的日志翻译 JSON（从 `locale/log/<lang>.json` 加载）。
/// Current active log translation JSON (loaded from `locale/log/<lang>.json`).
static LOG_TRANSLATIONS: OnceLock<ParkingRwLock<serde_json::Value>> = OnceLock::new();

/// 返回全局日志翻译缓存的引用 / Return the global log-translation cache
fn log_translations() -> &'static ParkingRwLock<serde_json::Value> {
    LOG_TRANSLATIONS
        .get_or_init(|| ParkingRwLock::new(serde_json::Value::Object(Default::default())))
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
