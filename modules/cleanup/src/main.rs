//! 最终清理后处理模块 / Final Cleanup Post-processing Module
//!
//! 删除本次处理视频相关联的数据，分两组开关：
//!
//! 1. **录制数据**（`delete_recording_data`，一个总开关）：视频文件本身、
//!    Contact Sheet 生成的预览图（及媒体包中携带的图片路径）、同名的 meta 文件——
//!    这三者是同一次录制的不同侧面，要么都在、要么都不在，不提供三个独立的勾选框
//!    分别控制。理由：这三者本质上是"同一份录制"的组成部分，若只删视频不删 meta，
//!    meta 会变成指向不存在文件的孤立记录（虽然 `cleanup_orphaned_meta_files` 最终
//!    会清理它，但期间前端列表会展示一条无法访问任何文件的"僵尸"记录）；若只删
//!    meta 不删视频，则丢失了该录制在 UI 上唯一的可发现入口（列表本身就是从 meta
//!    扫描出来的）；若只删视频/meta 却留着预览图，则会残留孤儿图片文件。分别独立
//!    勾选没有真正合理的使用场景，因此合并为一个开关，避免用户组合出上述任何一种
//!    不一致状态。
//! 2. **缓存文件**（`delete_tmp_files`，独立开关，默认开启）：其他模块在共享临时
//!    目录中残留的同名中间产物（如压缩后的封面图缓存）。这类文件本质上是可随时
//!    重新生成的缓存，不是录制数据的一部分，因此保留独立开关，默认开启（删除）。
//!
//! 通常作为流水线的最后一个节点使用，用于在后处理（上传/通知等）全部完成后按需
//! 清理本地磁盘空间。
//!
//! Deletes data associated with the video being processed, split into two groups:
//!
//! 1. **Recording data** (`delete_recording_data`, a single combined toggle): the
//!    video file itself, the Contact Sheet preview image (including any image path
//!    carried in a media bundle), and the same-named meta file — these three are
//!    different facets of the same recording; they should be either all present or
//!    all gone, not independently toggleable. Rationale: they're fundamentally parts
//!    of "the same recording" — deleting the video but not meta leaves an orphaned
//!    meta record pointing at a nonexistent file (eventually cleaned up by
//!    `cleanup_orphaned_meta_files`, but in the meantime the frontend list shows a
//!    "zombie" entry with no accessible file); deleting meta but not the video loses
//!    the only UI discovery path for that recording (the list itself is scanned from
//!    meta); deleting video/meta but leaving the preview image orphans an image file.
//!    None of these partial combinations correspond to a real use case, so they're
//!    merged into one toggle, preventing the user from ever landing in one of these
//!    inconsistent states.
//! 2. **Cache files** (`delete_tmp_files`, independent toggle, defaults to on):
//!    leftover same-named intermediate artifacts other modules left in the shared tmp
//!    directory (e.g. a resized cover image cache). These are disposable/regeneratable
//!    caches, not part of the recording data itself, so they keep their own toggle,
//!    defaulting to on (delete).
//!
//! Typically used as the last pipeline node to reclaim local disk space once all
//! post-processing (uploads, notifications, etc.) has completed.
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
    "name": "最终清理",
    "description": "删除本次处理视频的相关数据：录制数据（视频文件、预览图、meta 记录，作为一个整体同时清理或都不清理）和缓存文件（其他模块产生的同名临时文件，默认清理）",
    "inputTypes": ["any_file"],
    "outputTypes": [],
    "official": true,
    "params": [
        {
            "key": "delete_recording_data",
            "label": "删除录制数据（视频文件 + 预览图 + meta 记录）",
            "type": "boolean",
            "default": false
        },
        {
            "key": "delete_tmp_files",
            "label": "删除缓存文件（其他模块产生的残留临时文件）",
            "type": "boolean",
            "default": true
        },
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
    // 录制数据（视频/预览图/meta）作为一个整体，共用同一个开关——见文件头注释
    // 关于为何不提供三个独立勾选框的说明。
    // Recording data (video/preview image/meta) shares a single combined toggle —
    // see the file header comment for why three independent checkboxes aren't offered.
    let delete_recording_data = input.param_bool("delete_recording_data", true);
    let delete_tmp_files = input.param_bool("delete_tmp_files", true);

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
    const TOTAL_STEPS: u32 = 4;

    emit_progress_step(0, TOTAL_STEPS);

    // 第一步：删除视频文件本身（录制数据总开关的一部分）。先删主文件再删
    // sidecar/meta，确保不会出现"meta 已删但视频仍存在"的中间状态——若视频删除
    // 失败则直接返回错误，不继续后续清理。
    // Step 1 (part of the recording-data toggle): delete the video file itself.
    // Delete it before meta/sidecars to avoid ever leaving a "meta gone but video
    // still present" intermediate state; if this fails, abort immediately without
    // touching the rest.
    if delete_recording_data {
        if dry_run {
            eprintln!("DRY_RUN: would delete video '{}'", video_path.display());
            removed.push(video_path.display().to_string());
        } else {
            remove_path_with_retry(&video_path)?;
            removed.push(video_path.display().to_string());
        }
    }
    emit_progress_step(1, TOTAL_STEPS);

    // 第二步：删除 Contact Sheet 等 sidecar 图片（同目录同名，webp/jpg/jpeg/png）
    // 以及 media_bundle 中携带的额外路径（录制数据总开关的一部分）
    // Step 2 (part of the recording-data toggle): delete Contact Sheet and other
    // sidecar images (same dir, same stem, webp/jpg/jpeg/png), plus any extra paths
    // carried in the media_bundle
    if delete_recording_data {
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
    }
    emit_progress_step(2, TOTAL_STEPS);

    // 第三步：删除同名 meta 文件（录制数据总开关的一部分；路径由 exe_dir 字段
    // 推算，与后端 meta_dir()/{username}/{stem}.json 的按主播分子目录约定一致；
    // username 取 video_dir 的目录名，与后端 username_from_path 的推断规则相同）
    // Step 3 (part of the recording-data toggle): delete the same-named meta file
    // (path derived from the exe_dir field, matching the backend's per-streamer
    // meta_dir()/{username}/{stem}.json convention; username is video_dir's directory
    // name, same rule as the backend's username_from_path)
    if delete_recording_data {
        if let Some(exe_dir) = input.exe_dir.as_deref() {
            let username = video_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let meta_path = Path::new(exe_dir)
                .join("meta")
                .join(username)
                .join(format!("{}.json", stem));
            if meta_path.exists() {
                try_remove(&meta_path, dry_run, &mut removed);
            }
        }
    }
    emit_progress_step(3, TOTAL_STEPS);

    // 第四步：清理共享临时目录中前缀匹配的残留缓存文件（独立开关，默认开启；
    // 如其他模块生成的 `{stem}_xxx` 缩略图/中间产物，例如 notify_discord 压缩后
    // 的封面图缓存）
    // Step 4 (independent toggle, defaults to on): clean up leftover cache files in
    // the shared tmp directory whose filename is prefixed with this video's stem
    // (e.g. other modules' `{stem}_xxx` thumbnails/intermediate artifacts, such as
    // notify_discord's resized cover image cache)
    if delete_tmp_files {
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
    }
    emit_progress_step(4, TOTAL_STEPS);

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
        print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
        return;
    }
    if let Err(e) = run() {
        let json = serde_json::json!({ "code": "error", "message": e, "outputs": [] });
        println!("{}", json);
        std::process::exit(1);
    }
}
