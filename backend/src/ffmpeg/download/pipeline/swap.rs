//! staging 验证与原子换入 / Staging Verification & Atomic Swap-In
//!
//! 解压完成后、替换 tools/ 中正式二进制之前的最后一段：
//! - [`smoke_run_staged`]：对 staged 的 ffmpeg/ffprobe（及可选的 ffplay）各跑
//!   一次 `-version`，跑不起来的构建在触碰正式副本前就失败
//! - [`swap_staged_into`] / [`swap_one`] / [`rollback_swaps`]：`*.old` 备份式
//!   逐个换入，任一步失败回滚已完成的替换
//!
//! The last leg after extraction, before the real binaries in `tools/` are
//! replaced:
//! - [`smoke_run_staged`]: runs `-version` once on each staged ffmpeg/ffprobe
//!   (and ffplay when present) — a build that can't run here fails before the
//!   working copy is touched
//! - [`swap_staged_into`] / [`swap_one`] / [`rollback_swaps`]: per-file swap-in
//!   with `*.old` backups, rolling completed swaps back on any failure

use crate::core::error::{AppError, Result};
use crate::core::no_window::NoWindowExt;
use crate::ffmpeg::exe_name;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ─── 冒烟测试 / Smoke test ───────────────────────────────────────────────────

/// 对 staging 里的 ffmpeg、ffprobe（以及存在的 ffplay）各跑一次 `-version`，
/// 返回 ffmpeg 输出首行。任一二进制启动失败或退出码非零都算失败——说明该构建
/// 在本机不可用。ffplay 是可选条目：压缩包里有才校验。
///
/// Run `-version` once on each staged ffmpeg/ffprobe (and ffplay when present);
/// returns ffmpeg's first output line. A binary that won't launch or exits
/// non-zero fails the whole install — the build is unusable on this machine.
/// ffplay is optional: checked only when the archive shipped it.
pub(super) fn smoke_run_staged(staging: &Path) -> Result<String> {
    let version = run_version_line(&staging.join(exe_name("ffmpeg")))
        .map_err(|e| AppError::Other(format!("下载的 ffmpeg 无法运行: {}", e)))?;
    run_version_line(&staging.join(exe_name("ffprobe")))
        .map_err(|e| AppError::Other(format!("下载的 ffprobe 无法运行: {}", e)))?;
    let ffplay = staging.join(exe_name("ffplay"));
    if ffplay.is_file() {
        run_version_line(&ffplay)
            .map_err(|e| AppError::Other(format!("下载的 ffplay 无法运行: {}", e)))?;
    }
    Ok(version)
}

/// 运行 `bin -version`，成功时返回 stdout 首行。
/// Run `bin -version`; on success returns the first stdout line.
fn run_version_line(bin: &Path) -> Result<String> {
    let out = Command::new(bin)
        .arg("-version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_window()
        .output()
        .map_err(|e| AppError::Other(format!("{} 启动失败: {}", bin.display(), e)))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.trim();
        return Err(AppError::Other(format!(
            "{} -version 退出码 {}{}",
            bin.display(),
            out.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", detail)
            }
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().next().unwrap_or_default().trim().to_string())
}

// ─── 文件替换 / File swap ────────────────────────────────────────────────────

/// 目标文件对应的 `*.old` 备份路径（如 `ffmpeg.exe` → `ffmpeg.exe.old`）。
/// The `*.old` backup path for a target (e.g. `ffmpeg.exe` → `ffmpeg.exe.old`).
fn old_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    target.with_file_name(format!("{}.old", name))
}

/// 一次已完成的替换记录，供失败时回滚。
/// Record of one completed swap, for rollback on failure.
struct AppliedSwap {
    /// 刚换入的新文件当前所在路径（= 目标路径）/ Where the new file now sits (= target)
    target: PathBuf,
    /// 新文件的原始 staging 路径（回滚时挪回去）/ Original staging path (rolled back to)
    staged: PathBuf,
    /// 旧文件的 `*.old` 备份；目标原本不存在时为 `None`
    /// `*.old` backup of the previous file; `None` when the target didn't exist
    old: Option<PathBuf>,
}

/// 把 staging 中存在的 ffmpeg/ffprobe/ffplay 逐个换入 tools。
/// 任一文件失败时，回滚已完成的替换再返回错误；全部成功后尽力删除 `*.old`。
///
/// Swap each staged ffmpeg/ffprobe/ffplay into tools. On any failure,
/// completed swaps are rolled back before the error is returned; on success the
/// `*.old` backups are deleted best-effort.
pub(super) fn swap_staged_into(staging: &Path, tools: &Path) -> Result<()> {
    let names = [exe_name("ffmpeg"), exe_name("ffprobe"), exe_name("ffplay")];
    let mut applied: Vec<AppliedSwap> = Vec::new();
    for name in names {
        let staged = staging.join(&name);
        if !staged.exists() {
            continue;
        }
        match swap_one(&staged, &tools.join(&name)) {
            Ok(a) => applied.push(a),
            Err(e) => {
                rollback_swaps(applied);
                return Err(e);
            }
        }
    }
    // 全部换入成功后清理 *.old 备份（尽力而为；残留由 cleanup_leftovers 兜底）
    // All swapped in — delete the *.old backups (best-effort; leftovers are
    // caught by cleanup_leftovers)
    for a in applied {
        if let Some(old) = a.old {
            let _ = fs::remove_file(old);
        }
    }
    Ok(())
}

/// 单个文件的替换：
/// - 目标存在时先保留旧副本：Unix 用硬链接（rename 替换是原子的，旧 inode 对
///   运行中的进程继续有效，`.old` 仅用于回滚）；Windows 用 rename → `*.old`
///   （Windows 允许对正在运行的可执行文件改名，但不允许直接覆盖）
/// - 再把 staging 文件 rename 到目标；这一步失败时恢复旧文件
///
/// Swap one file:
/// - Preserve the previous file first when the target exists: a hard link on Unix
///   (the replacing rename stays atomic, the old inode keeps working for running
///   processes, `.old` only serves rollback); a rename → `*.old` on Windows
///   (Windows allows renaming a running executable but not overwriting it)
/// - Then rename the staged file onto the target; if that step fails, restore
///   the old file
fn swap_one(staged: &Path, target: &Path) -> Result<AppliedSwap> {
    let old = old_path(target);
    let _ = fs::remove_file(&old); // 清掉上次遗留的 .old / remove any leftover .old

    let mut had_old = false;
    if target.exists() {
        #[cfg(unix)]
        {
            // 硬链接保留旧内容；不支持硬链接的文件系统退化为复制
            // Hard-link to preserve the old content; fall back to a copy on
            // filesystems without hard links
            if fs::hard_link(target, &old).is_err() {
                fs::copy(target, &old)
                    .map_err(|e| AppError::Other(format!("备份旧文件失败: {}", e)))?;
            }
        }
        #[cfg(windows)]
        {
            fs::rename(target, &old).map_err(|e| {
                AppError::Other(format!("改名旧文件 {} 失败: {}", target.display(), e))
            })?;
        }
        had_old = true;
    }

    if let Err(e) = fs::rename(staged, target) {
        // 第二步失败 → 还原第一步（Windows 需要把 .old 改回去；
        // Unix 的 rename 失败时 target 未动，只需删掉 .old 链接）
        // Second step failed → undo the first (Windows must rename .old back;
        // on Unix a failed rename leaves target untouched, just drop the .old link)
        if had_old {
            #[cfg(windows)]
            let _ = fs::rename(&old, target);
            #[cfg(unix)]
            let _ = fs::remove_file(&old);
        }
        return Err(AppError::Other(format!(
            "替换 {} 失败: {}",
            target.display(),
            e
        )));
    }

    Ok(AppliedSwap {
        target: target.to_path_buf(),
        staged: staged.to_path_buf(),
        old: had_old.then_some(old),
    })
}

/// 回滚已完成的替换（逆序）：有 `.old` 的恢复旧文件，原本不存在的目标删掉新文件。
/// Roll back completed swaps (in reverse): restore `.old` where present, delete
/// the new file where the target didn't previously exist.
fn rollback_swaps(applied: Vec<AppliedSwap>) {
    for a in applied.into_iter().rev() {
        if let Some(old) = &a.old {
            let _ = fs::rename(&a.target, &a.staged);
            let _ = fs::rename(old, &a.target);
        } else {
            let _ = fs::remove_file(&a.target);
        }
    }
}
