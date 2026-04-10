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
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// 后处理任务状态快照（序列化后发送给前端）。
/// Post-processing task status snapshot (serialized and sent to the frontend).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpTaskStatus {
    /// 视频文件路径 / Video file path
    pub path: String,
    /// 整体进度百分比（0.0 - 100.0）/ Overall progress percentage (0.0 - 100.0)
    pub pct: f64,
    /// 当前模块已完成进度值 / Current module done progress value
    pub mod_done: u32,
    /// 当前模块总进度值 / Current module total progress value
    pub mod_total: u32,
    /// 当前模块名称 / Current module name
    pub module_name: String,
    /// 已完成的节点数 / Number of completed nodes
    pub done: usize,
    /// 总节点数 / Total number of nodes
    pub total: usize,
    /// 任务状态字符串（"waiting" / "running" / "done" / "error"）/ Task status string
    pub status: String,
    /// 是否来自内存（true = 运行中任务，false = 持久化结果）/ Whether from memory (true = in-progress, false = persisted result)
    pub from_memory: bool,
}

/// 用户可配置的录制器设置 / User-configurable recorder settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// 录制文件输出目录 / Recording output directory
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
    /// 最大并发录制数（0 = 不限制）/ Max concurrent recordings (0 = unlimited)
    pub max_concurrent: usize,
    /// 录制片段合并格式（默认 "mp4"）/ Recording segment merge format (default "mp4")
    #[serde(default = "default_merge_format")]
    pub merge_format: String,
    /// 界面语言（"zh-CN" 或 "en-US"）/ UI language ("zh-CN" or "en-US")
    #[serde(default = "default_language")]
    pub language: String,
    /// 运行模式（"desktop" 或 "server"）/ Run mode ("desktop" or "server")
    #[serde(default = "default_run_mode")]
    pub run_mode: String,
    /// Server 模式监听端口 / Server mode listen port
    #[serde(default = "default_server_port")]
    pub server_port: u16,
}

/// 合并格式的默认值 / Default value for merge format
fn default_merge_format() -> String {
    "mp4".to_string()
}

/// 语言的默认值 / Default value for language
fn default_language() -> String {
    "zh-CN".to_string()
}

/// 运行模式的默认值 / Default value for run mode
fn default_run_mode() -> String {
    String::new()
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
        // 默认输出目录为可执行文件同目录下的 recordings 文件夹
        // Default output directory is the recordings folder next to the executable
        let output_dir = exe_dir().join("recordings").to_string_lossy().to_string();

        Self {
            output_dir,
            poll_interval_secs: 30,
            auto_record: true,
            api_proxy_url: None,
            cdn_proxy_url: None,
            sc_mirror_url: None,
            max_concurrent: 0,
            merge_format: default_merge_format(),
            language: default_language(),
            run_mode: default_run_mode(),
            server_port: default_server_port(),
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
    /// Mouflon HLS 解密密钥（pkey -> pdkey）/ Mouflon HLS decryption keys (pkey -> pdkey)
    #[serde(default)]
    pub mouflon_keys: HashMap<String, String>,
    /// 后处理流水线配置 / Post-processing pipeline configuration
    #[serde(default)]
    pub pipeline: PipelineConfig,
    /// 后处理结果记录（文件路径 -> 是否成功）/ Post-processing results (file path -> success)
    #[serde(default)]
    pub pp_results: HashMap<String, bool>,
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
}

/// 视频时长缓存的键：(文件路径, 文件大小, 修改时间戳)
/// Cache key for video duration: (file path, file size, modification timestamp)
type DurationCacheKey = (String, u64, u64);

/// 应用运行时全局状态，通过 `Arc<AppState>` 在各模块间共享。
/// Global application runtime state, shared across modules via `Arc<AppState>`.
pub struct AppState {
    /// 持久化数据（受读写锁保护）/ Persisted data (protected by read-write lock)
    pub data: RwLock<AppData>,
    /// 配置目录路径（exe_dir/config/）/ Config directory path (exe_dir/config/)
    config_dir: PathBuf,
    /// 后处理任务状态表（文件路径 -> 任务状态）/ Post-processing task status map (file path -> status)
    pub pp_tasks: RwLock<HashMap<String, PpTaskStatus>>,
    /// 后处理取消标志（文件路径 -> 原子布尔）/ Post-processing cancel flags (file path -> atomic bool)
    pub pp_cancel_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
    /// 视频时长缓存，避免重复调用 ffprobe / Video duration cache to avoid repeated ffprobe calls
    pub duration_cache: RwLock<HashMap<DurationCacheKey, Option<u64>>>,
    /// 后处理串行锁，确保同一时刻只有一个后处理任务运行 / Serial lock ensuring only one post-processing task runs at a time
    pub pp_lock: std::sync::Mutex<()>,
    /// 启动合并锁，防止启动时的合并与正常录制并发 / Startup merge lock preventing concurrent startup merge and normal recording
    pub startup_lock: std::sync::Mutex<()>,
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
        let mouflon_keys: HashMap<String, String> = load_json("mouflon_keys.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let pipeline: PipelineConfig = load_json("pipeline.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let pp_results: HashMap<String, bool> = load_json("pp_results.json")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        let data = AppData { settings, streamers, mouflon_keys, pipeline, pp_results };

        fs::create_dir_all(&data.settings.output_dir)?;

        Ok(Arc::new(Self {
            data: RwLock::new(data),
            config_dir,
            pp_tasks: RwLock::new(HashMap::new()),
            pp_cancel_flags: RwLock::new(HashMap::new()),
            duration_cache: RwLock::new(HashMap::new()),
            pp_lock: std::sync::Mutex::new(()),
            startup_lock: std::sync::Mutex::new(()),
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
        fs::write(dir.join("pp_results.json"), serde_json::to_string_pretty(&data.pp_results)?)?;
        Ok(())
    }

    /// 获取当前设置的克隆副本。
    /// Get a cloned copy of the current settings.
    pub fn get_settings(&self) -> Settings {
        self.data.read().settings.clone()
    }

    /// 更新设置并保存到磁盘，同时确保新输出目录存在。
    /// Update settings and save to disk, also ensuring the new output directory exists.
    pub fn update_settings(&self, settings: Settings) -> Result<()> {
        fs::create_dir_all(&settings.output_dir)?;
        self.data.write().settings = settings;
        self.save()
    }

    /// 获取所有追踪主播的克隆列表。
    /// Get a cloned list of all tracked streamers.
    pub fn get_streamers(&self) -> Vec<StreamerData> {
        self.data.read().streamers.clone()
    }

    /// 添加新主播到追踪列表（若已存在则返回错误）。
    /// Add a new streamer to the tracking list (returns error if already exists).
    pub fn add_streamer(&self, username: &str) -> Result<()> {
        let mut data = self.data.write();
        if data.streamers.iter().any(|s| s.username == username) {
            return Err(AppError::Other(format!("模特 {} 已存在", username)));
        }
        let auto_record = data.settings.auto_record;
        data.streamers.push(StreamerData {
            username: username.to_string(),
            auto_record,
            added_at: chrono::Utc::now().to_rfc3339(),
        });
        drop(data);
        self.save()
    }

    /// 从追踪列表中移除主播并保存。
    /// Remove a streamer from the tracking list and save.
    pub fn remove_streamer(&self, username: &str) -> Result<()> {
        let mut data = self.data.write();
        data.streamers.retain(|s| s.username != username);
        drop(data);
        self.save()
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

    /// 获取所有 Mouflon 解密密钥的克隆副本。
    /// Get a cloned copy of all Mouflon decryption keys.
    pub fn get_mouflon_keys(&self) -> HashMap<String, String> {
        self.data.read().mouflon_keys.clone()
    }

    /// 添加或更新一个 Mouflon 密钥对并保存。
    /// Add or update a Mouflon key pair and save.
    pub fn add_mouflon_key(&self, pkey: &str, pdkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys
            .insert(pkey.to_string(), pdkey.to_string());
        drop(data);
        self.save()
    }

    /// 删除指定 pkey 的 Mouflon 密钥并保存。
    /// Remove the Mouflon key with the given pkey and save.
    pub fn remove_mouflon_key(&self, pkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.remove(pkey);
        drop(data);
        self.save()
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

    /// 将指定文件路径的后处理任务加入等待队列。
    /// Enqueue a post-processing task for the given file path.
    pub fn pp_task_enqueue(&self, path: &str) {
        self.pp_tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total: 0,
                status: "waiting".to_string(),
                from_memory: true,
            },
        );
        // 确保取消标志存在（若已存在则不覆盖）/ Ensure cancel flag exists (don't overwrite if already present)
        self.pp_cancel_flags
            .write()
            .entry(path.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
    }

    /// 将指定文件路径的后处理任务标记为运行中。
    /// Mark the post-processing task for the given file path as running.
    pub fn pp_task_start(&self, path: &str, total: usize) {
        self.pp_tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total,
                status: "running".to_string(),
                from_memory: true,
            },
        );
    }

    /// 获取或创建指定文件路径的取消标志。
    /// Get or create the cancel flag for the given file path.
    pub fn pp_task_make_cancel_flag(&self, path: &str) -> Arc<AtomicBool> {
        let mut flags = self.pp_cancel_flags.write();
        if let Some(existing) = flags.get(path) {
            return Arc::clone(existing);
        }
        let flag = Arc::new(AtomicBool::new(false));
        flags.insert(path.to_string(), Arc::clone(&flag));
        flag
    }

    /// 设置指定文件路径的取消标志为 true，请求中止后处理。
    /// Set the cancel flag for the given file path to true, requesting post-processing abort.
    pub fn pp_task_cancel(&self, path: &str) {
        if let Some(flag) = self.pp_cancel_flags.read().get(path) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 清除指定文件路径的取消标志（任务完成后调用）。
    /// Clear the cancel flag for the given file path (called after task completes).
    pub fn pp_task_clear_cancel_flag(&self, path: &str) {
        self.pp_cancel_flags.write().remove(path);
    }

    /// 更新指定文件路径的后处理进度信息。
    /// Update the post-processing progress for the given file path.
    pub fn pp_task_progress(
        &self,
        path: &str,
        pct: f64,
        mod_done: u32,
        mod_total: u32,
        module_name: &str,
        done: usize,
        total: usize,
    ) {
        if let Some(t) = self.pp_tasks.write().get_mut(path) {
            t.pct = pct;
            t.mod_done = mod_done;
            t.mod_total = mod_total;
            t.module_name = module_name.to_string();
            t.done = done;
            t.total = total;
        }
    }

    /// 将后处理任务标记为完成或失败，并将结果持久化到 pp_results.json。
    /// Mark the post-processing task as done or failed, and persist the result to pp_results.json.
    pub fn pp_task_finish(&self, path: &str, success: bool) {
        if let Some(t) = self.pp_tasks.write().get_mut(path) {
            t.status = if success { "done" } else { "error" }.to_string();
            t.pct = if success { 100.0 } else { t.pct };
        }
        self.data
            .write()
            .pp_results
            .insert(path.to_string(), success);
        let _ = self.save();
    }

    /// 获取所有后处理任务状态的列表，合并内存中的运行时状态和持久化的历史结果。
    /// Get a list of all post-processing task statuses, merging in-memory runtime state with persisted historical results.
    pub fn get_pp_tasks(&self) -> Vec<PpTaskStatus> {
        let mut tasks: HashMap<String, PpTaskStatus> = self.pp_tasks.read().clone();

        // 将持久化结果中不在内存任务表里的条目补充进来
        // Add persisted results that are not already in the in-memory task map
        for (path, success) in self.data.read().pp_results.iter() {
            tasks.entry(path.clone()).or_insert_with(|| PpTaskStatus {
                path: path.clone(),
                pct: if *success { 100.0 } else { 0.0 },
                mod_done: 0,
                mod_total: 0,
                module_name: String::new(),
                done: 0,
                total: 0,
                status: if *success { "done" } else { "error" }.to_string(),
                from_memory: false,
            });
        }

        tasks.into_values().collect()
    }
}

/// 执行一次配置检查：验证所有追踪主播是否仍然存在，并检查孤立的后处理记录。
/// 若发现问题，通过 emitter 向前端发送 `startup-warnings` 事件。
///
/// Perform a single config check: verify all tracked streamers still exist,
/// and check for orphaned post-processing records.
/// If issues are found, emit a `startup-warnings` event to the frontend via the emitter.
pub async fn run_config_check(state: &Arc<AppState>, emitter: &Arc<dyn crate::core::emitter::Emitter>) {
    use crate::core::emitter::EmitterExt;
    use crate::core::error::AppError;

    let settings = state.get_settings();
    let streamers = state.get_streamers();

    let api = match crate::streaming::stripchat::StripchatApi::new_api_only(
        settings.api_proxy_url.as_deref(),
        settings.cdn_proxy_url.as_deref(),
        settings.sc_mirror_url.as_deref(),
    ) {
        Ok(a) => a,
        Err(_) => return,
    };

    // 每个主播最多重试 3 次，间隔 10 秒，确认不存在后才加入缺失列表
    // Retry up to 3 times per streamer with 10s delay; only add to missing list after confirmed
    const MAX_ATTEMPTS: u32 = 3;
    const RETRY_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(10);

    let mut missing_streamers = Vec::new();
    for s in &streamers {
        let mut confirmed_missing = false;
        for attempt in 1..=MAX_ATTEMPTS {
            match api.get_stream_info(&s.username, false).await {
                Ok(_) => {
                    confirmed_missing = false;
                    break;
                }
                Err(AppError::UserNotFound(_)) => {
                    confirmed_missing = true;
                    break;
                }
                Err(_) => {
                    if attempt < MAX_ATTEMPTS {
                        tokio::time::sleep(RETRY_DELAY).await;
                    } else {
                        confirmed_missing = true;
                    }
                }
            }
        }
        if confirmed_missing {
            missing_streamers.push(s.username.clone());
        }
    }

    // 查找 pp_results 中对应文件已不存在的孤立记录
    // Find orphaned pp_results entries whose corresponding files no longer exist
    let missing_pp_results: Vec<String> = state
        .data
        .read()
        .pp_results
        .keys()
        .filter(|p| !std::path::Path::new(p).exists())
        .cloned()
        .collect();

    if !missing_streamers.is_empty() || !missing_pp_results.is_empty() {
        emitter.emit(
            "startup-warnings",
            &serde_json::json!({
                "missing_streamers": missing_streamers,
                "missing_pp_results": missing_pp_results,
            }),
        );
    }
}

/// 启动配置检查调度器：立即执行一次检查，之后每天午夜执行一次。
/// Start the config check scheduler: run once immediately, then once every day at midnight.
pub async fn schedule_config_checks(state: Arc<AppState>, emitter: Arc<dyn crate::core::emitter::Emitter>) {
    run_config_check(&state, &emitter).await;

    loop {
        // 计算到下一个午夜的等待秒数 / Calculate seconds until next midnight
        let now = chrono::Local::now();
        let secs_until = {
            let tomorrow = now.date_naive().succ_opt().unwrap_or(now.date_naive());
            let midnight = tomorrow.and_hms_opt(0, 0, 0).unwrap();
            let midnight_local = midnight
                .and_local_timezone(chrono::Local)
                .single()
                .unwrap_or_else(|| now + chrono::Duration::hours(24));
            (midnight_local - now).num_seconds().max(0) as u64
        };
        tokio::time::sleep(tokio::time::Duration::from_secs(secs_until)).await;
        run_config_check(&state, &emitter).await;
    }
}
