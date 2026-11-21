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
/// `shared` = 提取 `-shared` 变体：`bin/` 下所有条目（含旁位 .so/.dll）都收。
///
/// Dispatch extraction by extension: `.7z` via sevenz-rust2 (the reader decodes
/// solid blocks transparently), `.tar.xz` via `tar::Archive` over liblzma's
/// XzDecoder. `shared` = extracting a `-shared` variant: every entry under
/// `bin/` (incl. side-by-side .so/.dll) is kept.
pub(super) fn extract_archive(
	archive: &Path,
	ext: &str,
	staging: &Path,
	report: &dyn Fn(Option<u8>),
	shared: bool,
) -> Result<()> {
	match ext {
		"7z" => extract_7z(archive, staging, report, shared),
		"tar.xz" => extract_tar_xz(archive, staging, report, shared),
		_ => Err(AppError::Other(format!("不支持的压缩包格式: {}", ext))),
	}
}

/// 判断压缩包条目是否需要提取，返回它在 staging 里的目标文件名：
/// 静态/调试变体仅 `bin/` 下的 `ffmpeg(.exe)`、`ffprobe(.exe)`、`ffplay(.exe)`
/// 保留原文件名；`-shared` 变体则 `bin/` 下全部条目（旁位共享库与可执行文件
/// 同目录自解析，必须一并收进 tools 才能跑）。
///
/// 返回值始终只取条目的末段文件名，`staging.join(返回值)` 天然免疫条目名里的
/// `../` 路径穿越。
///
/// Decide whether an archive entry should be extracted, returning its target
/// file name inside staging: static/debug variants keep only `ffmpeg(.exe)` /
/// `ffprobe(.exe)` / `ffplay(.exe)` directly under a `bin/` dir under their
/// own names; `-shared` variants keep every `bin/` entry (the side-by-side
/// shared libs self-resolve next to the executables and must land in tools
/// together).
///
/// The return value is always just the entry's final file name, so
/// `staging.join(result)` is inherently immune to `../` traversal in entry names.
fn staged_file_name(entry_path: &Path, shared: bool) -> Option<String> {
	let file_name = entry_path.file_name()?.to_str()?;
	let in_bin = entry_path
		.parent()
		.and_then(|p| p.file_name())
		.and_then(|n| n.to_str())
		.is_some_and(|n| n.eq_ignore_ascii_case("bin"));
	if !in_bin {
		return None;
	}
	if shared {
		return Some(file_name.to_string());
	}
	let lower = file_name.to_ascii_lowercase();
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
fn extract_7z(
	archive: &Path,
	staging: &Path,
	report: &dyn Fn(Option<u8>),
	shared: bool,
) -> Result<()> {
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
		.filter(|e| staged_file_name(Path::new(&e.name().replace('\\', "/")), shared).is_some())
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
				let Some(out_name) = staged_file_name(Path::new(&name), shared) else {
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
				// 收齐后跳块：固实块剩余条目不再解码；-shared 变体要收 bin/
				// 全部条目，数量未知不能提前跳块 / Once everything is staged,
				// skip the block: no need to decode the remaining entries.
				// A -shared variant wants every bin/ entry — the count is
				// unknown, so never skip early
				Ok(shared || !(got_ffmpeg && got_ffprobe && got_ffplay))
			},
		)
		.map_err(|e| AppError::Other(format!("解压 7z 压缩包失败: {}", e)))?;
	verify_staged_binaries(staging)
}

/// 从 .tar.xz 解压需要的条目到 staging。
/// `shared`（-shared 变体）时 `bin/` 下普通文件与符号链接一并提取（旁位
/// .so 常以 `libfoo.so.x` → `libfoo.so.x.y.z` 符号链接并存）。
///
/// Extract the wanted entries of a .tar.xz into staging. With `shared`
/// (-shared variant), regular files AND symlinks under `bin/` are all taken
/// (side-by-side .so sets usually pair `libfoo.so.x` symlinks with a real
/// `libfoo.so.x.y.z`).
fn extract_tar_xz(
	archive: &Path,
	staging: &Path,
	report: &dyn Fn(Option<u8>),
	shared: bool,
) -> Result<()> {
	use std::io::{Read, Write};
	// 元数据预扫：先走一遍条目头累计目标解压总大小，给主扫描一个真实的
	// 分母（-shared 变体的目标条数事先未知，按文件均分的旧方案会失真）。
	// 预扫仍是完整解压缩一遍本地文件，耗时约秒级，换准确进度。
	// Metadata pre-pass: walk entry headers once summing the wanted entries'
	// sizes so the main pass has a real denominator (a -shared variant's
	// wanted-entry count is unknown upfront, so the old per-file-thirds split
	// would misreport). The pre-pass still decompresses the local file once —
	// a second or two for an accurate bar.
	let total: u64 = {
		let file = fs::File::open(archive)
			.map_err(|e| AppError::Other(format!("打开压缩包失败: {}", e)))?;
		let mut tar = tar::Archive::new(liblzma::read::XzDecoder::new(file));
		let entries = tar
			.entries()
			.map_err(|e| AppError::Other(format!("解析 tar 失败: {}", e)))?;
		let mut sum = 0u64;
		for entry in entries {
			let entry = entry.map_err(|e| AppError::Other(format!("读取 tar 条目失败: {}", e)))?;
			let ty = entry.header().entry_type();
			if !(ty.is_file() || (shared && ty.is_symlink())) {
				continue;
			}
			let Ok(path) = entry.path() else { continue };
			if staged_file_name(&path, shared).is_some() {
				sum += entry.header().size().unwrap_or(0);
			}
		}
		sum
	};

	let file =
		fs::File::open(archive).map_err(|e| AppError::Other(format!("打开压缩包失败: {}", e)))?;
	let xz = liblzma::read::XzDecoder::new(file);
	let mut tar = tar::Archive::new(xz);
	let entries = tar
		.entries()
		.map_err(|e| AppError::Other(format!("解析 tar 失败: {}", e)))?;
	let mut written: u64 = 0;
	let mut last_pct: u8 = 0;
	let emit = |report: &dyn Fn(Option<u8>), written: u64, last_pct: &mut u8| {
		if let Some(p) = written
			.saturating_mul(100)
			.checked_div(total)
			.map(|r| r.min(100) as u8)
			&& p != *last_pct
		{
			*last_pct = p;
			report(Some(p));
		}
	};
	for entry in entries {
		let mut entry = entry.map_err(|e| AppError::Other(format!("读取 tar 条目失败: {}", e)))?;
		let ty = entry.header().entry_type();
		if !(ty.is_file() || (shared && ty.is_symlink())) {
			continue;
		}
		let path = entry
			.path()
			.map_err(|e| AppError::Other(format!("读取 tar 条目路径失败: {}", e)))?
			.into_owned();
		let Some(out_name) = staged_file_name(&path, shared) else {
			continue;
		};
		let dst = staging.join(&out_name);
		if ty.is_symlink() {
			// 符号链接直接由 tar 自身还原（目标即归档内记录的相对链接名，
			// 同目录旁位库自解析）/ Restore symlinks via tar's own unpack —
			// the target is the relative link name recorded in the archive
			// (side-by-side libs self-resolve in the same dir)
			entry
				.unpack(&dst)
				.map_err(|e| AppError::Other(format!("还原符号链接失败: {}", e)))?;
		} else {
			let mut out = fs::File::create(&dst)
				.map_err(|e| AppError::Other(format!("创建解压文件失败: {}", e)))?;
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
				written += n as u64;
				emit(report, written, &mut last_pct);
			}
		}
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
