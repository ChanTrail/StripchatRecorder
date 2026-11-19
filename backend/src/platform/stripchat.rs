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
//!
//! # 子模块 / Submodules
//! - [`net`]：HTTP 客户端构建与 URL/错误工具 / HTTP client construction and URL/error utilities
//! - [`api`]：API 请求通道与 cam/user-ids/浏览列表端点 / API plumbing and cam/user-ids/browse endpoints
//! - [`stream`]：直播状态查询与播放列表获取 / live-status queries and playlist retrieval
//! - [`cdn`]：CDN 直取 + 多 TLD/ECH 兜底竞速下载 / CDN direct fetch + multi-TLD/ECH fallback racing

use crate::core::error::Result;
use crate::recording::hls::MouflonKeys;
use net::{build_api_client, build_client};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

mod api;
mod cdn;
pub(crate) mod net;
mod stream;

/// 请求 Referer 头 / Request Referer header
const REFERER: &str = "https://stripchat.com/";

/// Stripchat 错误响应固定的 title / The fixed title of Stripchat error responses
const API_ERROR_TITLE: &str = "An error occurred";

/// /broadcasts 接口（v1 与 v2）表示"主播已不存在"的 description 取值（title 恒为
/// [`API_ERROR_TITLE`]）：
/// - `Entity "Model" not found`：主播不存在（可能改名，上层会据此走改名/失效流程）
/// - `model already deleted`：主播账号已被删除
///   The description values meaning "the streamer no longer exists" on the /broadcasts
///   endpoints (v1 + v2; title is always [`API_ERROR_TITLE`]):
/// - `Entity "Model" not found`: the streamer does not exist (may have been renamed)
/// - `model already deleted`: the streamer's account has been deleted
const BROADCAST_GONE_DESCRIPTIONS: &[&str] =
    &["Entity \"Model\" not found", "model already deleted"];

/// 判断非 2xx 响应体是否为调用方已知的"主播不存在/已删除"错误：必须是 JSON、title
/// 精确等于 [`API_ERROR_TITLE`]，且 description 不区分大小写地匹配 `accepted_descriptions`
/// 之一（Stripchat 用户名不区分大小写，错误体中回显的用户名大小写可能与请求的不同）。
/// 其余一律返回 false，让调用方当作网络/API 错误，避免 429/5xx 等被误判为主播失效。
/// Whether a non-2xx response body is one of the caller's known "streamer not found /
/// deleted" errors: must be JSON, title exactly [`API_ERROR_TITLE`], and description
/// matching one of `accepted_descriptions` case-insensitively (Stripchat usernames are
/// case-insensitive, so the username echoed in `User {username} not found` may differ
/// in case from the one requested). Anything else returns false so callers treat it as
/// a network/API error and 429/5xx etc. are never misclassified as a dead streamer.
fn is_api_error_body(bytes: &[u8], accepted_descriptions: &[&str]) -> bool {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    if json["title"].as_str() != Some(API_ERROR_TITLE) {
        return false;
    }
    let Some(description) = json["description"].as_str() else {
        return false;
    };
    let description = description.to_lowercase();
    accepted_descriptions
        .iter()
        .any(|a| a.to_lowercase() == description)
}

/// 支持的 CDN 顶级域名列表（用于多 CDN 竞速）/ Supported CDN TLDs (for multi-CDN racing)
pub(crate) const CDN_TLDS: &[&str] = &[
    "doppiocdn.com",
    "doppiocdn.media",
    "doppiocdn.net",
    "doppiocdn.org",
    "doppiocdn.live",
];

/// 可选的首选视频编码，同时是 master playlist 的 `preferredVideoCodec` 参数取值（大小写即请求中的写法）。
/// Selectable preferred video codecs; also the values of the master playlist's `preferredVideoCodec`
/// parameter (the casing here is what goes into the request).
pub const VIDEO_CODECS: &[&str] = &["h264", "H265", "AV1"];

/// 把设置里的编码名规范化为 [`VIDEO_CODECS`] 中的取值（不区分大小写，忽略首尾空白）。
/// 空串或不认识的值返回 `None`（即不带 `preferredVideoCodec` 参数），因此设置文件里的任意
/// 字符串都不会被拼进请求 URL。
///
/// Normalize a codec name from the settings to a value in [`VIDEO_CODECS`] (case-insensitive,
/// surrounding whitespace ignored). An empty or unknown value returns `None` (no
/// `preferredVideoCodec` parameter), so an arbitrary string in the settings file is never
/// spliced into the request URL.
pub fn normalize_video_codec(codec: &str) -> Option<&'static str> {
    let codec = codec.trim();
    VIDEO_CODECS
        .iter()
        .copied()
        .find(|c| c.eq_ignore_ascii_case(codec))
}

/// 主播直播状态信息 / Streamer live status information
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// 是否在线 / Whether online
    pub is_online: bool,
    /// 是否可录制（公开秀状态）/ Whether recordable (public show status)
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
#[derive(Clone)]
pub struct StripchatApi {
    /// API 请求客户端 / API request client
    api_client: Client,
    /// CDN 分片下载客户端 / CDN segment download client
    cdn_client: Client,
    /// ECH 客户端缓存（API 侧；None = 未启用 ECH）
    /// ECH client cache (API side; None = ECH disabled)
    api_ech: Option<crate::ech::EchClients>,
    /// ECH 客户端缓存（CDN 侧；None = 未启用 ECH）——只作 `cdn_get` 兜底
    /// 竞速的腿：原生直取优先以保留 CDN 地理调度，直取失败后的竞速里
    /// 源 URL 与每个候选 URL 各挂一条 ECH 腿
    /// ECH client cache (CDN side; None = ECH disabled) — only a leg in
    /// `cdn_get`'s fallback race: the native try keeps the CDN's geo
    /// steering, and once direct fails the race carries one ECH leg on the
    /// source URL plus one per candidate
    cdn_ech: Option<crate::ech::EchClients>,
    /// 可选的镜像站域名 / Optional mirror site domain
    sc_mirror: Option<String>,
    /// 镜像站协议（"https" 或 "http"）/ Mirror site scheme ("https" or "http")
    sc_mirror_scheme: String,
    /// 各 CDN 节点的首选 TLD 缓存（节点 ID -> TLD）/ Preferred TLD cache per CDN node (node ID -> TLD)
    preferred_tld_by_node: Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
    /// Mouflon 解密密钥（pdkey 与 keystream，按 pkey 索引），用于 playlist URL 匹配与分片 URL 解密
    /// Mouflon decryption keys (pdkeys and keystreams, indexed by pkey) for playlist URL matching and segment URL decryption
    mouflon_keys: MouflonKeys,
    /// 首选分辨率高度（0 = 原始/最高画质）/ Preferred resolution height (0 = original/highest quality)
    preferred_resolution: u32,
    /// 首选分辨率缺失时是否优先向上选择 / Whether to prefer a higher resolution when the target is unavailable
    prefers_higher_resolution: bool,
    /// 首选视频编码（已规范化为 [`VIDEO_CODECS`] 之一；None = 不带 `preferredVideoCodec` 参数）
    /// Preferred video codec (normalized to one of [`VIDEO_CODECS`]; None = omit the `preferredVideoCodec` parameter)
    preferred_codec: Option<&'static str>,
}

/// 服务状态页探测结果：一次计时 GET 的关键回执。
/// Status-page probe result: the key receipts of one timed GET.
#[derive(Debug)]
pub(crate) struct ProbeResult {
    /// HTTP 状态码 / HTTP status code
    pub status: u16,
    /// 请求耗时（发出到拿到响应头）/ Request elapsed (send → response headers)
    pub elapsed: Duration,
    /// 实际请求的 URL（经镜像改写后的有效形态）/ Effective URL actually
    /// requested (post mirror-rewrite)
    pub url: String,
    /// 响应头（提供商判定用，如 server/cf-ray/x-amz-cf-id）/ Response
    /// headers (for provider detection, e.g. server/cf-ray/x-amz-cf-id)
    pub headers: reqwest::header::HeaderMap,
}

/// 探测请求超时（远小于生产客户端的 30s——状态页要的是快速判定）/
/// Probe request timeout (much shorter than the production client's 30s —
/// the status page wants a quick verdict)
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

impl StripchatApi {
    /// 创建完整的 API 客户端（API + CDN，带 CDN TLD 缓存）。
    /// Create a full API client (API + CDN, with CDN TLD cache).
    pub fn new(
        api_proxy: Option<&str>,
        cdn_proxy: Option<&str>,
        sc_mirror: Option<&str>,
        sc_mirror_scheme: Option<&str>,
        preferred_tld_by_node: Arc<parking_lot::Mutex<std::collections::HashMap<String, String>>>,
        ech_enabled: bool,
    ) -> Result<Self> {
        let api_client = build_api_client(api_proxy, None)?;
        let cdn_client = build_client(cdn_proxy, None)?;
        // ECH 客户端集合 API/CDN 各持一份（两者 base 客户端的代理/超时/H2
        // 参数不同；借用配置由进程级槽位共享，不会重复借用）。策略不同：
        // API 侧常态走 ECH；CDN 侧只在 cdn_get 的兜底竞速里作候选——分片
        // 流量大且钉死 CF 任播 IP 会绕开 CDN 地理调度，原生直取必须优先，
        // 仅当全部 TLD 直连失败（如 SNI 阻断）才经 ECH 通道重试源 URL。
        // `base` 客户端同时充当回退与 DoH 传输（已携带对应代理）。
        // One EchClients per side — API and CDN (their base clients differ
        // in proxy/timeout/H2 settings; the borrowed config itself is shared
        // through the process-global slot and never re-borrowed). Policies
        // differ: the API side rides ECH permanently, while the CDN side
        // only enters cdn_get's fallback race as a candidate — segment
        // traffic is heavy and pinning CF anycast IPs would bypass the CDN's
        // geo steering, so the native try stays first and ECH retries the
        // source URL only after every direct TLD attempt failed (e.g. under
        // SNI blocking). `base` doubles as fallback and DoH transport
        // (already proxy-aware).
        let (api_ech, cdn_ech) = if ech_enabled {
            let api_proxy_owned = api_proxy.map(|s| s.to_string());
            let cdn_proxy_owned = cdn_proxy.map(|s| s.to_string());
            (
                Some(crate::ech::EchClients::new(api_client.clone(), move |t| {
                    build_api_client(api_proxy_owned.as_deref(), Some(t))
                })),
                Some(crate::ech::EchClients::new(cdn_client.clone(), move |t| {
                    build_client(cdn_proxy_owned.as_deref(), Some(t))
                })),
            )
        } else {
            (None, None)
        };
        Ok(Self {
            api_client,
            cdn_client,
            api_ech,
            cdn_ech,
            sc_mirror: sc_mirror.filter(|s| !s.is_empty()).map(|s| s.to_string()),
            sc_mirror_scheme: sc_mirror_scheme
                .filter(|s| *s == "http" || *s == "https")
                .unwrap_or("https")
                .to_string(),
            preferred_tld_by_node,
            mouflon_keys: MouflonKeys::default(),
            preferred_resolution: 0,
            prefers_higher_resolution: false,
            preferred_codec: None,
        })
    }

    /// 用 StripChat 浏览页专用网络设置构造 API 客户端：镜像/代理与主 API 设置
    /// 相互独立——`sc_browse_*` 留空时直连 stripchat.com，不回退到全局值；
    /// ECH 与镜像协议仍沿用全局开关。
    ///
    /// Build an API client from the StripChat browse page's dedicated network
    /// settings: mirror/proxy are independent from the main API settings —
    /// empty `sc_browse_*` values mean a direct connection to stripchat.com
    /// (NOT a fallback to the global values); ECH and the mirror scheme still
    /// follow the global settings.
    pub fn new_for_browse(settings: &crate::config::app_state::Settings) -> Result<Self> {
        Self::new_api_only(
            settings.sc_browse_proxy_url.as_deref(),
            None,
            settings.sc_browse_mirror_url.as_deref(),
            Some(settings.sc_mirror_scheme.as_str()),
            settings.ech_enabled,
        )
    }

    /// 创建仅用于 API 请求的客户端（不需要 CDN TLD 缓存，适用于验证用户名等场景）。
    /// Create an API-only client (no CDN TLD cache, suitable for username verification, etc.).
    pub fn new_api_only(
        api_proxy: Option<&str>,
        cdn_proxy: Option<&str>,
        sc_mirror: Option<&str>,
        sc_mirror_scheme: Option<&str>,
        ech_enabled: bool,
    ) -> Result<Self> {
        Self::new(
            api_proxy,
            cdn_proxy,
            sc_mirror,
            sc_mirror_scheme,
            Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
            ech_enabled,
        )
    }

    /// 设置 Mouflon 解密密钥，返回 self 以支持链式调用。
    /// Set Mouflon decryption keys, returns self for method chaining.
    pub fn with_mouflon_keys(mut self, keys: MouflonKeys) -> Self {
        self.mouflon_keys = keys;
        self
    }

    /// 设置首选分辨率和回退方向，返回 self 以支持链式调用。
    /// Set the preferred resolution and fallback direction, returning self for method chaining.
    pub fn with_resolution_selection(
        mut self,
        preferred_resolution: u32,
        resolution_preference: &str,
    ) -> Self {
        self.preferred_resolution = preferred_resolution;
        self.prefers_higher_resolution = resolution_preference == "higher";
        self
    }

    /// 设置首选视频编码（见 [`normalize_video_codec`]，空串或不认识的值表示不指定），
    /// 返回 self 以支持链式调用。
    /// Set the preferred video codec (see [`normalize_video_codec`]; an empty or unknown value
    /// means unspecified), returning self for method chaining.
    pub fn with_codec_selection(mut self, preferred_codec: &str) -> Self {
        self.preferred_codec = normalize_video_codec(preferred_codec);
        self
    }

    /// 获取当前 Mouflon 解密密钥的引用。
    /// Get a reference to the current Mouflon decryption keys.
    pub fn mouflon_keys(&self) -> &MouflonKeys {
        &self.mouflon_keys
    }

    /// 获取当前首选分辨率。/ Get the current preferred resolution.
    pub fn preferred_resolution(&self) -> u32 {
        self.preferred_resolution
    }

    /// 返回是否优先选择更高分辨率。/ Return whether higher resolutions are preferred.
    pub fn prefers_higher_resolution(&self) -> bool {
        self.prefers_higher_resolution
    }

    /// 获取当前首选视频编码（规范化后的写法；未指定返回空串）。
    /// Get the current preferred video codec (normalized spelling; empty string when unspecified).
    pub fn preferred_codec(&self) -> &'static str {
        self.preferred_codec.unwrap_or("")
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

    /// 返回适配镜像站的站点根 Referer（scheme://host/）。
    /// Return the site-root Referer adapted for the mirror site (scheme://host/).
    fn referer_base(&self) -> String {
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

    /// 构造请求特定路径时使用的 Referer 头值（scheme://host + path）。
    /// 例如请求 /api/front/v1/broadcasts/qi33wei 时，Referer 为
    /// https://stripchat.com/api/front/v1/broadcasts/qi33wei。
    ///
    /// Build the Referer header for a specific request path (scheme://host + path).
    /// e.g. when requesting /api/front/v1/broadcasts/qi33wei, Referer is
    /// https://stripchat.com/api/front/v1/broadcasts/qi33wei.
    fn referer_for_path(&self, path: &str) -> String {
        let base = self.referer_base();
        // REFERER 以 "/" 结尾，path 以 "/" 开头时去掉一个 "/"，避免双斜杠
        // REFERER ends with "/"; strip the leading "/" from path to avoid double slashes
        let path = path.trim_start_matches('/');
        format!("{}{}", base, path)
    }
}
