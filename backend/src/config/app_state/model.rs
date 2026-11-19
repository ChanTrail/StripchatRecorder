//! 持久化数据模型：用户设置、应用数据、主播记录、Mouflon 密钥存储，
//! 以及 schedule 矩阵的紧凑 serde 编解码。
//! Persisted data models: user settings, app data, streamer records, the
//! Mouflon key store, and the compact serde codec for schedule matrices.

use super::io::exe_dir;
use crate::postprocess::pipeline::PipelineConfig;
use crate::recording::hls::{MouflonKeys, parse_keystream_hex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Mouflon 密钥存储结构，持久化到 mouflon_keys.json。
/// Mouflon key store, persisted to mouflon_keys.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MouflonKeysStore {
    /// pkey -> pdkey 密钥对 / pkey -> pdkey key pairs
    #[serde(default)]
    pub keys: HashMap<String, String>,
    /// pkey -> keystream（小写十六进制文本）。没有 pdkey 时用 keystream 直接解密分片 URL，
    /// 同一 pkey 两者都有时优先用 pdkey。
    /// pkey -> keystream (lowercase hex text). Used to decrypt segment URLs directly when there
    /// is no pdkey; when a pkey has both, the pdkey wins.
    #[serde(default)]
    pub keystreams: HashMap<String, String>,
    /// 最近一次成功同步时数据源（Worker）返回的 `updated_at`，即两份密钥表内容的 SHA-256 哈希；
    /// 下次同步时与新返回的值比对，相同说明数据源没有变化。只用于比对，不是时间。
    /// The Worker's `updated_at` from the last successful sync, i.e. a SHA-256 hash of both key
    /// tables' contents; compared with the newly returned value on the next sync, equal means the
    /// source has not changed. Used only for comparison, it is not a time.
    #[serde(default)]
    pub synced_hash: Option<String>,
    /// 最近一次自动同步写入变更的本地时间（RFC 3339）。
    /// Local time (RFC 3339) of the last auto-sync that applied a change.
    #[serde(default)]
    pub auto_synced_at: Option<String>,
    /// 最近一次手动添加/删除密钥的时间（RFC 3339）。
    /// Timestamp of the last manual key add/remove (RFC 3339).
    #[serde(default)]
    pub manual_updated_at: Option<String>,
}

impl MouflonKeysStore {
    /// 转换为解密用的密钥材料；keystream 不是合法十六进制（或太短）的条目会被忽略。
    /// Convert to the key material used for decryption; keystream entries that are not valid hex
    /// (or are too short) are ignored.
    pub fn to_keys(&self) -> MouflonKeys {
        MouflonKeys {
            pdkeys: self.keys.clone(),
            keystreams: self
                .keystreams
                .iter()
                .filter_map(|(pkey, text)| {
                    parse_keystream_hex(text)
                        .ok()
                        .map(|bytes| (pkey.clone(), bytes))
                })
                .collect(),
        }
    }
}

/// 用户可配置的录制器设置 / User-configurable recorder settings
///
/// 容器级 `#[serde(default)]`：settings.json 缺少的字段取 `Settings::default()` 中的值，
/// 因此 `{}` 或只写了部分字段的手动编辑文件也能正常读取，而不是整个文件解析失败。
/// Container-level `#[serde(default)]`: fields missing from settings.json take their value from
/// `Settings::default()`, so `{}` or a hand-edited file with only some fields still loads
/// instead of failing to parse as a whole.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// TS 分片流输出目录 / TS segment stream output directory
    pub output_dir: String,
    /// 主播状态轮询间隔（秒）/ Streamer status poll interval (seconds)
    pub poll_interval_secs: u64,
    /// 是否默认开启自动录制 / Whether auto-record is enabled by default
    pub auto_record: bool,
    /// Stripchat API 代理地址 / Stripchat API proxy URL
    pub api_proxy_url: Option<String>,
    /// CDN 分片下载代理地址 / CDN segment download proxy URL
    pub cdn_proxy_url: Option<String>,
    /// Stripchat 镜像站地址 / Stripchat mirror site URL
    pub sc_mirror_url: Option<String>,
    /// Stripchat 镜像站协议（"https" 或 "http"，默认 "https"）
    /// Stripchat mirror site scheme ("https" or "http", default "https")
    #[serde(default = "default_sc_mirror_scheme")]
    pub sc_mirror_scheme: String,
    /// StripChat 浏览页专用代理地址（与 `api_proxy_url` 相互独立；留空 = 直连，不回退全局）
    /// Dedicated proxy for the StripChat browse page (independent from `api_proxy_url`;
    /// empty = direct connection, no fallback to the global value)
    #[serde(default)]
    pub sc_browse_proxy_url: Option<String>,
    /// StripChat 浏览页专用镜像站（与 `sc_mirror_url` 相互独立；留空 = stripchat.com；
    /// 协议沿用 `sc_mirror_scheme`）
    /// Dedicated mirror site for the StripChat browse page (independent from `sc_mirror_url`;
    /// empty = stripchat.com; scheme follows `sc_mirror_scheme`)
    #[serde(default)]
    pub sc_browse_mirror_url: Option<String>,
    /// 每文件录制时长上限（秒，0 = 不限制）/ Per-file recording duration in seconds (0 = unlimited)
    #[serde(default)]
    pub max_recording_duration_secs: u64,
    /// 首选录制分辨率高度（0 = 原始/最高画质）/ Preferred recording resolution height (0 = original/highest quality)
    #[serde(default)]
    pub preferred_resolution: u32,
    /// 首选分辨率不可用时的回退方向（"lower" 或 "higher"）/ Fallback direction when preferred resolution is unavailable ("lower" or "higher")
    #[serde(default = "default_resolution_preference")]
    pub resolution_preference: String,
    /// 首选视频编码，请求 master playlist 时作为 `preferredVideoCodec` 参数发送
    /// （"h264" / "H265" / "AV1"，不区分大小写；空 = 不带该参数，由服务器决定）。
    /// Preferred video codec, sent as the `preferredVideoCodec` parameter when requesting the
    /// master playlist ("h264" / "H265" / "AV1", case-insensitive; empty = omit the parameter and
    /// let the server decide).
    #[serde(default)]
    pub preferred_codec: String,
    /// 后处理临时目录最大占用（GB，0 = 不限制，默认 50 GB）
    /// Max size of the post-processing tmp directory in GB (0 = unlimited, default 50 GB)
    #[serde(default = "default_max_tmp_dir_gb")]
    pub max_tmp_dir_gb: f64,
    /// 界面语言（"zh-CN" 或 "en-US"）/ UI language ("zh-CN" or "en-US")
    #[serde(default = "default_language")]
    pub language: String,
    /// 监听端口 / Listen port
    #[serde(default = "default_server_port")]
    pub server_port: u16,
    /// Mouflon Keys 同步 Worker URL（为空则不启用自动同步）
    /// Mouflon Keys sync Worker URL (empty = auto-sync disabled)
    #[serde(default = "default_mouflon_sync_url")]
    pub mouflon_sync_url: Option<String>,
    /// Mouflon Keys 同步 Worker 鉴权 Token（对应 Worker 的 AUTH_TOKEN 环境变量）
    /// Mouflon Keys sync Worker auth token (corresponds to Worker's AUTH_TOKEN env var)
    #[serde(default)]
    pub mouflon_sync_token: Option<String>,
    /// 社区模块代理地址（用于下载模块文件，留空不使用）
    /// Community module proxy URL (for downloading module files, empty = disabled)
    #[serde(default)]
    pub community_proxy_url: Option<String>,
    /// 社区模块镜像站地址（在 github.com URL 前加前缀，如 https://mirror.com/https://github.com/...）
    /// Community module mirror URL (prepended to github.com URLs, e.g. https://mirror.com/https://github.com/...)
    #[serde(default)]
    pub community_mirror_url: Option<String>,
    /// 用户是否已接受社区模块使用条款（false = 首次进入时显示警告弹窗）
    /// Whether the user has accepted the community module terms (false = show warning dialog on first visit)
    #[serde(default)]
    pub community_terms_accepted: bool,
    /// 首次启动向导是否已完成（false = 显示 Setup 页面）
    /// Whether the first-launch setup wizard has been completed (false = show Setup page)
    #[serde(default)]
    pub setup_done: bool,
    /// 管理员密码的 Argon2 哈希（PHC 格式）。None 表示尚未设置密码（初次使用）。
    /// Argon2 hash of the admin password (PHC string format). None = password not yet set (first run).
    #[serde(default)]
    pub admin_password_hash: Option<String>,
    /// 后处理最大并发数（0 = 自动 = CPU 逻辑核心数；≥1 = 固定并发数，上限 CPU 逻辑核心数）。
    /// Max concurrent post-processing tasks (0 = auto = logical CPU count; ≥1 = fixed, capped at logical CPU count).
    #[serde(default)]
    pub max_pp_concurrent: usize,
    /// 是否检查预发布（beta）版本更新（true = 同时检查 prerelease；beta 版本强制为 true）。
    /// Whether to check for pre-release (beta) updates (true = include prereleases; forced true on beta builds).
    #[serde(default)]
    pub check_prerelease: bool,
    /// camgirlfinder.net API 代理地址（用于拉取主播 schedule，留空不使用）。
    /// Proxy URL for accessing camgirlfinder.net API (used when fetching streamer schedules, empty = disabled).
    #[serde(default)]
    pub cgf_proxy_url: Option<String>,
    /// 是否在空闲时自动更新下载到 tools 目录的 FFmpeg（默认开启）。
    /// 只作用于本程序下载的副本（带 ffmpeg-build.json 标记）；PATH 中或手动放入
    /// tools 的 FFmpeg 不会被替换。
    /// Whether to auto-update the FFmpeg copy downloaded into tools while idle (on by
    /// default). Only applies to our managed copy (carrying the ffmpeg-build.json
    /// marker); a PATH ffmpeg or one placed in tools manually is never replaced.
    #[serde(default = "default_ffmpeg_auto_update")]
    pub ffmpeg_auto_update: bool,
    /// FFmpeg 首选项：0 = 使用系统环境变量（PATH）中的 FFmpeg，
    /// 1 = 使用程序目录 tools 文件夹中的 FFmpeg。`None` 表示用户尚未选择，
    /// 前端据此弹出首选项向导。
    /// FFmpeg preference: 0 = use the FFmpeg on the system PATH, 1 = use the
    /// FFmpeg in the program's tools folder. `None` means the user has not
    /// chosen yet — the frontend shows the preference wizard in that case.
    #[serde(default)]
    pub ffmpeg_preference: Option<u8>,
    /// 是否启用 ECH（Encrypted Client Hello）：开启后 SC API 的 TLS
    /// ClientHello 经借用 ECH 配置加密（连接钉到 CF 任播 IP），隐藏真实
    /// SNI，可能有效改善 API 返回 403 的情况；主机不可路由或解析失败时
    /// 按无 ECH 通道负缓存（`client_for` 调用方自动回退普通 TLS；机会性
    /// 增强，默认关闭）。CDN 分片流量直取优先以保留 CDN 地理调度，仅
    /// 全部直连失败后在兜底竞速中给源 URL 与每个候选各挂一条 ECH 腿。
    /// Whether ECH (Encrypted Client Hello) is enabled: when on, SC
    /// API ClientHellos are encrypted with a borrowed ECH config
    /// (connections pinned to CF anycast IPs), hiding the real SNI — may
    /// help mitigate HTTP 403 errors from the API; an unroutable host or
    /// failed resolution is negative-cached as no-ECH (`client_for`
    /// callers then fall back to plain TLS — opportunistic upgrade, off
    /// by default). CDN segment traffic prefers the direct try to keep
    /// CDN geo steering; only after every direct attempt fails does the
    /// fallback race carry one ECH leg on the source URL and one per
    /// candidate.
    #[serde(default)]
    pub ech_enabled: bool,
}

/// Mouflon 同步地址的默认值 / Default value for Mouflon sync URL
fn default_mouflon_sync_url() -> Option<String> {
    Some("https://mouflon.chantrail.com".to_string())
}

/// 分辨率回退方向的默认值 / Default resolution fallback direction
fn default_resolution_preference() -> String {
    "lower".to_string()
}

/// 镜像站协议的默认值 / Default value for mirror site scheme
fn default_sc_mirror_scheme() -> String {
    "https".to_string()
}

/// tmp 目录最大占用的默认值（50 GB）/ Default value for max tmp dir size (50 GB)
fn default_max_tmp_dir_gb() -> f64 {
    50.0
}

/// 语言的默认值 / Default value for language
fn default_language() -> String {
    "zh-CN".to_string()
}

/// Server 端口的默认值 / Default value for server port
fn default_server_port() -> u16 {
    3030
}

/// FFmpeg 空闲自动更新的默认值（开启）/ Default value for FFmpeg idle auto-update (on)
fn default_ffmpeg_auto_update() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        // 默认输出目录为可执行文件同目录下的 ts_fragment 文件夹（存放 TS 分片流）
        // Default output directory is the ts_fragment folder next to the executable (for TS segment streams)
        let output_dir = exe_dir().join("ts_fragment").to_string_lossy().to_string();

        Self {
            output_dir,
            poll_interval_secs: 60,
            auto_record: true,
            api_proxy_url: None,
            cdn_proxy_url: None,
            sc_mirror_url: None,
            sc_mirror_scheme: default_sc_mirror_scheme(),
            sc_browse_proxy_url: None,
            sc_browse_mirror_url: None,
            max_recording_duration_secs: 0,
            preferred_resolution: 0,
            resolution_preference: default_resolution_preference(),
            preferred_codec: String::new(),
            max_tmp_dir_gb: default_max_tmp_dir_gb(),
            language: default_language(),
            server_port: default_server_port(),
            mouflon_sync_url: default_mouflon_sync_url(),
            mouflon_sync_token: None,
            community_proxy_url: None,
            community_mirror_url: None,
            community_terms_accepted: false,
            setup_done: false,
            admin_password_hash: None,
            max_pp_concurrent: 0,
            check_prerelease: false,
            cgf_proxy_url: None,
            ffmpeg_auto_update: default_ffmpeg_auto_update(),
            ffmpeg_preference: None,
            ech_enabled: false,
        }
    }
}

/// 持久化到 config/ 目录下各 JSON 文件的全部应用数据 / All application data persisted to JSON files under the config/ directory
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppData {
    /// 用户配置 / User settings
    pub settings: Settings,
    /// 追踪的主播列表 / List of tracked streamers
    pub streamers: Vec<StreamerData>,
    /// Mouflon HLS 解密密钥存储（含密钥对和时间戳）/ Mouflon HLS decryption key store (keys + timestamps)
    #[serde(default)]
    pub mouflon_keys: MouflonKeysStore,
    /// 后处理流水线配置 / Post-processing pipeline configuration
    #[serde(default)]
    pub pipeline: PipelineConfig,
}

/// 单个主播的持久化数据 / Persisted data for a single streamer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerData {
    /// 主播用户名（小写）/ Streamer username (lowercase)
    pub username: String,
    /// 是否开启自动录制 / Whether auto-record is enabled
    pub auto_record: bool,
    /// 添加时间（RFC 3339 格式）/ Time added (RFC 3339 format)
    pub added_at: String,
    /// Stripchat 内部主播 ID（首次添加时从 v1/broadcasts/{username} 的 `modelId`
    /// 字段获取并固定保存）。
    ///
    /// 用途：username 是可被主播修改的显示名，而 modelId 是稳定不变的内部标识。
    /// 当 v1/broadcasts/{username} 查询不到（主播已改名，旧用户名不再解析）时，
    /// 用这个缓存的 model_id 反查 v2/models/{model_id}/cam 接口获取主播当前
    /// 的真实用户名，从而在不丢失追踪记录的情况下跟随改名更新。
    ///
    /// `Option` 是为了兼容升级前已存在、尚未回填 model_id 的旧数据；这类记录在
    /// 下一次成功查询到该主播时会被自动回填（见 `StatusMonitor::poll_streamer`）。
    ///
    /// Stripchat's internal streamer ID (fetched from v1/broadcasts/{username}'s
    /// `modelId` field on first add, and persisted permanently).
    ///
    /// Purpose: username is a display name the streamer can change, while modelId is a
    /// stable, unchanging internal identifier. When v1/broadcasts/{username} can't find
    /// the streamer (renamed, old username no longer resolves), this cached model_id is
    /// used to query v2/models/{model_id}/cam to discover the streamer's current real
    /// username, allowing the tracked record to follow the rename without being lost.
    ///
    /// `Option` accommodates pre-upgrade data that hasn't backfilled model_id yet; such
    /// records get it backfilled automatically the next time the streamer is
    /// successfully queried (see `StatusMonitor::poll_streamer`).
    #[serde(default)]
    pub model_id: Option<i64>,
    /// 是否已确认失效（用户名和内部 ID 均无法找到）。
    /// 失效后跳过轮询请求以节约带宽，前端展示通知提醒用户处理。
    ///
    /// Whether this streamer is confirmed dead (not found by username or model_id).
    /// Dead streamers are skipped in polling to save bandwidth; a notification is shown
    /// in the frontend for the user to take action.
    #[serde(default)]
    pub is_dead: bool,
    /// 来自 camgirlfinder.net 的历史在线规律（7×48 float 矩阵，序列化为 base64 字符串）。
    ///
    /// **内存格式**：`Vec<Vec<f32>>`，外层长度 7（星期，0=周日），内层长度 48（30 分钟时段）。
    /// **存储格式**：336 字节（每格量化为 u8，0–255 对应 0.0–1.0）经 Base64 编码后的字符串，
    /// 比原始 JSON float 数组小约 3 倍（~448 字符 vs ~1.5 KB）。
    ///
    /// Historical online schedule from camgirlfinder.net (7×48 float matrix,
    /// serialized as a base64-encoded string).
    ///
    /// **In-memory**: `Vec<Vec<f32>>`, outer length 7 (weekday, 0=Sunday),
    /// inner length 48 (half-hour buckets).
    /// **On-disk**: 336 bytes (each cell quantized to u8 in [0,255]) encoded as Base64
    /// (~448 chars vs ~1.5 KB for raw JSON floats, ~3× smaller).
    #[serde(default, with = "schedule_serde")]
    pub schedule: Option<Vec<Vec<f32>>>,
    /// 主播离线时的预览图 URL 缓存（来自 v2/models/{model_id}/cam 接口）。
    /// 由状态轮询在每个主播每天（本地时间）第一次轮询成功时顺带更新，与在线状态无关
    /// （失败时下一轮再试，每天最多 3 次）；离线轮询直接使用此缓存，避免每次轮询都向 cam
    /// 接口发起 HTTP 请求。
    ///
    /// Cached offline preview image URL (from v2/models/{model_id}/cam endpoint).
    /// Updated by status polling on each streamer's first successful poll of the (local) day,
    /// regardless of online status (retried the next round on failure, at most 3 times a day);
    /// offline polls use this cache directly, avoiding a cam endpoint HTTP request on every poll.
    #[serde(default)]
    pub cached_preview_url: Option<String>,
}

/// 自定义 serde 模块：将 `Option<Vec<Vec<f32>>>` 序列化为 Base64 字符串以节省空间。
///
/// **编码**：将 7×48 矩阵展平为 336 字节，每个 f32 值量化为 u8（0–255 对应 0.0–1.0），
/// 然后 Base64 编码为约 448 字符的字符串，比原始 JSON float 数组小约 3 倍。
///
/// **解码**：Base64 字符串 → 336 字节 → 反量化为 f32 → 7×48 矩阵。
/// `null` 和缺失字段均解码为 `None`（`#[serde(default)]` 处理缺失字段）。
///
/// Custom serde module: serializes `Option<Vec<Vec<f32>>>` as a Base64 string to save space.
///
/// **Encoding**: flatten 7×48 matrix to 336 bytes, quantize each f32 to u8 (0–255 → 0.0–1.0),
/// then Base64-encode to ~448 chars — ~3× smaller than raw JSON float arrays.
/// **Decoding**: Base64 → 336 bytes → dequantize to f32 → 7×48 matrix.
/// `null` and missing fields both decode to `None`.
mod schedule_serde {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    const ROWS: usize = 7;
    const COLS: usize = 48;
    const BYTES: usize = ROWS * COLS; // 336

    pub fn serialize<S>(value: &Option<Vec<Vec<f32>>>, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            None => ser.serialize_none(),
            Some(matrix) => {
                let mut buf = Vec::with_capacity(BYTES);
                for row in matrix {
                    for &v in row {
                        // 量化：f32 [0.0, 1.0] → u8 [0, 255]
                        // Quantize: f32 [0.0, 1.0] → u8 [0, 255]
                        buf.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
                    }
                }
                STANDARD.encode(&buf).serialize(ser)
            }
        }
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Option<Vec<Vec<f32>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<String> = Option::deserialize(de)?;
        let s = match s {
            None => return Ok(None),
            Some(s) => s,
        };
        let bytes = STANDARD.decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != BYTES {
            return Err(serde::de::Error::custom(format!(
                "schedule base64 decoded to {} bytes, expected {}",
                bytes.len(),
                BYTES
            )));
        }
        let matrix = bytes
            .as_chunks::<COLS>()
            .0
            .iter()
            .map(|row| {
                // 反量化：u8 [0, 255] → f32 [0.0, 1.0]
                // Dequantize: u8 [0, 255] → f32 [0.0, 1.0]
                row.iter().map(|&b| b as f32 / 255.0).collect()
            })
            .collect();
        Ok(Some(matrix))
    }
}
