//! FFmpeg 压缩包解压 / FFmpeg Archive Extraction
//!
//! 从下载的发布压缩包中挑出需要的二进制并解压到 staging 目录：
//! - [`extract_archive`]：按扩展名分派 `.7z`（sevenz-rust2）/ `.tar.xz`
//!   （`tar::Archive` + liblzma `XzDecoder`），经 `report` 回调汇报 0–100% 进度
//! - [`staged_file_name`] / [`verify_staged_binaries`]：目标条目筛选
//!   （仅 `bin/` 下的 ffmpeg/ffprobe/ffplay）与解压后校验
//! - [`ensure_executable`]：Unix 下给 staged 二进制补可执行位
//!
//! Picks the wanted binaries out of the downloaded release archive into the
//! staging directory:
//! - [`extract_archive`]: dispatches on extension — `.7z` (sevenz-rust2) or
//!   `.tar.xz` (`tar::Archive` over liblzma `XzDecoder`), reporting 0–100 %
//!   progress through `report`
//! - [`staged_file_name`] / [`verify_staged_binaries`]: wanted-entry filter
//!   (only ffmpeg/ffprobe/ffplay directly under a `bin/` dir) and the
//!   post-extraction check
//! - [`ensure_executable`]: restores the executable bit on Unix

use crate::core::error::{AppError, Result};
use crate::ffmpeg::exe_name;
use std::fs;
use std::path::Path;

/// 按扩展名分派解压：`.7z` 用 sevenz-rust2（固实压缩由读取器自动解码），
/// `.tar.xz` 用 `tar::Archive` 套 `liblzma` 的 XzDecoder。
///
/// Dispatch extraction by extension: `.7z` via sevenz-rust2 (the reader decodes
/// solid blocks transparently), `.tar.xz` via `tar::Archive` over liblzma's
/// XzDecoder.
pub(super) fn extract_archive(
    archive: &Path,
    ext: &str,
    staging: &Path,
    report: &dyn Fn(Option<u8>),
) -> Result<()> {
    match ext {
        "7z" => extract_7z(archive, staging, report),
        "tar.xz" => extract_tar_xz(archive, staging, report),
        _ => Err(AppError::Other(format!("不支持的压缩包格式: {}", ext))),
    }
}

/// 判断压缩包条目是否需要提取，返回它在 staging 里的目标文件名：
/// 仅 `bin/` 下的 `ffmpeg(.exe)`、`ffprobe(.exe)`、`ffplay(.exe)` 保留原文件名，
/// 其余一律忽略。
///
/// 返回值只是固定的几个文件名之一，`staging.join(返回值)` 天然免疫条目名里的
/// `../` 路径穿越。
///
/// Decide whether an archive entry should be extracted, returning its target
/// file name inside staging: only `ffmpeg(.exe)` / `ffprobe(.exe)` /
/// `ffplay(.exe)` directly under a `bin/` dir are kept, under their own name;
/// everything else is ignored.
///
/// The return value is always one of a few fixed names, so
/// `staging.join(result)` is inherently immune to `../` traversal in entry names.
fn staged_file_name(entry_path: &Path) -> Option<String> {
    let file_name = entry_path.file_name()?.to_str()?;
    let lower = file_name.to_ascii_lowercase();
    let in_bin = entry_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"));
    if !in_bin {
        return None;
    }
    match lower.as_str() {
        "ffmpeg" | "ffmpeg.exe" | "ffprobe" | "ffprobe.exe" | "ffplay" | "ffplay.exe" => {
            Some(file_name.to_string())
        }
        _ => None,
    }
}

/// 解压后的统一校验：staging 里必须同时拿到 ffmpeg 与 ffprobe。
/// Shared post-extraction check: staging must contain both ffmpeg and ffprobe.
fn verify_staged_binaries(staging: &Path) -> Result<()> {
    for base in ["ffmpeg", "ffprobe"] {
        let p = staging.join(exe_name(base));
        if !p.is_file() {
            return Err(AppError::Other(format!(
                "FFmpeg 压缩包中缺少 {}（解压后未找到）",
                exe_name(base)
            )));
        }
    }
    Ok(())
}

/// 从 .7z 解压需要的条目到 staging。
///
/// 固实包里后面的条目依赖前面的解压数据，不能随机跳过：不想要的条目直接不读
/// reader，由 sevenz-rust2 自动解码并丢弃其数据后再进入下一条。ffmpeg、ffprobe
/// 与 ffplay 都拿到后返回 `SkipBlock` 提前跳过本块剩余条目。
///
/// Extract the wanted entries of a .7z into staging.
///
/// In a solid archive later entries depend on data decompressed before them, so
/// they can't be skipped at random: unwanted entries simply go unread and the
/// reader decodes + discards their data before the next entry. Once ffmpeg,
/// ffprobe and ffplay have all been staged we return `SkipBlock` to skip the
/// rest of the block early.
fn extract_7z(archive: &Path, staging: &Path, report: &dyn Fn(Option<u8>)) -> Result<()> {
    use sevenz_rust2::{ArchiveReader, Password};
    use std::io::Write;
    let mut reader = ArchiveReader::open(archive, Password::empty())
        .map_err(|e| AppError::Other(format!("解析 7z 压缩包失败: {}", e)))?;
    // 目标条目的解压总大小驱动真实进度（压缩包元数据记录了每条目的解压尺寸）
    // The wanted entries' total uncompressed size drives a real progress bar
    // (the archive metadata records every entry's uncompressed size)
    let total: u64 = reader
        .archive()
        .files
        .iter()
        .filter(|e| staged_file_name(Path::new(&e.name().replace('\\', "/"))).is_some())
        .map(|e| e.size())
        .sum();
    let mut written: u64 = 0;
    let mut last_pct: u8 = 0;
    // 已收齐的条目：ffmpeg、ffprobe 收齐是硬要求，ffplay 顺带（缺失不算失败）
    // Collected entries: ffmpeg + ffprobe are hard requirements; ffplay is
    // opportunistic (absence is not a failure)
    let mut got_ffmpeg = false;
    let mut got_ffprobe = false;
    let mut got_ffplay = false;
    reader
        .for_each_entries(
            |entry, rd| -> std::result::Result<bool, sevenz_rust2::Error> {
                if entry.is_directory() {
                    return Ok(true);
                }
                // 7z 条目名可能用 \ 或 / 作分隔符；统一为 / 再解析（Unix 的 Path 不认 \）
                // 7z entry names may use \ or / separators; normalize to / first
                // (Unix Path doesn't treat \ as a separator)
                let name = entry.name().replace('\\', "/");
                let Some(out_name) = staged_file_name(Path::new(&name)) else {
                    return Ok(true);
                };
                let mut out = fs::File::create(staging.join(&out_name))?;
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = rd.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                    written += n as u64;
                    // 整数百分比变化才广播，避免 SSE 洪泛 /
                    // Only broadcast on whole-percent changes to avoid flooding SSE
                    if let Some(p) = written
                        .saturating_mul(100)
                        .checked_div(total)
                        .map(|r| r.min(100) as u8)
                        && p != last_pct
                    {
                        last_pct = p;
                        report(Some(p));
                    }
                }
                let lower = out_name.to_ascii_lowercase();
                if lower == exe_name("ffmpeg") {
                    got_ffmpeg = true;
                } else if lower == exe_name("ffprobe") {
                    got_ffprobe = true;
                } else if lower == exe_name("ffplay") {
                    got_ffplay = true;
                }
                // 收齐后跳块：固实块剩余条目不再解码 / Once everything is staged,
                // skip the block: no need to decode the remaining entries
                Ok(!(got_ffmpeg && got_ffprobe && got_ffplay))
            },
        )
        .map_err(|e| AppError::Other(format!("解压 7z 压缩包失败: {}", e)))?;
    verify_staged_binaries(staging)
}

/// 从 .tar.xz 解压需要的条目到 staging。
/// Extract the wanted entries of a .tar.xz into staging.
fn extract_tar_xz(archive: &Path, staging: &Path, report: &dyn Fn(Option<u8>)) -> Result<()> {
    use std::io::{Read, Write};
    let file =
        fs::File::open(archive).map_err(|e| AppError::Other(format!("打开压缩包失败: {}", e)))?;
    let xz = liblzma::read::XzDecoder::new(file);
    let mut tar = tar::Archive::new(xz);
    let entries = tar
        .entries()
        .map_err(|e| AppError::Other(format!("解析 tar 失败: {}", e)))?;
    // 分文件进度：每个目标条目占三分之一，文件内按条目头部记录的解压尺寸
    // 推进（tar 顺序读，目标总大小无法预知，但三个二进制体量相近，按文件
    // 均分是足够真实的进度）
    // Per-file progress: each wanted entry covers a third of the bar and is
    // advanced by the size recorded in its tar header (tar reads sequentially
    // so the wanted total can't be known upfront; the three binaries are
    // comparable in size, so equal thirds are honest enough)
    let mut done_files: u32 = 0;
    let mut last_pct: u8 = 0;
    for entry in entries {
        let mut entry = entry.map_err(|e| AppError::Other(format!("读取 tar 条目失败: {}", e)))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| AppError::Other(format!("读取 tar 条目路径失败: {}", e)))?
            .into_owned();
        let Some(out_name) = staged_file_name(&path) else {
            continue;
        };
        let entry_size = entry.header().size().unwrap_or(0);
        let mut out = fs::File::create(staging.join(&out_name))
            .map_err(|e| AppError::Other(format!("创建解压文件失败: {}", e)))?;
        let mut cur: u64 = 0;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|e| AppError::Other(format!("解压文件失败: {}", e)))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|e| AppError::Other(format!("写入解压文件失败: {}", e)))?;
            cur += n as u64;
            let frac = if entry_size > 0 {
                (cur as f64 / entry_size as f64).min(1.0)
            } else {
                0.0
            };
            let p = (((done_files as f64 + frac) / 3.0) * 100.0).min(100.0) as u8;
            if p != last_pct {
                last_pct = p;
                report(Some(p));
            }
        }
        done_files += 1;
    }
    verify_staged_binaries(staging)
}

/// Unix：给 staging 里的 ffmpeg/ffprobe/ffplay 加可执行权限。Windows 无需处理。
/// Unix: mark the staged ffmpeg/ffprobe/ffplay executable. No-op on Windows.
#[cfg(unix)]
pub(super) fn ensure_executable(staging: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for base in ["ffmpeg", "ffprobe", "ffplay"] {
        let p = staging.join(exe_name(base));
        if p.is_file()
            && let Ok(meta) = fs::metadata(&p)
        {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o111);
            fs::set_permissions(&p, perms)?;
        }
    }
    Ok(())
}

/// Windows 下无需设置可执行权限。/ No executable-bit handling needed on Windows.
#[cfg(not(unix))]
pub(super) fn ensure_executable(_staging: &Path) -> Result<()> {
    Ok(())
}
