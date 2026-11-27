//! Meta 一次性迁移子模块：旧版扁平/截断命名的 meta 文件迁移到新布局。
//! One-shot meta migrations: legacy flat / truncated-stem meta files are
//! migrated into the new layout.

use super::super::model::{
	VideoMeta, legacy_truncated_meta_target, list_all_meta_paths_in, meta_dir, meta_dir_for,
	username_from_path,
};
use std::path::Path;

/// 一次性迁移：将 `meta_dir()` 根目录下的旧版扁平 meta 文件（升级前生成，直接平铺
/// 存放、不含主播子目录）移动到按主播分子目录的新结构（`meta_dir()/{username}/{stem}.json`）。
///
/// 每个文件的目标子目录从其自身 `video_path` 字段推断用户名（[`username_from_path`]）；
/// 若文件内容无法解析或缺少 `video_path` 字段，保守跳过（保留在原位，不强行猜测，
/// 后续若确实孤立会被 [`super::cleanup::cleanup_orphaned_meta_files`] 处理，但由于本函数仅基于文件
/// 内容判断而非路径存在性，跳过不会误删任何数据）。已经位于子目录下的文件不受影响。
///
/// 应在程序启动时、任何其他 meta 扫描/读取发生之前调用一次；重复调用是幂等的——
/// 已迁移的文件不再存在于根目录，不会被再次处理。
///
/// One-shot migration: move legacy flat meta files directly under `meta_dir()` root
/// (generated before this change, with no per-streamer subdirectory) into the new
/// per-streamer layout (`meta_dir()/{username}/{stem}.json`).
///
/// Each file's target subdirectory is inferred from its own `video_path` field
/// ([`username_from_path`]); files that fail to parse or lack `video_path` are skipped
/// conservatively (left in place rather than guessed at — if genuinely orphaned they'll
/// later be handled by [`super::cleanup::cleanup_orphaned_meta_files`], and since this function only
/// acts based on file content rather than path existence, skipping never deletes data).
/// Files already inside a subdirectory are untouched.
///
/// Must be called once at startup, before any other meta scan/read happens; repeated
/// calls are idempotent — migrated files no longer exist at the root and won't be
/// reprocessed.
pub fn migrate_flat_meta_files() -> usize {
	let dir = meta_dir();
	let entries = match std::fs::read_dir(&dir) {
		Ok(e) => e,
		Err(_) => return 0,
	};

	let mut migrated = 0usize;
	for entry in entries.flatten() {
		let path = entry.path();
		// 子目录（新结构）或非 json 文件，跳过 / Subdirectory (new layout) or non-json file, skip
		if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
			continue;
		}

		let content = match std::fs::read_to_string(&path) {
			Ok(c) => c,
			Err(_) => continue,
		};
		let meta: VideoMeta = match serde_json::from_str(&content) {
			Ok(m) => m,
			Err(_) => continue,
		};
		let video_path_str = match meta.video_path.as_deref() {
			Some(p) => p,
			None => continue,
		};

		let username = username_from_path(Path::new(video_path_str));
		let target_dir = meta_dir_for(&username);
		if let Err(e) = std::fs::create_dir_all(&target_dir) {
			tracing::warn!(
				"{}",
				crate::tl!(
					"meta.migrationCreateDirFailed",
					dir = target_dir.display(),
					error = e
				)
			);
			continue;
		}
		let Some(file_name) = path.file_name() else {
			continue;
		};
		let target_path = target_dir.join(file_name);
		if target_path.exists() {
			tracing::warn!(
				"{}",
				crate::tl!("meta.migrationTargetExists", path = path.display())
			);
			continue;
		}
		match std::fs::rename(&path, &target_path) {
			Ok(()) => {
				tracing::info!(
					"{}",
					crate::tl!(
						"meta.migrationMoved",
						from = path.display(),
						to = target_path.display()
					)
				);
				migrated += 1;
			}
			Err(e) => {
				tracing::warn!(
					"{}",
					crate::tl!("meta.migrationMoveFailed", path = path.display(), error = e)
				);
			}
		}
	}

	if migrated > 0 {
		tracing::info!("{}", crate::tl!("meta.migrationDone", count = migrated));
	}
	migrated
}

/// 一次性迁移：把按旧 stem 规则（`file_stem()`）截断文件名的 meta 重命名为新规则
/// （[`super::super::model::recording_stem`]）下的文件名（B16 兼容）。
///
/// 旧规则会把含 `.` 用户名的 session_dir（如 `a.b_20240101_120000`）截断成 `a.json`。
/// 判断依据是 meta 自身的 `video_path`（见 [`legacy_truncated_meta_target`]）；普通用户名
/// 不受影响。只在同一目录内 rename，不删除任何文件；目标已存在时保留原文件并记录警告。
/// 必须在首次扫描前调用（两端启动时都会调用），重复调用是幂等的。
///
/// One-shot migration: rename meta files whose names were truncated by the old stem rule
/// (`file_stem()`) to the name under the new rule ([`super::super::model::recording_stem`]) (B16
/// compatibility).
///
/// The old rule truncated a session_dir with a `.` in the username (e.g.
/// `a.b_20240101_120000`) to `a.json`. The decision is based on the meta's own
/// `video_path` (see [`legacy_truncated_meta_target`]); plain usernames are unaffected.
/// Only renames within the same directory and never deletes files; when the target
/// already exists the original is kept and a warning is logged. Must be called before the
/// first scan (both ends call it at startup); repeated calls are idempotent.
pub fn migrate_truncated_stem_meta_files() -> usize {
	migrate_truncated_stem_meta_files_in(&meta_dir())
}

/// [`migrate_truncated_stem_meta_files`] 的实现，作用于指定 meta 根目录。
/// Implementation of [`migrate_truncated_stem_meta_files`] for the given meta root.
fn migrate_truncated_stem_meta_files_in(root: &Path) -> usize {
	let mut count = 0usize;
	for file in list_all_meta_paths_in(root) {
		let Ok(content) = std::fs::read_to_string(&file) else {
			continue;
		};
		let Ok(meta) = serde_json::from_str::<VideoMeta>(&content) else {
			continue;
		};
		let Some(vp) = meta.video_path.as_deref() else {
			continue;
		};
		let Some(target) = legacy_truncated_meta_target(&file, Path::new(vp)) else {
			continue;
		};
		if target.exists() {
			tracing::warn!(
				"{}",
				crate::tl!(
					"meta.migrationStemTargetExists",
					path = file.display(),
					to = target.display()
				)
			);
			continue;
		}
		match std::fs::rename(&file, &target) {
			Ok(()) => {
				tracing::info!(
					"{}",
					crate::tl!(
						"meta.migrationStemMoved",
						from = file.display(),
						to = target.display()
					)
				);
				count += 1;
			}
			Err(e) => {
				tracing::warn!(
					"{}",
					crate::tl!(
						"meta.migrationStemMoveFailed",
						path = file.display(),
						error = e
					)
				);
			}
		}
	}
	if count > 0 {
		tracing::info!("{}", crate::tl!("meta.migrationStemDone", count = count));
	}
	count
}
