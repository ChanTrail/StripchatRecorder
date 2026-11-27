//! FFmpeg 下载-安装流水线 / FFmpeg Download-Install Pipeline
//!
//! 一次安装的完整链路（[`install`] 为唯一入口，`INSTALL_MUTEX` 串行化）：
//! - [`install`] / [`install_inner`]：锁前占位事件 → 校验文件 → 分段探测 →
//!   并行分段或单流下载 → sha256 校验 → 等空闲 → 阻塞安装 → 终态事件；
//!   `install_inner` 包进 `tokio::spawn` 兜住 panic 转成 `Error` 事件
//! - [`blocking_install`]：阻塞线程上的解压（进度回调）→ chmod → 冒烟 →
//!   换入 → 写标记 → 清理临时目录
//! - [`extract`] 子模块：7z（`sevenz_rust2`）与 `tar.xz`（`liblzma`）解压、
//!   目标条目筛选与 staged 组件验证、Unix 可执行位
//! - [`swap`] 子模块：`-version` 冒烟测试、`.old` 备份式原子换入与回滚
//!
//! The full install chain ([`install`] is the single entry, serialized by
//! `INSTALL_MUTEX`):
//! - [`install`] / [`install_inner`]: pre-lock placeholder event → checksum →
//!   range probe → parallel or single-stream download → sha256 verify →
//!   wait for idle → blocking install → terminal event; `install_inner` runs
//!   inside `tokio::spawn` so a panic becomes an `Error` event
//! - [`blocking_install`]: on a blocking thread — extract (progress callback)
//!   → chmod → smoke → swap → write marker → clean temp dirs
//! - [`extract`] submodule: 7z (`sevenz_rust2`) / `tar.xz` (`liblzma`)
//!   extraction, wanted-entry filtering, staged verification, Unix exec bits
//! - [`swap`] submodule: `-version` smoke test, `.old`-backup atomic swap with
//!   rollback

mod extract;
mod swap;

use extract::{ensure_executable, extract_archive};
use swap::{smoke_run_staged, swap_staged_into};

use super::net::{content_range_total, download_parts, download_proxy, hash_file, http_client};
use super::progress::{emit_progress, emit_terminal};
use super::release::{
	FFmpegDownloadChoice, parse_sha256_file, read_marker, resolve_asset, write_marker,
};
use super::{
	DOWNLOAD_DIR_NAME, DOWNLOAD_TIMEOUT_SECS, IDLE_POLL_INTERVAL, INSTALL_MUTEX,
	PARALLEL_MIN_BYTES, RELEASE_BASE, STAGING_DIR_NAME,
};
use crate::config::app_state::AppState;
use crate::core::emitter::Emitter;
use crate::core::error::{AppError, Result};
use crate::core::notifications::NotificationLevel;
use crate::ffmpeg::{FFmpegKind, FFmpegProgress, FFmpegStateStore, tools_dir};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

// ─── 下载 + 安装 / Download + install ────────────────────────────────────────

/// 下载压缩包、校验、解压并安装到 `tools/`。同一时间只允许一个任务
/// （[`INSTALL_MUTEX`]），任何错误都会广播 `Error` 进度后返回 `Err`。
/// `choice` 是手动下载请求里可选的构建选择（平台轨/变体缺省时自动解析：
/// 保留已安装标记的轨与形态，否则回退宿主默认轨 + 静态）。
///
/// Download the archive, verify, extract and install into `tools/`. Only one task
/// runs at a time ([`INSTALL_MUTEX`]); any error broadcasts an `Error` progress
/// before returning `Err`. `choice` carries the optional build selection from
/// a manual download request (auto-resolved when the track/variant are absent:
/// keep the installed marker's values, else host default + static).
pub(super) async fn install(
	kind: FFmpegKind,
	choice: FFmpegDownloadChoice,
	app_state: &Arc<AppState>,
	emitter: &Arc<dyn Emitter>,
) -> Result<()> {
	let store = Arc::clone(&app_state.ffmpeg_state);
	// 排队期也要可见：在拿锁之前先广播一次占位——若前一个安装还持有锁
	// （如慢网络下的长下载），新任务在互斥锁上等待期间本不会有任何事件，
	// 前端强制窗口也就不会打开。
	// Stay visible while queued: broadcast a placeholder BEFORE taking the
	// lock — while a previous install still holds it (e.g. a long download on
	// a slow network) a queued task would emit nothing and the forced window
	// would never open.
	emit_progress(
		&store,
		emitter,
		FFmpegProgress::Downloading {
			kind,
			downloaded: 0,
			total: 0,
			pct: None,
		},
	);
	let _lock = INSTALL_MUTEX.lock().await;
	// install_inner 放进独立任务执行：panic（而非 Err）也会经 JoinError 转成
	// 错误事件，而不是让进度停在最后一条无声冻结。
	// Run install_inner as its own task: a panic (rather than an Err) still
	// surfaces via the JoinError as an error event instead of silently
	// freezing the progress at the last emit.
	let state2 = Arc::clone(app_state);
	let emitter2 = Arc::clone(emitter);
	let store2 = Arc::clone(&store);
	let inner =
		tokio::spawn(async move { install_inner(kind, choice, &state2, &emitter2, &store2).await })
			.await;
	let res = match inner {
		Ok(res) => res,
		Err(join_err) => Err(AppError::Other(format!("安装任务崩溃: {}", join_err))),
	};
	match res {
		Ok(()) => Ok(()),
		Err(e) => {
			emit_terminal(
				&store,
				emitter,
				FFmpegProgress::Error {
					kind,
					message: e.to_string(),
				},
			);
			// 任何失败都尽力清理临时目录，避免失败的 ~130–210MB 压缩包残留到下次启动
			// Best-effort temp-dir cleanup on any failure so a failed ~130–210 MB
			// archive doesn't linger until the next launch
			let tools = tools_dir();
			let _ = fs::remove_dir_all(tools.join(DOWNLOAD_DIR_NAME));
			let _ = fs::remove_dir_all(tools.join(STAGING_DIR_NAME));
			Err(e)
		}
	}
}

/// [`install`] 的主流程：网络下载（async）→ 解压/冒烟/替换（`spawn_blocking`）。
/// The main body of [`install`]: network download (async) → extract/smoke-test/
/// swap (`spawn_blocking`).
pub(super) async fn install_inner(
	kind: FFmpegKind,
	choice: FFmpegDownloadChoice,
	app_state: &Arc<AppState>,
	emitter: &Arc<dyn Emitter>,
	store: &FFmpegStateStore,
) -> Result<()> {
	use futures_util::StreamExt;
	use sha2::{Digest, Sha256};
	use tokio::io::AsyncWriteExt;

	// 在任何网络 I/O 之前先广播一次下载占位：首个 Downloading 事件原本要等
	// checksum 与压缩包响应头两次请求后才发出，慢网络下前端长时间无反馈。
	// Emit a download placeholder before ANY network I/O: the first real
	// Downloading event used to wait for both the checksum and archive-header
	// requests, leaving the frontend silent for a long while on slow networks.
	emit_progress(
		store,
		emitter,
		FFmpegProgress::Downloading {
			kind,
			downloaded: 0,
			total: 0,
			pct: None,
		},
	);

	// 里程碑入库：下载开始（面板可回溯；逐 % 的实时进度仍只走
	// `ffmpeg-progress` toast/强制窗口，不入库以免刷屏）
	// Milestone into the notification store: download started (the panel can
	// look back on it; per-% live progress stays on the `ffmpeg-progress`
	// toast/forced window only — storing every tick would flood the panel)
	app_state.notification_store.emit_i18n(
		emitter.as_ref(),
		NotificationLevel::Info,
		"ffmpeg",
		"FFmpeg download started",
		match kind {
			FFmpegKind::Install => "notifications.backend.ffmpegInstallStarted",
			FFmpegKind::Update => "notifications.backend.ffmpegUpdateStarted",
		},
		None,
	);

	// 解析下载目标：显式请求先过白名单，缺省按"已安装标记 → 宿主默认"回落
	// （手动重下与自动补装/更新保持同一轨）
	// Resolve the download target: explicit requests are allowlist-validated,
	// absent fields fall back "installed marker → host default" (manual
	// re-downloads and automatic refills/updates stay on the same track)
	let spec = resolve_asset(
		choice.platform.as_deref(),
		choice.variant.as_deref(),
		read_marker().as_ref(),
	)?;

	let archive_name = spec.archive_name();
	let archive_url = format!("{}/{}", RELEASE_BASE, archive_name);
	let checksum_url = format!("{}.sha256", archive_url);

	let settings = app_state.get_settings();
	let proxy = download_proxy(&settings);
	let mirror = settings.community_mirror_url.clone();
	let archive_url = crate::postprocess::community::apply_mirror(&archive_url, mirror.as_deref());
	let checksum_url =
		crate::postprocess::community::apply_mirror(&checksum_url, mirror.as_deref());

	let client = http_client(proxy.as_deref(), DOWNLOAD_TIMEOUT_SECS)?;

	// 先取校验文件：拿不到或解析不出哈希都视为失败
	// Fetch the checksum first: unfetchable or unparseable both count as failure
	let checksum_resp = client
		.get(&checksum_url)
		.send()
		.await
		.map_err(|e| AppError::Other(format!("获取 FFmpeg 校验文件失败: {}", e)))?;
	if !checksum_resp.status().is_success() {
		return Err(AppError::Other(format!(
			"获取 FFmpeg 校验文件失败: HTTP {}",
			checksum_resp.status()
		)));
	}
	let checksum_text = checksum_resp
		.text()
		.await
		.map_err(|e| AppError::Other(format!("读取 FFmpeg 校验文件失败: {}", e)))?;
	let expected_sha = parse_sha256_file(&checksum_text)
		.ok_or_else(|| AppError::Other("FFmpeg 校验文件内容无效".to_string()))?;

	// ── 下载压缩包到 tools/.download/ ──
	// Download the archive into tools/.download/
	tracing::info!(
		"{}",
		crate::tl!("ffmpeg.downloadStarted", url = archive_url)
	);

	let tools = tools_dir();
	fs::create_dir_all(&tools)?;
	let download_dir = tools.join(DOWNLOAD_DIR_NAME);
	fs::create_dir_all(&download_dir)?;
	let archive_path = download_dir.join(&archive_name);

	// 分段探测：`Range: bytes=0-0`——服务器接受分段时 `Content-Range` 顺带
	// 给出总大小，体积够大就走并行分段下载；不接受则回落单流（探测响应体
	// 未读即丢弃，浪费极少）。探测失败同样回落，真正的错误由下面的 GET 报出。
	// Range probe (`bytes=0-0`): a range-aware server also reveals the total
	// size via `Content-Range`, and a large enough payload takes the parallel
	// segmented path; anything else falls back to the single stream (the probe
	// body is dropped unread, wasting almost nothing). A failed probe falls
	// back too — the GET below reports the real error.
	let probe = client
		.get(&archive_url)
		.header(reqwest::header::RANGE, "bytes=0-0")
		.send()
		.await;
	let (ranged, probe_total) = match &probe {
		Ok(r) if r.status() == reqwest::StatusCode::PARTIAL_CONTENT => {
			(true, content_range_total(r.headers()).unwrap_or(0))
		}
		Ok(r) => (false, r.content_length().unwrap_or(0)),
		Err(_) => (false, 0),
	};
	drop(probe);

	let downloaded: u64;
	let total: u64;
	let actual_sha: String;

	if ranged && probe_total >= PARALLEL_MIN_BYTES {
		// 并行分段下载：8 路 Range 请求写预分配文件的对应区段；完成后整文件
		// 再算哈希（分段无法边下边算全局哈希）
		// Parallel segmented download: 8 Range requests write their slices of
		// the preallocated file; the hash runs over the finished file
		// (segments cannot update one global hash mid-flight)
		total = probe_total;
		emit_progress(
			store,
			emitter,
			FFmpegProgress::Downloading {
				kind,
				downloaded: 0,
				total,
				pct: Some(0),
			},
		);
		downloaded = download_parts(
			kind,
			&client,
			&archive_url,
			&archive_path,
			total,
			store,
			emitter,
		)
		.await?;
		actual_sha = hash_file(&archive_path).await?;
	} else {
		// 单流下载：一条流边下边算哈希（服务器不接受分段或体积太小的回落）
		// Single stream: hash inline while streaming (fallback when ranges are
		// not honored or the payload is too small)
		let resp = client
			.get(&archive_url)
			.send()
			.await
			.map_err(|e| AppError::Other(format!("下载 FFmpeg 失败: {}", e)))?;
		if !resp.status().is_success() {
			return Err(AppError::Other(format!(
				"下载 FFmpeg 失败: HTTP {}",
				resp.status()
			)));
		}

		total = resp.content_length().unwrap_or(0);
		emit_progress(
			store,
			emitter,
			FFmpegProgress::Downloading {
				kind,
				downloaded: 0,
				total,
				pct: if total > 0 { Some(0) } else { None },
			},
		);

		let mut file = tokio::fs::File::create(&archive_path)
			.await
			.map_err(|e| AppError::Other(format!("创建下载临时文件失败: {}", e)))?;
		let mut hasher = Sha256::new();
		let mut fetched: u64 = 0;

		// 节流：total 已知时每 1% 或 200ms 广播；未知时每 512KB 或 200ms 广播
		// Throttle: broadcast every 1% or 200ms (known total), or every 512KB or 200ms (unknown)
		const THROTTLE_MS: u128 = 200;
		const BYTES_THRESHOLD: u64 = 512 * 1024;
		let mut last_emit = std::time::Instant::now();
		let mut last_pct: Option<u8> = if total > 0 { Some(0) } else { None };
		let mut last_emit_bytes: u64 = 0;

		let mut stream = resp.bytes_stream();
		while let Some(chunk) = stream.next().await {
			let bytes = chunk.map_err(|e| AppError::Other(format!("下载 FFmpeg 中断: {}", e)))?;
			file.write_all(&bytes)
				.await
				.map_err(|e| AppError::Other(format!("写入下载临时文件失败: {}", e)))?;
			hasher.update(&bytes);
			fetched += bytes.len() as u64;

			let pct = (fetched * 100).checked_div(total).map(|r| r.min(100) as u8);
			let elapsed = last_emit.elapsed().as_millis() >= THROTTLE_MS;
			let pct_changed = pct != last_pct;
			let bytes_threshold = total == 0 && (fetched - last_emit_bytes) >= BYTES_THRESHOLD;
			if elapsed || pct_changed || bytes_threshold {
				emit_progress(
					store,
					emitter,
					FFmpegProgress::Downloading {
						kind,
						downloaded: fetched,
						total,
						pct,
					},
				);
				last_emit = std::time::Instant::now();
				last_pct = pct;
				last_emit_bytes = fetched;
			}
		}
		file.flush()
			.await
			.map_err(|e| AppError::Other(format!("写入下载临时文件失败: {}", e)))?;
		drop(file);

		downloaded = fetched;
		actual_sha = hasher
			.finalize()
			.iter()
			.map(|b| format!("{:02x}", b))
			.collect::<String>();
	}

	emit_progress(
		store,
		emitter,
		FFmpegProgress::Downloading {
			kind,
			downloaded,
			total,
			pct: if total > 0 { Some(100) } else { None },
		},
	);

	if !actual_sha.eq_ignore_ascii_case(&expected_sha) {
		return Err(AppError::Other(format!(
			"FFmpeg 压缩包校验失败（期望 {}，实际 {}）",
			expected_sha, actual_sha
		)));
	}

	// 自动更新在替换前等待空闲（下载期间可能已有录制/模块拉起了 ffmpeg）：
	// 忙碌时先广播 Idle 让前端收起 toast，而不是挂着 "Installing" 状态等几小时。
	// 首轮安装（kind=Install）在 ffmpeg 缺失时进行，不存在使用中的进程，不等待
	// Auto-updates wait for idle before swapping (a recording or module may have
	// spawned ffmpeg during the download): while busy, broadcast Idle first so the
	// frontend dismisses its toast instead of showing "Installing" for hours.
	// First installs happen while ffmpeg is missing, so nothing can be using it
	if kind == FFmpegKind::Update {
		wait_for_idle(app_state, store, emitter).await?;
	}

	// SSE 推送完成前稍作等待 / Brief pause to let SSE deliver the final download state
	tokio::time::sleep(Duration::from_millis(300)).await;

	emit_progress(
		store,
		emitter,
		FFmpegProgress::Installing { kind, pct: Some(0) },
	);

	// 解压、冒烟测试与文件替换放阻塞线程，避免长时间占用 tokio worker；
	// 解压进度经 report 回调继续广播 `Installing{pct}`
	// Extraction, smoke test and file swap run on a blocking thread so the
	// tokio workers stay free; extraction progress keeps broadcasting
	// `Installing{pct}` via the report callback
	let report_store = Arc::clone(store);
	let report_emitter = Arc::clone(emitter);
	let sha_owned = actual_sha.clone();
	let version = tokio::task::spawn_blocking(move || {
		let report = |pct: Option<u8>| {
			emit_progress(
				&report_store,
				&report_emitter,
				FFmpegProgress::Installing { kind, pct },
			);
		};
		blocking_install(&archive_path, &spec, &tools, &sha_owned, &report)
	})
	.await
	.map_err(|e| AppError::Other(format!("安装任务崩溃: {}", e)))??;

	emit_terminal(store, emitter, FFmpegProgress::Done { kind });
	// 里程碑入库：安装/更新完成（带版本号），启动安装的无人值守场景下
	// 用户事后也能在通知面板看到结果。{version} 只取构建标签——`version`
	// 是 `ffmpeg -version` 整行（含 " Copyright (c) ..." 尾巴），toast 里
	// 是一堵墙；日志仍记完整行
	// Milestone into the store: install/update done (with the version) so an
	// unattended startup install still leaves a visible result in the panel.
	// {version} keeps just the build tag — `version` is the whole
	// `ffmpeg -version` first line (incl. the " Copyright (c) ..." tail),
	// a wall of text in a toast; the log still records the full line
	let display_version = version
		.strip_prefix("ffmpeg version ")
		.unwrap_or(version.as_str())
		.split(" Copyright")
		.next()
		.unwrap_or(version.as_str());
	let mut args = HashMap::new();
	args.insert("version".to_string(), serde_json::json!(display_version));
	app_state.notification_store.emit_i18n(
		emitter.as_ref(),
		NotificationLevel::Info,
		"ffmpeg",
		match kind {
			FFmpegKind::Install => format!("FFmpeg installed: {}", display_version),
			FFmpegKind::Update => format!("FFmpeg updated: {}", display_version),
		},
		match kind {
			FFmpegKind::Install => "notifications.backend.ffmpegInstalled",
			FFmpegKind::Update => "notifications.backend.ffmpegUpdated",
		},
		Some(args),
	);
	match kind {
		FFmpegKind::Install => {
			tracing::info!("{}", crate::tl!("ffmpeg.installDone", version = version))
		}
		FFmpegKind::Update => {
			tracing::info!("{}", crate::tl!("ffmpeg.updateDone", version = version))
		}
	}
	Ok(())
}

/// 阻塞阶段的安装：解压（经 `report` 汇报 0–100% 进度）→ 赋可执行权限 →
/// staging 冒烟测试 → 换入 tools → 写标记 → 清上一版遗留的受管文件 →
/// 清理临时目录。返回 `ffmpeg -version` 首行。
///
/// Blocking install phase: extract (reporting 0–100 % progress via `report`)
/// → chmod → smoke-run the staged binaries → swap into tools → write the
/// marker → purge stale managed files from the previous install → clean up
/// temp dirs. Returns the first line of `ffmpeg -version`.
fn blocking_install(
	archive_path: &Path,
	spec: &super::release::AssetSpec,
	tools: &Path,
	archive_sha256: &str,
	report: &dyn Fn(Option<u8>),
) -> Result<String> {
	let staging = tools.join(STAGING_DIR_NAME);
	if staging.exists() {
		fs::remove_dir_all(&staging)?;
	}
	fs::create_dir_all(&staging)?;

	// 换轨/换形态前的旧受管清单：标记在换入后才重写，先记下上一版
	// managed files（无标记 = 手动放置副本，不做任何清理）
	// The previous managed file set before the track/variant switch: the
	// marker is only rewritten after the swap, so capture it up front
	// (no marker = a manually placed copy — purge nothing)
	let old_files = read_marker().map(|m| m.files).unwrap_or_default();

	let result = (|| {
		extract_archive(
			archive_path,
			spec.ext,
			&staging,
			report,
			spec.variant.contains("-shared"),
		)?;
		ensure_executable(&staging)?;
		// 先对 staging 里的二进制冒烟测试：若构建在本机跑不起来（如 libc 不匹配），
		// 在触碰 tools 里已可用的副本之前就失败
		// Smoke-run the staged binaries first: a build that can't run here (e.g.
		// wrong libc) fails before we touch the working copy in tools
		let version = smoke_run_staged(&staging)?;
		let names = swap_staged_into(&staging, tools)?;
		write_marker(
			tools,
			archive_sha256,
			&spec.platform,
			&spec.variant,
			&names,
			&version,
		)?;
		// 上一版受管而新清单没有的文件（如 shared→静态后残留的旁位库）
		// 连同其 .old 备份一并清掉 / Purge files managed by the previous
		// install but absent from the new set (e.g. leftover side-by-side
		// libs after shared→static), together with their .old backups
		for name in old_files.iter().filter(|n| !names.contains(n)) {
			let _ = fs::remove_file(tools.join(name));
			let _ = fs::remove_file(tools.join(format!("{}.old", name)));
		}
		Ok(version)
	})();

	// 收尾清理（尽力而为；残留由下次启动的 cleanup_leftovers 兜底）
	// Final cleanup (best-effort; leftovers are caught by cleanup_leftovers next launch)
	let _ = fs::remove_dir_all(tools.join(DOWNLOAD_DIR_NAME));
	let _ = fs::remove_dir_all(&staging);

	result
}

/// 自动更新在替换二进制前等待空闲（async 版本）：忙碌时先广播 `Idle` 进度让
/// 前端收起进行中的 toast，然后每 30 秒轮询；等待期间用户关闭自动更新则中止
/// 本次更新。
///
/// Wait for idle before an auto-update swaps binaries (async version): while
/// busy, first broadcast an `Idle` progress so the frontend dismisses its
/// in-progress toast, then poll every 30 s; aborts this round if the user
/// disables auto-update meanwhile.
async fn wait_for_idle(
	app_state: &AppState,
	store: &FFmpegStateStore,
	emitter: &Arc<dyn Emitter>,
) -> Result<()> {
	if crate::ffmpeg::is_idle(app_state) {
		return Ok(());
	}
	emit_progress(store, emitter, FFmpegProgress::Idle);
	loop {
		if !app_state.get_settings().ffmpeg_auto_update {
			return Err(AppError::Other(
				"FFmpeg 自动更新已关闭，本次更新取消".to_string(),
			));
		}
		if crate::ffmpeg::is_idle(app_state) {
			return Ok(());
		}
		tokio::time::sleep(IDLE_POLL_INTERVAL).await;
	}
}
