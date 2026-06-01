//! 转发会话状态 / Relay Session State

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

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
}

/// 全局转发会话管理器 / Global relay session manager
pub struct RelayManager {
    pub sessions: RwLock<HashMap<String, RelaySession>>,
}

impl RelayManager {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: RwLock::new(HashMap::new()),
        })
    }

    /// 创建或替换会话。
    pub fn create_session(
        &self,
        username: &str,
        stop_tx: mpsc::Sender<()>,
        ts_tx: broadcast::Sender<Arc<Vec<u8>>>,
    ) {
        let now_instant = Instant::now();
        let created_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.sessions.write().insert(
            username.to_string(),
            RelaySession {
                playlist_url: None,
                stream_state: RelayStreamState::Connecting,
                streamer_is_online: false,
                streamer_status: String::new(),
                active_connections: 0,
                created_at: now_instant,
                created_at_ms,
                last_active: now_instant,
                stop_tx,
                ts_tx,
            },
        );
    }

    /// 订阅 TS 数据流，同时增加连接计数。
    pub fn subscribe(&self, username: &str) -> Option<broadcast::Receiver<Arc<Vec<u8>>>> {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.active_connections += 1;
            s.last_active = Instant::now();
            return Some(s.ts_tx.subscribe());
        }
        None
    }

    /// 减少连接计数，并在连接数归零时更新最后活跃时间。
    /// Decrement connection count and update last_active when it reaches zero.
    pub fn unsubscribe(&self, username: &str) {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.active_connections = s.active_connections.saturating_sub(1);
            if s.active_connections == 0 {
                s.last_active = Instant::now();
            }
        }
    }

    /// 检查会话是否处于空闲状态（无连接且超过指定秒数未活跃）。
    /// Check if a session is idle (no connections and inactive for more than the given seconds).
    pub fn is_idle(&self, username: &str, idle_secs: u64) -> bool {
        let sessions = self.sessions.read();
        if let Some(s) = sessions.get(username) {
            s.active_connections == 0 && s.last_active.elapsed().as_secs() >= idle_secs
        } else {
            false
        }
    }

    /// 更新流状态。
    pub fn set_state(&self, username: &str, state: RelayStreamState) {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.stream_state = state;
            s.last_active = Instant::now();
        }
    }

    /// 更新主播真实状态（由 worker 在每次 API 查询后调用）。
    /// Update the streamer's real status (called by worker after each API query).
    pub fn set_streamer_status(&self, username: &str, is_online: bool, status: String) {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.streamer_is_online = is_online;
            s.streamer_status = status;
        }
    }

    /// 更新播放列表 URL。
    pub fn set_playlist_url(&self, username: &str, url: Option<String>) {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.playlist_url = url;
        }
    }

    /// 获取当前播放列表 URL。
    #[allow(dead_code)]
    pub fn get_playlist_url(&self, username: &str) -> Option<String> {
        self.sessions.read().get(username).and_then(|s| s.playlist_url.clone())
    }

    /// 停止并移除会话。
    pub fn remove(&self, username: &str) {
        if let Some(session) = self.sessions.write().remove(username) {
            let _ = session.stop_tx.try_send(());
        }
    }

    /// 检查是否有活跃会话。
    pub fn has_session(&self, username: &str) -> bool {
        self.sessions.read().contains_key(username)
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
