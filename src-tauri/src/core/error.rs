//! 应用错误类型定义 / Application Error Type Definitions
//!
//! 定义统一的 `AppError` 枚举，覆盖 IO、网络、JSON 解析及业务逻辑错误，
//! 并实现 `serde::Serialize` 以便通过 Tauri 命令直接返回给前端。
//!
//! Defines a unified `AppError` enum covering IO, network, JSON parsing, and business logic errors,
//! with `serde::Serialize` implemented so it can be returned directly to the frontend via Tauri commands.

use std::fmt;

/// 应用统一错误类型 / Unified application error type
#[derive(Debug)]
pub enum AppError {
    /// 文件系统 IO 错误 / File system IO error
    Io(std::io::Error),
    /// HTTP 网络请求错误 / HTTP network request error
    Reqwest(reqwest::Error),
    /// JSON 序列化/反序列化错误 / JSON serialization/deserialization error
    Json(serde_json::Error),
    /// 直播间已下线 / Stream is offline
    StreamOffline(String),
    /// 该主播已在录制中 / Streamer is already being recorded
    AlreadyRecording(String),
    /// 该主播当前未在录制 / Streamer is not currently being recorded
    NotRecording(String),
    /// 用户不存在 / User not found
    UserNotFound(String),
    /// 其他错误 / Other errors
    Other(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Reqwest(e) => write!(f, "Network error: {}", e),
            Self::Json(e) => write!(f, "JSON error: {}", e),
            Self::StreamOffline(s) => write!(f, "Stream offline: {}", s),
            Self::AlreadyRecording(s) => write!(f, "Already recording: {}", s),
            Self::NotRecording(s) => write!(f, "Not recording: {}", s),
            Self::UserNotFound(s) => write!(f, "User not found: {}", s),
            Self::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        Self::Reqwest(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

/// 序列化为错误消息字符串，使 Tauri 命令可以直接将错误返回给前端。
/// Serializes to an error message string so Tauri commands can return errors directly to the frontend.
impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// 应用统一 Result 类型别名 / Unified Result type alias for the application
pub type Result<T> = std::result::Result<T, AppError>;
