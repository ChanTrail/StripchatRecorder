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
    /// TS 数据广播发送端 / TS data broadcast sender
    pub ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
    /// 最近 TS 数据 ring buffer，供新连接立即推送，减少首帧等待。
    /// Recent TS data ring buffer for immediate push to new connections, reducing first-frame wait.
    pub prebuffer: VecDeque<Arc<Vec<u8>>>,
    /// ring buffer 当前总字节数 / Total bytes currently in prebuffer
    pub prebuffer_bytes: usize,
}

/// 新会话需要启动的 worker 所持有的通道端。
/// Channel ends handed to the worker that must be started for a newly created session.
pub struct NewRelayWorker {
    /// 停止信号接收端 / Stop signal receiver
    pub stop_rx: mpsc::Receiver<()>,
    /// TS 数据广播发送端 / TS data broadcast sender
    pub ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
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
    /// Subscribe to a streamer's relay stream; if no session exists, create one under the
    /// same write lock and return the worker channels. Check and create happen within a
    /// single write lock, so among concurrent requests for the same streamer only one gets
    /// `new_worker`, and therefore only one worker is started.
    pub fn subscribe_or_create(&self, username: &str) -> RelaySubscription {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.active_connections += 1;
            s.last_active = Instant::now();
            return RelaySubscription {
                session_id: s.id,
                rx: s.ts_tx.subscribe(),
                prebuffer: s.prebuffer.iter().cloned().collect(),
                new_worker: None,
            };
        }

        let id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let (stop_tx, stop_rx) = mpsc::channel::<()>(1);
        let (ts_tx, _) = broadcast::channel::<Arc<Vec<u8>>>(256);
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
                ts_tx: ts_tx.clone(),
                prebuffer: VecDeque::new(),
                prebuffer_bytes: 0,
            },
        );
        RelaySubscription {
            session_id: id,
            rx,
            prebuffer: Vec::new(),
            new_worker: Some(NewRelayWorker { stop_rx, ts_tx }),
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

    /// 将一块 TS 数据推入预缓冲 ring buffer，超出上限时淘汰最旧的块；仅在会话 ID 匹配时生效。
    /// Push a TS chunk into the prebuffer, evicting the oldest chunk(s) when the limit is
    /// exceeded; only takes effect when the session id matches.
    pub fn push_prebuffer(&self, username: &str, session_id: u64, chunk: Arc<Vec<u8>>) {
        let mut sessions = self.sessions.write();
        if let Some(s) = Self::current_mut(&mut sessions, username, session_id) {
            let len = chunk.len();
            s.prebuffer.push_back(chunk);
            s.prebuffer_bytes += len;
            // 淘汰最旧的块直到总字节数不超过上限
            // Evict oldest chunks until total bytes are within limit
            while s.prebuffer_bytes > PREBUFFER_MAX_BYTES {
                if let Some(oldest) = s.prebuffer.pop_front() {
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
    pub fn set_streamer_status(&self, username: &str, session_id: u64, is_online: bool, status: String) {
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
                stream_url: format!("/stream/{}", username),
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