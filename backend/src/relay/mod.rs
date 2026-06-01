//! 流转发模块 / Stream Relay Module
//!
//! 提供按需启动的持久 MPEG-TS 流转发服务。
//! 访问 /stream/{modelname} 即可播放，无需手动启动。
//! 上游离线时自动输出黑屏+状态文字画面。

pub mod handler;
pub mod state;
pub mod streamer;
