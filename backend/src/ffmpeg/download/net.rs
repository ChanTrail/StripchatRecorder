//! FFmpeg 下载网络层 / FFmpeg Download Networking
//!
//! - [`download_proxy`]：FFmpeg 下载的唯一代理通道——社区模块代理（社区镜像在
//!   调用方经 `apply_mirror` 处理）；显式不回落 API 代理与环境变量
//! - [`http_client`]：按代理策略构造 reqwest 客户端（与 SCAPI 一致：未配代理
//!   时 `.no_proxy()` 禁用环境变量代理）
//! - [`content_range_total`]：分段探测响应 `Content-Range` 里的总大小
//! - [`download_parts`]：多线程 `Range` 分段并发下载，`AtomicU64` 合计进度
//! - [`hash_file`]：整文件 sha256（分段路径完成后统一算哈希）
//!
//! - [`download_proxy`]: the only proxy channel for FFmpeg downloads — the
//!   community proxy (the community mirror is applied by callers via
//!   `apply_mirror`); explicitly no fallback to the API proxy or env vars
//! - [`http_client`]: build the reqwest client per proxy policy (same as
//!   SCAPI: explicit `.no_proxy()` without a configured proxy)
//! - [`content_range_total`]: total size from the range-probe `Content-Range`
//! - [`download_parts`]: multi-threaded ranged download, progress aggregated
//!   via `AtomicU64`
//! - [`hash_file`]: whole-file sha256 (computed after the ranged path finishes)

use super::PARALLEL_PARTS;
use super::progress::emit_progress;
use crate::config::app_state::Settings;
use crate::core::emitter::Emitter;
use crate::core::error::{AppError, Result};
use crate::ffmpeg::{FFmpegKind, FFmpegProgress, FFmpegStateStore};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// ─── HTTP 客户端 / HTTP client ───────────────────────────────────────────────

/// 下载使用的代理：仅社区模块代理（非空时生效）。API 代理、系统/环境变量
/// 代理均不适用于 FFmpeg 下载（与镜像配合，下载通道统一走社区配置）。
/// Proxy for downloads: the community-module proxy only (when non-empty).
/// The API proxy and system/env proxies do not apply to FFmpeg downloads
/// (paired with the mirror, the download channel uniformly follows the
/// community configuration).
pub(super) fn download_proxy(settings: &Settings) -> Option<String> {
	settings
		.community_proxy_url
		.clone()
		.filter(|s| !s.is_empty())
}

/// 构造下载用 reqwest 客户端（connect 30s / read 60s / UA 带版本号），
/// 整体超时按场景传入。代理策略与 SCAPI 一致：配置了社区模块代理用
/// `Proxy::all`，未配置时显式 `.no_proxy()` 禁用环境变量代理。
///
/// Build the download reqwest client (connect 30s / read 60s / versioned UA),
/// the overall timeout passed per scenario. Proxy policy mirrors SCAPI:
/// `Proxy::all` when a community proxy is configured, otherwise an explicit
/// `.no_proxy()` disables environment-variable proxies.
pub(super) fn http_client(proxy: Option<&str>, timeout_secs: u64) -> Result<reqwest::Client> {
	let mut builder = reqwest::Client::builder()
		.user_agent(format!("StripchatRecorder/{}", crate::update::APP_VERSION))
		.connect_timeout(Duration::from_secs(30))
		.read_timeout(Duration::from_secs(60))
		.timeout(Duration::from_secs(timeout_secs));
	if let Some(p) = proxy.filter(|s| !s.is_empty()) {
		let proxy =
			reqwest::Proxy::all(p).map_err(|e| AppError::Other(format!("代理配置错误: {}", e)))?;
		builder = builder.proxy(proxy);
	} else {
		builder = builder.no_proxy();
	}
	builder
		.build()
		.map_err(|e| AppError::Other(format!("HTTP 客户端初始化失败: {}", e)))
}

/// 从 `Content-Range: bytes 0-0/209715200` 解析总大小（分段探测响应头）。
/// Parse the total size out of `Content-Range: bytes 0-0/209715200`
/// (the range-probe response header).
pub(super) fn content_range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
	headers
		.get(reqwest::header::CONTENT_RANGE)?
		.to_str()
		.ok()?
		.rsplit('/')
		.next()?
		.trim()
		.parse()
		.ok()
}

/// 并行分段下载（与 FFmpegGUI 相同的方案）：N 个并发 `Range` 请求各自写入
/// 预分配文件的对应区段——每个分块持有独立文件句柄与文件指针，
/// `seek`+`write_all` 无竞态；共享 `AtomicU64` 计数由 200ms 节流汇报器广播
/// 合并进度。任一分块失败即整体失败；字节数与 total 不符同样报错。
///
/// Parallel segmented download (same scheme as FFmpegGUI): N concurrent
/// `Range` requests each writing their own slice of the preallocated file —
/// every part owns an independent handle/seek pointer, so `seek`+`write_all`
/// is race-free; a shared `AtomicU64` feeds a 200ms-throttled progress
/// ticker. Any part failure fails the whole download; a byte count that
/// doesn't add up to `total` is an error too.
///
/// 返回实际写入总字节数。/ Returns the total bytes written.
pub(super) async fn download_parts(
	kind: FFmpegKind,
	client: &reqwest::Client,
	url: &str,
	path: &Path,
	total: u64,
	store: &FFmpegStateStore,
	emitter: &Arc<dyn Emitter>,
) -> Result<u64> {
	use futures_util::StreamExt;
	use std::io::SeekFrom;
	use std::sync::atomic::{AtomicU64, Ordering};
	use tokio::io::{AsyncSeekExt, AsyncWriteExt};

	// 预分配文件，分块写手可随意定位（稀疏文件，实际占用按写入增长）
	// Preallocate so part writers can seek anywhere (sparse — real usage
	// grows with the bytes written)
	tokio::fs::File::create(path)
		.await
		.map_err(|e| AppError::Other(format!("创建下载临时文件失败: {}", e)))?
		.set_len(total)
		.await
		.map_err(|e| AppError::Other(format!("预分配下载临时文件失败: {}", e)))?;

	let fetched = Arc::new(AtomicU64::new(0));
	let span = total.div_ceil(PARALLEL_PARTS);
	let mut tasks = Vec::new();
	for i in 0..PARALLEL_PARTS {
		let start = i * span;
		if start >= total {
			break;
		}
		let end = (start + span).min(total) - 1;
		let client = client.clone();
		let url = url.to_string();
		let path = path.to_path_buf();
		let fetched = Arc::clone(&fetched);
		tasks.push(tokio::spawn(async move {
			let resp = client
				.get(&url)
				.header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
				.send()
				.await
				.map_err(|e| AppError::Other(format!("分段下载请求失败: {}", e)))?;
			if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
				return Err(AppError::Other(format!(
					"服务器未按分段返回（HTTP {}）",
					resp.status()
				)));
			}
			let mut f = tokio::fs::OpenOptions::new()
				.write(true)
				.open(&path)
				.await
				.map_err(|e| AppError::Other(format!("打开下载临时文件失败: {}", e)))?;
			f.seek(SeekFrom::Start(start))
				.await
				.map_err(|e| AppError::Other(format!("定位写入位置失败: {}", e)))?;
			let mut stream = resp.bytes_stream();
			while let Some(chunk) = stream.next().await {
				let chunk = chunk.map_err(|e| AppError::Other(format!("分段下载中断: {}", e)))?;
				f.write_all(&chunk)
					.await
					.map_err(|e| AppError::Other(format!("写入分段失败: {}", e)))?;
				fetched.fetch_add(chunk.len() as u64, Ordering::Relaxed);
			}
			Ok::<(), AppError>(())
		}));
	}

	// 进度汇报：每 200ms 广播一次合并进度，以 join_all 为停止条件
	// （不能用计数到 100%——某个分块挂起时计数会停滞）
	// Progress ticker: broadcasts the merged count every 200ms, stopped by
	// join_all finishing — not the counter reaching 100% (a stalled part
	// would freeze the counter)
	let ticker = tokio::spawn({
		let fetched = Arc::clone(&fetched);
		let store = store.clone();
		let emitter = Arc::clone(emitter);
		async move {
			loop {
				tokio::time::sleep(Duration::from_millis(200)).await;
				let f = fetched.load(Ordering::Relaxed);
				emit_progress(
					&store,
					&emitter,
					FFmpegProgress::Downloading {
						kind,
						downloaded: f,
						total,
						pct: Some(((f.saturating_mul(100) / total.max(1)).min(100)) as u8),
					},
				);
				if f >= total {
					break;
				}
			}
		}
	});

	let mut first_err: Option<AppError> = None;
	for res in futures_util::future::join_all(tasks).await {
		match res {
			Ok(Ok(())) => {}
			Ok(Err(e)) => {
				if first_err.is_none() {
					first_err = Some(e);
				}
			}
			Err(e) => {
				if first_err.is_none() {
					first_err = Some(AppError::Other(format!("分段下载任务崩溃: {}", e)));
				}
			}
		}
	}
	ticker.abort();
	if let Some(e) = first_err {
		return Err(e);
	}
	let done = fetched.load(Ordering::Relaxed);
	if done != total {
		return Err(AppError::Other(format!(
			"分段下载不完整（{} / {} 字节）",
			done, total
		)));
	}
	Ok(done)
}

/// 对已下载完成的文件计算 sha256（并行分段路径：分段各自写自己的区段，
/// 无法边下边算整包哈希，完成后整文件重读一遍）。返回小写十六进制串。
///
/// Hash a fully downloaded file (parallel path: each segment writes its own
/// slice, so a global hash can't run mid-flight — the finished file is read
/// once more). Returns the lowercase hex digest.
pub(super) async fn hash_file(path: &Path) -> Result<String> {
	use sha2::{Digest, Sha256};
	use tokio::io::AsyncReadExt;

	let mut f = tokio::fs::File::open(path)
		.await
		.map_err(|e| AppError::Other(format!("打开下载文件失败: {}", e)))?;
	let mut hasher = Sha256::new();
	let mut buf = vec![0u8; 1024 * 1024];
	loop {
		let n = f
			.read(&mut buf)
			.await
			.map_err(|e| AppError::Other(format!("读取下载文件失败: {}", e)))?;
		if n == 0 {
			break;
		}
		hasher.update(&buf[..n]);
	}
	Ok(hasher
		.finalize()
		.iter()
		.map(|b| format!("{:02x}", b))
		.collect())
}
