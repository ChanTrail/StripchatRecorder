//! Stripchat API 客户端 / Stripchat API Client
//!
//! 封装对 Stripchat 前端 API 的访问，包括：
//! - 获取主播直播状态和播放列表 URL
//! - 对主播放列表（master playlist）进行多 CDN 竞速请求
//! - 下载 HLS 分片（支持多 CDN 竞速）
//!
//! 播放列表文本本身的解析（含 Mouflon 加密处理）委托给 `recording::hls`；
//! 本模块只负责网络请求与竞速调度。
//!
//! Wraps access to the Stripchat frontend API, including:
//! - Fetching streamer live status and playlist URLs
//! - Racing multiple CDN TLDs for the master playlist
//! - Downloading HLS segments (with multi-CDN racing)
//!
//! Parsing of the playlist text itself (including Mouflon encryption handling) is
//! delegated to `recording::hls`; this module is responsible only for network
//! requests and CDN racing.

use crate::core::error::{AppError, Result};
use reqwest::{Client, Response};
use std::collections::HashMap;
use std::sync::Arc;

/// 模拟浏览器的 User-Agent / Browser-mimicking User-Agent
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36";
/// 请求 Referer 头 / Request Referer header
const REFERER: &str = "https://stripchat.com/";

/// 支持的 CDN 顶级域名列表（用于多 CDN 竞速）/ Supported CDN TLDs (for multi-CDN racing)
const CDN_TLDS: &[&str] = &[
    "doppiocdn.com",
    "doppiocdn.media",
    "doppiocdn.net",
    "doppiocdn.org",
    "doppiocdn.live",
];

/// 构建用于 CDN 分片下载的 HTTP 客户端（支持代理，启用 TCP keepalive）。
/// Build an HTTP client for CDN segment downloads (supports proxy, enables TCP keepalive).
fn build_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .connection_verbose(false);

    if let Some(proxy) = proxy_url
        && !proxy.is_empty() {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
    } else {
        builder = builder.no_proxy();
    }

    Ok(builder.build()?)
}

/// 构建用于 API 请求的 HTTP 客户端（支持代理，不启用 keepalive）。
/// Build an HTTP client for API requests (supports proxy, no keepalive).
fn build_api_client(proxy_url: Option<&str>) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30));

    if let Some(proxy) = proxy_url
        && !proxy.is_empty() {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
        return Ok(builder.build()?);
    }
    builder = builder.no_proxy();
    Ok(builder.build()?)
}

/// 主播直播状态信息 / Streamer live status information
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// 是否在线 / Whether online
    pub is_online: bool,
    /// 是否可录制（公开秀状态）/ Whether recordable (public show status)
    #[allow(dead_code)]
    pub is_recordable: bool,
    /// 直播间状态文字（中文）/ Stream status text (Chinese)
    pub status: String,
    /// 缩略图 URL / Thumbnail URL
    pub thumbnail_url: Option<String>,
    /// HLS 播放列表 URL（仅在 fetch_playlist=true 且可录制时有值）/ HLS playlist URL (only when fetch_playlist=true and recordable)
    pub playlist_url: Option<String>,
    /// 主播的 Stripchat 内部 ID（从 v1/broadcasts 响应的 `modelId` 字段解析）。
    /// 调用方应在自身缓存的 model_id 为空时用此值回填（见 `AppState::backfill_model_id`）。
    ///
    /// The streamer's Stripchat internal ID (parsed from the v1/broadcasts response's
    /// `modelId` field). Callers should use this to backfill their own cached model_id
    /// when it's empty (see `AppState::backfill_model_id`).
    pub model_id: Option<i64>,
    /// 若本次查询是通过 `known_model_id` 参数反查确认该主播已改名而成功的，
    /// 此处携带反查得到的新用户名，供调用方更新持久化记录
    /// （见 `AppState::rename_streamer`）。原始查询成功（未触发改名回退）时为 `None`。
    ///
    /// If this query succeeded via the `known_model_id` fallback confirming the streamer
    /// was renamed, this carries the newly resolved username, for the caller to update
    /// its persisted record (see `AppState::rename_streamer`). `None` when the original
    /// lookup succeeded directly (rename fallback wasn't triggered).
    pub renamed_to: Option<String>,
}

/// Stripchat API 客户端，封装 API 请求和 CDN 分片下载。
/// Stripchat API client wrapping API requests and CDN segment downloads.
pub struct StripchatApi {
    /// API 请求客户端 / API request client
    api_client: Client,
    /// CDN 分片下载客户端 / CDN segment download client
    cdn_client: Client,
    /// 可选的镜像站域名 / Optional mirror site domain
    sc_mirror: Option<String>,
    /// 镜像站协议（"https" 或 "http"）/ Mirror site scheme ("https" or "http")
    sc_mirror_scheme: String,
    /// 各 CDN 节点的首选 TLD 缓存（节点 ID -> TLD）/ Preferred TLD cache per CDN node (node ID -> TLD)
    preferred_tld_by_node: Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
    /// Mouflon 解密密钥（pkey -> pdkey），用于 playlist URL 匹配 / Mouflon decryption keys (pkey -> pdkey) for playlist URL matching
    mouflon_keys: HashMap<String, String>,
}

impl StripchatApi {
    /// 创建完整的 API 客户端（API + CDN，带 CDN TLD 缓存）。
    /// Create a full API client (API + CDN, with CDN TLD cache).
    pub fn new(
        api_proxy: Option<&str>,
        cdn_proxy: Option<&str>,
        sc_mirror: Option<&str>,
        sc_mirror_scheme: Option<&str>,
        preferred_tld_by_node: Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
    ) -> Result<Self> {
        Ok(Self {
            api_client: build_api_client(api_proxy)?,
            cdn_client: build_client(cdn_proxy)?,
            sc_mirror: sc_mirror.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            sc_mirror_scheme: sc_mirror_scheme
                .filter(|s| *s == "http" || *s == "https")
                .unwrap_or("https")
                .to_string(),
            preferred_tld_by_node,
            mouflon_keys: HashMap::new(),
        })
    }

    /// 创建仅用于 API 请求的客户端（不需要 CDN TLD 缓存，适用于验证用户名等场景）。
    /// Create an API-only client (no CDN TLD cache, suitable for username verification, etc.).
    pub fn new_api_only(
        api_proxy: Option<&str>,
        cdn_proxy: Option<&str>,
        sc_mirror: Option<&str>,
        sc_mirror_scheme: Option<&str>,
    ) -> Result<Self> {
        Self::new(
            api_proxy,
            cdn_proxy,
            sc_mirror,
            sc_mirror_scheme,
            Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        )
    }

    /// 设置 Mouflon 解密密钥，返回 self 以支持链式调用。
    /// Set Mouflon decryption keys, returns self for method chaining.
    pub fn with_mouflon_keys(mut self, keys: HashMap<String, String>) -> Self {
        self.mouflon_keys = keys;
        self
    }

    /// 获取当前 Mouflon 解密密钥的引用。
    /// Get a reference to the current Mouflon decryption keys.
    pub fn mouflon_keys(&self) -> &HashMap<String, String> {
        &self.mouflon_keys
    }

    /// 将 stripchat.com 域名替换为镜像站域名（若已配置），并替换协议。
    /// Replace the stripchat.com domain (and scheme if configured) with the mirror site.
    fn api_url(&self, url: &str) -> String {
        match &self.sc_mirror {
            Some(mirror) => {
                let replaced = url.replace("stripchat.com", mirror);
                // 替换协议：将 https:// 替换为配置的 scheme://
                // Replace scheme: https:// → configured scheme://
                if self.sc_mirror_scheme == "http" {
                    replaced.replacen("https://", "http://", 1)
                } else {
                    replaced
                }
            }
            None => url.to_string(),
        }
    }

    /// 返回适配镜像站的 Referer 头值。
    /// Return the Referer header value adapted for the mirror site.
    fn referer(&self) -> String {
        match &self.sc_mirror {
            Some(mirror) => {
                let replaced = REFERER.replace("stripchat.com", mirror);
                if self.sc_mirror_scheme == "http" {
                    replaced.replacen("https://", "http://", 1)
                } else {
                    replaced
                }
            }
            None => REFERER.to_string(),
        }
    }

    /// 解析 v1/broadcasts/{username} 响应，统一处理"用户不存在"判定。
    ///
    /// 关键点：Stripchat 对不存在的用户名返回的是 **HTTP 404**（而不是文档假设的
    /// 200），body 是 `{"title":"An error occurred","description":"...not found..."}`。
    /// 若先检查 `status.is_success()` 再决定是否解析 body（旧实现的做法），404 会在
    /// body 被检查之前就被当作普通网络错误短路返回，导致"用户不存在"永远被误判为
    /// 泛化的 API 错误——改名反查兜底也就永远不会被触发（`get_stream_info` 只在
    /// 拿到 `UserNotFound` 时才走反查逻辑）。
    ///
    /// 因此这里反过来：无论 HTTP 状态码是什么，先尝试把 body 解析为 JSON 并检查
    /// 是否匹配"用户不存在"的错误形状；只有当 body 完全无法解析、或状态失败且
    /// body 也不是这个已知错误形状时，才归类为其他网络/API 错误。
    ///
    /// Parse the v1/broadcasts/{username} response, uniformly handling "user not
    /// found" detection.
    ///
    /// Key point: Stripchat returns **HTTP 404** (not 200, as previously assumed) for a
    /// nonexistent username, with body
    /// `{"title":"An error occurred","description":"...not found..."}`. If
    /// `status.is_success()` is checked before deciding whether to parse the body (the
    /// old implementation's approach), a 404 short-circuits as a generic network error
    /// before the body is ever inspected — meaning "user not found" was permanently
    /// misclassified as a generic API error, and the rename-lookup fallback (which only
    /// triggers on `UserNotFound`, see `get_stream_info`) never fired.
    ///
    /// So this is inverted here: regardless of HTTP status, first try parsing the body
    /// as JSON and check whether it matches the "user not found" error shape; only when
    /// the body can't be parsed at all, or the status failed AND the body doesn't match
    /// this known error shape, is it classified as some other network/API error.
    async fn parse_broadcast_response(
        resp: Response,
        username: &str,
    ) -> Result<serde_json::Value> {
        let status = resp.status();
        let bytes = resp.bytes().await?;

        let json: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                return Err(AppError::Other(format!(
                    "API 返回 {} ({})",
                    status.as_u16(),
                    username
                )));
            }
        };

        // "用户不存在"的错误形状：无论 HTTP 状态码是 200 还是 404 都可能出现
        // "User not found" error shape: can appear under either HTTP 200 or 404
        if json["title"].as_str() == Some("An error occurred")
            && json["description"]
                .as_str()
                .is_some_and(|d| d.contains("not found"))
        {
            return Err(AppError::UserNotFound(format!("用户 {} 不存在", username)));
        }

        if !status.is_success() {
            return Err(AppError::Other(format!(
                "API 返回 {} ({})",
                status.as_u16(),
                username
            )));
        }

        Ok(json)
    }

    /// 从 CDN URL 中提取节点 ID（URL 主机名的第一段）。
    /// Extract the node ID from a CDN URL (first segment of the hostname).
    fn extract_node_id(url: &str) -> Option<&str> {
        let without_scheme = url.strip_prefix("https://")?;
        let host = without_scheme.split('/').next()?;
        host.split('.').next()
    }

    /// 对 CDN URL 进行多 TLD 竞速请求，返回最先成功响应的结果。
    /// 同时更新节点的首选 TLD 缓存，加速后续请求。
    ///
    /// Race a CDN URL across multiple TLDs and return the first successful response.
    /// Also updates the preferred TLD cache for the node to speed up subsequent requests.
    async fn cdn_get(&self, url: &str) -> Result<Response> {
        let src_tld = match CDN_TLDS.iter().find(|&&tld| url.contains(tld)) {
            Some(&tld) => tld,
            None => {
                return Ok(self
                    .cdn_client
                    .get(url)
                    .header("Referer", REFERER)
                    .send()
                    .await?);
            }
        };

        let node_id = Self::extract_node_id(url).unwrap_or("unknown").to_string();

        let client = &self.cdn_client;
        let mut tasks = tokio::task::JoinSet::new();

        for &tld in CDN_TLDS {
            let candidate = url.replace(src_tld, tld);
            let client = client.clone();
            let tld = tld.to_string();
            tasks.spawn(async move {
                let resp = client
                    .get(&candidate)
                    .header("Referer", REFERER)
                    .send()
                    .await;
                (tld, resp)
            });
        }

        let mut errors: Vec<(String, String)> = Vec::new();

        while let Some(join_result) = tasks.join_next().await {
            let (tld, result) = match join_result {
                Ok(r) => r,
                Err(_) => continue,
            };
            match result {
                Ok(resp) if resp.status().is_success() => {
                    tasks.abort_all();
                    let preferred = self.preferred_tld_by_node.lock().get(&node_id).cloned();
                    if preferred.as_deref() != Some(tld.as_str()) {
                        tracing::debug!(
                            "CDN [{}] {} -> {}",
                            node_id,
                            preferred.as_deref().unwrap_or(src_tld),
                            tld
                        );
                        self.preferred_tld_by_node.lock().insert(node_id, tld);
                    }
                    return Ok(resp);
                }
                Ok(resp) => {
                    errors.push((tld, format!("HTTP {}", resp.status())));
                }
                Err(e) => {
                    errors.push((tld, e.to_string()));
                }
            }
        }

        for (tld, err) in &errors {
            tracing::error!("CDN [{}] {}", tld, err);
        }
        Err(AppError::Other(format!("All CDN TLDs failed → {}", url)))
    }

    /// 查询主播在 groupShow 状态时的具体秀类型，通过 v2/models/{model_id}/cam 接口获取。
    /// 返回 show.mode，以及 ticket/perMinute 子类型（仅 groupShow mode 时有值）。
    ///
    /// Query the specific show type when a streamer is in groupShow status,
    /// via the v2/models/{model_id}/cam endpoint.
    /// Returns the show.mode and ticket/perMinute subtype (only present in groupShow mode).
    async fn get_group_show_detail(&self, username: &str, model_id: i64) -> Option<String> {
        let json = self.fetch_cam_json(username, model_id).await?;
        let show = &json["cam"]["show"];
        if show.is_null() || !show.is_object() {
            return None;
        }
        let mode = show["mode"].as_str()?;
        if mode == "groupShow" {
            let subtype = show["details"]["groupShow"]["type"].as_str().unwrap_or("").to_string();
            return Some(format!("groupShow:{}", subtype));
        }
        Some(mode.to_string())
    }

    /// 从 v2/models/{model_id}/cam 接口获取主播的离线预览图 URL。
    /// 仅在主播离线且 v1/broadcasts 没有 previewUrl 时调用。
    ///
    /// Fetch the offline preview image URL from the v2/models/{model_id}/cam endpoint.
    /// Only called when the streamer is offline and v1/broadcasts has no previewUrl.
    async fn get_cam_preview_url(&self, username: &str, model_id: i64) -> Option<String> {
        let json = self.fetch_cam_json(username, model_id).await?;
        json["user"]["user"]["previewUrl"]
            .as_str()
            .map(|s| s.to_string())
    }

    /// 请求 v2/models/{model_id}/cam 接口，返回解析后的 JSON。
    /// 供 get_group_show_detail 和 get_cam_preview_url 共用，避免重复请求逻辑。
    ///
    /// 端点已由 Stripchat 从按用户名查询（v2/models/username/{username}/cam）改为
    /// 按内部 ID 查询（v2/models/{model_id}/cam）——`username` 参数仅用于日志和
    /// Referer 头，不再出现在请求路径中。
    ///
    /// Fetch and parse the v2/models/{model_id}/cam endpoint JSON.
    /// Shared by get_group_show_detail and get_cam_preview_url to avoid duplicate request logic.
    ///
    /// Stripchat changed this endpoint from username-based lookup
    /// (v2/models/username/{username}/cam) to internal-ID-based lookup
    /// (v2/models/{model_id}/cam) — the `username` parameter is only used for logging
    /// and the Referer header, no longer appearing in the request path.
    async fn fetch_cam_json(&self, username: &str, model_id: i64) -> Option<serde_json::Value> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v2/models/{}/cam",
            model_id
        ));
        let resp = self
            .api_client
            .get(&url)
            .header("Referer", format!("{}{}", self.referer(), username))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json().await.ok()
    }

    /// 通过缓存的 model_id 反查主播当前的真实用户名（用于改名检测）。
    ///
    /// 当 v1/broadcasts/{旧用户名} 查询不到该主播时，说明用户名可能已变更（Stripchat
    /// 允许主播随时修改显示用户名，但内部 model_id 保持不变）。用之前缓存的
    /// model_id 请求 v2/models/{model_id}/cam，若返回数据中的 `user.user.username`
    /// 与旧用户名不同，即可确认是改名而非账号被删除/封禁，返回新用户名。
    ///
    /// Look up a streamer's current real username via a cached model_id (for rename
    /// detection).
    ///
    /// When v1/broadcasts/{old_username} can't find the streamer, the username may have
    /// changed (Stripchat lets streamers change their display username at any time,
    /// but the internal model_id stays fixed). Using the previously cached model_id to
    /// query v2/models/{model_id}/cam, if the returned `user.user.username` differs
    /// from the old username, this confirms a rename (rather than the account being
    /// deleted/banned) and returns the new username.
    pub async fn lookup_username_by_model_id(&self, model_id: i64) -> Option<String> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v2/models/{}/cam",
            model_id
        ));
        let resp = self.api_client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        json["user"]["user"]["username"]
            .as_str()
            .map(|s| s.to_string())
    }

    /// 验证主播用户名是否存在，仅发一次轻量请求，不解析直播状态。
    /// 专用于添加主播时的用户名校验，避免触发 groupShow 二次请求等额外开销。
    /// 成功时返回该主播的 model_id（来自响应的 `modelId` 字段），供添加流程
    /// 一并持久化，供日后改名反查使用。
    ///
    /// Verify whether a streamer username exists with a single lightweight request,
    /// without parsing any live status. Intended for username validation on add,
    /// avoiding the extra groupShow secondary request overhead. On success, returns the
    /// streamer's model_id (from the response's `modelId` field) for the add flow to
    /// persist alongside, for later rename lookups.
    pub async fn verify_user_exists(&self, username: &str) -> Result<Option<i64>> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v1/broadcasts/{}",
            username
        ));

        let resp = self
            .api_client
            .get(&url)
            .header("Referer", format!("{}{}", self.referer(), username))
            .send()
            .await?;

        let json = Self::parse_broadcast_response(resp, username).await?;

        Ok(json["item"]["modelId"].as_i64())
    }
    ///
    /// 主接口使用 v1/broadcasts/{username}，该接口轻量且无需登录。
    /// - 在线且状态为 groupShow 时，追加请求 v2/models/{model_id}/cam 获取具体秀类型。
    /// - 仅 public 状态时才获取播放列表 URL。
    /// - 若 v1/broadcasts/{username} 查不到该用户，且提供了 `known_model_id`
    ///   （之前缓存的该主播内部 ID），会用它反查 v2/models/{model_id}/cam 确认是否
    ///   为改名（而非账号被删/封禁）；确认改名后自动改用新用户名重新走一次完整查询，
    ///   返回结果的 `renamed_to` 字段会带上新用户名，供调用方更新持久化记录。
    ///
    /// # 参数 / Parameters
    /// - `username`: 主播用户名 / Streamer username
    /// - `fetch_playlist`: 是否同时获取 HLS 播放列表 URL（仅在可录制时有效）/ Whether to also fetch the HLS playlist URL (only effective when recordable)
    /// - `known_model_id`: 调用方缓存的该主播内部 ID，用于用户名查询失败时的改名反查兜底
    ///   （可为 `None`，此时查询失败直接返回 `UserNotFound`，不做改名检测）/
    ///   Caller-cached internal ID for this streamer, used as a fallback rename lookup
    ///   when the username query fails (`None` means no fallback; a failed lookup
    ///   returns `UserNotFound` directly without rename detection)
    pub async fn get_stream_info(
        &self,
        username: &str,
        fetch_playlist: bool,
        known_model_id: Option<i64>,
    ) -> Result<StreamInfo> {
        match self.fetch_stream_info_by_username(username, fetch_playlist).await {
            Ok(info) => Ok(info),
            Err(AppError::UserNotFound(_)) if known_model_id.is_some() => {
                let model_id = known_model_id.unwrap();
                match self.lookup_username_by_model_id(model_id).await {
                    Some(new_username) if new_username.to_lowercase() != username.to_lowercase() => {
                        tracing::info!(
                            "Streamer renamed detected: {} -> {} (model_id={})",
                            username, new_username, model_id
                        );
                        let mut info = self
                            .fetch_stream_info_by_username(&new_username, fetch_playlist)
                            .await?;
                        info.renamed_to = Some(new_username);
                        Ok(info)
                    }
                    // model_id 反查也找不到用户名（None），或反查到的用户名与旧用户名相同
                    // （说明不是改名问题，可能是账号被封禁/删除等其他原因导致 v1/broadcasts
                    // 查不到），原样返回最初的 UserNotFound，不掩盖真实错误原因。
                    //
                    // The model_id lookup also failed to find a username (None), or found
                    // the same username as before (meaning this isn't a rename — likely
                    // the account being banned/deleted or another reason v1/broadcasts
                    // can't find it). Return the original UserNotFound as-is, without
                    // masking the real cause.
                    _ => Err(AppError::UserNotFound(format!("用户 {} 不存在", username))),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// `get_stream_info` 的实际实现：按用户名查询一次，不含改名回退逻辑。
    /// 拆出为独立函数，便于改名确认后用新用户名重新调用一次完整查询。
    ///
    /// The actual implementation behind `get_stream_info`: a single username-based
    /// query, without rename fallback logic. Split out as its own function so it can be
    /// re-invoked with the new username once a rename is confirmed.
    async fn fetch_stream_info_by_username(
        &self,
        username: &str,
        fetch_playlist: bool,
    ) -> Result<StreamInfo> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v1/broadcasts/{}",
            username
        ));

        let resp = self
            .api_client
            .get(&url)
            .header("Referer", format!("{}{}", self.referer(), username))
            .send()
            .await?;

        let json = Self::parse_broadcast_response(resp, username).await?;

        let item = &json["item"];

        let is_live = item["isLive"].as_bool().unwrap_or(false);
        let status_text = item["status"].as_str().unwrap_or("unknown");
        let model_id = item["modelId"].as_i64();

        // groupShow 时，二次请求 cam 接口获取具体秀类型（ticket / perMinute / private / p2pVoice 等）
        // For groupShow, make a secondary request to cam endpoint to get the specific show type
        let status = if is_live && status_text == "groupShow" {
            match model_id {
                Some(mid) => match self.get_group_show_detail(username, mid).await {
                    Some(ref detail) if detail.starts_with("groupShow:") => {
                        match detail.strip_prefix("groupShow:").unwrap_or("") {
                            "ticket" => "票务秀".to_string(),
                            "perMinute" => "计时秀".to_string(),
                            _ => "群组秀".to_string(),
                        }
                    }
                    Some(ref mode) => match mode.as_str() {
                        "private" => "私密秀".to_string(),
                        "p2pVoice" | "p2p" => "P2P".to_string(),
                        "virtualPrivate" => "虚拟私密".to_string(),
                        _ => "群组秀".to_string(),
                    },
                    None => "群组秀".to_string(),
                },
                None => "群组秀".to_string(),
            }
        } else {
            match status_text {
                "public" => "公开秀".to_string(),
                "private" => "私密秀".to_string(),
                "virtualPrivate" => "虚拟私密".to_string(),
                "p2p" | "p2pVoice" => "P2P".to_string(),
                "idle" => "等待".to_string(),
                "off" => "离线".to_string(),
                _ => status_text.to_string(),
            }
        };

        let thumbnail_url = if is_live {
            let snapshot_ts = item["snapshotTimestamp"]
                .as_i64()
                .or_else(|| {
                    item["snapshotTimestamp"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(0);
            let stream_name = item["streamName"].as_str().unwrap_or("");
            if snapshot_ts > 0 && !stream_name.is_empty() {
                Some(format!(
                    "https://img.doppiocdn.net/thumbs/{}/{}",
                    snapshot_ts, stream_name
                ))
            } else {
                item["previewUrl"].as_str().map(|s| s.to_string())
            }
        } else {
            // v1/broadcasts 离线数据不含 previewUrl，回退到 cam 接口获取
            // v1/broadcasts offline data has no previewUrl; fall back to cam endpoint
            match model_id {
                Some(mid) => self.get_cam_preview_url(username, mid).await,
                None => None,
            }
        };

        let is_recordable = is_live && status_text == "public";

        // 构建一个最小化的 model_json 供 get_playlist_url 使用（仅需 user.user.id）
        // Build a minimal model_json for get_playlist_url (only needs user.user.id)
        let playlist_url = if is_recordable && fetch_playlist {
            if let Some(mid) = model_id {
                let model_json = serde_json::json!({ "user": { "user": { "id": mid } } });
                self.get_playlist_url(username, &model_json).await.ok()
            } else {
                None
            }
        } else {
            None
        };

        Ok(StreamInfo {
            is_online: is_live,
            is_recordable,
            status,
            thumbnail_url,
            playlist_url,
            model_id,
            renamed_to: None,
        })
    }

    /// 对所有 CDN TLD 竞速请求 `_auto.m3u8` master playlist，返回最先成功的响应文本。
    /// Race all CDN TLDs for the `_auto.m3u8` master playlist and return the first successful response text.
    async fn fetch_auto_playlist(&self, model_id: i64) -> Result<String> {
        let client = &self.cdn_client;
        let mut tasks = tokio::task::JoinSet::new();

        for &tld in CDN_TLDS {
            // 使用固定路径模板：edge-hls.{tld}/hls/{model_id}/master/{model_id}_auto.m3u8
            let url = format!(
                "https://edge-hls.{}/hls/{}/master/{}_auto.m3u8",
                tld, model_id, model_id
            );
            let client = client.clone();
            tasks.spawn(async move {
                let resp = client.get(&url).header("Referer", REFERER).send().await;
                (tld, url, resp)
            });
        }

        let mut errors: Vec<(String, String)> = Vec::new();

        while let Some(join_result) = tasks.join_next().await {
            let (tld, url, result) = match join_result {
                Ok(r) => r,
                Err(_) => continue,
            };
            match result {
                Ok(resp) if resp.status().is_success() => {
                    tasks.abort_all();
                    tracing::debug!("auto.m3u8 via CDN TLD: {}", tld);
                    return Ok(resp.text().await?);
                }
                Ok(resp) => {
                    errors.push((url, format!("HTTP {}", resp.status())));
                }
                Err(e) => {
                    errors.push((url, e.to_string()));
                }
            }
        }

        for (url, err) in &errors {
            tracing::error!("auto.m3u8 fetch failed [{}]: {}", url, err);
        }
        Err(AppError::Other(format!(
            "All CDN TLDs failed for model {} _auto.m3u8",
            model_id
        )))
    }

    /// 获取主播的 HLS 播放列表 URL。
    /// 直接对所有 CDN TLD 竞速请求 `{model_id}_auto.m3u8`，解析最高清晰度流。
    /// 若 playlist 包含 Mouflon 加密参数，则按用户配置的 Mouflon Keys 顺序逐一比对，
    /// 取第一个匹配的 pkey 对应的 psch 拼入 URL。
    ///
    /// Get the HLS playlist URL for a streamer.
    /// Races all CDN TLDs for `{model_id}_auto.m3u8` and picks the highest-quality stream.
    /// If the playlist contains Mouflon encryption parameters, iterates through the user-configured
    /// Mouflon Keys in order and uses the first matching pkey's psch in the URL.
    async fn get_playlist_url(
        &self,
        username: &str,
        model_json: &serde_json::Value,
    ) -> Result<String> {
        let model_id = model_json["user"]["user"]["id"]
            .as_i64()
            .ok_or_else(|| AppError::Other("Cannot get model ID".to_string()))?;

        let playlist_text = self.fetch_auto_playlist(model_id).await?;

        let parsed = crate::recording::hls::parse_master_playlist(&playlist_text);

        let (url, mouflon_pairs) =
            parsed.ok_or_else(|| AppError::StreamOffline(username.to_string()))?;

        // 若存在 Mouflon 加密参数，则遍历用户配置的 keys，取第一个匹配的
        // If Mouflon encryption parameters exist, iterate user-configured keys and use the first match
        let final_url = if mouflon_pairs.is_empty() {
            url
        } else {
            // 按 mouflon_pairs 顺序遍历，找到第一个在用户 keys 中存在的 pkey
            // Iterate mouflon_pairs in order, find the first pkey present in user keys
            let matched = mouflon_pairs
                .iter()
                .find(|(_, pkey)| self.mouflon_keys.contains_key(pkey.as_str()));

            match matched {
                Some((psch, pkey)) => {
                    let sep = if url.contains('?') { "&" } else { "?" };
                    format!("{}{}psch={}&pkey={}", url, sep, psch, pkey)
                }
                None => {
                    // 没有匹配的 key，回退到第一个 pair（无解密密钥）
                    // No matching key found, fall back to the first pair (no decryption key)
                    let (psch, pkey) = &mouflon_pairs[0];
                    let sep = if url.contains('?') { "&" } else { "?" };
                    format!("{}{}psch={}&pkey={}", url, sep, psch, pkey)
                }
            }
        };

        tracing::info!("Using the URL: {}", final_url);

        Ok(final_url)
    }

    /// 下载 HLS 播放列表文本内容。
    /// Download the HLS playlist text content.
    pub async fn fetch_playlist(&self, playlist_url: &str) -> Result<String> {
        let resp = self.cdn_get(playlist_url).await?;
        Ok(resp.text().await?)
    }

    /// 下载单个 HLS 分片的字节数据。
    /// Download the byte data of a single HLS segment.
    pub async fn download_segment(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.cdn_get(url).await?;
        Ok(resp.bytes().await?.to_vec())
    }
}
