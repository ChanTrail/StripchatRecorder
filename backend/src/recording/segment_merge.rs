//! 输出目录空目录清理 / Output Directory Empty-Dir Cleanup
//!
//! 本模块只负责输出目录的空目录清理（[`startup_remove_empty_dirs`]），由
//! `recording::meta::maintenance` 周期性调用（含启动时的首次立即执行）；带最小年龄保护的
//! 递归空目录删除（`remove_empty_dirs_recursive`）也供 tmp 目录清理和 meta 空主播子目录
//! 清理（`meta::maintenance::remove_empty_meta_subdirs`）复用。不涉及活跃
//! 录制会话的生命周期管理（见 `recording::recorder`）。
//!
//! 遗留分片与未完成后处理由 `meta::ensure_meta_files` 统一处理：session_dir 和视频文件
//! 采用相同的（重新）触发规则，是否需要合并交给流水线首节点（ts_merge）自行判断。
//! 原先的启动遗留分片合并函数在 Server 与 Desktop 均已无调用方，且绕过录制身份占位
//! 与 claim，因此已删除。
//!
//! This module only removes empty directories in the output directory
//! ([`startup_remove_empty_dirs`]); it is called periodically by
//! `recording::meta::maintenance` (including an immediate first run at startup). The
//! recursive empty-dir removal with minimum-age protection (`remove_empty_dirs_recursive`)
//! is also reused by the tmp dir cleanup and the meta empty streamer-subdirectory cleanup
//! (`meta::maintenance::remove_empty_meta_subdirs`). It does not manage active recording session
//! lifecycle (see `recording::recorder`).
//!
//! Leftover segments and unfinished post-processing are handled uniformly by
//! `meta::ensure_meta_files`: session_dirs and video files share the same (re-)trigger rules,
//! and the pipeline's first node (ts_merge) decides whether merging is needed. The former
//! startup leftover-segment merge function had no callers in either Server or Desktop and
//! bypassed the recording-identity reservation and claim, so it has been removed.

use crate::recording::recorder::RecorderManager;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// 空目录的最小年龄：修改时间距今小于该值的目录不删除，防止误删 ts_merge 等后处理
/// 刚创建、尚未写入文件的输出目录（维护不再等待后处理完成，两者可能并行）。
/// tmp 目录的空子目录清理（`meta::maintenance::cleanup_stale_tmp`）同样使用该值，
/// 避免误删 notify_telegram 刚创建、ffmpeg 尚未写入的 `split_*` 目录。
/// meta 空主播子目录清理（`meta::maintenance::remove_empty_meta_subdirs`）同样复用，
/// 避免误删 `write_meta` 刚创建、尚未写入 meta 文件的子目录。
///
/// Minimum age of an empty directory: directories modified more recently than this are kept,
/// so output directories just created by post-processing (e.g. ts_merge) and not yet written
/// to aren't removed (maintenance no longer waits for post-processing, so they can overlap).
/// The tmp dir's empty-subdirectory cleanup (`meta::maintenance::cleanup_stale_tmp`) uses the
/// same value, so `split_*` dirs just created by notify_telegram and not yet written by ffmpeg
/// aren't removed. The meta empty streamer-subdirectory cleanup
/// (`meta::maintenance::remove_empty_meta_subdirs`) reuses it too, so a subdirectory just
/// created by `write_meta` and not yet holding a meta file isn't removed.
pub(crate) const EMPTY_DIR_MIN_AGE: Duration = Duration::from_secs(60);

/// 递归清理输出目录下的所有空目录，跳过活跃录制会话的目录和 60 秒内修改过的目录。
/// Recursively remove all empty directories under the output directory, skipping
/// directories locked by active recording sessions and those modified within 60 seconds.
pub fn startup_remove_empty_dirs(output_dir: &Path, recorder: &RecorderManager) {
    if !output_dir.exists() {
        return;
    }

    let removed = remove_empty_dirs_recursive(
        output_dir,
        false,
        &|p| recorder.is_file_locked(p),
        EMPTY_DIR_MIN_AGE,
    );
    if removed > 0 {
        tracing::info!("{}", crate::tl!("segment.startupRemovedEmpty", count = removed, dir = output_dir.display())
        );
    }
}

/// 递归删除 `dir` 下的空目录（`remove_self` 为真时也尝试删除 `dir` 本身），返回删除数量。
/// `is_locked` 为真或修改时间距今小于 `min_age` 的目录不删除。
///
/// Recursively remove empty directories under `dir` (also `dir` itself when `remove_self`),
/// returning the number removed. Directories for which `is_locked` is true, or modified more
/// recently than `min_age`, are kept.
pub(crate) fn remove_empty_dirs_recursive(
    dir: &Path,
    remove_self: bool,
    is_locked: &dyn Fn(&Path) -> bool,
    min_age: Duration,
) -> usize {
    let mut removed = 0;

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            removed += remove_empty_dirs_recursive(&path, true, is_locked, min_age);
        }
    }

    if remove_self {
        // 跳过活跃录制会话的目录，避免删掉刚创建但还未写入第一个分片的 session 目录
        // Skip directories locked by active recording sessions to avoid deleting a
        // session directory that was just created but hasn't received its first segment yet
        if is_locked(dir) {
            return removed;
        }
        // 跳过最近修改过的目录（读取修改时间失败时保守跳过）
        // Skip recently modified directories (skip conservatively if mtime can't be read)
        let old_enough = fs::metadata(dir)
            .and_then(|m| m.modified())
            .ok()
            // 修改时间晚于当前时间（时钟误差）视为年龄 0 / mtime in the future (clock skew) counts as age 0
            .map(|t| SystemTime::now().duration_since(t).unwrap_or(Duration::ZERO))
            .is_some_and(|age| age >= min_age);
        if !old_enough {
            return removed;
        }
        let is_empty = fs::read_dir(dir)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if is_empty && fs::remove_dir(dir).is_ok() {
            removed += 1;
        }
    }

    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// min_age 为 0 时删除嵌套空目录，保留非空目录和根目录。
    /// With min_age 0, nested empty dirs are removed; non-empty dirs and the root are kept.
    #[test]
    fn removes_nested_empty_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("a").join("b")).expect("mkdir a/b");
        fs::create_dir_all(root.join("c")).expect("mkdir c");
        fs::write(root.join("c").join("file.txt"), b"x").expect("write file");

        let removed = remove_empty_dirs_recursive(root, false, &|_| false, Duration::ZERO);
        assert_eq!(removed, 2);
        assert!(!root.join("a").exists());
        assert!(root.join("c").join("file.txt").is_file());
        assert!(root.exists());
    }

    /// 新建的空目录在最小年龄内不被删除。
    /// A freshly created empty dir is kept within the minimum age.
    #[test]
    fn keeps_recent_empty_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join("fresh")).expect("mkdir fresh");

        let removed =
            remove_empty_dirs_recursive(root, false, &|_| false, Duration::from_secs(3600));
        assert_eq!(removed, 0);
        assert!(root.join("fresh").is_dir());
    }

    /// is_locked 返回真的目录不被删除。
    /// Directories for which is_locked returns true are kept.
    #[test]
    fn keeps_locked_dirs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let locked = root.join("locked");
        fs::create_dir_all(&locked).expect("mkdir locked");

        let removed =
            remove_empty_dirs_recursive(root, false, &|p| p == locked.as_path(), Duration::ZERO);
        assert_eq!(removed, 0);
        assert!(locked.is_dir());
    }
}
