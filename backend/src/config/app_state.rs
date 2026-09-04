//! 应用配置与全局状态管理 / Application Configuration and Global State Management
//!
//! 定义 `Settings`（用户配置）、`AppData`（持久化数据）和 `AppState`（运行时状态）。
//! `AppState` 通过 `parking_lot::RwLock` 保护共享数据，并提供后处理任务状态跟踪。
//!
//! Defines `Settings` (user configuration), `AppData` (persisted data), and `AppState` (runtime state).
//! `AppState` protects shared data with `parking_lot::RwLock` and provides post-processing task state tracking.

use crate::core::error::{AppError, Result};
use crate::postprocess::pipeline::PipelineConfig;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Mouflon 密钥存储结构，持久化到 mouflon_keys.json。
/// Mouflon key store, persisted to mouflon_keys.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MouflonKeysStore {
    /// pkey -> pdkey 密钥对 / pkey -> pdkey key pairs
    #[serde(default)]
    pub keys: HashMap<String, String>,
    /// 数据源（Worker）的密钥更新时间（RFC 3339），同步时与 Worker 返回的 updated_at 比对，相同则跳过写入。
    /// Key update timestamp from the data source (Worker, RFC 3339).
    /// Compared against the Worker's `updated_at`; skip write if equal.
    #[serde(default)]
    pub auto_synced_at: Option<String>,
    /// 最近一次手动添加/删除密钥的时间（RFC 3339）。
    /// Timestamp of the last manual key add/remove (RFC 3339).
    #[serde(default)]
    pub manual_updated_at: Option<String>,
}

/// 用户可配置的录制器设置 / User-configurable recorder settings
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// 最大并发录制数（0 = 不限制）/ Max concurrent recordings (0 = unlimited)
    pub max_concurrent: usize,
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
    /// 首次启动向导是否已完成（false = 显示 Setup 页面）
    /// Whether the first-launch setup wizard has been completed (false = show Setup page)
    #[serde(default)]
    pub setup_done: bool,
    /// 管理员密码的 Argon2 哈希（PHC 格式）。None 表示尚未设置密码（初次使用）。
    /// Argon2 hash of the admin password (PHC string format). None = password not yet set (first run).
    #[serde(default)]
    pub admin_password_hash: Option<String>,
}

/// Mouflon 同步地址的默认值 / Default value for Mouflon sync URL
fn default_mouflon_sync_url() -> Option<String> {
    Some("https://mouflon.chantrail.com".to_string())
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

/// 返回可执行文件所在目录，用于定位配置文件和模块目录。
/// Returns the directory containing the executable, used to locate config files and module directories.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

impl Default for Settings {
    fn default() -> Self {
        // 默认输出目录为可执行文件同目录下的 recordings 文件夹（存放 TS 分片流）
        // Default output directory is the recordings folder next to the executable (for TS segment streams)
        let output_dir = exe_dir().join("recordings").to_string_lossy().to_string();

        Self {
            output_dir,
            poll_interval_secs: 30,
            auto_record: true,
            api_proxy_url: None,
            cdn_proxy_url: None,
            sc_mirror_url: None,
            sc_mirror_scheme: default_sc_mirror_scheme(),
            max_concurrent: 0,
            max_tmp_dir_gb: default_max_tmp_dir_gb(),
            language: default_language(),
            server_port: default_server_port(),
            mouflon_sync_url: default_mouflon_sync_url(),
            mouflon_sync_token: None,
            setup_done: false,
            admin_password_hash: None,
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
}

/// 应用运行时全局状态，通过 `Arc<AppState>` 在各模块间共享。
/// Global application runtime state, shared across modules via `Arc<AppState>`.
pub struct AppState {
    /// 持久化数据（受读写锁保护）/ Persisted data (protected by read-write lock)
    pub(crate) data: RwLock<AppData>,
    /// 配置目录路径（exe_dir/config/）/ Config directory path (exe_dir/config/)
    config_dir: PathBuf,
    /// 后处理任务队列（状态表 + 取消标志 + 串行锁），详见 `postprocess::queue`
    /// Post-processing task queue (status table + cancel flags + serial lock), see `postprocess::queue`
    pub pp_queue: crate::postprocess::queue::PpQueue,
    /// 启动合并锁，防止启动时的合并与正常录制并发 / Startup merge lock preventing concurrent startup merge and normal recording
    pub startup_lock: std::sync::Mutex<()>,
    /// 通知监控器 poll_interval_secs 已变更的发送端（可选，启动后注入）
    /// Sender to notify the monitor that poll_interval_secs has changed (optional, injected after startup)
    pub poll_interval_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
    /// 通知 Mouflon 同步调度器立即触发同步的发送端（可选，启动后注入）
    /// Sender to notify the Mouflon sync scheduler to trigger an immediate sync (optional, injected after startup)
    pub mouflon_sync_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
    /// 进程内通知存储（定时任务写入，前端上线后拉取）
    /// In-process notification store (written by scheduled tasks, pulled by frontend on connect)
    pub notification_store: crate::core::notifications::NotificationStore,
}

impl AppState {
    /// 返回配置目录路径（exe_dir/config/）。
    /// Returns the config directory path (exe_dir/config/).
    pub fn config_dir() -> PathBuf {
        exe_dir().join("config")
    }

    /// 从磁盘加载配置并初始化应用状态，确保输出目录存在。
    /// Load configuration from disk and initialize application state, ensuring the output directory exists.
    pub fn new() -> Result<Arc<Self>> {
        let config_dir = Self::config_dir();
        fs::create_dir_all(&config_dir)?;
        fs::create_dir_all(crate::recording::meta::meta_dir())?;

        // 从拆分文件加载各部分数据 / Load each section from split files
        let load_json = |name: &str| -> Option<String> {
            fs::read_to_string(config_dir.join(name)).ok()
        };

        let settings: Settings = load_json("settings.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let streamers: Vec<StreamerData> = load_json("streamers.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mouflon_keys: MouflonKeysStore = load_json("mouflon_keys.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let pipeline: PipelineConfig = {
            let raw = load_json("pipeline.json");
            // pipeline.json 不存在时（首次启动），注入默认 ts_merge 节点
            // On first startup (pipeline.json absent), inject default ts_merge node
            if raw.is_none() {
                let mut p = PipelineConfig::default();
                p.nodes.push(crate::postprocess::pipeline::PipelineNode {
                    node_id: None,
                    module_id: "ts_merge".to_string(),
                    params: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("format".to_string(), serde_json::json!("mp4"));
                        m.insert("split_by_streamer".to_string(), serde_json::json!(true));
                        m
                    },
                    enabled: true,
                    position: None,
                    inputs: {
                        let mut m = std::collections::HashMap::new();
                        m.insert(0, crate::postprocess::pipeline::NodeInputRef {
                            node_id: "0".to_string(),
                            port: 0,
                        });
                        m
                    },
                });
                p
            } else {
                raw.and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            }
        };
        let data = AppData { settings, streamers, mouflon_keys, pipeline };

        fs::create_dir_all(&data.settings.output_dir)?;

        Ok(Arc::new(Self {
            data: RwLock::new(data),
            config_dir,
            pp_queue: crate::postprocess::queue::PpQueue::new(),
            startup_lock: std::sync::Mutex::new(()),
            poll_interval_notify_tx: RwLock::new(None),
            mouflon_sync_notify_tx: RwLock::new(None),
            notification_store: crate::core::notifications::NotificationStore::new(),
        }))
    }

    /// 返回日志目录路径（可执行文件同目录下的 logs 文件夹）。
    /// Returns the log directory path (logs folder next to the executable).
    pub fn log_dir() -> PathBuf {
        exe_dir().join("logs")
    }

    /// 将当前 `AppData` 拆分序列化并分别写入各配置文件。
    /// Serialize the current `AppData` into split config files.
    pub fn save(&self) -> Result<()> {
        let data = self.data.read();
        let dir = &self.config_dir;
        fs::write(dir.join("settings.json"), serde_json::to_string_pretty(&data.settings)?)?;
        fs::write(dir.join("streamers.json"), serde_json::to_string_pretty(&data.streamers)?)?;
        fs::write(dir.join("mouflon_keys.json"), serde_json::to_string_pretty(&data.mouflon_keys)?)?;
        fs::write(dir.join("pipeline.json"), serde_json::to_string_pretty(&data.pipeline)?)?;
        Ok(())
    }

    /// 获取当前设置的克隆副本。
    /// Get a cloned copy of the current settings.
    pub fn get_settings(&self) -> Settings {
        self.data.read().settings.clone()
    }

    /// 更新设置并保存到磁盘，同时确保新输出目录存在。
    /// 若 poll_interval_secs 发生变化，通知监控器立即以新间隔重新计时。
    /// 若 mouflon_sync_url 或 mouflon_sync_token 发生变化，通知同步调度器立即触发一次同步。
    ///
    /// Update settings and save to disk, also ensuring the new output directory exists.
    /// If poll_interval_secs changed, notify the monitor to restart its timer with the new interval.
    /// If mouflon_sync_url or mouflon_sync_token changed, notify the sync scheduler to trigger immediately.
    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        fs::create_dir_all(&settings.output_dir)?;
        let old = self.data.read().settings.clone();
        let poll_interval_changed = old.poll_interval_secs != settings.poll_interval_secs;
        let mouflon_sync_changed = old.mouflon_sync_url != settings.mouflon_sync_url
            || old.mouflon_sync_token != settings.mouflon_sync_token;
        self.data.write().settings = settings;
        self.save()?;
        if poll_interval_changed
            && let Some(tx) = self.poll_interval_notify_tx.read().as_ref() {
            let _ = tx.try_send(());
        }
        if mouflon_sync_changed
            && let Some(tx) = self.mouflon_sync_notify_tx.read().as_ref() {
            let _ = tx.try_send(());
        }
        Ok(())
    }

    /// 获取所有追踪主播的克隆列表。
    /// Get a cloned list of all tracked streamers.
    pub fn get_streamers(&self) -> Vec<StreamerData> {
        self.data.read().streamers.clone()
    }

    /// 添加新主播到追踪列表（若已存在则返回错误）。
    /// `model_id` 通常来自添加时对 v1/broadcasts/{username} 的查询结果（见
    /// `streamer::add_streamer` 路由），用于日后改名反查；查询失败时传 `None`
    /// 也不阻塞添加，仅是后续改名跟随会失效。
    ///
    /// Add a new streamer to the tracking list (returns error if already exists).
    /// `model_id` typically comes from the v1/broadcasts/{username} lookup performed at
    /// add time (see the `streamer::add_streamer` route), used later for rename lookups;
    /// passing `None` when that lookup fails doesn't block adding the streamer, it just
    /// means rename-following won't work for this entry.
    pub fn add_streamer(&self, username: &str, model_id: Option<i64>) -> Result<()> {
        let mut data = self.data.write();
        if data.streamers.iter().any(|s| s.username == username) {
            return Err(AppError::Other(format!("模特 {} 已存在", username)));
        }
        let auto_record = data.settings.auto_record;
        data.streamers.push(StreamerData {
            username: username.to_string(),
            auto_record,
            added_at: chrono::Utc::now().to_rfc3339(),
            model_id,
            is_dead: false,
        });
        drop(data);
        self.save()
    }

    /// 回填指定主播的 model_id（仅当当前为 `None` 时才写入，避免覆盖已有值）。
    /// 用于升级前的旧数据在下一次成功查询时补齐 model_id。
    ///
    /// Backfill the model_id for a streamer (only writes when currently `None`, to
    /// avoid overwriting an existing value). Used to backfill model_id for pre-upgrade
    /// data on its next successful lookup.
    pub fn backfill_model_id(&self, username: &str, model_id: i64) {
        let mut data = self.data.write();
        if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username)
            && s.model_id.is_none()
        {
            s.model_id = Some(model_id);
            drop(data);
            let _ = self.save();
        }
    }

    /// 将主播记录从旧用户名重命名为新用户名（改名跟随），保留 model_id/auto_record/added_at
    /// 不变。若新用户名已存在于列表中（如同时手动添加过），则放弃改名，保留原记录不变，
    /// 避免产生两条指向同一 model_id 的重复记录。
    ///
    /// Rename a streamer record from the old username to the new one (rename-following),
    /// preserving model_id/auto_record/added_at. If the new username already exists in
    /// the list (e.g. manually added separately), the rename is abandoned and the
    /// original record is left unchanged, avoiding two records pointing at the same
    /// model_id.
    pub fn rename_streamer(&self, old_username: &str, new_username: &str) -> Result<()> {
        let mut data = self.data.write();
        if data.streamers.iter().any(|s| s.username == new_username) {
            return Err(AppError::Other(format!(
                "无法改名：{} 已存在于追踪列表中",
                new_username
            )));
        }
        if let Some(s) = data.streamers.iter_mut().find(|s| s.username == old_username) {
            s.username = new_username.to_string();
        }
        drop(data);
        self.save()
    }

    /// 将主播标记为已失效（is_dead = true）并保存。
    /// Mark a streamer as dead (is_dead = true) and save.
    pub fn mark_streamer_dead(&self, username: &str) {
        let mut data = self.data.write();
        if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username)
            && !s.is_dead
        {
            s.is_dead = true;
            drop(data);
            let _ = self.save();
        }
    }

    /// 返回所有已标记为失效的主播用户名集合。
    /// Returns the set of all usernames marked as dead.
    pub fn get_dead_streamers(&self) -> std::collections::HashSet<String> {
        self.data
            .read()
            .streamers
            .iter()
            .filter(|s| s.is_dead)
            .map(|s| s.username.clone())
            .collect()
    }

    /// 从追踪列表中移除主播并保存。
    /// Remove a streamer from the tracking list and save.
    pub fn remove_streamer(&self, username: &str) -> Result<()> {
        let mut data = self.data.write();
        data.streamers.retain(|s| s.username != username);
        drop(data);        self.save()
    }

    /// 设置指定主播的自动录制开关并保存。
    /// Set the auto-record toggle for a specific streamer and save.
    pub fn set_auto_record(&self, username: &str, enabled: bool) -> Result<()> {
        let mut data = self.data.write();
        if let Some(s) = data.streamers.iter_mut().find(|s| s.username == username) {
            s.auto_record = enabled;
        }
        drop(data);
        self.save()
    }

    /// 获取所有 Mouflon 解密密钥的克隆副本（仅 keys 部分，供录制/转发使用）。
    /// Get a cloned copy of all Mouflon decryption keys (keys map only, for recording/relay use).
    pub fn get_mouflon_keys(&self) -> HashMap<String, String> {
        self.data.read().mouflon_keys.keys.clone()
    }

    /// 获取完整的 Mouflon 密钥存储（含时间戳），供前端展示。
    /// Get the full Mouflon key store (including timestamps), for frontend display.
    pub fn get_mouflon_keys_store(&self) -> MouflonKeysStore {
        self.data.read().mouflon_keys.clone()
    }

    /// 添加或更新一个 Mouflon 密钥对，更新 manual_updated_at 并保存。
    /// Add or update a Mouflon key pair, update manual_updated_at, and save.
    pub fn add_mouflon_key(&self, pkey: &str, pdkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.keys.insert(pkey.to_string(), pdkey.to_string());
        data.mouflon_keys.manual_updated_at = Some(chrono::Utc::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 删除指定 pkey 的 Mouflon 密钥，更新 manual_updated_at 并保存。
    /// Remove the Mouflon key with the given pkey, update manual_updated_at, and save.
    pub fn remove_mouflon_key(&self, pkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.keys.remove(pkey);
        data.mouflon_keys.manual_updated_at = Some(chrono::Utc::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 从 Cloudflare Worker 同步 Mouflon 密钥。
    /// 比对 Worker 返回的 updated_at 与本地 auto_synced_at：
    ///   - 相同 → 跳过，返回 false（无需更新）
    ///   - 不同 → 覆盖 keys、更新 auto_synced_at，返回 true（已更新）
    ///
    /// Sync Mouflon keys from the Cloudflare Worker.
    /// Compares Worker's `updated_at` against local `auto_synced_at`:
    ///   - Equal   → skip, return false (no update needed)
    ///   - Different → overwrite keys, update auto_synced_at, return true (updated)
    pub async fn sync_mouflon_keys_from_worker(
        &self,
        worker_url: &str,
        auth_token: Option<&str>,
    ) -> Result<bool> {
        #[derive(Deserialize)]
        struct WorkerResponse {
            keys: HashMap<String, String>,
            updated_at: String,
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| AppError::Other(e.to_string()))?;

        let mut req = client.get(worker_url);
        if let Some(token) = auth_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Other(format!("Worker 请求失败: {}", e)))?;

        if !resp.status().is_success() {
            return Err(AppError::Other(format!(
                "Worker 返回错误状态: {}",
                resp.status()
            )));
        }

        let body: WorkerResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("Worker 响应解析失败: {}", e)))?;

        // 比对 updated_at：解析为时间点后比较，避免格式差异导致误判
        // Compare updated_at by parsing to a time point, avoiding false mismatches due to format differences
        let worker_ts = chrono::DateTime::parse_from_rfc3339(&body.updated_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok();

        let same_timestamp = {
            let data = self.data.read();
            match (&worker_ts, &data.mouflon_keys.auto_synced_at) {
                (Some(wt), Some(local)) => {
                    chrono::DateTime::parse_from_rfc3339(local)
                        .map(|lt| lt.with_timezone(&chrono::Utc) == *wt)
                        .unwrap_or(false)
                }
                _ => false,
            }
        };

        if same_timestamp {
            // 时间戳相同，检查是否有本地缺失的 key / Same timestamp, check for locally missing keys
            let missing: Vec<(String, String)> = {
                let data = self.data.read();
                body.keys
                    .iter()
                    .filter(|(pkey, _)| !data.mouflon_keys.keys.contains_key(pkey.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            };
            if missing.is_empty() {
                return Ok(false);
            }
            // 补充缺失的 key / Insert missing keys
            let mut data = self.data.write();
            for (pkey, pdkey) in missing {
                data.mouflon_keys.keys.insert(pkey, pdkey);
            }
            drop(data);
            self.save()?;
            return Ok(true);
        }

        // 时间戳不同，插入本地不存在的键对，更新 auto_synced_at
        // Different timestamp: insert missing key pairs, update auto_synced_at
        {
            // 将 Worker 返回的时间戳规范化为 chrono RFC 3339 格式，与 manual_updated_at 保持一致
            // Normalize Worker timestamp to chrono RFC 3339 format, consistent with manual_updated_at
            let normalized_at = chrono::DateTime::parse_from_rfc3339(&body.updated_at)
                .map(|dt| dt.with_timezone(&chrono::Utc).to_rfc3339())
                .unwrap_or(body.updated_at);
            let mut data = self.data.write();
            for (pkey, pdkey) in body.keys {
                data.mouflon_keys.keys.entry(pkey).or_insert(pdkey);
            }
            data.mouflon_keys.auto_synced_at = Some(normalized_at);
        }
        self.save()?;
        Ok(true)
    }

    /// 获取当前流水线配置的克隆副本。
    /// Get a cloned copy of the current pipeline configuration.
    pub fn get_pipeline(&self) -> crate::postprocess::pipeline::PipelineConfig {
        self.data.read().pipeline.clone()
    }

    /// 更新流水线配置并保存到磁盘。
    /// Update the pipeline configuration and save to disk.
    pub fn update_pipeline(&self, pipeline: crate::postprocess::pipeline::PipelineConfig) -> Result<()> {
        self.data.write().pipeline = pipeline;
        self.save()
    }

    // ─── 管理员密码 / Admin password ──────────────────────────────────────────

    /// 返回是否已设置管理员密码。
    /// Returns whether an admin password has been configured.
    pub fn has_admin_password(&self) -> bool {
        self.data.read().settings.admin_password_hash.is_some()
    }

    /// 将明文密码哈希后保存。使用 Argon2id 默认参数。
    /// Hash the given plaintext password and persist it. Uses Argon2id default params.
    pub fn set_admin_password(&self, password: &str) -> Result<()> {
        use argon2::{
            Argon2,
            password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
        };
        let salt = SaltString::generate(&mut OsRng);
        let hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AppError::Other(format!("密码哈希失败: {e}")))?
            .to_string();
        self.data.write().settings.admin_password_hash = Some(hash);
        self.save()
    }

    /// 验证明文密码是否与存储的哈希匹配。
    /// 未设置密码时返回 false。
    ///
    /// Verify a plaintext password against the stored hash.
    /// Returns false if no password has been set.
    pub fn verify_admin_password(&self, password: &str) -> bool {
        use argon2::{
            Argon2,
            password_hash::{PasswordHash, PasswordVerifier},
        };
        let hash_str = match self.data.read().settings.admin_password_hash.clone() {
            Some(h) => h,
            None => return false,
        };
        let parsed = match PasswordHash::new(&hash_str) {
            Ok(p) => p,
            Err(_) => return false,
        };
        Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
    }

}
