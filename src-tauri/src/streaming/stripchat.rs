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
        .use_native_tls()
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .connection_verbose(false);

    if let Some(proxy) = proxy_url {
        if !proxy.is_empty() {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
        } else {
            builder = builder.no_proxy();
        }
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

    if let Some(proxy) = proxy_url {
        if !proxy.is_empty() {
            builder = builder
                .proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
            return Ok(builder.build()?);
        }
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

    /// 获取主播的 HLS 播放列表 URL（需要先获取 models 列表以确定 HLS 前缀）。
    /// Get the HLS playlist URL for a streamer (requires fetching the models list to determine the HLS prefix).
    async fn get_playlist_url(
        &self,
        username: &str,
        model_json: &serde_json::Value,
    ) -> Result<String> {
        let models_url = self.api_url("https://stripchat.com/api/front/models?primaryTag=girls");
        let resp = self
            .api_client
            .get(models_url)
            .header("Referer", format!("{}{}", self.referer(), username))
            .send()
            .await?;

        let models_json: serde_json::Value = resp.json().await?;
        let ref_hls = models_json["models"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|m| m["hlsPlaylist"].as_str())
            .ok_or_else(|| AppError::Other("Cannot get HLS prefix".to_string()))?;

        let hls_prefix: String = ref_hls.split('/').take(3).collect::<Vec<_>>().join("/");

        let model_id = model_json["user"]["user"]["id"]
            .as_i64()
            .ok_or_else(|| AppError::Other("Cannot get model ID".to_string()))?;

        let master_url = format!("{}/hls/{}/master/{}.m3u8", hls_prefix, model_id, model_id);

        let resp = self.cdn_get(&master_url).await?;
        let playlist = resp.text().await?;
        let mut playlist_url: Option<String> = None;
        let mut psch: Option<String> = None;
        let mut pkey: Option<String> = None;

        for line in playlist.lines() {
            if line.contains("EXT-X-MOUFLON") {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 4 {
                    psch = Some(parts[2].to_string());
                    pkey = Some(parts[3].to_string());
                }
            }
            if !line.is_empty() && !line.starts_with('#') {
                playlist_url = Some(line.to_string());
            }
        }

        let mut url = playlist_url.ok_or_else(|| AppError::StreamOffline(username.to_string()))?;

        if let (Some(psch), Some(pkey)) = (psch, pkey) {
            url = format!("{}?&psch={}&pkey={}", url, psch, pkey);
        }

        Ok(url)
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
