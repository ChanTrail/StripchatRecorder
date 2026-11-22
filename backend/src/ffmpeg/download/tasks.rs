//! FFmpeg 安装/更新入口任务 / FFmpeg Install and Update Entry Tasks
//!
//! - [`cleanup_leftovers`]：清理上次中断遗留的 `.download` / `.staging` 临时
//!   目录与 `*.old` 备份
//! - [`spawn_startup_install`]：启动检查——已选 tools 但组件不齐时广播
//!   `ffmpeg-choice-needed` 让前端引导用户**选择构建版本**下载（不再自动装
//!   默认版）；不可用统一推 `ffmpegMissing` 通知（UI 由首选项向导/选择
//!   对话框覆盖）；已可用时顺带跑一次 [`check_and_update`]（周期循环首轮
//!   有数分钟延迟，启动不等它）
//! - [`spawn_manual_install`]：手动触发下载（首选项向导"下载"按钮、
//!   `POST /api/ffmpeg/download`）；平台不支持时 no-op。手动点击是明确意图，
//!   不按 PATH 首选项拦截——"PATH 自管"策略只约束自动路径
//! - [`auto_update_loop`]：tools 不齐时提醒用户选择构建 + 按远端 sha256
//!   比较维护受管 tools 副本的更新（只在空闲时替换）
//! - [`current_progress`]：当前的下载/安装进度（状态接口 / Tauri 命令用）
//!
//! 唯一直接自动下载的场景：受管副本（带安装标记）的**更新**——更新沿用
//! 标记记录的用户已选平台轨/形态，其余缺失场景一律让用户选择版本。
//!
//! - [`cleanup_leftovers`]: remove `.download` / `.staging` temp dirs and
//!   `*.old` backups left by an interrupted run
//! - [`spawn_startup_install`]: startup check — when the chosen tools set is
//!   incomplete it broadcasts `ffmpeg-choice-needed` so the frontend lets the
//!   user **pick a build** to download (no more silent default install);
//!   the unavailable case pushes the `ffmpegMissing` notification (the UI is
//!   covered by the preference wizard / choice dialog); when ffmpeg is already
//!   usable it also runs one [`check_and_update`] (the periodic loop's first
//!   round is delayed by minutes — startup doesn't wait for it)
//! - [`spawn_manual_install`]: manual download trigger (wizard Download
//!   button, `POST /api/ffmpeg/download`); no-op on unsupported platforms.
//!   An explicit click is deliberate intent — the PATH self-manage policy
//!   only constrains the automatic paths
//! - [`auto_update_loop`]: reminds the user to pick a build when the chosen
//!   tools set is incomplete, plus remote-sha256-driven updates of the managed
//!   tools copy (swapped only while idle)
//! - [`current_progress`]: the current download/install progress (status
//!   endpoint / Tauri command)
//!
//! The only direct auto-download left: **updating** a managed (marker-bearing)
//! copy — the update keeps the user's chosen platform track/variant recorded
//! in the marker; every missing case defers the version choice to the user.

use super::net::{download_proxy, http_client};
use super::pipeline::install;
use super::release::{
	FFmpegDownloadChoice, build_platform, parse_sha256_file, read_marker, resolve_asset,
};
use super::{
	AUTO_UPDATE_FIRST_DELAY, AUTO_UPDATE_INTERVAL, DOWNLOAD_DIR_NAME, IDLE_POLL_INTERVAL,
	RELEASE_BASE, SMALL_TIMEOUT_SECS, STAGING_DIR_NAME,
};
use crate::config::app_state::AppState;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::AppError;
use crate::core::notifications::NotificationLevel;
use crate::ffmpeg::{FFmpegKind, FFmpegProgress, tools_dir};
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

// ─── 启动安装 / Startup install ──────────────────────────────────────────────

/// 清理上次中断（崩溃/强杀）遗留的临时目录与 `.old` 备份：在启动时的可用性
/// 检查之前调用，忽略所有错误（仅记录日志）。
///
/// Remove temp dirs and `.old` backups left by an interrupted run (crash/kill);
/// called at startup before the availability check. All errors are ignored
/// (logged only).
pub fn cleanup_leftovers() {
	let tools = tools_dir();
	for dir in [tools.join(DOWNLOAD_DIR_NAME), tools.join(STAGING_DIR_NAME)] {
		if dir.exists()
			&& let Err(e) = fs::remove_dir_all(&dir)
		{
			tracing::warn!(
				"{}",
				crate::tl!(
					"ffmpeg.leftoverCleanupFailed",
					path = dir.display(),
					error = e
				)
			);
		}
	}
	if let Ok(entries) = fs::read_dir(&tools) {
		for entry in entries.flatten() {
			let p = entry.path();
			if p.is_file()
				&& p.extension().is_some_and(|ext| ext == "old")
				&& let Err(e) = fs::remove_file(&p)
			{
				tracing::warn!(
					"{}",
					crate::tl!(
						"ffmpeg.leftoverCleanupFailed",
						path = p.display(),
						error = e
					)
				);
			}
		}
	}
}

/// 启动时的 FFmpeg 检查入口（由 `server::startup::run_all` 经 `check_ffmpeg` 调用）：
/// `run_all` 是同步函数但运行在 tokio runtime 里（Server 与 Desktop 皆是），
/// 因此这里用 `tokio::spawn` 起后台任务，不阻塞启动流程：
/// 1. 清理上次中断的残留
/// 2. 检查 ffmpeg/ffprobe 是否已可用（按 `ffmpeg_preference` 解析）；同时算出
///    `refill_tools`——用户选了 tools 但 tools 三组件不齐（下载中断/文件被删），
///    即使 PATH 顶得上也要处理所选位置。可用且无需补装 → 再做一次
///    [`check_and_update`]（可用≠最新；条件不满足时内部短路）后结束
/// 3. `refill_tools` 且平台支持下载 → 广播 `ffmpeg-choice-needed`：前端弹
///    "选择构建版本"对话框，确认后走手动下载（携带所选平台轨/变体）——
///    缺失不再自动装默认版，版本交给用户选择
/// 4. 不可用 → 统一 `ffmpegMissing` 通知（日志按 PATH 首选/平台不支持/通用
///    缺失分级）；未设首选项时的 UI 由前端首选项向导/Setup 向导覆盖
///
/// Startup FFmpeg check entry (called by `server::startup::run_all` via
/// `check_ffmpeg`). `run_all` is sync but runs inside a tokio runtime on both
/// Server and Desktop, so this spawns a background task instead of blocking:
/// 1. clean up leftovers from an interrupted previous run
/// 2. check whether ffmpeg/ffprobe are usable (resolved per `ffmpeg_preference`);
///    also compute `refill_tools` — the user chose tools but the set is
///    incomplete (interrupted download / deleted files) and must be handled
///    even when PATH covers. Usable and no refill → run one
///    [`check_and_update`] (usable ≠ latest; it short-circuits internally),
///    then done
/// 3. `refill_tools` on a downloadable platform → broadcast
///    `ffmpeg-choice-needed`: the frontend shows a pick-a-build dialog and
///    confirming goes through the manual download (carrying the chosen
///    track/variant) — a missing FFmpeg never auto-installs the default
///    build; the version is the user's choice
/// 4. unusable → one `ffmpegMissing` notification (logs differ for PATH
///    preference / unsupported platform / generic missing); with the
///    preference unset the UI is covered by the preference/setup wizard
pub fn spawn_startup_install(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
	tokio::spawn(async move {
		// 旧版本把下载副本放在 lib/，先整体迁移到 tools/ 再开始检查
		// Older versions kept the downloaded copy in lib/; migrate it to tools/
		// before the checks below
		crate::ffmpeg::migrate_legacy_lib_dir();
		cleanup_leftovers();

		let available = tokio::task::spawn_blocking(crate::ffmpeg::ffmpeg_available)
			.await
			.unwrap_or(false);
		// 用户选了 tools 但 tools 组件不齐（下载被中断/文件被删）：即使 PATH
		// 顶得上（生效值已回落 None、自动解析可用），也要处理所选位置。
		// The user chose tools but the tools set is incomplete (an interrupted
		// download or deleted files): handle the chosen location even when
		// PATH covers (the effective value already fell back to None and
		// PATH resolution works).
		let refill_tools =
			crate::ffmpeg::raw_preference() == Some(1) && !crate::ffmpeg::location_complete(1);
		if available && !refill_tools {
			tracing::debug!(
				"{}",
				crate::tl!(
					"ffmpeg.alreadyAvailable",
					path = crate::ffmpeg::ffmpeg_program().to_string_lossy()
				)
			);
			// 可用≠最新：启动即做一次更新检查（自动更新关闭 / PATH 首选 /
			// 无标记非受管副本等不满足条件时内部各自短路）；否则要等到
			// 更新循环首轮的 AUTO_UPDATE_FIRST_DELAY 才检查
			// Available ≠ latest: run an update check right at startup (it
			// short-circuits internally when auto-update is off, the PATH
			// preference is set, there's no managed marker, etc.) —
			// otherwise nothing checks until the loop's first round after
			// AUTO_UPDATE_FIRST_DELAY
			check_and_update(&app_state, &emitter).await;
			return;
		}

		// 已选 tools 但组件不齐 → 让用户选择要下载的构建版本（平台轨 × 变体），
		// 不再自动安装默认版；平台不支持下载时跳过（弹了也下载不了）
		// The chosen tools set is incomplete → let the user pick the build to
		// download (platform track × variant) instead of auto-installing the
		// default; skipped on platforms that can't download anyway
		if refill_tools {
			prompt_build_choice(&emitter);
		}

		if !available {
			// 分级日志后统一发缺失通知：PATH 首选尊重用户自管；平台不支持说明
			// 原因；其余（未设首选项 / tools 文件在但跑不起来）按通用缺失处理。
			// 前端 UI 由首选项向导（preference 未设）、Setup 向导与
			// `ffmpeg-choice-needed` 选择对话框覆盖。
			// A differentiated log then one shared missing notification: a PATH
			// preference means the user manages it; an unsupported platform says
			// why; the rest (unset preference / tools files present but not
			// runnable) is generic. The frontend UI is covered by the preference
			// wizard (preference unset), the setup wizard and the
			// `ffmpeg-choice-needed` build-choice dialog.
			if crate::ffmpeg::raw_preference() == Some(0) {
				tracing::info!("{}", crate::tl!("ffmpeg.systemPreferenceMissing"));
			} else if build_platform().is_none() {
				tracing::warn!(
					"{}",
					crate::tl!(
						"ffmpeg.unsupportedPlatform",
						os = std::env::consts::OS,
						arch = std::env::consts::ARCH
					)
				);
			} else {
				tracing::warn!("{}", crate::tl!("startup.ffmpegMissing"));
			}
			app_state.notification_store.emit_i18n(
				emitter.as_ref(),
				NotificationLevel::Error,
				"startup",
				"ffmpeg not found. Recording and post-processing will be unavailable.",
				"notifications.backend.ffmpegMissing",
				None,
			);
		}
	});
}

/// 已选 tools 但组件不齐且平台支持下载时广播 `ffmpeg-choice-needed` 事件：
/// 前端据此弹"选择构建版本"对话框，确认后走 `POST /api/ffmpeg/download` /
/// `download_ffmpeg` 手动路径（携带所选平台轨/变体）。缺失场景不再自动装
/// 默认版——唯一保留的自动下载是已装副本的更新（沿用标记里的已选轨）。
/// 启动与更新循环共用。
///
/// Broadcast `ffmpeg-choice-needed` when the chosen tools set is incomplete
/// and the platform supports downloads: the frontend pops a pick-a-build
/// dialog whose confirm goes through the manual download path (carrying the
/// chosen track/variant). Missing installs never auto-install the default —
/// the only remaining auto-download is updating an installed copy (keeping
/// the marker's chosen track). Shared by startup and the update loop.
fn prompt_build_choice(emitter: &Arc<dyn Emitter>) {
	if build_platform().is_some() {
		tracing::info!("{}", crate::tl!("ffmpeg.choiceNeeded"));
		emitter.emit("ffmpeg-choice-needed", &serde_json::json!({}));
	}
}

/// 手动触发 tools 目录的 FFmpeg 下载安装（首选项向导"下载"按钮、
/// `POST /api/ffmpeg/download` / `download_ffmpeg` 命令用）。
/// 平台不支持下载时直接返回；手动点击是明确意图，不按 PATH 首选项拦截——
/// "PATH 自管"策略只约束启动/更新等自动路径，因此前端可以先触发下载、
/// 等安装完成后再把 tools 写入首选项。与启动/更新路径共用
/// [`INSTALL_MUTEX`] 串行化，已有任务在跑时会排队等待而非并发。
/// 后台 `tokio::spawn` 执行不等待完成；进度与错误经 `ffmpeg-progress`
/// SSE 事件广播，失败只记日志（用户在界面前，可重新点击触发）。
///
/// Manually trigger a tools-directory FFmpeg download/install (the preference
/// wizard's Download button, `POST /api/ffmpeg/download`, and the
/// `download_ffmpeg` command). No-op when the platform can't download; an
/// explicit click is deliberate intent — the PATH preference does not block
/// it (the PATH self-manage policy only constrains the automatic startup/
/// update paths), so the frontend may download first and persist the tools
/// preference after the install lands. Shares [`INSTALL_MUTEX`] with the
/// startup/update paths so a running task queues instead of racing. Runs via
/// `tokio::spawn` without awaiting; progress and errors broadcast over the
/// `ffmpeg-progress` SSE event, failures are log-only (the user can re-click).
///
/// `choice` 携带前端选择的平台轨/构建变体；任一字段缺省时自动解析
/// （保留已安装标记的轨与形态，否则宿主默认轨 + 静态）。
/// `choice` carries the frontend's platform-track/variant pick; each field
/// auto-resolves when absent (keeping the installed marker's values, else
/// host default + static).
pub fn spawn_manual_install(
	app_state: Arc<AppState>,
	emitter: Arc<dyn Emitter>,
	choice: FFmpegDownloadChoice,
) {
	// 手动点击即明确意图：不按 PATH 首选项拦截（PATH 自管策略只约束自动
	// 路径）——前端可借此"先下载、装完再写首选项" / An explicit click is
	// deliberate intent: the PATH preference does not block it (the PATH
	// self-manage policy only constrains the automatic paths) — the frontend
	// can download first and persist the preference after install
	if build_platform().is_none() {
		return;
	}
	tokio::spawn(async move {
		if let Err(e) = install(FFmpegKind::Install, choice, &app_state, &emitter).await {
			tracing::warn!(
				"{}",
				crate::tl!("ffmpeg.manualDownloadFailed", error = e.to_string())
			);
		}
	});
}

// ─── 自动更新 / Auto-update ──────────────────────────────────────────────────

/// FFmpeg 自动更新循环（由 `scheduler::start_ffmpeg_update` 启动）：
/// 启动 5 分钟后跑第一轮，之后每小时一轮。每轮：
/// (a) 已选 tools 但组件不齐 → 广播 `ffmpeg-choice-needed` 提醒用户选择
///     构建下载（不自动安装；启动路径已就"缺失"发过通知/弹过提示，
///     这里不重复通知）
/// (b) ffmpeg 可用 + `ffmpeg_auto_update` 开启 + tools 里是带标记的受管副本 +
///     平台支持 → 拉远端 `.sha256` 与标记比对，不同则等到空闲后执行更新；
///     更新失败推送 `ffmpegUpdateFailed` 警告通知，保留旧版本继续用；
///     检查本身失败只记 debug 日志
///
/// FFmpeg auto-update loop (launched by `scheduler::start_ffmpeg_update`): first
/// round 5 minutes after launch, then every hour. Each round:
/// (a) the chosen tools set incomplete → broadcast `ffmpeg-choice-needed`
///     reminding the user to pick a build (never auto-installs; the startup
///     path already notified/prompted about the missing ffmpeg, so nothing
///     is re-notified here)
/// (b) ffmpeg usable + `ffmpeg_auto_update` on + a managed (marker-bearing) tools
///     copy + supported platform → fetch the remote `.sha256` and compare with
///     the marker; on difference, wait for idle then run the update. A failed
///     update pushes a `ffmpegUpdateFailed` warning notification and keeps the
///     old binaries; check failures are debug-logged only
pub async fn auto_update_loop(app_state: Arc<AppState>, emitter: Arc<dyn Emitter>) {
	tokio::time::sleep(AUTO_UPDATE_FIRST_DELAY).await;
	loop {
		run_update_round(&app_state, &emitter).await;
		tokio::time::sleep(AUTO_UPDATE_INTERVAL).await;
	}
}

/// [`auto_update_loop`] 的单轮逻辑。
/// One round of [`auto_update_loop`].
async fn run_update_round(app_state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
	// (a) 已选 tools 但组件不齐 → 提醒用户选择构建下载，不自动安装（启动路径
	//     已就"缺失"发过通知/弹过提示，这里不重复通知）。其余不可用形态
	//     （PATH 首选缺失 / 未设首选项）分别由用户自管与前端首选项向导覆盖，
	//     本循环不处理。
	// (a) The chosen tools set is incomplete → remind the user to pick a build
	//     (never auto-installs; the startup path already notified/prompted the
	//     missing state, nothing is re-notified). Other unavailable shapes
	//     (PATH preference missing / preference unset) are the user's own
	//     management or covered by the preference wizard — not handled here.
	let available = tokio::task::spawn_blocking(crate::ffmpeg::ffmpeg_available)
		.await
		.unwrap_or(false);
	let refill_tools =
		crate::ffmpeg::raw_preference() == Some(1) && !crate::ffmpeg::location_complete(1);
	if refill_tools {
		prompt_build_choice(emitter);
		return;
	}
	if !available {
		return;
	}

	// (b) ffmpeg 可用 → 受管副本的更新检查
	// (b) ffmpeg usable → update check for the managed copy
	check_and_update(app_state, emitter).await;
}

/// FFmpeg 更新检查（要求 ffmpeg 已可用，由调用方保证）：`ffmpeg_auto_update`
/// 关闭、首选项为系统 PATH、平台不支持下载或 tools 副本无标记（非本程序
/// 安装，如用户手动放置）时直接返回；否则拉远端
/// `.sha256` 与标记比对，不同则等空闲后执行更新。更新失败推送
/// `ffmpegUpdateFailed` 警告通知并保留旧版；检查本身失败只记 debug 日志。
/// 启动（ffmpeg 已可用分支）与周期循环共用此检查。
///
/// FFmpeg update check (caller guarantees ffmpeg is usable): returns early
/// when `ffmpeg_auto_update` is off, the preference is the system PATH, the
/// platform can't download, or the tools copy has no marker (not installed by
/// us — e.g. placed manually). Otherwise
/// fetches the remote `.sha256`, compares with the marker, and on difference
/// waits for idle and updates. A failed update pushes the
/// `ffmpegUpdateFailed` warning notification and keeps the old version;
/// check failures are debug-logged only. Shared by startup (ffmpeg-already-
/// usable branch) and the periodic loop.
async fn check_and_update(app_state: &Arc<AppState>, emitter: &Arc<dyn Emitter>) {
	// 首选项为系统环境变量时不维护 tools 副本
	// Skipped when the preference is the system PATH — no point maintaining a
	// tools copy the user doesn't use
	let settings = app_state.get_settings();
	if !settings.ffmpeg_auto_update || crate::ffmpeg::raw_preference() == Some(0) {
		return;
	}
	// 无标记 = 非本程序安装（PATH 或用户手动放入 tools），不替换
	// No marker = not installed by us (PATH or manually placed in tools) — never replaced
	let Some(marker) = read_marker() else {
		return;
	};
	// 校验文件取标记记录的平台轨+变体：更新保持与安装同一轨（请求缺省 →
	// 标记 → 宿主默认的解析链与 install 内一致）
	// The checksum asset follows the marker's recorded track + variant so an
	// update stays on the same track as the install (absent-request → marker
	// → host-default, matching the resolution inside install)
	let Ok(spec) = resolve_asset(None, None, Some(&marker)) else {
		return;
	};

	let checksum_name = format!("{}.sha256", spec.archive_name());
	let checksum_url = format!("{}/{}", RELEASE_BASE, checksum_name);
	let checksum_url = crate::postprocess::community::apply_mirror(
		&checksum_url,
		settings.community_mirror_url.as_deref(),
	);
	let proxy = download_proxy(&settings);

	let remote_sha = async {
		let client = http_client(proxy.as_deref(), SMALL_TIMEOUT_SECS)?;
		let resp = client
			.get(&checksum_url)
			.send()
			.await
			.map_err(|e| AppError::Other(e.to_string()))?;
		if !resp.status().is_success() {
			return Err(AppError::Other(format!("HTTP {}", resp.status())));
		}
		let text = resp
			.text()
			.await
			.map_err(|e| AppError::Other(e.to_string()))?;
		parse_sha256_file(&text)
			.ok_or_else(|| AppError::Other("invalid checksum content".to_string()))
	}
	.await;

	let remote_sha = match remote_sha {
		Ok(h) => h,
		Err(e) => {
			tracing::debug!("{}", crate::tl!("ffmpeg.updateCheckFailed", error = e));
			return;
		}
	};

	if remote_sha.eq_ignore_ascii_case(&marker.archive_sha256) {
		return;
	}

	tracing::info!(
		"{}",
		crate::tl!("ffmpeg.updateAvailable", hash = remote_sha)
	);
	// 入库通知"检测到新版本"：到真正下载之间还隔着等待空闲（可能数小时），
	// 没有这条的话更新已在排队的事实完全无迹可寻
	// Store an "update detected" notification: the wait-for-idle below can
	// last hours, and without this entry a queued update leaves no trace
	app_state.notification_store.emit_i18n(
		emitter.as_ref(),
		NotificationLevel::Info,
		"ffmpeg",
		"A new FFmpeg version is available and will be installed when the program is idle",
		"notifications.backend.ffmpegUpdateAvailable",
		None,
	);

	// 等到空闲再更新；等待期间用户可能关闭自动更新 → 本轮中止
	// Wait for idle before updating; the user may disable auto-update meanwhile →
	// abort this round
	if !crate::ffmpeg::is_idle(app_state) {
		tracing::debug!("{}", crate::tl!("ffmpeg.waitingForIdle"));
	}
	loop {
		if !app_state.get_settings().ffmpeg_auto_update {
			return;
		}
		if crate::ffmpeg::is_idle(app_state) {
			break;
		}
		tokio::time::sleep(IDLE_POLL_INTERVAL).await;
	}

	match install(
		FFmpegKind::Update,
		FFmpegDownloadChoice::default(),
		app_state,
		emitter,
	)
	.await
	{
		Ok(()) => {}
		Err(e) => {
			// install 内部因设置被关闭而中止时不发通知（不算失败）
			// No notification when install aborted because the setting was turned
			// off mid-flight (not a failure)
			if app_state.get_settings().ffmpeg_auto_update {
				tracing::warn!("{}", crate::tl!("ffmpeg.updateFailed", error = e));
				let mut args = HashMap::new();
				args.insert("error".to_string(), serde_json::json!(e.to_string()));
				app_state.notification_store.emit_i18n(
					emitter.as_ref(),
					NotificationLevel::Warning,
					"ffmpeg",
					format!(
						"FFmpeg auto-update failed: {} (continuing with the current version)",
						e
					),
					"notifications.backend.ffmpegUpdateFailed",
					Some(args),
				);
			} else {
				tracing::debug!("{}", crate::tl!("ffmpeg.updateFailed", error = e));
			}
		}
	}
}

/// 当前的 FFmpeg 下载/安装进度（供 `GET /api/ffmpeg/status` 与 Tauri 命令使用）。
/// Current FFmpeg download/install progress (for `GET /api/ffmpeg/status` and
/// the Tauri command).
pub fn current_progress(app_state: &AppState) -> FFmpegProgress {
	app_state.ffmpeg_state.read().clone()
}
