//! 更新下载/安装状态与主流程。
//! Update download/install state and the main driver.
//!
//! 保留与旧版完全一致的 SSE `update-progress` 事件接口，前端无需任何改动。
//! Keeps the SSE `update-progress` event interface identical — no frontend changes needed.

use parking_lot::RwLock;
use serde::Serialize;
use std::sync::Arc;

use super::APP_VERSION;
use super::extract::extract_and_replace_with_self_update;

/// 更新下载/安装的进度状态，通过 SSE `update-progress` 事件广播给前端。
/// Download/install progress, broadcast to the frontend via SSE `update-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateProgress {
	Idle,
	Downloading {
		downloaded: u64,
		total: u64,
		pct: Option<u8>,
	},
	Installing,
	Done,
	Error {
		message: String,
	},
}

/// 共享的进度状态存储（RwLock 包装，多任务读写）/
/// Shared progress store (RwLock-wrapped for concurrent access)
pub type UpdateStateStore = Arc<RwLock<UpdateProgress>>;

/// 新建一份初始为 Idle 的进度状态存储 /
/// Create a new progress store initialized to Idle
pub fn new_update_state() -> UpdateStateStore {
	Arc::new(RwLock::new(UpdateProgress::Idle))
}

/// 后台下载 zip 包、用 self_update 解压并多文件事务性替换，然后用 duct 启动新进程后退出。
///
/// 保留与旧版完全一致的 SSE `update-progress` 事件接口，前端无需任何改动。
///
/// Downloads the zip, extracts and atomically replaces files using self_update::MoveAll,
/// then spawns the new process via duct and exits immediately.
/// SSE `update-progress` interface is identical to the old version — no frontend changes needed.
pub async fn download_and_install(
	download_url: String,
	proxy_url: Option<String>,
	state_store: UpdateStateStore,
	emitter: Arc<dyn crate::core::emitter::Emitter>,
) {
	use crate::core::emitter::EmitterExt;
	use futures_util::StreamExt;

	macro_rules! emit_state {
		($s:expr) => {{
			*state_store.write() = $s.clone();
			emitter.emit("update-progress", &$s);
		}};
	}

	macro_rules! bail {
		($msg:expr) => {{
			let e = UpdateProgress::Error {
				message: $msg.to_string(),
			};
			emit_state!(e);
			return;
		}};
	}

	// ── 1. 流式下载 zip，实时广播进度 / Stream-download with live progress ──
	let mut builder = reqwest::Client::builder()
		.user_agent(format!("StripchatRecorder/{}", APP_VERSION))
		.connect_timeout(std::time::Duration::from_secs(30))
		.read_timeout(std::time::Duration::from_secs(60))
		.timeout(std::time::Duration::from_secs(600));

	if let Some(proxy) = proxy_url.as_deref().filter(|s| !s.is_empty()) {
		match reqwest::Proxy::all(proxy) {
			Ok(p) => builder = builder.proxy(p),
			Err(e) => bail!(format!("代理配置错误: {}", e)),
		}
	}

	let client = match builder.build() {
		Ok(c) => c,
		Err(e) => bail!(format!("HTTP 客户端初始化失败: {}", e)),
	};

	let resp = match client.get(&download_url).send().await {
		Ok(r) if r.status().is_success() => r,
		Ok(r) => bail!(format!("下载请求失败: HTTP {}", r.status())),
		Err(e) => bail!(format!("下载失败: {}", e)),
	};

	let total = resp.content_length().unwrap_or(0);
	let mut downloaded: u64 = 0;
	let mut zip_bytes: Vec<u8> = if total > 0 {
		Vec::with_capacity(total as usize)
	} else {
		Vec::new()
	};

	emit_state!(UpdateProgress::Downloading {
		downloaded: 0,
		total,
		pct: if total > 0 { Some(0) } else { None },
	});

	// 节流：total 已知时每 1% 或 200ms 广播；未知时每 512KB 或 200ms 广播
	// Throttle: broadcast every 1% or 200ms (known total), or every 512KB or 200ms (unknown)
	const THROTTLE_MS: u128 = 200;
	const BYTES_THRESHOLD: u64 = 512 * 1024;

	let mut last_emit = std::time::Instant::now();
	let mut last_pct: Option<u8> = if total > 0 { Some(0) } else { None };
	let mut last_emit_bytes: u64 = 0;

	let mut stream = resp.bytes_stream();
	while let Some(chunk) = stream.next().await {
		match chunk {
			Ok(bytes) => {
				zip_bytes.extend_from_slice(&bytes);
				downloaded += bytes.len() as u64;
				let pct = (downloaded * 100)
					.checked_div(total)
					.map(|r| r.min(100) as u8);
				let elapsed = last_emit.elapsed().as_millis() >= THROTTLE_MS;
				let pct_changed = pct != last_pct;
				let bytes_threshold =
					total == 0 && (downloaded - last_emit_bytes) >= BYTES_THRESHOLD;
				if elapsed || pct_changed || bytes_threshold {
					emit_state!(UpdateProgress::Downloading {
						downloaded,
						total,
						pct
					});
					last_emit = std::time::Instant::now();
					last_pct = pct;
					last_emit_bytes = downloaded;
				}
			}
			Err(e) => bail!(format!("下载中断: {}", e)),
		}
	}

	let final_pct = if total > 0 { Some(100u8) } else { None };
	emit_state!(UpdateProgress::Downloading {
		downloaded,
		total,
		pct: final_pct
	});

	// SSE 推送完成前稍作等待 / Brief pause to let SSE deliver the final download state
	tokio::time::sleep(std::time::Duration::from_millis(300)).await;

	// ── 2. 解压 + 事务性替换 / Extract + atomic replace via self_update ──
	emit_state!(UpdateProgress::Installing);

	let exe_path = match std::env::current_exe() {
		Ok(p) => p,
		Err(e) => bail!(format!("无法获取当前可执行文件路径: {}", e)),
	};
	let exe_dir = match exe_path.parent() {
		Some(d) => d.to_path_buf(),
		None => bail!("无法获取可执行文件所在目录"),
	};

	// 解压和替换在阻塞线程中执行，避免阻塞 tokio executor
	// Run on a blocking thread so we don't stall the tokio executor
	let result = tokio::task::spawn_blocking(move || {
		extract_and_replace_with_self_update(&zip_bytes, &exe_dir)
	})
	.await;

	match result {
		Ok(Ok(())) => {}
		Ok(Err(e)) => bail!(e),
		Err(e) => bail!(format!("安装任务崩溃: {}", e)),
	}

	// ── 3. 完成，用 duct 启动新进程后立即退出 / Done: detached spawn via duct, then exit ──
	emit_state!(UpdateProgress::Done);
	tracing::info!("{}", crate::tl!("update.installDone"));

	// 给 SSE 一点时间送达完成状态 / Give SSE a moment to deliver the done state
	tokio::time::sleep(std::time::Duration::from_secs(1)).await;

	let exe = match std::env::current_exe() {
		Ok(p) => p,
		Err(e) => {
			tracing::error!("{}", crate::tl!("update.exePathFailed", error = e));
			std::process::exit(0);
		}
	};
	let args: Vec<String> = std::env::args().skip(1).collect();

	tracing::info!(
		"{}",
		crate::tl!(
			"update.launching",
			exe = exe.display(),
			args = format!("{:?}", args)
		)
	);

	// duct::cmd 构建命令，.unchecked() 不因子进程退出码非零而报错
	// .unchecked() so duct doesn't error if the child exits with a nonzero code
	let cmd = duct::cmd(exe.as_os_str(), &args)
		.env("STRIPCHAT_RESTART_DELAY_MS", "2000")
		.unchecked();

	match cmd.start() {
		Ok(handle) => {
			tracing::info!(
				"{}",
				crate::tl!("update.launchSuccess", pid = handle.pids()[0])
			);
			// 分离子进程：遗忘 handle，父进程直接退出，无需等待子进程
			// Detach: forget the handle, parent exits immediately to free the port
			std::mem::forget(handle);
		}
		Err(e) => {
			tracing::error!("{}", crate::tl!("update.launchFailed", error = e));
		}
	}

	std::process::exit(0);
}
