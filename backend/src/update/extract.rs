//! zip 解压与事务性文件替换（`download_and_install` 的私有子步骤）。
//! Zip extraction and transactional file replacement (private step of
//! `download_and_install`).
//!
//! zip 内层目录结构：`{package-name}/{files...}`，跳过顶层目录直接提取内容。
//! MoveAll 要求源文件与目标在同一文件系统（使用 rename），因此
//! staging/stash 临时目录均在 exe_dir 内创建。
//! The zip nests everything under `{package-name}/`, which is skipped.
//! MoveAll renames files and thus requires source and destination on the same
//! filesystem — staging/stash temp dirs are created inside exe_dir.

/// 用 self_update::Extract 解压 zip 到临时目录，再用 self_update::MoveAll 事务性多文件替换。
///
/// Extract zip to a staging temp dir via self_update::Extract, then atomically replace
/// all files via self_update::MoveAll (all-or-nothing; rolls back on failure).
/// Staging and stash are created inside exe_dir to stay on the same filesystem as the install target.
pub(super) fn extract_and_replace_with_self_update(
	zip_bytes: &[u8],
	exe_dir: &std::path::Path,
) -> Result<(), String> {
	// 将 zip 字节写入临时文件，self_update::Extract 需要文件路径而非内存切片
	// self_update::Extract needs a file path, not a byte slice — write to a temp file first
	let tmp = tempfile::Builder::new()
		.prefix("stripchat_update_")
		.suffix(".zip")
		.tempfile_in(exe_dir)
		.map_err(|e| format!("创建临时 zip 文件失败: {}", e))?;

	std::fs::write(tmp.path(), zip_bytes).map_err(|e| format!("写入临时 zip 失败: {}", e))?;

	// 解压到 staging 临时目录（在 exe_dir 内保证同一文件系统）
	// Extract to a staging temp dir inside exe_dir to guarantee same filesystem
	let staging = tempfile::Builder::new()
		.prefix("stripchat_staging_")
		.tempdir_in(exe_dir)
		.map_err(|e| format!("创建解压目录失败: {}", e))?;

	self_update::Extract::from_source(tmp.path())
		.archive(self_update::ArchiveKind::Zip)
		.extract_into(staging.path())
		.map_err(|e| format!("解压失败: {}", e))?;

	// zip 内第一层是包名目录，跳过它找到实际内容
	// Skip the top-level package-name directory inside the zip
	let content_dir =
		find_single_subdir(staging.path()).unwrap_or_else(|| staging.path().to_path_buf());

	// stash 目录同样在 exe_dir 内，MoveAll 需要它与目标在同一文件系统
	// Stash dir also inside exe_dir — MoveAll requires same filesystem as destinations
	let stash = tempfile::Builder::new()
		.prefix("stripchat_stash_")
		.tempdir_in(exe_dir)
		.map_err(|e| format!("创建 stash 目录失败: {}", e))?;

	// 收集所有 (src → dest) 文件对
	// Collect all (src → dest) file pairs
	let mut pairs: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
	collect_file_pairs(&content_dir, exe_dir, &mut pairs);

	// 确保所有目标父目录存在（MoveAll 本身不创建目录）
	// Ensure all destination parent directories exist (MoveAll doesn't create them)
	for (_, dest) in &pairs {
		if let Some(parent) = dest.parent() {
			std::fs::create_dir_all(parent)
				.map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
		}
	}

	// 构建 MoveAll 并提交
	// Build MoveAll and commit
	let mut mover = self_update::MoveAll::from_temp(stash.path());
	for (src, dest) in pairs {
		mover.add(src, dest);
	}
	mover
		.commit()
		.map_err(|e| format!("文件替换失败（MoveAll）: {}", e))?;

	tracing::info!("{}", crate::tl!("update.unzipDone"));

	// Linux/macOS：为 exe_dir 根级文件设置执行权限
	// Linux/macOS: set executable bit on root-level files in exe_dir
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		if let Ok(entries) = std::fs::read_dir(exe_dir) {
			for entry in entries.flatten() {
				let path = entry.path();
				if path.is_file()
					&& let Ok(meta) = std::fs::metadata(&path)
				{
					let mut perms = meta.permissions();
					perms.set_mode(perms.mode() | 0o111);
					let _ = std::fs::set_permissions(&path, perms);
				}
			}
		}
	}

	Ok(())
}

/// 如果 `dir` 下恰好只有一个子目录（且没有文件），返回该子目录；否则返回 None。
/// Returns the single subdirectory of `dir` if there is exactly one dir entry and it's a dir.
fn find_single_subdir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
	let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
	if entries.len() == 1 {
		let entry = &entries[0];
		if entry.file_type().ok()?.is_dir() {
			return Some(entry.path());
		}
	}
	None
}

/// 递归收集 `src_base` 下所有文件的 `(src, dest)` 对，dest 路径相对于 `dest_base`。
/// Recursively collect (src, dest) pairs for all files under `src_base`,
/// with dest paths rooted at `dest_base`.
fn collect_file_pairs(
	src_base: &std::path::Path,
	dest_base: &std::path::Path,
	pairs: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>,
) {
	let Ok(entries) = std::fs::read_dir(src_base) else {
		return;
	};
	for entry in entries.flatten() {
		let src = entry.path();
		let rel = src.strip_prefix(src_base).unwrap_or(&src);
		let dest = dest_base.join(rel);
		if src.is_dir() {
			collect_file_pairs(&src, &dest_base.join(rel), pairs);
		} else if src.is_file() {
			pairs.push((src, dest));
		}
	}
}
