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
	/// 后处理并发数的输入上限（= 逻辑核数，与后端 resolve_concurrency 的 cap 一致）
	/// Input cap for post-processing concurrency (= logical core count, matching backend resolve_concurrency cap)
	pub max_pp_concurrent_cap: usize,
}

/// 返回只读系统信息（CPU 核心数及各并发数的输入上限）。
/// Returns read-only system information (CPU count and input caps for concurrency fields).
pub async fn get_system_info() -> ApiResult<SystemInfo> {
	let cpu_count = std::thread::available_parallelism()
		.map(|n| n.get())
		.unwrap_or(1);
	// 后处理上限与 resolve_concurrency 一致：等于逻辑核数（实测解码型 pp 在 ~12 并发
	// 已使 32 核机器饱和，核数×1 即合理上限）；录制不设限故无 cap 字段
	// pp cap matches resolve_concurrency: logical core count (measured: decode-type
	// pp saturates a 32-core box at ~12); recordings are uncapped → no cap field
	Ok(Json(SystemInfo {
		cpu_count,
		max_pp_concurrent_cap: cpu_count.max(1),
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

/// 添加密钥的请求体：`pdkey` 与 `keystream`（十六进制）至少给一个。
/// Request body for adding a key: at least one of `pdkey` and `keystream` (hex) is required.
#[derive(Deserialize)]
pub struct MouflonKeyBody {
	pub pkey: String,
	#[serde(default)]
	pub pdkey: Option<String>,
	#[serde(default)]
	pub keystream: Option<String>,
}

pub async fn add_mouflon_key(
	AxumState(s): AxumState<ServerState>,
	Json(body): Json<MouflonKeyBody>,
) -> ApiResult<serde_json::Value> {
	s.app_state
		.add_mouflon_key(&body.pkey, body.pdkey.as_deref(), body.keystream.as_deref())
		.map_err(ApiError::from)?;
	s.emitter.emit(
		"mouflon-keys-updated",
		&s.app_state.get_mouflon_keys_store(),
	);
	Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_mouflon_key(
	AxumState(s): AxumState<ServerState>,
	Path(pkey): Path<String>,
) -> ApiResult<serde_json::Value> {
	s.app_state
		.remove_mouflon_key(&pkey)
		.map_err(ApiError::from)?;
	s.emitter.emit(
		"mouflon-keys-updated",
		&s.app_state.get_mouflon_keys_store(),
	);
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
		s.emitter.emit(
			"mouflon-keys-updated",
			&s.app_state.get_mouflon_keys_store(),
		);
	}

	Ok(Json(serde_json::json!({ "updated": updated })))
}
