//! 应用配置与全局状态管理 / Application Configuration and Global State Management
//!
//! 定义 `Settings`（用户配置）、`AppData`（持久化数据）和 `AppState`（运行时状态）。
//! `AppState` 通过 `parking_lot::RwLock` 保护共享数据，并提供后处理任务状态跟踪。
//!
//! Defines `Settings` (user configuration), `AppData` (persisted data), and `AppState` (runtime state).
//! `AppState` protects shared data with `parking_lot::RwLock` and provides post-processing task state tracking.
//!
//! # 子模块 / Submodules
//! - [`model`]：持久化数据模型（Settings/AppData/StreamerData/MouflonKeysStore）/ persisted data models
//! - [`io`]：路径定位与配置文件容错读写 / path resolution and tolerant config-file IO
//! - [`streamers`]：主播追踪记录增删改查 / streamer record CRUD
//! - [`mouflon`]：Mouflon 密钥读写与 Worker 同步 / Mouflon key access and Worker sync

use crate::core::error::{AppError, Result};
use crate::postprocess::pipeline::PipelineConfig;
use io::{
	ConfigFile, apply_settings_overrides_in, backup_invalid_config, load_config_file,
	load_config_or_backup,
};
use parking_lot::RwLock;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

mod io;
mod model;
mod mouflon;
mod streamers;

pub use io::{exe_dir, set_exe_dir_override, strip_utf8_bom};
pub use model::{AppData, MouflonKeysStore, Settings, StreamerData};

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
	/// 社区模块安装任务状态表（模块 ID → 已下载字节数）
	/// 安装开始时写入，完成/失败时删除；前端重连 SSE 后可通过接口查询恢复状态。
	/// Community module install tasks (module ID → downloaded bytes).
	/// Written on install start, removed on complete/fail;
	/// frontend can query on SSE reconnect to restore in-progress state.
	pub install_tasks: parking_lot::RwLock<std::collections::HashMap<String, u64>>,
	/// 通知监控器 poll_interval_secs 已变更的发送端（可选，启动后注入）
	/// Sender to notify the monitor that poll_interval_secs has changed (optional, injected after startup)
	pub poll_interval_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
	/// 通知 Mouflon 同步调度器立即触发同步的发送端（可选，启动后注入）
	/// Sender to notify the Mouflon sync scheduler to trigger an immediate sync (optional, injected after startup)
	pub mouflon_sync_notify_tx: RwLock<Option<tokio::sync::mpsc::Sender<()>>>,
	/// 进程内通知存储（定时任务写入，前端上线后拉取）
	/// In-process notification store (written by scheduled tasks, pulled by frontend on connect)
	pub notification_store: crate::core::notifications::NotificationStore,
	/// 更新下载/安装进度状态（通过 SSE 广播给前端）
	/// Update download/install progress state (broadcast to frontend via SSE)
	pub update_state: crate::update::UpdateStateStore,
	/// FFmpeg 下载/安装进度状态（通过 SSE `ffmpeg-progress` 广播，状态接口供重连恢复）
	/// FFmpeg download/install progress state (broadcast via SSE `ffmpeg-progress`;
	/// the status endpoint lets reconnecting clients restore it)
	pub ffmpeg_state: crate::ffmpeg::FFmpegStateStore,
	/// 负载自适应后的有效最大并发录制数（由后台负载监控实时更新）。
	/// 0 = 不限制（用户设置为不限制时保持不限制，不受负载约束）。
	///
	/// 当前活跃录制会话数（由 RecorderManager 维护，供 ffmpeg 二进制保护判断繁忙度）。
	/// Current active recording session count (maintained by RecorderManager, read by the ffmpeg binary guard).
	pub active_recording_count: std::sync::atomic::AtomicUsize,
}

impl AppState {
	/// 返回配置目录路径（exe_dir/config/）。
	/// Returns the config directory path (exe_dir/config/).
	pub fn config_dir() -> PathBuf {
		exe_dir().join("config")
	}

	/// 只读取 config/settings.json（去掉 UTF-8 BOM）；文件不存在、为空或解析失败时返回
	/// `Settings::default()`。不创建目录、不初始化信号量、不备份也不记日志（会被频繁调用，
	/// 无效文件的备份和警告由 [`AppState::new`] 在启动时处理一次），适合只需读取个别配置项的场景。
	///
	/// Read only config/settings.json (UTF-8 BOM stripped); returns `Settings::default()` if
	/// the file is missing, blank or fails to parse. Creates no directories, initializes no
	/// semaphores, and neither backs up nor logs (it's called often; [`AppState::new`] handles
	/// the backup and warning for an invalid file once at startup). Suitable when only a single
	/// setting is needed.
	pub fn load_settings_from_disk() -> Settings {
		let mut settings = match load_config_file(&Self::config_dir().join("settings.json")) {
			ConfigFile::Parsed(s) => s,
			ConfigFile::Missing | ConfigFile::Invalid(_) => Settings::default(),
		};
		settings.sanitize();
		settings
	}

	/// 把启动时的覆盖值（Docker 环境变量 LANGUAGE / PORT，见
	/// `server::startup::apply_docker_env_overrides`）写入 config/settings.json，在
	/// [`AppState::new`] 之前调用。返回语言是否因此改变（调用方据此重新加载日志翻译）。
	///
	/// - 文件不存在或为空：以默认值创建，再写入覆盖值（首次运行时就生成 settings.json）
	/// - 文件存在：只修改覆盖的字段，其他字段原样保留；值没有变化时不写盘
	/// - 文件无效：不处理，记录警告，交给 `AppState::new` 备份后改用默认值
	///
	/// 取代原来 Docker entrypoint 里的 `sed`：sed 在 settings.json 尚不存在时报错退出，程序
	/// 根本不会启动；文件里缺少对应字段时也不会生效。
	///
	/// Write startup overrides (the Docker LANGUAGE / PORT env vars, see
	/// `server::startup::apply_docker_env_overrides`) into config/settings.json; called before
	/// [`AppState::new`]. Returns whether the language changed as a result (the caller reloads
	/// log translations accordingly).
	///
	/// - file missing or blank: create it from defaults, then apply the overrides (so
	///   settings.json exists from the very first run)
	/// - file present: only the overridden fields change, everything else is kept; nothing is
	///   written when the values are unchanged
	/// - file invalid: left alone with a warning; `AppState::new` backs it up and uses defaults
	///
	/// Replaces the former `sed` in the Docker entrypoint: sed failed and exited when
	/// settings.json didn't exist yet, so the program never started; it also had no effect when
	/// the field was missing from the file.
	pub fn apply_settings_overrides(language: Option<&str>, server_port: Option<u16>) -> bool {
		apply_settings_overrides_in(
			&Self::config_dir().join("settings.json"),
			language,
			server_port,
		)
	}

	/// 从磁盘加载配置并初始化应用状态，确保输出目录存在。
	/// Load configuration from disk and initialize application state, ensuring the output directory exists.
	pub fn new() -> Result<Arc<Self>> {
		let config_dir = Self::config_dir();
		fs::create_dir_all(&config_dir)?;
		fs::create_dir_all(crate::recording::meta::meta_dir())?;

		// 从拆分文件加载各部分数据：读取时去掉 UTF-8 BOM，空白文件视为不存在；
		// 无法读取或解析的文件先备份再改用默认值（见 backup_invalid_config）
		// Load each section from split files: the UTF-8 BOM is stripped and blank files count
		// as missing; files that can't be read or parsed are backed up before falling back to
		// defaults (see backup_invalid_config)
		let settings_path = config_dir.join("settings.json");
		let (mut settings, settings_missing): (Settings, bool) =
			match load_config_file(&settings_path) {
				ConfigFile::Parsed(s) => (s, false),
				ConfigFile::Missing => (Settings::default(), true),
				ConfigFile::Invalid(err) => {
					backup_invalid_config(&settings_path, &err);
					(Settings::default(), false)
				}
			};

		// 归一化（如 beta 构建强制 check_prerelease）：让内存值与实际生效行为一致；
		// 若实际改动了值则标记待回写（与首次运行同一处写盘），磁盘文件随启动自愈
		// Sanitize (e.g. force check_prerelease on beta builds) so the in-memory
		// value matches the effective behavior; if it changed anything, flag the
		// file for write-back (same write site as first-run) so the on-disk file
		// self-heals at startup
		let settings_sanitized = settings.sanitize();

		// 分片输出目录旧默认值 ts_fragment 已改名为 fragment：把仍指向旧默认的
		// settings 收敛到新约定，并尝试把磁盘目录一并改名。用户自定义的路径不动；
		// 改名失败时保留旧路径，避免分片写到别处而丢失对既有数据的引用。
		// The old default segment output dir ts_fragment was renamed to fragment:
		// converge settings still pointing at the old default onto the new
		// convention and try to rename the on-disk directory alongside. Custom
		// paths are untouched; on rename failure the old path is kept so
		// segments aren't written elsewhere while existing data becomes stale.
		let old_output_default = exe_dir().join("ts_fragment");
		if std::path::Path::new(settings.output_dir.trim()) == old_output_default {
			let new_output_default = exe_dir().join("fragment");
			if old_output_default.exists()
				&& !new_output_default.exists()
				&& let Err(e) = fs::rename(&old_output_default, &new_output_default)
			{
				tracing::warn!(
					"{}",
					crate::tl!(
						"startup.outputDirRenameFailed",
						old = old_output_default.display(),
						new = new_output_default.display(),
						error = e
					)
				);
			}
			if new_output_default.exists() || !old_output_default.exists() {
				settings.output_dir = new_output_default.to_string_lossy().into_owned();
			}
		}

		// 首次运行（settings.json 不存在或为空）时立即用默认值落盘，保证文件始终存在、可被读写；
		// Server 与 Desktop 都经由此处初始化，两端行为一致。文件无效时不在此覆盖（已备份），
		// 之后第一次保存时再写入。
		//
		// On first run (settings.json absent or blank), persist the defaults immediately so the
		// file always exists and can be read/written; both Server and Desktop initialize through
		// here, so the behavior is identical on both ends. An invalid file isn't overwritten here
		// (it has been backed up); the first later save writes it.
		//
		// 归一化改动（sanitize 返回 true）也走同一写盘路径：启动即把生效值落盘，
		// 配置文件不需要等下一次保存才与行为一致。
		// Sanitize-driven changes (sanitize returned true) share this write path:
		// the effective value is persisted at startup so the config file agrees
		// with behavior without waiting for the next save.
		if settings_missing || settings_sanitized {
			let write_result = serde_json::to_string_pretty(&settings)
				.map_err(|e| e.to_string())
				.and_then(|json| fs::write(&settings_path, json).map_err(|e| e.to_string()));
			if let Err(e) = write_result {
				// 两端都在 AppState::new 之前调用 startup::init_logging_and_locale 加载了日志翻译
				// Both ends load log translations via startup::init_logging_and_locale before
				// AppState::new
				tracing::warn!(
					"{}",
					crate::tl!(
						"startup.settingsCreateFailed",
						path = settings_path.display(),
						error = e
					)
				);
			}
		}

		let streamers: Vec<StreamerData> =
			load_config_or_backup(&config_dir.join("streamers.json")).unwrap_or_default();
		let mouflon_keys: MouflonKeysStore =
			load_config_or_backup(&config_dir.join("mouflon_keys.json")).unwrap_or_default();
		let pipeline_path = config_dir.join("pipeline.json");
		let mut pipeline: PipelineConfig = match load_config_file(&pipeline_path) {
			ConfigFile::Parsed(p) => p,
			ConfigFile::Invalid(err) => {
				backup_invalid_config(&pipeline_path, &err);
				PipelineConfig::default()
			}
			// pipeline.json 不存在或为空时（首次启动），注入默认 segment_merge 节点
			// On first startup (pipeline.json absent or blank), inject default segment_merge node
			ConfigFile::Missing => {
				let mut p = PipelineConfig::default();
				// segment_merge 的 output_dir 默认指向程序所在目录下的 recordings 文件夹，
				// 与录制分片输出目录（fragment）分离，便于区分原始分片和合并后视频。
				// segment_merge output_dir defaults to the recordings folder next to the
				// executable, separate from the segment output dir (fragment), making it
				// easy to distinguish raw segments from merged videos.
				let default_output_dir = exe_dir().join("recordings").to_string_lossy().to_string();
				p.nodes.push(crate::postprocess::pipeline::PipelineNode {
					node_id: None,
					module_id: crate::postprocess::pipeline::SEGMENT_MERGE_MODULE_ID.to_string(),
					params: {
						let mut m = std::collections::HashMap::new();
						m.insert("format".to_string(), serde_json::json!("mkv"));
						m.insert(
							"output_dir".to_string(),
							serde_json::json!(default_output_dir),
						);
						m.insert("split_by_streamer".to_string(), serde_json::json!(true));
						m
					},
					enabled: true,
					position: None,
					inputs: {
						let mut m = std::collections::HashMap::new();
						m.insert(
							0,
							crate::postprocess::pipeline::NodeInputRef {
								node_id: "0".to_string(),
								port: 0,
							},
						);
						m
					},
				});
				p
			}
		};

		// 旧配置的 segment_merge 节点 output_dir 可能为空——空值在模块侧的语义是
		// "写入分片目录的父目录"，与新约定（分片目录同级的 recordings 子目录，见
		// update_settings 的联动逻辑）不符。为空的节点统一填上默认推导路径。
		// segment_merge nodes in old configs may carry an empty output_dir—an
		// empty value means "the session dir's parent" on the module side, which
		// differs from the new convention (a sibling 'recordings' dir of the
		// segment dir, see the sync logic in update_settings). Fill empty nodes
		// with the default derived path.
		let default_merge_dir = PathBuf::from(&settings.output_dir)
			.parent()
			.map(|p| p.join("recordings").to_string_lossy().into_owned());
		if let Some(ref default_dir) = default_merge_dir {
			for node in &mut pipeline.nodes {
				if node.module_id != crate::postprocess::pipeline::SEGMENT_MERGE_MODULE_ID {
					continue;
				}
				let empty = node
					.params
					.get("output_dir")
					.and_then(|v| v.as_str())
					.map(|s| s.trim().is_empty())
					.unwrap_or(true);
				if empty {
					node.params
						.insert("output_dir".to_string(), serde_json::json!(default_dir));
				}
			}
		}
		let data = AppData {
			settings,
			streamers,
			mouflon_keys,
			pipeline,
		};

		fs::create_dir_all(&data.settings.output_dir)?;

		let state = Arc::new(Self {
			data: RwLock::new(data),
			config_dir,
			pp_queue: crate::postprocess::queue::PpQueue::new(),
			install_tasks: parking_lot::RwLock::new(std::collections::HashMap::new()),
			poll_interval_notify_tx: RwLock::new(None),
			mouflon_sync_notify_tx: RwLock::new(None),
			notification_store: crate::core::notifications::NotificationStore::new(),
			update_state: crate::update::new_update_state(),
			ffmpeg_state: crate::ffmpeg::new_ffmpeg_state(),
			active_recording_count: std::sync::atomic::AtomicUsize::new(0),
		});

		// 同步 FFmpeg 位置首选项镜像：ffmpeg 解析逻辑逐次读取它，
		// 启动时播种 + update_settings 时更新即可保证"下一次调用立即生效"
		// Seed the FFmpeg location preference mirror: resolution reads it per
		// call — seeding here plus updating in update_settings guarantees a new
		// choice takes effect on the very next invocation
		crate::ffmpeg::set_preference(state.data.read().settings.ffmpeg_preference);
		// 同步 setup_done 镜像：auth 中间件据此决定是否放行全部 API
		// Seed the setup_done mirror: the auth middleware uses it to decide
		// whether all API routes pass
		crate::server::auth::set_setup_done(state.data.read().settings.setup_done);

		// 根据设置中的后处理并发数初始化信号量
		// Initialize the semaphore from the max_pp_concurrent setting
		let init_concurrency = state.data.read().settings.max_pp_concurrent;
		state.pp_queue.set_concurrency(init_concurrency);

		Ok(state)
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
		fs::write(
			dir.join("settings.json"),
			serde_json::to_string_pretty(&data.settings)?,
		)?;
		fs::write(
			dir.join("streamers.json"),
			serde_json::to_string_pretty(&data.streamers)?,
		)?;
		fs::write(
			dir.join("mouflon_keys.json"),
			serde_json::to_string_pretty(&data.mouflon_keys)?,
		)?;
		fs::write(
			dir.join("pipeline.json"),
			serde_json::to_string_pretty(&data.pipeline)?,
		)?;
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
	pub fn update_settings(&self, mut settings: Settings) -> Result<()> {
		// 归一化入参：客户端回传的旧值（如 beta 上的 check_prerelease=false）
		// 在这里收敛为实际生效值，写盘后配置文件与行为一致，自愈
		// Sanitize the input: stale values posted by clients (e.g.
		// check_prerelease=false on beta) are converged to the effective
		// value here, so the persisted file matches actual behavior
		settings.sanitize();
		fs::create_dir_all(&settings.output_dir)?;
		let old = self.data.read().settings.clone();
		let poll_interval_changed = old.poll_interval_secs != settings.poll_interval_secs;
		let mouflon_sync_changed = old.mouflon_sync_url != settings.mouflon_sync_url
			|| old.mouflon_sync_token != settings.mouflon_sync_token;
		let pp_concurrent_changed = old.max_pp_concurrent != settings.max_pp_concurrent;
		let new_pp_concurrent = settings.max_pp_concurrent;
		let output_dir_changed = old.output_dir != settings.output_dir;
		let new_output_dir = settings.output_dir.clone();
		let old_output_dir = old.output_dir.clone();
		let new_ffmpeg_preference = settings.ffmpeg_preference;
		let new_setup_done = settings.setup_done;
		self.data.write().settings = settings;
		// 同步 FFmpeg 位置首选项镜像——下一次 ffmpeg 调用即用新选择
		// Sync the FFmpeg preference mirror — the next ffmpeg call uses it
		crate::ffmpeg::set_preference(new_ffmpeg_preference);
		// 同步 setup_done 镜像——向导完成后立即恢复鉴权（以及反向重置时立即放行）
		// Sync the setup_done mirror — auth resumes as soon as the wizard
		// completes (and releases immediately when it is reset)
		crate::server::auth::set_setup_done(new_setup_done);
		self.save()?;

		// 若分片输出目录发生变化，检查 segment_merge 节点的合并输出目录是否遵循
		// "与分片目录同级的 recordings 子目录"的默认关系；若是则联动更新，
		// 若用户已手动配置为其他路径则保持不变。
		//
		// If the segment output directory changed, check whether the segment_merge
		// node's output_dir follows the default convention of "a 'recordings'
		// sibling of the segment dir"; if so, update it to match the new
		// location. If the user configured a custom path, leave it unchanged.
		if output_dir_changed {
			let old_default = std::path::PathBuf::from(&old_output_dir)
				.parent()
				.map(|p| p.join("recordings"));
			let mut pipeline = self.data.read().pipeline.clone();
			let mut pipeline_changed = false;
			for node in &mut pipeline.nodes {
				if node.module_id != crate::postprocess::pipeline::SEGMENT_MERGE_MODULE_ID {
					continue;
				}
				let current_ts_out = node
					.params
					.get("output_dir")
					.and_then(|v| v.as_str())
					.map(|s| std::path::PathBuf::from(s.trim()));
				// 仅当当前值等于旧的默认推导路径时才联动更新
				// Only update when the current value equals the old default-derived path
				if let (Some(old_def), Some(current)) = (&old_default, &current_ts_out)
					&& current == old_def
				{
					let new_default = std::path::PathBuf::from(&new_output_dir)
						.parent()
						.map(|p| p.join("recordings"))
						.unwrap_or_else(|| std::path::PathBuf::from(&new_output_dir));
					node.params.insert(
						"output_dir".to_string(),
						serde_json::json!(new_default.to_string_lossy()),
					);
					pipeline_changed = true;
					tracing::info!(
						"{}",
						crate::tl!(
							"scheduler.mergeOutputDirSynced",
							old = old_def.display(),
							new = new_default.display()
						)
					);
				}
			}
			if pipeline_changed {
				self.data.write().pipeline = pipeline;
				self.save()?;
			}
		}

		if poll_interval_changed && let Some(tx) = self.poll_interval_notify_tx.read().as_ref() {
			let _ = tx.try_send(());
		}
		if mouflon_sync_changed && let Some(tx) = self.mouflon_sync_notify_tx.read().as_ref() {
			let _ = tx.try_send(());
		}
		if pp_concurrent_changed {
			self.pp_queue.set_concurrency(new_pp_concurrent);
		}
		Ok(())
	}

	/// 获取当前流水线配置的克隆副本。
	/// Get a cloned copy of the current pipeline configuration.
	pub fn get_pipeline(&self) -> crate::postprocess::pipeline::PipelineConfig {
		self.data.read().pipeline.clone()
	}

	/// 更新流水线配置并保存到磁盘。
	/// Update the pipeline configuration and save to disk.
	pub fn update_pipeline(
		&self,
		pipeline: crate::postprocess::pipeline::PipelineConfig,
	) -> Result<()> {
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
	/// 设置密码即退出无密码模式（`admin_password_disabled` 复位）。
	///
	/// Hash the given plaintext password and persist it. Uses Argon2id default
	/// params. Setting a password also exits no-password mode
	/// (`admin_password_disabled` resets to false).
	pub fn set_admin_password(&self, password: &str) -> Result<()> {
		use argon2::{Argon2, password_hash::PasswordHasher};
		let hash = Argon2::default()
			.hash_password(password.as_bytes())
			.map_err(|e| AppError::Other(format!("密码哈希失败: {e}")))?
			.to_string();
		let mut data = self.data.write();
		data.settings.admin_password_hash = Some(hash);
		data.settings.admin_password_disabled = false;
		drop(data);
		self.save()
	}

	/// 清除管理员密码并标记无密码模式（`admin_password_disabled = true`）——
	/// 之后鉴权中间件整体放行，前端路由直接进首页而非登录页。
	///
	/// Clear the admin password and flag no-password mode
	/// (`admin_password_disabled = true`) — the auth middleware then passes
	/// every request and the frontend router goes straight to home instead
	/// of the login page.
	pub fn clear_admin_password(&self) -> Result<()> {
		let mut data = self.data.write();
		data.settings.admin_password_hash = None;
		data.settings.admin_password_disabled = true;
		drop(data);
		self.save()
	}

	/// 返回用户是否显式选择了无密码模式。
	/// Returns whether the user explicitly opted into no-password mode.
	pub fn admin_password_disabled(&self) -> bool {
		self.data.read().settings.admin_password_disabled
	}

	/// 验证明文密码是否与存储的哈希匹配。
	/// 未设置密码时返回 false。
	///
	/// Verify a plaintext password against the stored hash.
	/// Returns false if no password has been set.
	pub fn verify_admin_password(&self, password: &str) -> bool {
		use argon2::{
			Argon2,
			password_hash::{PasswordVerifier, phc::PasswordHash},
		};
		let hash_str = match self.data.read().settings.admin_password_hash.clone() {
			Some(h) => h,
			None => return false,
		};
		let parsed = match PasswordHash::new(&hash_str) {
			Ok(p) => p,
			Err(_) => return false,
		};
		Argon2::default()
			.verify_password(password.as_bytes(), &parsed)
			.is_ok()
	}
}
