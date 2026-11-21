//! 清理子模块：孤立 meta、meta 临时文件、空主播子目录与 tmp 目录的清理。
//! Cleanup submodule: orphaned meta files, meta temp files, empty streamer
//! subdirectories and tmp-directory cleanup.

use super::super::model::{VideoMeta, list_all_meta_paths, meta_dir};
use super::super::store::is_meta_write_tmp_name;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// meta 原子写入临时文件的最小清理年龄。`write_meta` 从写临时文件到 rename 只需毫秒级，
/// 超过 1 小时仍存在的必然是进程崩溃留下的残留，不会误删正在写入的文件。
/// Minimum age before a meta atomic-write temp file is cleaned up. `write_meta` goes from
/// writing the temp file to the rename within milliseconds, so one still present after an
/// hour is certainly a crash leftover and never a file being written.
const META_TMP_MIN_AGE: Duration = Duration::from_secs(3600);

/// tmp 过期文件的年龄阈值：修改时间距今超过该值的文件视为残留并删除。
/// 维护每 5 分钟执行一次，因此残留文件最晚在约 1 小时 5 分钟后被清理。
/// Age threshold for stale tmp files: files modified longer ago than this are treated as
/// leftovers and removed. Maintenance runs every 5 minutes, so leftovers are gone within
/// roughly 1 hour 5 minutes.
const STALE_TMP_MAX_AGE: Duration = Duration::from_secs(3600);

/// 扫描 meta/ 目录（含所有主播子目录），删除所有对应视频文件（或 session_dir）
/// 已不存在的孤立 meta 文件；之后每轮都会移除已空置超过 60 秒的主播子目录
/// （见 [`remove_empty_meta_subdirs`]，本轮刚变空的子目录由下一轮清理）。
///
/// 孤立判断的唯一依据：`video_path` 字段指向的路径（目录或视频文件）是否存在。
/// 不再叠加任何基于 `status` 的前置过滤——`status` 是否为 "recording"/"pp_waiting"/
/// "pp_running" 与孤立判断无关：只要该路径确实存在（无论录制还是处理中都会持续占用该
/// 路径），meta 自然不会被判定为孤立；只要该路径不存在，无论 status 停留在什么值
/// （包括因进程重启等原因卡在中间状态的陈旧记录），都应视为孤立并清理。
/// 之前基于 status 的前置跳过会掩盖这类陈旧记录，导致孤立 meta 无法被清理。
///
/// 每轮还会删除 `write_meta` 崩溃残留、超过 1 小时的临时文件（见
/// [`cleanup_stale_meta_tmp_files_in`]）。
///
/// Each pass also removes `write_meta` crash-leftover temp files older than one hour (see
/// [`cleanup_stale_meta_tmp_files_in`]).
///
/// Scan the meta/ directory (including all per-streamer subdirectories) and delete
/// orphaned meta files whose corresponding video file or session_dir no longer exists;
/// afterwards, every pass removes streamer subdirectories that have been empty for more than
/// 60 seconds (see [`remove_empty_meta_subdirs`]; subdirectories emptied in this pass are
/// removed by the next one).
///
/// The sole criterion for "orphaned": whether the path referenced by `video_path`
/// (a directory or video file) exists. No status-based pre-filter is applied anymore —
/// whether `status` is "recording"/"pp_waiting"/"pp_running" is irrelevant to the orphan
/// check: as long as the path genuinely exists (recording or processing both keep the path
/// present), the meta naturally won't be flagged as orphaned; as long as the path doesn't
/// exist, it's orphaned and should be cleaned up regardless of what `status` says (including
/// stale records stuck mid-state due to a process restart). The previous status-based
/// pre-filter masked exactly these stale records, preventing orphaned meta from being cleaned.
pub fn cleanup_orphaned_meta_files() -> usize {
	let paths = list_all_meta_paths();

	let mut count = 0usize;
	for path in paths {
		let name = path.to_string_lossy().to_string();

		let meta_content = match std::fs::read_to_string(&path) {
			Ok(c) => c,
			Err(_) => continue,
		};
		let meta: VideoMeta = match serde_json::from_str(&meta_content) {
			Ok(m) => m,
			Err(_) => continue,
		};

		// 没有 video_path 字段时无法判断，保守跳过（旧版 meta 无此字段）
		// Cannot determine without video_path; skip conservatively (old meta format lacks it)
		let video_path_str = match meta.video_path.as_deref() {
			Some(p) => p.to_string(),
			None => continue,
		};

		// 唯一依据：路径存在则不是孤立 / Sole criterion: path exists → not orphaned
		if std::path::Path::new(&video_path_str).exists() {
			continue;
		}

		if let Err(e) = std::fs::remove_file(&path) {
			tracing::warn!(
				"{}",
				crate::tl!("meta.cleanupDeleteFailed", name = name, error = e)
			);
		} else {
			tracing::info!("{}", crate::tl!("meta.cleanupDeleted", name = name));
			count += 1;
		}
	}

	if count > 0 {
		tracing::info!("{}", crate::tl!("meta.cleanupDone", count = count));
	}
	// 清理 write_meta 崩溃残留的临时文件（不计入返回值：返回值用于"孤立 meta 已清理"通知）。
	// 放在空子目录清理之前，否则只剩残留临时文件的子目录永远不会变空。
	// Clean up temp files left by write_meta crashes (not counted in the return value, which
	// drives the "orphaned meta cleaned" notification). Done before the empty-subdirectory
	// cleanup, otherwise a subdirectory holding only leftover temp files would never become empty.
	cleanup_stale_meta_tmp_files_in(&meta_dir(), META_TMP_MIN_AGE);
	// 每轮都清理空子目录（不只在本轮删过 meta 时）：有最小年龄保护后，本轮因删掉最后一个
	// meta 而变空的子目录修改时间刚被刷新，会被本轮跳过，需由下一轮清理
	// Clean up empty subdirectories on every pass (not only when this pass deleted meta): with
	// the minimum-age protection, a subdirectory emptied by deleting its last meta in this pass
	// has a fresh mtime and is skipped now, so a later pass has to remove it
	remove_empty_meta_subdirs();
	count
}

/// 删除 meta 根目录（含一层主播子目录）下 `write_meta` 原子写入残留的临时文件
/// （`{stem}.json.{pid}.{seq}.tmp`，见 [`is_meta_write_tmp_name`]），只删除修改时间距今
/// 不少于 `min_age` 的文件；读不到修改时间时保守跳过。返回删除数量。
///
/// Remove `write_meta` atomic-write temp files (`{stem}.json.{pid}.{seq}.tmp`, see
/// [`is_meta_write_tmp_name`]) left under the meta root (including the single level of
/// streamer subdirectories), only when modified at least `min_age` ago; files whose mtime
/// can't be read are skipped conservatively. Returns the number removed.
fn cleanup_stale_meta_tmp_files_in(root: &Path, min_age: Duration) -> usize {
	let mut dirs = vec![root.to_path_buf()];
	if let Ok(entries) = std::fs::read_dir(root) {
		dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
	}
	let now = SystemTime::now();
	let mut removed = 0usize;
	for dir in dirs {
		let Ok(entries) = std::fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries.flatten() {
			let path = entry.path();
			if !path.is_file()
				|| !path
					.file_name()
					.and_then(|n| n.to_str())
					.is_some_and(is_meta_write_tmp_name)
			{
				continue;
			}
			let old_enough = std::fs::metadata(&path)
				.and_then(|m| m.modified())
				.ok()
				// 修改时间晚于当前时间（时钟误差）视为年龄 0 / mtime in the future (clock skew) counts as age 0
				.map(|t| now.duration_since(t).unwrap_or(Duration::ZERO))
				.is_some_and(|age| age >= min_age);
			if !old_enough {
				continue;
			}
			match std::fs::remove_file(&path) {
				Ok(()) => removed += 1,
				Err(e) => tracing::warn!(
					"{}",
					crate::tl!(
						"meta.cleanupTmpDeleteFailed",
						name = path.display(),
						error = e
					)
				),
			}
		}
	}
	if removed > 0 {
		tracing::info!("{}", crate::tl!("meta.cleanupTmpDone", count = removed));
	}
	removed
}

/// 移除 meta 根目录下已变空的主播子目录（如该主播的所有录制都已被删除/清理）。
///
/// 复用输出目录空目录清理的最小年龄保护（`segment_merge::EMPTY_DIR_MIN_AGE`）：修改时间
/// 不足 60 秒或读不到修改时间的目录跳过，删除前再确认目录为空，meta 根目录本身不删。
/// 保护的是 `write_meta` 在 `create_dir_all` 之后、写临时文件之前的窗口：新主播的子目录
/// 刚建好、尚未写入 meta 时不会被删掉。本轮跳过的旧空目录由后续轮次清理。
///
/// Remove now-empty streamer subdirectories under the meta root (e.g. all of a
/// streamer's recordings have been deleted/cleaned up).
///
/// Reuses the output-dir empty-dir cleanup's minimum-age protection
/// (`segment_merge::EMPTY_DIR_MIN_AGE`): directories modified within the last 60 seconds, or
/// whose mtime can't be read, are skipped; emptiness is confirmed right before removal, and
/// the meta root itself is never removed. This protects `write_meta`'s window between
/// `create_dir_all` and writing the temp file: a new streamer's subdirectory that was just
/// created and has no meta yet isn't removed. Old empty directories skipped in this pass are
/// cleaned up by later passes.
fn remove_empty_meta_subdirs() {
	remove_empty_meta_subdirs_in(&meta_dir());
}

/// [`remove_empty_meta_subdirs`] 的实现，作用于指定 meta 根目录，返回删除的目录数。
/// meta 子目录只有一层，递归版本的效果等同于一层检查。
///
/// Implementation of [`remove_empty_meta_subdirs`] for the given meta root, returning the
/// number of directories removed. Meta subdirectories are a single level deep, so the
/// recursive helper behaves the same as a one-level check.
fn remove_empty_meta_subdirs_in(root: &Path) -> usize {
	crate::recording::segment_merge::remove_empty_dirs_recursive(
		root,
		false,
		&|_| false,
		crate::recording::segment_merge::EMPTY_DIR_MIN_AGE,
	)
}

/// 清理 tmp 目录中的过期内容：
///
/// 1. **过期文件**：修改时间超过 [`STALE_TMP_MAX_AGE`]（1 小时）的文件直接删除
///    （不受大小限制影响）。这类文件是后处理任务失败/中断时遗留的中间产物。
/// 2. **空子目录**：删除文件后遗留的空子目录一并清理（跳过 60 秒内修改过的目录）。
/// 3. **大小上限兜底**：若清理过期文件后目录仍超出 `max_tmp_gb` 限制，
///    再按修改时间从旧到新继续删文件，直到大小低于上限。
///
/// **运行中任务保护**：阈值缩短到 1 小时后，运行时间较长的任务（如通过慢速代理上传大分片
/// 的 notify_telegram）自己创建、仍在使用的文件也可能超过 1 小时。模块写入 tmp 的顶层
/// 文件/目录名都带输入视频的 stem，因此名称包含 `protected_stems`（已 claim 的录制 stem，
/// 见 `PpQueue::active_recording_stems`）中任一项的顶层条目及其下所有内容在以上三步中都
/// 整体跳过，等任务结束后再按规则清理。
///
/// `max_tmp_gb` 为 0 时跳过大小兜底逻辑（不限制大小，但仍清理过期文件）。
///
/// Clean up stale content in the tmp directory:
///
/// 1. **Stale files**: files not modified within [`STALE_TMP_MAX_AGE`] (1 hour) are deleted
///    unconditionally (regardless of size limit). These are leftover intermediates from
///    failed/interrupted post-processing tasks.
/// 2. **Empty subdirectories**: empty subdirectories left after file deletion are also removed
///    (directories modified within the last 60 seconds are skipped).
/// 3. **Size cap fallback**: if the directory still exceeds `max_tmp_gb` after removing
///    stale files, continue deleting from oldest to newest until under the limit.
///
/// **Running-task protection**: with the threshold down to 1 hour, files a long-running task
/// created and is still using (e.g. notify_telegram uploading large parts over a slow proxy)
/// may also be older than an hour. Modules name their top-level tmp files/dirs after the input
/// video's stem, so any top-level entry whose name contains one of `protected_stems` (stems of
/// claimed recordings, see `PpQueue::active_recording_stems`) is skipped entirely, with
/// everything under it, by all three steps, and is cleaned by the usual rules once the task ends.
///
/// When `max_tmp_gb` is 0, the size cap fallback is skipped (size is unlimited, but stale file
/// cleanup still runs).
pub(super) fn cleanup_stale_tmp(
	tmp: &std::path::Path,
	max_tmp_gb: f64,
	protected_stems: &[String],
) {
	if !tmp.exists() {
		return;
	}

	let max_age = STALE_TMP_MAX_AGE;
	let now = std::time::SystemTime::now();

	// 路径所属的 tmp 顶层条目名包含运行中录制的 stem 时视为受保护
	// A path is protected when its top-level tmp entry's name contains a running recording's stem
	let is_protected = |p: &Path| -> bool {
		p.strip_prefix(tmp)
			.ok()
			.and_then(|rel| rel.components().next())
			.and_then(|c| c.as_os_str().to_str())
			.is_some_and(|name| protected_stems.iter().any(|s| name.contains(s.as_str())))
	};

	// 递归收集所有文件（含子目录内文件），排除受保护条目
	// Recursively collect all files (including inside subdirectories), excluding protected entries
	let mut all_files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
	collect_files_recursive(tmp, &mut all_files);
	all_files.retain(|(p, _, _)| !is_protected(p));

	let mut removed_bytes: u64 = 0;
	let mut removed_count: usize = 0;

	// 第一轮：删除超过 1 小时未修改的文件
	// Round 1: delete files not modified within the last hour
	let mut remaining_files: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
	for (path, size, modified) in all_files {
		let age = now.duration_since(modified).unwrap_or(max_age);
		if age >= max_age {
			if std::fs::remove_file(&path).is_ok() {
				removed_bytes += size;
				removed_count += 1;
			}
		} else {
			remaining_files.push((path, size, modified));
		}
	}

	// 第二轮：大小兜底——若仍超出上限，从旧到新继续删
	// Round 2: size fallback — if still over cap, delete from oldest to newest
	if max_tmp_gb > 0.0 {
		let max_bytes = (max_tmp_gb * 1024.0 * 1024.0 * 1024.0) as u64;
		let current: u64 = remaining_files.iter().map(|(_, s, _)| s).sum();
		if current > max_bytes {
			remaining_files.sort_by_key(|(_, _, t)| *t);
			let mut total = current;
			for (path, size, _) in &remaining_files {
				if total <= max_bytes {
					break;
				}
				if std::fs::remove_file(path).is_ok() {
					removed_bytes += size;
					removed_count += 1;
					total = total.saturating_sub(*size);
				}
			}
		}
	}

	// 清理空子目录（递归，从深到浅，不删 tmp 根目录）。本函数可能与维护触发的后台后处理
	// 并行，因此复用输出目录空目录清理的最小年龄保护：跳过 60 秒内修改过的目录，避免误删
	// notify_telegram 刚创建、ffmpeg 尚未写入的 split_* 目录。本轮刚删掉文件或子目录的
	// 目录修改时间会被刷新，由下一次维护（5 分钟后）清理。
	// Remove empty subdirectories (recursive, deepest first, keeping the tmp root). This may
	// run in parallel with background post-processing triggered by maintenance, so reuse the
	// output dir cleanup's minimum-age protection: skip directories modified within the last
	// 60 seconds, so split_* dirs just created by notify_telegram and not yet written by ffmpeg
	// aren't removed. Directories whose files/subdirs were removed in this pass get a fresh
	// mtime and are cleaned up by the next maintenance pass (5 minutes later).
	// 运行中任务的受保护目录同样跳过 / Protected directories of running tasks are skipped as well
	crate::recording::segment_merge::remove_empty_dirs_recursive(
		tmp,
		false,
		&is_protected,
		crate::recording::segment_merge::EMPTY_DIR_MIN_AGE,
	);

	if removed_count > 0 {
		tracing::info!(
			"{}",
			crate::tl!(
				"maintenance.tmpCleanup",
				count = removed_count,
				mb = format!("{:.1}", removed_bytes as f64 / 1024.0 / 1024.0)
			)
		);
	}
}

/// 程序启动时清空 tmp 目录（`exe_dir()/tmp`）中的全部内容，保留 tmp 根目录本身。
///
/// 启动时上一个进程的后处理都已结束，tmp 里的一切都是残留（中断的上传分片、缩略图、
/// 截帧目录等），无需等定时清理的 1 小时阈值，也不需要运行中任务保护。必须在任何可能
/// 运行模块的组件（状态监控、维护调度、录制）启动之前调用：两端都在启动迁移阶段调用。
/// 代价：Telegram 上传失败后保留的分片不再能跨重启复用，下次重试会重新切割（流复制，较快）。
///
/// Clear everything in the tmp directory (`exe_dir()/tmp`) at program startup, keeping the
/// tmp root itself.
///
/// At startup all post-processing of the previous process has ended, so everything in tmp is
/// a leftover (interrupted upload parts, thumbnails, frame-extraction dirs, etc.); there is no
/// need to wait for the scheduled cleanup's 1-hour threshold, nor for running-task protection.
/// Must be called before any component that may run modules (status monitor, maintenance
/// scheduler, recording) starts: both ends call it during the startup migration phase.
/// Trade-off: split parts kept after a failed Telegram upload can no longer be reused across a
/// restart; the next retry re-splits (stream copy, fairly fast).
pub fn cleanup_tmp_on_startup() -> usize {
	cleanup_tmp_on_startup_in(&crate::config::app_state::exe_dir().join("tmp"))
}

/// [`cleanup_tmp_on_startup`] 的实现，作用于指定 tmp 目录，返回删除的文件数。
/// Implementation of [`cleanup_tmp_on_startup`] for the given tmp dir; returns the number of
/// files removed.
fn cleanup_tmp_on_startup_in(tmp: &Path) -> usize {
	let Ok(entries) = std::fs::read_dir(tmp) else {
		return 0;
	};
	let mut removed_count = 0usize;
	let mut removed_bytes = 0u64;
	for entry in entries.flatten() {
		let path = entry.path();
		// 不跟随符号链接 / Don't follow symlinks
		let Ok(meta) = std::fs::symlink_metadata(&path) else {
			continue;
		};
		let (result, count, bytes) = if meta.is_dir() {
			let mut files = Vec::new();
			collect_files_recursive(&path, &mut files);
			let bytes: u64 = files.iter().map(|(_, s, _)| s).sum();
			(std::fs::remove_dir_all(&path), files.len(), bytes)
		} else {
			(std::fs::remove_file(&path), 1, meta.len())
		};
		match result {
			Ok(()) => {
				removed_count += count;
				removed_bytes += bytes;
			}
			Err(e) => tracing::warn!(
				"{}",
				crate::tl!(
					"maintenance.tmpStartupDeleteFailed",
					path = path.display(),
					error = e
				)
			),
		}
	}
	if removed_count > 0 {
		tracing::info!(
			"{}",
			crate::tl!(
				"maintenance.tmpStartupCleanup",
				count = removed_count,
				mb = format!("{:.1}", removed_bytes as f64 / 1024.0 / 1024.0)
			)
		);
	}
	removed_count
}

/// 递归收集目录下所有文件及其元数据。
/// Recursively collect all files under a directory with their metadata.
fn collect_files_recursive(
	dir: &std::path::Path,
	out: &mut Vec<(std::path::PathBuf, u64, std::time::SystemTime)>,
) {
	let entries = match std::fs::read_dir(dir) {
		Ok(e) => e,
		Err(_) => return,
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			collect_files_recursive(&path, out);
		} else if let Ok(meta) = std::fs::metadata(&path) {
			let size = meta.len();
			let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
			out.push((path, size, modified));
		}
	}
}
