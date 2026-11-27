//! 流转发模块 / Stream Relay Module
//!
//! 提供按需启动的持久 MPEG-TS 流转发服务。
//! 访问 /stream/{modelname} 即可播放，无需手动启动。
//! 上游离线时自动输出黑屏+状态文字画面。

pub mod handler;
pub mod player;
pub mod state;
pub mod worker;

/// 桌面端专用：在 `127.0.0.1:0` 上启动仅含 `/stream/{modelname}` 转发路由的
/// 本地监听，返回实际绑定端口。由「系统播放器打开转发流」命令懒启动——
/// 播放器随后连 `http://127.0.0.1:{port}/stream/{username}`，连接本身即
/// 触发 `stream_handler` 的按需建会话/起 worker 逻辑。（路径与 Server 侧
/// `.nest("/stream", …)` 的最终形态一致。）
///
/// Desktop-only: starts a local listener on `127.0.0.1:0` carrying just the
/// `/stream/{modelname}` relay route and returns the bound port. Lazily
/// started by the "open relay stream in system player" command — the player
/// then hits `http://127.0.0.1:{port}/stream/{username}`, and the connection
/// itself triggers `stream_handler`'s on-demand session/worker creation.
/// (Path shape matches the Server side's `.nest("/stream", …)` result.)
///
/// ## 参数 / Parameters
/// - `state`：转发路由共享状态 / Shared state for the relay routes
///
/// ## 错误 / Errors
/// - 绑定监听或读取本地地址失败 → IO 错误 /
///   binding the listener or reading its address fails → IO error
pub async fn start_stream_listener(state: handler::RelayState) -> crate::core::error::Result<u16> {
	use axum::{Router, routing::get};
	let router: Router<()> = Router::new()
		.route("/stream/{modelname}", get(handler::stream_handler))
		.with_state(state);
	let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
	let port = listener.local_addr()?.port();
	tokio::spawn(async move {
		let _ = axum::serve(listener, router).await;
	});
	Ok(port)
}
