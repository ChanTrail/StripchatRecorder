//! 设置与 Mouflon Keys handler
//! Settings and Mouflon Keys handlers

use crate::core::emitter::EmitterExt;
use crate::server::error::{ApiError, ApiResult};
use crate::server::router::ServerState;
use axum::{
    Json,
    extract::{Path, State as AxumState},
};
use serde::{Deserialize, Serialize};

pub async fn get_settings(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<crate::config::app_state::Settings> {
    Ok(Json(s.app_state.get_settings()))
}

/// 只读系统信息（不持久化，每次从操作系统实时读取）。
/// Read-only system information (not persisted; read from the OS on each request).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// 逻辑 CPU 核心数 / Logical CPU core count
    pub cpu_count: usize,
    /// 后处理并发数的输入上限（= cpu * 4）
    /// Input cap for post-processing concurrency (= cpu * 4)
    pub max_pp_concurrent_cap: usize,
    /// 录制并发数的输入上限（= cpu * 4，0 = 不限制时不受限）
    /// Input cap for recording concurrency (= cpu * 4)
    pub max_concurrent_cap: usize,
}

/// 返回只读系统信息（CPU 核心数及各并发数的输入上限）。
/// Returns read-only system information (CPU count and input caps for concurrency fields).
pub async fn get_system_info() -> ApiResult<SystemInfo> {
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // 上限与后端 resolve_concurrency 的 cap 保持一致：cpu * 2
    // Matches the cap in backend resolve_concurrency: cpu * 2
    let cap = (cpu_count * 2).max(1);
    Ok(Json(SystemInfo {
        cpu_count,
        max_pp_concurrent_cap: cap,
        max_concurrent_cap: cap,
    }))
}

pub async fn save_settings(
    AxumState(s): AxumState<ServerState>,
    Json(new_settings): Json<crate::config::app_state::Settings>,
) -> ApiResult<serde_json::Value> {
    // 若语言发生变化，重新加载日志翻译 / Reload log translations if language changed
    let old_lang = s.app_state.get_settings().language;
    let new_lang = new_settings.language.clone();
    s.app_state
        .update_settings(new_settings)
        .map_err(ApiError::from)?;
    if old_lang != new_lang {
        crate::locale::manager::load_log_translations(&new_lang);
    }
    s.emitter
        .emit("settings-updated", &s.app_state.get_settings());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn list_mouflon_keys(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(
        serde_json::to_value(s.app_state.get_mouflon_keys_store()).unwrap(),
    ))
}

#[derive(Deserialize)]
pub struct MouflonKeyBody {
    pub pkey: String,
    pub pdkey: String,
}

pub async fn add_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<MouflonKeyBody>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .add_mouflon_key(&body.pkey, &body.pdkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_mouflon_key(
    AxumState(s): AxumState<ServerState>,
    Path(pkey): Path<String>,
) -> ApiResult<serde_json::Value> {
    s.app_state
        .remove_mouflon_key(&pkey)
        .map_err(ApiError::from)?;
    s.emitter
        .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 手动触发一次 Mouflon Keys 从 Worker 同步（忽略时间间隔，强制比对 updated_at）。
/// Manually trigger a Mouflon Keys sync from the Worker (bypasses interval, still compares updated_at).
pub async fn sync_mouflon_keys(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    let settings = s.app_state.get_settings();
    let url = settings
        .mouflon_sync_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| ApiError("未配置 mouflon_sync_url".into()))?
        .to_string();
    let token = settings.mouflon_sync_token.clone();

    let updated = s
        .app_state
        .sync_mouflon_keys_from_worker(&url, token.as_deref())
        .await
        .map_err(ApiError::from)?;

    if updated {
        s.emitter
            .emit("mouflon-keys-updated", &s.app_state.get_mouflon_keys_store());
    }

    Ok(Json(serde_json::json!({ "updated": updated })))
}
