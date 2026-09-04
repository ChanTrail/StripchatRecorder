//! locale 路由 handler / Locale route handlers

use crate::server::error::{ApiError, ApiResult};
use axum::{Json, extract::Path};

/// 返回指定语言代码的完整 locale 数据（主程序翻译 + 所有模块翻译覆盖）。
/// 若语言文件存在但校验失败，响应中附带 `warning` 字段。
///
/// Return the full locale data for the given locale code (app translations + all module overrides).
/// If the locale file exists but fails validation, the response includes a `warning` field.
pub async fn get_locale_handler(
    Path(locale_code): Path<String>,
) -> ApiResult<serde_json::Value> {
    let lc = locale_code.clone();
    let (locale, warning) = tokio::task::spawn_blocking(move || {
        let data = crate::locale::manager::get_full_locale(&lc);
        let warning = crate::locale::manager::validate_locale_file(&lc);
        (data, warning)
    })
    .await
    .map_err(|e| ApiError(e.to_string()))?;

    let mut result = locale;
    if let Some(w) = warning {
        result["warning"] = serde_json::Value::String(w);
    }
    Ok(Json(result))
}

/// 返回可用语言列表（扫描 locale/app/ 目录）。
/// Return the list of available locales (scanned from locale/app/ directory).
pub async fn list_locales_handler() -> ApiResult<serde_json::Value> {
    let locales =
        tokio::task::spawn_blocking(crate::locale::manager::list_available_locales)
            .await
            .map_err(|e| ApiError(e.to_string()))?;
    Ok(Json(serde_json::to_value(locales).unwrap()))
}
