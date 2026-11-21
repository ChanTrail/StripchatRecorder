//! 转发会话状态 / Relay Session State

use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

/// TS 数据预缓冲 ring buffer，保留最近约 N 字节，供新连接立即推送。
/// Pre-buffer ring buffer for TS data; retains the last ~N bytes for immediate push to new connections.
const PREBUFFER_MAX_BYTES: usize = 512 * 1024; // 512 KB ≈ 2–3 秒黑屏或 1–2 秒直播

/// FLV 头保留区：流开头的前 N 字节（FLV 签名 + onMetaData + 编码 sequence
/// headers）不入 ring buffer 而单独留存，订阅时先发它们再发 ring 尾部——
/// 否则流运行超过 ring 上限后，晚接入的连接拿到的是无 FLV 签名的中段字节，
/// 播放器 probe 直接失败（ffplay/VLC 都放不了）。
///
/// FLV header region: the first N stream bytes (FLV signature + onMetaData +
/// codec sequence headers) are kept separately from the ring and pushed to
/// every new subscriber before the ring tail — otherwise once the stream has
/// outgrown the ring, a late-joining connection receives mid-stream bytes
/// with no FLV signature and the player's probe fails outright (ffplay and
/// VLC alike).
const HEADER_TARGET_BYTES: usize = 4096;

/// 转发流的当前状态 / Current state of a relay stream
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayStreamState {
	/// 正在连接上游 / Connecting to upstream
	Connecting,
	/// 正在转发直播流 / Relaying live stream
	Live,
	/// 上游离线，正在输出状态画面 / Upstream offline, outputting status frame
	Offline { status: String },
	/// 发生错误 / Error occurred
	Error { message: String },
}

/// 播放 URL 传入的流参数覆盖（`/stream/{modelname}?resolution=720&codec=h264&preference=higher`）。
/// 每个字段缺省时回退到程序设置，回退逻辑与程序设置路径完全一致。
/// 参数在**会话创建时**生效：对已有会话发起参数不同的订阅会重建会话使新参数生效。
///
/// Per-request stream overrides passed via the play URL
/// (`/stream/{modelname}?resolution=720&codec=h264&preference=higher`).
/// Missing fields fall back to the program settings with identical fallback semantics.
/// Params take effect at session creation: subscribing to a live session with
/// different params restarts the session so the new params apply.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct RelayStreamParams {
	/// 首选分辨率高度（像素；0 = 原始画质）/ Preferred resolution height in px (0 = source quality)
	pub resolution: Option<u32>,
	/// 目标分辨率不可用时的回退方向（"higher" = 优先向上）/ Fallback direction when the target resolution is unavailable ("higher" = prefer upward)
	#[serde(alias = "resolution_preference")]
	pub preference: Option<String>,
	/// 首选视频编码（h264 / H265 / AV1，大小写不敏感）/ Preferred video codec (h264 / H265 / AV1, case-insensitive)
	pub codec: Option<String>,
}

impl RelayStreamParams {
	/// 是否带任何覆盖参数 / Whether any override is present
	pub fn is_empty(&self) -> bool {
		self.resolution.is_none() && self.preference.is_none() && self.codec.is_none()
	}

	/// 序列化为 URL 查询串（不带前导 `?`；值为简单 token 无需转义）。
	/// Serialize into a URL query string (no leading `?`; values are simple tokens needing no escaping).
	pub fn to_query(&self) -> String {
		let mut parts = Vec::new();
		if let Some(r) = self.resolution {
			parts.push(format!("resolution={r}"));
		}
		if let Some(p) = &self.preference {
			parts.push(format!("preference={p}"));
		}
		if let Some(c) = &self.codec {
			parts.push(format!("codec={c}"));
		}
		parts.join("&")
	}

	/// 语义化比较两组参数的流选择是否等价（编码经 `normalize_video_codec`、
	/// 回退方向经小写规范化后逐项对比）。用于判定新订阅的参数与运行中会话的
	/// 生效参数是否一致——不一致则应重启会话使新参数生效。
	/// 无法识别的编码串均规范化为 None，彼此视为等价（应用后都不带编码参数）。
	///
	/// Semantically compare two param sets for selection equality (codec via
	/// `normalize_video_codec`, preference lowercased, field by field). Used to
	/// decide whether an incoming subscription matches the running session's
	/// effective params — a mismatch restarts the session so the new params
	/// take effect. Unrecognized codec strings all normalize to None and are
	/// treated as equal (they apply identically: no codec parameter upstream).
	pub fn same_selection(&self, other: &Self) -> bool {
		use crate::platform::stripchat::normalize_video_codec;
		let pref_eq = self
			.preference
			.as_deref()
			.map(|p| p.trim().to_ascii_lowercase())
			== other
				.preference
				.as_deref()
				.map(|p| p.trim().to_ascii_lowercase());
		let codec_eq = match (&self.codec, &other.codec) {
			(None, None) => true,
			(Some(a), Some(b)) => normalize_video_codec(a) == normalize_video_codec(b),
			_ => false,
		};
		self.resolution == other.resolution && pref_eq && codec_eq
	}
}

/// 转发会话 / Relay session
pub struct RelaySession {
	/// 会话唯一 ID（单调递增），worker 用它确认自己操作的仍是创建它的那个会话。
	/// Unique, monotonically increasing session id; the worker uses it to make sure it
	/// still operates on the session that spawned it.
	pub id: u64,
	/// 上游播放列表 URL（若已获取）/ Upstream playlist URL (if obtained)
	pub playlist_url: Option<String>,
	/// 当前流状态 / Current stream state
	pub stream_state: RelayStreamState,
	/// 主播真实在线状态（由 worker 实时更新）/ Streamer real online status (updated by worker in real time)
	pub streamer_is_online: bool,
	/// 主播真实直播间状态文字（由 worker 实时更新）/ Streamer real status text (updated by worker in real time)
	pub streamer_status: String,
	/// 活跃连接数 / Number of active connections
	pub active_connections: u32,
	/// 会话创建时间（用于计算运行时长）/ Session creation time (for uptime calculation)
	pub created_at: Instant,
	/// 会话创建的 Unix 时间戳（毫秒，供前端本地计时）/ Session creation Unix timestamp in ms (for client-side timer)
	pub created_at_ms: u64,
	/// 最后活跃时间 / Last active time
	pub last_active: Instant,
	/// 停止 worker 的信号 / Signal to stop worker
	pub stop_tx: mpsc::Sender<()>,
	/// 本会话的流参数覆盖（首个订阅请求的 URL 参数）/ Per-session stream overrides (from the first subscriber's URL params)
	pub stream_params: RelayStreamParams,
	/// TS 数据广播发送端 / TS data broadcast sender
	pub ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
	/// 最近数据 ring buffer，供新连接立即推送，减少首帧等待。
	/// 每条目为一个完整 FLV 元素（文件头块或完整标签），bool 标记视频关键帧标签。
	/// Recent data ring buffer for immediate push to new connections, reducing
	/// first-frame wait. Each entry is one complete FLV element (file header
	/// block or a whole tag); the bool flags video keyframe tags.
	pub prebuffer: VecDeque<(Arc<Vec<u8>>, bool)>,
	/// ring buffer 当前总字节数 / Total bytes currently in prebuffer
	pub prebuffer_bytes: usize,
	/// 流起始块（FLV 签名 + 序列头等），累积满 `HEADER_TARGET_BYTES` 后不再追加；
	/// 新订阅先收到它再接 ring 尾部。
	/// Stream-leading chunks (FLV signature + sequence headers), accumulated
	/// until `HEADER_TARGET_BYTES`; prepended to new subscriptions before the ring tail.
	pub header_chunks: VecDeque<Arc<Vec<u8>>>,
	/// 头保留区当前总字节数 / Total bytes currently held in the header region
	pub header_bytes: usize,
}

/// 新会话需要启动的 worker 所持有的通道端。
/// Channel ends handed to the worker that must be started for a newly created session.
pub struct NewRelayWorker {
	/// 停止信号接收端 / Stop signal receiver
	pub stop_rx: mpsc::Receiver<()>,
	/// TS 数据广播发送端 / TS data broadcast sender
	pub ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
	/// 会话创建时带入的流参数覆盖 / Stream overrides carried in at session creation
	pub stream_params: RelayStreamParams,
}

/// `subscribe_or_create` 的结果。
/// Result of `subscribe_or_create`.
pub struct RelaySubscription {
	/// 本次订阅所属会话的 ID / Id of the session this subscription belongs to
	pub session_id: u64,
	/// TS 数据接收端 / TS data receiver
	pub rx: broadcast::Receiver<Arc<Vec<u8>>>,
	/// 订阅时刻的预缓冲快照 / Prebuffer snapshot taken at subscription time
	pub prebuffer: Vec<Arc<Vec<u8>>>,
	/// 仅当本次调用新建了会话时为 Some，调用方必须用它启动唯一的 worker。
	/// Some only when this call created the session; the caller must start the single worker with it.
	pub new_worker: Option<NewRelayWorker>,
}

/// 全局转发会话管理器 / Global relay session manager
pub struct RelayManager {
	pub sessions: RwLock<HashMap<String, RelaySession>>,
	/// 下一个会话 ID（从 1 开始单调递增）/ Next session id (monotonically increasing from 1)
	next_session_id: AtomicU64,
}

impl RelayManager {
	pub fn new() -> Arc<Self> {
		Arc::new(Self {
			sessions: RwLock::new(HashMap::new()),
			next_session_id: AtomicU64::new(1),
		})
	}

	/// 订阅主播的转发流；若会话不存在则在同一把写锁内创建会话并返回 worker 通道。
	/// 检查与创建在单次写锁内完成，因此同一主播的并发请求只会有一个拿到 `new_worker`，
	/// 也就只会启动一个 worker。
	///
	/// `params` 在创建会话时生效；对已有会话发起**参数不同**的订阅时，会先移除
	/// 旧会话并通知其 worker 停止（旧订阅者的流随 worker 退出而结束，重连即落
	/// 到新会话），再按新参数创建会话——新请求的参数总是立即生效。
	///
	/// Subscribe to a streamer's relay stream; if no session exists, create one under the
	/// same write lock and return the worker channels. Check and create happen within a
	/// single write lock, so among concurrent requests for the same streamer only one gets
	/// `new_worker`, and therefore only one worker is started.
	///
	/// `params` take effect at session creation; a subscription whose params
	/// differ from the running session's first drops it and signals its worker
	/// to stop (existing subscribers' streams end with the worker, and a
	/// reconnect lands on the new session) before creating a fresh session with
	/// the new params — the latest request's params always apply.
	pub fn subscribe_or_create(
		&self,
		username: &str,
		params: RelayStreamParams,
	) -> RelaySubscription {
		let mut sessions = self.sessions.write();
		// 参数不同的订阅 → 移除旧会话并通知其 worker 停止，落到下方按新参数建会话 /
		// A subscription with different params → drop the old session and signal
		// its worker to stop, then fall through to create with the new params
		let restart = sessions
			.get(username)
			.is_some_and(|s| !s.stream_params.same_selection(&params));
		if restart && let Some(old) = sessions.remove(username) {
			let _ = old.stop_tx.try_send(());
		}
		if let Some(s) = sessions.get_mut(username) {
			s.active_connections += 1;
			s.last_active = Instant::now();
			// 头保留区 + ring 尾部：保证新连接始终以 FLV 签名开头；尾部再从
			// 最近一个关键帧标签起截取——FLV 标签没有同步字，中段 GOP 切入会让
			// 严格解复用器（如 PotPlayer）探测垃圾字节后直接判流损坏
			// Header region + ring tail: new connections always start with the
			// FLV signature; the tail is additionally cut from the most recent
			// keyframe tag — FLV tags carry no syncword, so entering mid-GOP
			// hands stricter demuxers (e.g. PotPlayer) garbage that fails probing
			let kf_pos = s.prebuffer.iter().rposition(|(_, kf)| *kf).unwrap_or(0);
			return RelaySubscription {
				session_id: s.id,
				rx: s.ts_tx.subscribe(),
				prebuffer: s
					.header_chunks
					.iter()
					.cloned()
					.chain(s.prebuffer.iter().skip(kf_pos).map(|(c, _)| c.clone()))
					.collect(),
				new_worker: None,
			};
		}

		let id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
		let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
		// 频道按逐标签块计量（每块一个完整 FLV 元素），2048 条约覆盖 10s 窗口；
		// Arc 共享实际数据，内存开销反而低于原先 256 个 64KB 任意切块 /
		// Capacity counts whole-tag chunks (one complete FLV element each);
		// ~2048 tags cover a ~10s window, and Arc-shared data keeps memory
		// below the old 256 × 64KB arbitrary chunks
		let (ts_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(2048);
		let rx = ts_tx.subscribe();
		let now_instant = Instant::now();
		let created_at_ms = SystemTime::now()
			.duration_since(UNIX_EPOCH)
			.map(|d| d.as_millis() as u64)
			.unwrap_or(0);
		sessions.insert(
			username.to_string(),
			RelaySession {
				id,
				playlist_url: None,
				stream_state: RelayStreamState::Connecting,
				streamer_is_online: false,
				streamer_status: String::new(),
				active_connections: 1,
				created_at: now_instant,
				created_at_ms,
				last_active: now_instant,
				stop_tx,
				stream_params: params.clone(),
				ts_tx: ts_tx.clone(),
				prebuffer: VecDeque::new(),
				prebuffer_bytes: 0,
				header_chunks: VecDeque::new(),
				header_bytes: 0,
			},
		);
		RelaySubscription {
			session_id: id,
			rx,
			prebuffer: Vec::new(),
			new_worker: Some(NewRelayWorker {
				stop_rx,
				ts_tx,
				stream_params: params,
			}),
		}
	}

	/// 取得 ID 匹配的会话的可变引用；会话不存在或 ID 不匹配时返回 None。
	/// Get a mutable reference to the session only if its id matches; None otherwise.
	fn current_mut<'a>(
		sessions: &'a mut HashMap<String, RelaySession>,
		username: &str,
		session_id: u64,
	) -> Option<&'a mut RelaySession> {
		sessions.get_mut(username).filter(|s| s.id == session_id)
	}

	/// 将一个完整 FLV 元素推入预缓冲 ring buffer，超出上限时淘汰最旧条目；
	/// `is_keyframe` 标记视频关键帧标签，供新订阅截取播放起点；仅在会话 ID 匹配时生效。
	/// Push a complete FLV element into the prebuffer, evicting the oldest
	/// entry(ies) when the limit is exceeded; `is_keyframe` marks video keyframe
	/// tags used to align the playback start of new subscriptions; only takes
	/// effect when the session id matches.
	pub fn push_prebuffer(
		&self,
		username: &str,
		session_id: u64,
		chunk: Arc<Vec<u8>>,
		is_keyframe: bool,
	) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			let len = chunk.len();
			// 流起始块先填头保留区（不进 ring），凑够目标后才开始入 ring——
			// 晚接入的连接必须先拿到 FLV 签名，否则 probe 失败
			// Leading chunks fill the header region first (never the ring); the
			// ring only starts after the target is reached — late joiners must
			// receive the FLV signature or probing fails
			if s.header_bytes < HEADER_TARGET_BYTES {
				s.header_chunks.push_back(chunk);
				s.header_bytes += len;
				return;
			}
			s.prebuffer.push_back((chunk, is_keyframe));
			s.prebuffer_bytes += len;
			// 淘汰最旧的块直到总字节数不超过上限
			// Evict oldest chunks until total bytes are within limit
			while s.prebuffer_bytes > PREBUFFER_MAX_BYTES {
				if let Some((oldest, _)) = s.prebuffer.pop_front() {
					s.prebuffer_bytes = s.prebuffer_bytes.saturating_sub(oldest.len());
				} else {
					break;
				}
			}
		}
	}

	/// 清空预缓冲（状态切换时调用，避免将旧内容推给新连接）；仅在会话 ID 匹配时生效。
	/// Clear the prebuffer (called on state transition to avoid pushing stale content to new
	/// connections); only takes effect when the session id matches.
	pub fn clear_prebuffer(&self, username: &str, session_id: u64) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			s.prebuffer.clear();
			s.prebuffer_bytes = 0;
			s.header_chunks.clear();
			s.header_bytes = 0;
		}
	}

	/// 减少连接计数，并在连接数归零时更新最后活跃时间；仅在会话 ID 匹配时生效，
	/// 避免旧会话的连接断开时扣减新会话的计数。
	/// Decrement connection count and update last_active when it reaches zero; only takes
	/// effect when the session id matches, so a connection of an old session cannot
	/// decrement the count of a new one.
	pub fn unsubscribe(&self, username: &str, session_id: u64) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			s.active_connections = s.active_connections.saturating_sub(1);
			if s.active_connections == 0 {
				s.last_active = Instant::now();
			}
		}
	}

	/// 检查会话是否处于空闲状态（无连接且超过指定秒数未活跃）。
	/// 会话不存在或 ID 不匹配时返回 true，让旧 worker 尽快退出。
	/// Check if a session is idle (no connections and inactive for more than the given
	/// seconds). Returns true when the session is gone or the id does not match, so a
	/// stale worker exits promptly.
	pub fn is_idle(&self, username: &str, session_id: u64, idle_secs: u64) -> bool {
		let sessions = self.sessions.read();
		match sessions.get(username) {
			Some(s) if s.id == session_id => {
				s.active_connections == 0 && s.last_active.elapsed().as_secs() >= idle_secs
			}
			_ => true,
		}
	}

	/// 更新流状态；仅在会话 ID 匹配时生效。
	/// Update the stream state; only takes effect when the session id matches.
	pub fn set_state(&self, username: &str, session_id: u64, state: RelayStreamState) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			s.stream_state = state;
			s.last_active = Instant::now();
		}
	}

	/// 更新主播真实状态（由 worker 在每次 API 查询后调用）；仅在会话 ID 匹配时生效。
	/// Update the streamer's real status (called by worker after each API query); only
	/// takes effect when the session id matches.
	pub fn set_streamer_status(
		&self,
		username: &str,
		session_id: u64,
		is_online: bool,
		status: String,
	) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			s.streamer_is_online = is_online;
			s.streamer_status = status;
		}
	}

	/// 更新播放列表 URL；仅在会话 ID 匹配时生效。
	/// Update the playlist URL; only takes effect when the session id matches.
	pub fn set_playlist_url(&self, username: &str, session_id: u64, url: Option<String>) {
		let mut sessions = self.sessions.write();
		if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
			s.playlist_url = url;
		}
	}

	/// 停止并移除会话（不校验 ID，供手动停止接口使用）。
	/// Stop and remove the session without checking the id (used by the manual stop endpoint).
	pub fn remove(&self, username: &str) {
		if let Some(session) = self.sessions.write().remove(username) {
			let _ = session.stop_tx.try_send(());
		}
	}

	/// 仅当当前会话 ID 匹配时停止并移除会话，防止旧 worker 退出时删掉新会话。
	/// Stop and remove the session only if its id matches, so a stale worker exiting
	/// cannot remove a newer session.
	pub fn remove_if_current(&self, username: &str, session_id: u64) {
		let mut sessions = self.sessions.write();
		if sessions.get(username).is_some_and(|s| s.id == session_id)
			&& let Some(session) = sessions.remove(username)
		{
			let _ = session.stop_tx.try_send(());
		}
	}

	/// 获取所有会话的状态快照（用于前端展示）。
	pub fn get_all_status(&self) -> Vec<RelaySessionStatus> {
		self.sessions
			.read()
			.iter()
			.map(|(username, s)| RelaySessionStatus {
				username: username.clone(),
				stream_state: s.stream_state.clone(),
				streamer_is_online: s.streamer_is_online,
				streamer_status: s.streamer_status.clone(),
				active_connections: s.active_connections,
				uptime_secs: s.created_at.elapsed().as_secs(),
				created_at_ms: s.created_at_ms,
				stream_url: {
					// 带上会话生效的 URL 参数，前端复制按钮拿到的即是实际在用的地址 /
					// Include the session's effective URL params so the frontend copy
					// button yields the address actually in use
					let q = s.stream_params.to_query();
					if q.is_empty() {
						format!("/stream/{username}")
					} else {
						format!("/stream/{username}?{q}")
					}
				},
			})
			.collect()
	}
}

/// 会话状态快照（序列化给前端）/ Session status snapshot (serialized for frontend)
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelaySessionStatus {
	pub username: String,
	pub stream_state: RelayStreamState,
	/// 主播真实在线状态 / Streamer real online status
	pub streamer_is_online: bool,
	/// 主播真实直播间状态文字 / Streamer real status text
	pub streamer_status: String,
	pub active_connections: u32,
	/// 会话已运行秒数（服务端计算，用于初始值）/ Uptime in seconds (server-computed, used as initial value)
	pub uptime_secs: u64,
	/// 会话创建时的 Unix 时间戳（毫秒），供前端本地计时 / Session creation Unix timestamp (ms) for client-side timer
	pub created_at_ms: u64,
	pub stream_url: String,
}
