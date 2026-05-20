//! 转发会话状态 / Relay Session State

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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
    /// 活跃连接数 / Number of active connections
    pub active_connections: u32,
    /// 会话创建时间 / Session creation time
    pub created_at: Instant,
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
        self.sessions.write().insert(
            username.to_string(),
            RelaySession {
                playlist_url: None,
                stream_state: RelayStreamState::Connecting,
                active_connections: 0,
                created_at: Instant::now(),
                last_active: Instant::now(),
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

    /// 减少连接计数。
    pub fn unsubscribe(&self, username: &str) {
        let mut sessions = self.sessions.write();
        if let Some(s) = sessions.get_mut(username) {
            s.active_connections = s.active_connections.saturating_sub(1);
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
                active_connections: s.active_connections,
                uptime_secs: s.created_at.elapsed().as_secs(),
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
    pub active_connections: u32,
    pub uptime_secs: u64,
    pub stream_url: String,
}
