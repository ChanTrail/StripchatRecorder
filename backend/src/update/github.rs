//! GitHub Release 查询与响应类型。
//! GitHub release queries and response types.
//!
//! `check_prerelease = true` 时改用 `/releases?per_page=10` 取最新的一条（含 prerelease）；
//! `check_prerelease = false` 时使用 `/releases/latest`（仅正式版）。
//! With `check_prerelease = true` the query uses `/releases?per_page=10` and takes
//! the newest entry (pre-releases included); `false` uses `/releases/latest`
//! (stable only).

use serde::Serialize;

use super::{APP_VERSION, OWNER, REPO, platform::current_platform};

/// GitHub Release 信息（仅包含前端需要的字段）
/// GitHub release info (only the fields the frontend needs)
#[derive(Debug, Serialize)]
pub struct ReleaseInfo {
	pub latest_version: String,
	pub release_url: String,
	pub release_notes: String,
	pub published_at: String,
	pub download_url: Option<String>,
	pub download_size: Option<u64>,
}

/// GET /api/update/info 的完整响应结构
/// Full response shape of GET /api/update/info
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
	pub current_version: String,
	pub platform: String,
	pub is_docker: bool,
	/// 当前运行的是预发布版本（版本号含 `-beta`，如 `0.4.0-beta`）。
	/// Whether the currently running build is a pre-release (version string contains `-beta`).
	pub is_beta: bool,
	pub release: Option<ReleaseInfo>,
	pub asset_names: Vec<String>,
}

/// 向 GitHub API 查询最新 Release，使用可选的代理地址。
/// Query the GitHub API for the latest release, optionally through a proxy.
pub async fn fetch_latest_release(
	proxy_url: Option<&str>,
	check_prerelease: bool,
) -> crate::core::error::Result<ReleaseInfo> {
	let (info, _) = fetch_latest_release_with_assets(proxy_url, check_prerelease).await?;
	Ok(info)
}

/// 向 GitHub API 查询最新 Release，同时返回所有 asset 名称列表（用于调试）。
///
/// `check_prerelease = true` 时改用 `/releases?per_page=10` 取最新的一条（含 prerelease）；
/// `check_prerelease = false` 时使用 `/releases/latest`（仅正式版）。
///
/// Query the GitHub API for the latest release, also returning every asset name
/// (for debugging). When `check_prerelease = true`, uses `/releases?per_page=10`
/// to get the most recent release including pre-releases; when `false`, uses
/// `/releases/latest` (stable only).
pub async fn fetch_latest_release_with_assets(
	proxy_url: Option<&str>,
	check_prerelease: bool,
) -> crate::core::error::Result<(ReleaseInfo, Vec<String>)> {
	let api_url = if check_prerelease {
		format!(
			"https://api.github.com/repos/{}/{}/releases?per_page=10",
			OWNER, REPO
		)
	} else {
		format!(
			"https://api.github.com/repos/{}/{}/releases/latest",
			OWNER, REPO
		)
	};

	let mut builder = reqwest::Client::builder()
		.user_agent(format!("StripchatRecorder/{}", APP_VERSION))
		.timeout(std::time::Duration::from_secs(15));

	if let Some(proxy) = proxy_url.filter(|s| !s.is_empty()) {
		let p = reqwest::Proxy::all(proxy)
			.map_err(|e| crate::core::error::AppError::Other(format!("代理配置错误: {}", e)))?;
		builder = builder.proxy(p);
	}

	let client = builder.build().map_err(|e| {
		crate::core::error::AppError::Other(format!("HTTP 客户端初始化失败: {}", e))
	})?;

	let resp = client
		.get(&api_url)
		.header("Accept", "application/vnd.github+json")
		.send()
		.await
		.map_err(|e| crate::core::error::AppError::Other(format!("GitHub API 请求失败: {}", e)))?;

	if !resp.status().is_success() {
		return Err(crate::core::error::AppError::Other(format!(
			"GitHub API 返回 {}",
			resp.status()
		)));
	}

	let json: serde_json::Value = resp
		.json()
		.await
		.map_err(|e| crate::core::error::AppError::Other(format!("解析响应失败: {}", e)))?;

	// check_prerelease 模式下返回数组，取第一个（GitHub 按发布时间倒序，第一个即最新）；
	// 否则直接使用对象（/releases/latest 返回单个对象）。
	//
	// In check_prerelease mode the response is an array; take the first element
	// (GitHub returns releases in reverse chronological order).
	let release_obj = if check_prerelease {
		match json.as_array().and_then(|arr| arr.first()) {
			Some(obj) => obj.clone(),
			None => {
				return Err(crate::core::error::AppError::Other(
					"GitHub API 未返回任何 Release".to_string(),
				));
			}
		}
	} else {
		json
	};

	let latest_version = release_obj["tag_name"]
		.as_str()
		.unwrap_or("")
		.trim_start_matches('v')
		.to_string();
	let release_url = release_obj["html_url"].as_str().unwrap_or("").to_string();
	let release_notes = release_obj["body"].as_str().unwrap_or("").to_string();
	let published_at = release_obj["published_at"]
		.as_str()
		.unwrap_or("")
		.to_string();

	// asset 命名规则：StripchatRecorder-server-{platform}[-{version}].zip
	// 用前缀+后缀匹配，兼容带版本号和不带版本号两种命名。
	// Asset naming: StripchatRecorder-server-{platform}[-{version}].zip
	// Match by prefix + suffix to handle both versioned and non-versioned names.
	let platform = current_platform();
	let asset_prefix = format!("StripchatRecorder-server-{}", platform);

	let asset_names: Vec<String> = release_obj["assets"]
		.as_array()
		.map(|assets| {
			assets
				.iter()
				.filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
				.collect()
		})
		.unwrap_or_default();

	let (download_url, download_size) = release_obj["assets"]
		.as_array()
		.and_then(|assets| {
			assets.iter().find(|a| {
				a["name"]
					.as_str()
					.map(|n| n.starts_with(&asset_prefix) && n.ends_with(".zip"))
					.unwrap_or(false)
			})
		})
		.map(|a| {
			let url = a["browser_download_url"].as_str().unwrap_or("").to_string();
			let size = a["size"].as_u64();
			(Some(url), size)
		})
		.unwrap_or((None, None));

	Ok((
		ReleaseInfo {
			latest_version,
			release_url,
			release_notes,
			published_at,
			download_url,
			download_size,
		},
		asset_names,
	))
}
