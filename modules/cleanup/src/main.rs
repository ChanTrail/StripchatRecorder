//! 最终清理后处理模块 / Final Cleanup Post-processing Module
//!
//! 删除本次处理视频相关联的所有数据：视频文件本身、同名的 meta 文件、
//! Contact Sheet 生成的预览图（及媒体包中携带的图片路径），以及其他模块
//! 在共享临时目录中残留的同名临时文件。通常作为流水线的最后一个节点使用，
//! 用于在后处理（上传/通知等）全部完成后彻底清理本地磁盘空间。
//!
//! Deletes all data associated with the video being processed: the video file
//! itself, its same-named meta file, the Contact Sheet preview image (including
//! any image path carried in a media bundle), and any leftover same-named temp
//! files other modules left in the shared tmp directory. Typically used as the
//! last pipeline node to fully reclaim local disk space once all post-processing
//! (uploads, notifications, etc.) has completed.
//!
//! # 协议 / Protocol
//! - `--describe`: 输出 JSON 格式的模块元数据 / Output module metadata as JSON
//! - stdin: JSON 输入（inputs[0] 接受 video_file / image_file / media_bundle）
//!   / JSON input (inputs[0] accepts video_file / image_file / media_bundle)
//! - stdout: 进度行 + 最终 JSON 结果（始终为 done，流水线在此终止，因为本模块
//!   声明的输出端口数量为 0）
//!   / Progress lines + final JSON result (always `done`; the pipeline terminates
//!   here since this module declares zero output ports)

use pp_utils::{emit_progress_step, output_done, tmp_dir, ModuleInput};
use std::path::{Path, PathBuf};

const DESCRIBE: &str = r#"{
    "id": "cleanup",
    "name": "最终清理 0.1.0",
    "description": "删除本次处理视频的所有相关数据：视频文件、同名 meta 文件、Contact Sheet 预览图，以及其他模块产生的同名临时文件",
    "inputTypes": ["any_file"],
    "outputTypes": [],
    "official": true,
    "params": [
        {
            "key": "dry_run",
            "label": "仅预览，不实际删除",
            "type": "boolean",
            "default": false
        }
    ]
}"#;

/// 将输入路径按 `\n` 拆分（兼容 media_bundle 的"视频路径\n图片路径"格式；
/// 普通单一路径按原样返回单元素列表）。
///
/// Split the input path on `\n` (compatible with media_bundle's
/// "video_path\nimage_path" format; a plain single path returns a one-element list).
fn split_bundle(raw: &str) -> Vec<PathBuf> {
    raw.split('\n')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 删除单个文件/目录，最多重试 20 次（间隔 200ms），用于处理 Windows 上文件
/// 短暂被占用（如刚关闭的 ffmpeg 句柄、杀毒软件扫描）的情况。
///
/// Delete a single file/directory with up to 20 retries (200ms apart), to handle
/// transient locks on Windows (e.g. a just-closed ffmpeg handle, antivirus scan).
fn remove_path_with_retry(path: &Path) -> Result<(), String> {
    let is_dir = path.is_dir();
    let mut last_err = None;
    for _ in 0..20 {
        let result = if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
    Err(format!(
        "Failed to delete '{}': {}",
        path.display(),
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// 尽力删除单个次要文件（sidecar 图片 / meta / 临时文件），失败仅记录警告，
/// 不中断整体清理流程；`dry_run` 时只打印将要删除的路径而不实际操作。
///
/// Best-effort delete of a secondary file (sidecar image / meta / temp file);
/// failures are only logged as warnings and don't abort the overall cleanup.
/// In `dry_run` mode, only prints the path that would be deleted without acting.
fn try_remove(path: &Path, dry_run: bool, removed: &mut Vec<String>) {
    if dry_run {
        eprintln!("DRY_RUN: would delete '{}'", path.display());
        removed.push(path.display().to_string());
        return;
    }
    match std::fs::remove_file(path) {
        Ok(()) => removed.push(path.display().to_string()),
        Err(e) => eprintln!("Warning: failed to delete '{}': {}", path.display(), e),
    }
}

fn run() -> Result<(), String> {
    let input = ModuleInput::read();
    let raw = input
        .first_input()
        .ok_or_else(|| "inputs[0] is required".to_string())?;
    let paths = split_bundle(&raw.to_string_lossy());
    let video_path = paths
        .first()
        .cloned()
        .ok_or_else(|| "inputs[0] is empty".to_string())?;
    // media_bundle 中携带的额外路径（如 Contact Sheet 图片）
    // Extra paths carried in a media_bundle (e.g. the Contact Sheet image)
    let extra_paths: Vec<PathBuf> = paths.into_iter().skip(1).collect();

    if !video_path.exists() {
        return Err(format!("Input file not found: {}", video_path.display()));
    }

    let dry_run = input.param_bool("dry_run", false);

    let stem = video_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("Cannot determine stem for {}", video_path.display()))?
        .to_string();
    let video_dir = video_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut removed: Vec<String> = Vec::new();

    emit_progress_step(0, 4);

    // 第一步：删除视频文件本身（先删主文件，确保不会出现"meta 已删但视频仍存在"
    // 的中间状态——若视频删除失败则直接返回错误，不继续后续清理）
    // Step 1: delete the video file itself first (deleting it before meta/sidecars
    // avoids ever leaving a "meta gone but video still present" intermediate state;
    // if this fails, abort immediately without touching the rest)
    if dry_run {
        eprintln!("DRY_RUN: would delete video '{}'", video_path.display());
    } else {
        remove_path_with_retry(&video_path)?;
    }
    removed.push(video_path.display().to_string());
    emit_progress_step(1, 4);

    // 第二步：删除 Contact Sheet 等 sidecar 图片（同目录同名，webp/jpg/jpeg/png）
    // 以及 media_bundle 中携带的额外路径
    // Step 2: delete Contact Sheet and other sidecar images (same dir, same stem,
    // webp/jpg/jpeg/png), plus any extra paths carried in the media_bundle
    for ext in &["webp", "jpg", "jpeg", "png"] {
        let sidecar = video_dir.join(format!("{}.{}", stem, ext));
        if sidecar.exists() {
            try_remove(&sidecar, dry_run, &mut removed);
        }
    }
    for p in &extra_paths {
        if p.exists() {
            try_remove(p, dry_run, &mut removed);
        }
    }
    emit_progress_step(2, 4);

    // 第三步：删除同名 meta 文件（路径由 exe_dir 字段推算，与后端 meta_dir() 约定一致）
    // Step 3: delete the same-named meta file (path derived from the exe_dir field,
    // matching the backend's meta_dir() convention)
    if let Some(exe_dir) = input.exe_dir.as_deref() {
        let meta_path = Path::new(exe_dir)
            .join("meta")
            .join(format!("{}.json", stem));
        if meta_path.exists() {
            try_remove(&meta_path, dry_run, &mut removed);
        }
    }
    emit_progress_step(3, 4);

    // 第四步：清理共享临时目录中前缀匹配的残留临时文件（如其他模块生成的
    // `{stem}_xxx` 缩略图/中间产物，例如 notify_discord 压缩后的封面图）
    // Step 4: clean up leftover temp files in the shared tmp directory whose
    // filename is prefixed with this video's stem (e.g. other modules'
    // `{stem}_xxx` thumbnails/intermediate artifacts, such as notify_discord's
    // resized cover image)
    let tmp = tmp_dir();
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        let prefix_underscore = format!("{}_", stem);
        let prefix_dot = format!("{}.", stem);
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with(&prefix_underscore) || name.starts_with(&prefix_dot) {
                try_remove(&p, dry_run, &mut removed);
            }
        }
    }
    emit_progress_step(4, 4);

    let msg = if dry_run {
        format!("DRY_RUN: would remove {} item(s) for '{}'", removed.len(), stem)
    } else {
        format!("Removed {} item(s) for '{}'", removed.len(), stem)
    };
    output_done(&msg);
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", DESCRIBE);
        return;
    }
    if let Err(e) = run() {
        let json = serde_json::json!({ "code": "error", "message": e, "outputs": [] });
        println!("{}", json);
        std::process::exit(1);
    }
}
