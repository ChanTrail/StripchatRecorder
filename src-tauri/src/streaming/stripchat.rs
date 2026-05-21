//! Stripchat API 客户端 / Stripchat API Client
//!
//! 封装对 Stripchat 前端 API 的访问，包括：
//! - 获取主播直播状态和播放列表 URL
//! - 下载 HLS 分片（支持多 CDN 竞速）
//! - 解析 Mouflon 加密的播放列表
//!
//! Wraps access to the Stripchat frontend API, including:
//! - Fetching streamer live status and playlist URLs
//! - Downloading HLS segments (with multi-CDN racing)
//! - Parsing Mouflon-encrypted playlists

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
    "doppiocdn.org",
    "doppiocdn.live",
    "doppiocdn.net",
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
    /// 观看人数 / Viewer count
    pub viewers: i64,
    /// 直播间状态文字（中文）/ Stream status text (Chinese)
    pub status: String,
    /// 缩略图 URL / Thumbnail URL
    pub thumbnail_url: Option<String>,
    /// HLS 播放列表 URL（仅在 fetch_playlist=true 且可录制时有值）/ HLS playlist URL (only when fetch_playlist=true and recordable)
    pub playlist_url: Option<String>,
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
        preferred_tld_by_node: Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
    ) -> Result<Self> {
        Ok(Self {
            api_client: build_api_client(api_proxy)?,
            cdn_client: build_client(cdn_proxy)?,
            sc_mirror: sc_mirror.filter(|s| !s.is_empty()).map(|s| s.to_string()),
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
    ) -> Result<Self> {
        Self::new(
            api_proxy,
            cdn_proxy,
            sc_mirror,
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

    /// 将 stripchat.com 域名替换为镜像站域名（若已配置）。
    /// Replace the stripchat.com domain with the mirror site domain (if configured).
    fn api_url(&self, url: &str) -> String {
        match &self.sc_mirror {
            Some(mirror) => url.replace("stripchat.com", mirror),
            None => url.to_string(),
        }
    }

    /// 返回适配镜像站的 Referer 头值。
    /// Return the Referer header value adapted for the mirror site.
    fn referer(&self) -> String {
        match &self.sc_mirror {
            Some(mirror) => REFERER.replace("stripchat.com", mirror),
            None => REFERER.to_string(),
        }
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

    /// 查询主播是否处于群组秀状态，并返回群组秀类型（ticket / perMinute）。
    /// Query whether a streamer is in a group show and return the group show type (ticket / perMinute).
    async fn get_group_show_type(&self, username: &str) -> Option<String> {
        const LIMIT: usize = 60;
        let mut offset = 0usize;
        loop {
            let url = self.api_url(&format!(
                "https://stripchat.com/api/front/models?removeShows=false&recInFeatured=false&limit={}&offset={}&primaryTag=girls&filterGroupTags=[[\"groupShow\"]]",
                LIMIT, offset
            ));
            let Ok(resp) = self
                .api_client
                .get(&url)
                .header("Referer", self.referer())
                .send()
                .await
            else {
                return None;
            };
            let Ok(json) = resp.json::<serde_json::Value>().await else {
                return None;
            };
            let models = json["models"].as_array()?;
            for m in models.iter() {
                if m["username"].as_str() == Some(username) {
                    return m["groupShowType"].as_str().map(|s| s.to_string());
                }
            }
            if models.len() < LIMIT {
                return None;
            }
            offset += LIMIT;
        }
    }

    /// 获取主播的直播状态信息。
    ///
    /// # 参数 / Parameters
    /// - `username`: 主播用户名 / Streamer username
    /// - `fetch_playlist`: 是否同时获取 HLS 播放列表 URL（仅在可录制时有效）/ Whether to also fetch the HLS playlist URL (only effective when recordable)
    pub async fn get_stream_info(
        &self,
        username: &str,
        fetch_playlist: bool,
    ) -> Result<StreamInfo> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v2/models/username/{}/cam",
            username
        ));

        let resp = self
            .api_client
            .get(&url)
            .header("Referer", format!("{}{}", self.referer(), username))
            .send()
            .await?;

        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(AppError::UserNotFound(format!("用户 {} 不存在", username)));
            }
            return Err(AppError::Other(format!(
                "API 返回 {} ({})",
                resp.status().as_u16(),
                username
            )));
        }

        let json: serde_json::Value = resp.json().await?;
        let user = &json["user"]["user"];
        let cam = &json["cam"];

        let is_live = user["isLive"].as_bool().unwrap_or(false);
        let viewers = user["viewersCount"].as_i64().unwrap_or(0);
        let status_text = user["status"].as_str().unwrap_or("unknown");

        let group_show_type = if status_text == "groupShow" {
            self.get_group_show_type(username).await
        } else {
            None
        };

        let status = match status_text {
            "public" => "公开秀".to_string(),
            "private" => "私密秀".to_string(),
            "groupShow" => match group_show_type.as_deref() {
                Some("ticket") => "票务秀".to_string(),
                Some("perMinute") => "计时秀".to_string(),
                _ => "群组秀".to_string(),
            },
            "virtualPrivate" => "虚拟私密".to_string(),
            "p2p" => "P2P".to_string(),
            "idle" => "等待".to_string(),
            "off" => "离线".to_string(),
            _ => status_text.to_string(),
        };

        let thumbnail_url = if is_live {
            let snapshot_ts = user["snapshotTimestamp"]
                .as_i64()
                .or_else(|| {
                    user["snapshotTimestamp"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(0);
            let stream_name = cam["streamName"].as_str().unwrap_or("");
            if snapshot_ts > 0 && !stream_name.is_empty() {
                Some(format!(
                    "https://img.doppiocdn.net/thumbs/{}/{}",
                    snapshot_ts, stream_name
                ))
            } else {
                user["previewUrl"].as_str().map(|s| s.to_string())
            }
        } else {
            user["previewUrl"].as_str().map(|s| s.to_string())
        };

        let is_recordable = is_live && status_text == "public";
        let playlist_url = if is_recordable && fetch_playlist {
            self.get_playlist_url(username, &json).await.ok()
        } else {
            None
        };

        Ok(StreamInfo {
            is_online: is_live,
            is_recordable,
            viewers,
            status,
            thumbnail_url,
            playlist_url,
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

    /// 从 master playlist 文本中解析出 BANDWIDTH 最高的流 URL，以及所有 Mouflon PSCH 参数对。
    /// Parse the stream URL with the highest BANDWIDTH from the master playlist text,
    /// along with all Mouflon PSCH parameter pairs.
    fn parse_best_stream(playlist: &str) -> Option<(String, Vec<(String, String)>)> {
        // 先把 \r\n 统一成 \n，再按 \n 分割
        let normalized = playlist.replace("\r\n", "\n").replace('\r', "\n");
        let lines: Vec<&str> = normalized.split('\n').map(|l| l.trim()).collect();

        // 收集所有 Mouflon PSCH 参数对 (psch, pkey)
        let mut mouflon_pairs: Vec<(String, String)> = Vec::new();
        for &line in &lines {
            if let Some(rest) = line.strip_prefix("#EXT-X-MOUFLON:PSCH:")
                && let Some((scheme, key)) = rest.split_once(':') {
                mouflon_pairs.push((scheme.to_string(), key.to_string()));
            }
        }

        // 解析 BANDWIDTH 最高的流
        let mut best_bandwidth: u64 = 0;
        let mut best_url: Option<String> = None;
        let mut pending_bandwidth: Option<u64> = None;

        for &line in &lines {
            if let Some(attrs) = line.strip_prefix("#EXT-X-STREAM-INF:") {
                // 去掉标签前缀后再按逗号分割，避免标签名干扰 BANDWIDTH= 匹配
                pending_bandwidth = attrs
                    .split(',')
                    .find(|seg| seg.trim_start().starts_with("BANDWIDTH="))
                    .and_then(|seg| seg.trim_start().strip_prefix("BANDWIDTH="))
                    .and_then(|v| v.parse::<u64>().ok());
            } else if !line.is_empty() && !line.starts_with('#') {
                if let Some(bw) = pending_bandwidth.take()
                    && bw > best_bandwidth {
                    best_bandwidth = bw;
                    best_url = Some(line.to_string());
                }
            } else {
                pending_bandwidth = None;
            }
        }

        best_url.map(|url| (url, mouflon_pairs))
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

        let parsed = Self::parse_best_stream(&playlist_text);

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
