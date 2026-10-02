//! Meta 数据结构与路径计算 / Meta Data Structures and Path Resolution
//!
//! 定义 `VideoMeta` 及其嵌套的后处理执行记录/进度类型，以及 meta 文件路径的计算规则。
//! 不涉及文件读写（见 `super::store`）或扫描重建逻辑（见 `super::scan`）。
//!
//! Defines `VideoMeta` and its nested post-processing execution/progress types, plus
//! meta file path resolution rules. Does not perform file I/O (see `super::store`) or
//! scan/rebuild logic (see `super::scan`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 当前 meta 文件格式版本。
/// Current meta file format version.
pub const META_VERSION: u32 = 4;

/// 返回 meta 根目录（exe_dir/meta/）。
/// 实际的 meta 文件按主播分子目录存储在此目录下（见 [`meta_dir_for`]），
/// 本函数主要用于确保根目录存在、以及需要递归扫描全部 meta 文件的场景
/// （见 [`list_all_meta_paths`]）。
///
/// Returns the meta root directory (exe_dir/meta/).
/// Actual meta files are stored in per-streamer subdirectories under this directory
/// (see [`meta_dir_for`]); this function is mainly used to ensure the root exists and
/// for scenarios that need to recursively scan all meta files (see [`list_all_meta_paths`]).
pub fn meta_dir() -> PathBuf {
    crate::config::app_state::exe_dir().join("meta")
}

/// 返回指定主播的 meta 子目录（exe_dir/meta/{username}/）。
/// Returns the meta subdirectory for a specific streamer (exe_dir/meta/{username}/).
pub fn meta_dir_for(username: &str) -> PathBuf {
    meta_dir().join(username)
}

/// 从视频文件路径（或 session_dir）推断所属主播用户名：取路径的直接父目录名。
///
/// 与 `RecordingContext.username`（postprocess_cmd.rs）、`RecordingFile.username`
/// （recording_cmd.rs）使用完全相同的推断规则——因为 session_dir 的结构始终是
/// `{output_dir}/{username}/{username}_{timestamp}`，ts_merge 在 `split_by_streamer`
/// 开启时也会保持同样的 `{output_dir}/{username}/{stem}.{format}` 结构。这保证同一份
/// 录制在 UI 展示、模块执行上下文和 meta 存储路径三处的"用户名"定义完全一致。
///
/// 若路径没有父目录（极端边界情况），返回 `"unknown"`。
///
/// Infer the streamer username from a video file path (or session_dir): the direct
/// parent directory's name.
///
/// Uses exactly the same derivation rule as `RecordingContext.username`
/// (postprocess_cmd.rs) and `RecordingFile.username` (recording_cmd.rs) — since a
/// session_dir is always structured as `{output_dir}/{username}/{username}_{timestamp}`,
/// and ts_merge preserves the same `{output_dir}/{username}/{stem}.{format}` structure
/// when `split_by_streamer` is enabled. This keeps the "username" concept consistent
/// across the UI, module execution context, and meta storage path.
///
/// Returns `"unknown"` if the path has no parent directory (edge case).
pub fn username_from_path(video_path: &Path) -> String {
    video_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

/// 判断一个后缀是否像文件扩展名：长度 1..=5 且全部为 ASCII 字母数字。
/// Check whether a suffix looks like a file extension: 1..=5 ASCII alphanumeric chars.
fn looks_like_file_extension(ext: &str) -> bool {
    (1..=5).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 计算录制路径（视频文件或 session_dir）的 stem，用作 meta 文件名。
///
/// 与 `Path::file_stem()` 的区别：只有最后一个 `.` 之后的部分像扩展名
/// （见 [`looks_like_file_extension`]）且 `.` 之前非空时才去掉它，否则返回完整文件名。
/// session_dir 名总以 `_YYYYMMDD_HHMMSS[_n]` 结尾，因此含 `.` 的用户名
/// （如 `a.b_20240101_120000`）不会被截断成 `a`，与其合并后的
/// `a.b_20240101_120000.mp4` 得到同一 stem（B16）。不含 `.` 的名字以及视频文件的结果
/// 与旧的 `file_stem()` 完全相同。
///
/// Compute the stem of a recording path (video file or session_dir), used as the meta
/// file name.
///
/// Unlike `Path::file_stem()`, the part after the last `.` is only stripped when it looks
/// like an extension (see [`looks_like_file_extension`]) and the part before it is
/// non-empty; otherwise the full file name is returned. Session_dir names always end with
/// `_YYYYMMDD_HHMMSS[_n]`, so a username containing `.` (e.g. `a.b_20240101_120000`) is no
/// longer truncated to `a`, and yields the same stem as its merged
/// `a.b_20240101_120000.mp4` (B16). Names without `.` and video files produce exactly the
/// same result as the old `file_stem()`.
pub fn recording_stem(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    if let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
        && looks_like_file_extension(ext)
    {
        return Some(stem);
    }
    Some(name)
}

/// 从录制器生成的 stem（`{user}_{YYYYMMDD}_{HHMMSS}[_{n}]`）中解析主播名。
/// 末尾的轮转序号 `_{n}`（全数字且长度不为 6）会先被去掉；格式不符或主播名为空时返回 `None`。
///
/// Parse the streamer name from a recorder-generated stem
/// (`{user}_{YYYYMMDD}_{HHMMSS}[_{n}]`). A trailing rotation suffix `_{n}` (all digits,
/// length != 6) is stripped first; returns `None` if the format doesn't match or the
/// username is empty.
pub fn username_from_stem(stem: &str) -> Option<&str> {
    // 定长全数字段 / Fixed-length all-digit segment
    fn is_digits(s: &str, len: usize) -> bool {
        s.len() == len && s.bytes().all(|b| b.is_ascii_digit())
    }
    // 剥离 `_{8 位日期}_{6 位时间}`，返回前面的部分 / Strip `_{8-digit date}_{6-digit time}`, return the prefix
    fn strip_timestamp(s: &str) -> Option<&str> {
        let (rest, time) = s.rsplit_once('_')?;
        if !is_digits(time, 6) {
            return None;
        }
        let (user, date) = rest.rsplit_once('_')?;
        if !is_digits(date, 8) {
            return None;
        }
        Some(user)
    }

    let mut base = stem;
    // 轮转后缀：全数字、长度不为 6，且去掉后仍以时间戳结尾
    // Rotation suffix: all digits, length != 6, and the remainder still ends with a timestamp
    if let Some((rest, last)) = stem.rsplit_once('_')
        && !last.is_empty()
        && last.len() != 6
        && last.bytes().all(|b| b.is_ascii_digit())
        && strip_timestamp(rest).is_some()
    {
        base = rest;
    }
    strip_timestamp(base).filter(|u| !u.is_empty())
}

/// 在指定 meta 根目录下计算录制路径的派生 meta 路径：
/// `meta_root/{username_from_path}/{recording_stem}.json`。
///
/// Compute the derived meta path of a recording path under the given meta root:
/// `meta_root/{username_from_path}/{recording_stem}.json`.
pub fn meta_path_in(meta_root: &Path, path: &Path) -> Option<PathBuf> {
    let stem = recording_stem(path)?;
    Some(
        meta_root
            .join(username_from_path(path))
            .join(format!("{}.json", stem)),
    )
}

/// 根据视频文件路径（或 session_dir）计算派生的元数据文件路径。
/// 按主播分子目录存储：`meta_dir/{username}/{stem}.json`，username 从 video_path
/// 推断（见 [`username_from_path`]），stem 由 [`recording_stem`] 计算（含 `.` 的用户名
/// 不会被截断）。读写 meta 时应使用 [`resolve_meta_path`]，它在派生 meta 缺失时还会查找
/// 归属 meta。
///
/// Compute the derived metadata file path for a given video file path or session_dir.
/// Stored in a per-streamer subdirectory: `meta_dir/{username}/{stem}.json`, with
/// username inferred from video_path (see [`username_from_path`]) and the stem computed
/// by [`recording_stem`] (usernames containing `.` are not truncated). Meta reads/writes
/// should use [`resolve_meta_path`], which also looks up the owning meta when the derived
/// one is missing.
pub fn meta_path_for(video_path: &Path) -> Option<PathBuf> {
    meta_path_in(&meta_dir(), video_path)
}

/// 在指定 meta 根目录下解析录制路径的实际 meta 文件（录制身份）。
///
/// 1. 派生 meta（[`meta_path_in`]）存在时直接返回它（常见情况，无额外 IO）；
/// 2. 否则按 stem 查找 `video_path` 恰好等于该路径的归属 meta：stem 是录制器格式时只查
///    `meta_root/{user}/{stem}.json`（录制器生成的 meta 必在该目录下），否则遍历 meta 根下
///    一层子目录；
/// 3. 都找不到时返回派生路径（供新建 meta 使用）。
///
/// 只有派生 meta 缺失时才有额外 IO。用于 split_by_streamer=false 时合并文件落在扁平目录、
/// 其 meta 仍在 `meta/{user}/` 下的归属解析，使手动、批量、取消、删除与重跑都解析到同一身份
/// （A1/A4）。
///
/// Resolve the actual meta file (recording identity) of a recording path under the given
/// meta root.
///
/// 1. If the derived meta ([`meta_path_in`]) exists, return it (common case, no extra IO);
/// 2. otherwise look for the owning meta by stem whose `video_path` equals this path
///    exactly: for recorder-format stems only `meta_root/{user}/{stem}.json` is checked
///    (recorder-generated meta always lives there), otherwise each first-level
///    subdirectory of the meta root is checked;
/// 3. if nothing is found, return the derived path (used to create a new meta).
///
/// Extra IO only happens when the derived meta is missing. Used to resolve ownership when
/// split_by_streamer=false puts the merged file in a flat directory while its meta stays
/// under `meta/{user}/`, so manual, batch, cancel, delete and re-runs all resolve to the
/// same identity (A1/A4).
pub fn resolve_meta_path_in(meta_root: &Path, path: &Path) -> Option<PathBuf> {
    let derived = meta_path_in(meta_root, path)?;
    if derived.is_file() {
        return Some(derived);
    }
    let stem = recording_stem(path)?;
    let file = format!("{stem}.json");
    // 候选 meta 的 video_path 恰好指向该路径时才算归属
    // A candidate only owns the path when its video_path points exactly at it
    let owns = |cand: &Path| -> bool {
        cand != derived
            && cand.is_file()
            && std::fs::read_to_string(cand)
                .ok()
                .and_then(|c| serde_json::from_str::<VideoMeta>(&c).ok())
                .is_some_and(|m| m.video_path.is_some_and(|vp| Path::new(&vp) == path))
    };
    if let Some(user) = username_from_stem(stem) {
        let cand = meta_root.join(user).join(&file);
        if owns(&cand) {
            return Some(cand);
        }
        // 录制器 stem 的归属 meta 必在 meta/{user}/ 下，快路径即可定论
        // A recorder stem's owning meta always lives under meta/{user}/; the fast path is conclusive
        return Some(derived);
    }
    if let Ok(entries) = std::fs::read_dir(meta_root) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let cand = dir.join(&file);
            if owns(&cand) {
                return Some(cand);
            }
        }
    }
    Some(derived)
}

/// 解析录制路径的实际 meta 文件（= [`resolve_meta_path_in`] 作用于 [`meta_dir`]）。
/// Resolve the actual meta file of a recording path (= [`resolve_meta_path_in`] on [`meta_dir`]).
pub fn resolve_meta_path(path: &Path) -> Option<PathBuf> {
    resolve_meta_path_in(&meta_dir(), path)
}

/// 判断 meta 文件名是否是旧 stem 规则（`file_stem()`）截断的结果，是则返回新规则下的
/// 同目录目标路径；不需要迁移时返回 `None`。纯函数，不做 IO。
///
/// Check whether a meta file name is the result of truncation by the old stem rule
/// (`file_stem()`); if so, return the target path under the new rule in the same
/// directory, otherwise `None`. Pure function, no IO.
pub fn legacy_truncated_meta_target(meta_file: &Path, video_path: &Path) -> Option<PathBuf> {
    let meta_stem = meta_file.file_stem()?.to_str()?;
    let full = recording_stem(video_path)?;
    if full == meta_stem {
        return None;
    }
    // 旧规则：对新 stem 再取一次 file_stem / Old rule: file_stem applied to the new stem
    let truncated = Path::new(full).file_stem()?.to_str()?;
    if truncated != meta_stem {
        return None;
    }
    Some(meta_file.with_file_name(format!("{full}.json")))
}

/// 递归收集 meta 根目录下所有 `.json` 元数据文件的完整路径。
///
/// 新结构为 `meta_dir/{username}/{stem}.json`（一层子目录）；同时兼容扫描
/// meta_dir 根目录下可能残留的旧版扁平文件（尚未被 [`super::maintenance::migrate_flat_meta_files`]
/// 迁移的情况，如迁移失败或迁移函数尚未运行）。
///
/// 供所有需要枚举全部 meta 文件的场景使用（前端录制列表、后处理任务历史、
/// 启动扫描、孤立文件清理），避免每处各自重复实现子目录遍历逻辑。
///
/// Recursively collect the full paths of all `.json` metadata files under the meta root.
///
/// The new layout is `meta_dir/{username}/{stem}.json` (one level of subdirectories);
/// also scans for legacy flat files directly under meta_dir root that may not have been
/// migrated yet (e.g. migration failed or hasn't run — see
/// [`super::maintenance::migrate_flat_meta_files`]).
///
/// Used by every scenario that needs to enumerate all meta files (frontend recording
/// list, post-processing task history, startup scan, orphan cleanup), avoiding each
/// call site reimplementing subdirectory traversal.
pub fn list_all_meta_paths() -> Vec<PathBuf> {
    list_all_meta_paths_in(&meta_dir())
}

/// 在指定 meta 根目录下收集所有 `.json` 元数据文件（规则同 [`list_all_meta_paths`]）。
/// Collect all `.json` metadata files under the given meta root (same rules as [`list_all_meta_paths`]).
pub fn list_all_meta_paths_in(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            // 兼容尚未迁移的旧版扁平文件 / Legacy flat file not yet migrated
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                result.push(path);
            }
            continue;
        }
        if path.is_dir()
            && let Ok(sub_entries) = std::fs::read_dir(&path)
        {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.is_file()
                    && sub_path.extension().and_then(|e| e.to_str()) == Some("json")
                {
                    result.push(sub_path);
                }
            }
        }
    }
    result
}

/// 视频文件扩展名列表（用于从模块输出路径中挑出"非视频"的预览图/图片路径）。
/// List of video file extensions (used to pick out "non-video" preview image paths
/// from a module's output paths).
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "ts", "avi", "mov"];

/// 从 `pp_execution` 中提取"已验证存在于磁盘上"的模块输出路径，按 module_id 建立映射。
///
/// 前端（`frontend/`）是纯 HTTP 客户端，没有文件系统访问权限，无法自行判断某个
/// 路径当前是否真实存在于磁盘上——因此这一验证必须在后端完成，作为录制列表接口
/// （`list_recordings_inner`）返回数据的一部分，而不是让前端仅凭 meta 中记录的
/// 路径字符串就断定预览图可用。
///
/// 纳入条件（两者都满足）：
/// 1. 节点执行结果码为 `"ok"`（唯一表示真正产出了有效输出的结果码；`done` 表示无
///    输出即终止、`error`/`cancelled` 表示失败、`skipped` 表示未执行）
/// 2. 该输出路径指向的文件**此刻仍然存在**——排除已被后续操作（如 cleanup 模块
///    清理预览图、用户手动删除、磁盘清理工具误删等）删除的陈旧记录
///
/// Extract module output paths that are "verified to currently exist on disk" from
/// `pp_execution`, keyed by module_id.
///
/// The frontend (`frontend/`) is a pure HTTP client with no filesystem access, so it
/// cannot determine on its own whether a given path currently exists on disk — this
/// verification must happen on the backend, as part of the data returned by the
/// recording list endpoint (`list_recordings_inner`), rather than the frontend assuming
/// a preview is available just because meta records a path string for it.
///
/// Inclusion requires both:
/// 1. The node's result code is `"ok"` (the only code indicating genuine valid output;
///    `done` means no output/pipeline terminated, `error`/`cancelled` mean failure,
///    `skipped` means not executed)
/// 2. The file at that output path **currently still exists** — excluding stale records
///    for files removed by a later operation (cleanup module removing the preview,
///    manual user deletion, an external disk-cleanup tool, etc.)
pub fn extract_verified_module_outputs(
    pp_execution: Option<&[PpExecutionEntry]>,
) -> std::collections::HashMap<String, String> {
    let mut result = std::collections::HashMap::new();
    for entry in pp_execution.into_iter().flatten() {
        if entry.result.as_ref().map(|r| &r.code) != Some(&PpExecCode::Ok) {
            continue;
        }
        let non_video = entry.outputs.iter().flatten().find(|p| {
            let lower = p.to_ascii_lowercase();
            !VIDEO_EXTENSIONS.iter().any(|ext| lower.ends_with(&format!(".{}", ext)))
        });
        if let Some(path) = non_video
            && Path::new(path).is_file()
        {
            result.insert(entry.module_id.clone(), path.clone());
        }
    }
    result
}

/// 从文件名 stem（格式：`{name}_{YYYYMMDD}_{HHmmss}`）中解析录制开始时间。
/// Parse the recording start time from a filename stem (format: `{name}_{YYYYMMDD}_{HHmmss}`).
pub fn parse_timestamp_from_stem(stem: &str) -> Option<String> {
    use chrono::TimeZone;
    let parts: Vec<&str> = stem.rsplitn(3, '_').collect();
    if parts.len() < 2 {
        return None;
    }
    let time_part = parts[0];
    let date_part = parts[1];
    if date_part.len() == 8 && time_part.len() == 6 {
        let combined = format!("{}{}", date_part, time_part);
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&combined, "%Y%m%d%H%M%S") {
            let local = chrono::Local.from_local_datetime(&dt).single()?;
            return Some(local.to_rfc3339());
        }
    }
    None
}

/// 后处理模块的执行结果 / Execution result of a post-processing module
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpModuleResult {
    pub module_id: String,
    pub success: bool,
    pub message: String,
}

/// 后处理执行结果码 / Post-processing execution result code
///
/// - `ok`        — 执行成功，有输出传递给后续节点
/// - `done`      — 执行成功，无输出（流在此终止）
/// - `skipped`   — 节点被禁用或模块主动跳过
/// - `error`     — 执行失败
/// - `cancelled` — 被用户取消
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PpExecCode {
    Ok,
    Done,
    Skipped,
    Error,
    Cancelled,
}

/// 后处理单步执行结果 / Result of a single post-processing step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpExecResult {
    pub code: PpExecCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 后处理流水线中单个节点的执行记录
///
/// `inputs`/`outputs` 均按输入/输出端口分组：外层 `Vec` 的下标对应端口索引
/// （与 `ModuleInfo.input_types`/`output_types` 的下标一致），内层 `Vec<String>`
/// 是该端口承载的路径列表——大多数端口只有一个路径，但 `MediaBundle` 类型的端口
/// 在传输时是"视频路径 + `\n` + 图片路径"的单一字符串（见 `builtin_nodes::pack_bundle`），
/// 在 meta 中记录时会拆分为该端口下的多个数组元素，避免下游读取者各自用 `\n`
/// 手动拆分。两个字段始终存在（不用 `Option`），流水线尚未产生输出时为空数组。
///
/// Execution record for a single node in the post-processing pipeline.
///
/// Both `inputs`/`outputs` are grouped by port: the outer `Vec`'s index is the port
/// index (matching `ModuleInfo.input_types`/`output_types`), and the inner `Vec<String>`
/// holds the path(s) carried on that port — most ports carry a single path, but a
/// `MediaBundle` port is transmitted as one "video path + `\n` + image path" string
/// (see `builtin_nodes::pack_bundle`) and is split into multiple array elements for that
/// port when recorded in meta, so downstream readers don't each have to split on `\n`
/// themselves. Both fields are always present (not `Option`); they're empty arrays
/// before the pipeline has produced any output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpExecutionEntry {
    /// 模块 ID（普通节点的主标识）/ Module ID (primary identifier for regular nodes)
    pub module_id: String,
    /// 节点实例 ID（仅可复用内置节点的多个实例需要）/ Node instance ID (only for multiple instances of reusable built-ins)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<PpExecResult>,
    #[serde(default)]
    pub inputs: Vec<Vec<String>>,
    #[serde(default)]
    pub outputs: Vec<Vec<String>>,
    /// 节点参数 + 连线的指纹（模块参数值和上游连线的组合哈希），用于检测流水线
    /// 重新执行时该节点的配置是否发生变更。不直接存储原始 params/wiring——
    /// 那些已经是 `pipeline.json` 的权威数据，在 meta 中重复保存一份属于冗余。
    ///
    /// Fingerprint of the node's params + wiring (a combined hash of module param
    /// values and upstream wiring), used to detect whether the node's configuration
    /// changed since its last run. Does not store the raw params/wiring — those are
    /// already authoritatively stored in `pipeline.json`; keeping a duplicate copy
    /// in meta would be redundant.
    pub config_fingerprint: String,
}

impl PpExecutionEntry {
    /// 返回节点有效标识：node_id 优先，否则用 module_id。
    /// Returns the effective identifier: node_id if present, otherwise module_id.
    pub fn effective_id(&self) -> &str {
        self.node_id.as_deref().unwrap_or(&self.module_id)
    }
}

/// 正在执行的后处理节点的模块内进度快照。
/// 节点开始时写入，on_progress 实时更新，节点完成（无论成功/失败）后清空。
/// 当 `pp_execution` 中存在 `result == null` 的条目时，此字段表示该节点的当前进度。
///
/// In-progress post-processing node's intra-module progress snapshot.
/// Written on node start, updated by on_progress, cleared when the node finishes.
/// When a `pp_execution` entry has `result == null`, this field shows its current progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpNodeProgress {
    /// 模块 ID / Module ID
    pub module_id: String,
    /// 节点实例 ID（仅可复用内置节点多实例时需要）/ Node instance ID (only for reusable built-in multi-instances)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub mod_done: u32,
}

impl PpNodeProgress {
    pub fn effective_id(&self) -> &str {
        self.node_id.as_deref().unwrap_or(&self.module_id)
    }
}

/// 视频元数据，持久化到 `meta/{stem}.json`。
/// Video metadata persisted to `meta/{stem}.json`.
///
/// `status` 字段记录当前处理阶段：
/// - `"recording"`  — 正在录制
/// - `"pp_waiting"` — 等待后处理
/// - `"pp_running"` — 后处理执行中
/// - `"pp_error"`   — 后处理失败
/// - `"finish"`     — 全部完成
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoMeta {
    /// meta 格式版本 / Meta format version
    #[serde(default)]
    pub meta_version: u32,
    /// 当前处理状态 / Current processing status
    pub status: String,
    /// 录制开始时间（RFC 3339）/ Recording start time (RFC 3339)
    pub started_at: String,
    /// 文件大小（字节）/ File size (bytes)
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pp_execution: Option<Vec<PpExecutionEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_downloaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments_failed: Option<u64>,
    /// 对应视频文件或 session_dir 的绝对路径（write_meta 自动填入，供孤立清理使用）。
    /// Absolute path of the corresponding video file or session_dir (auto-filled by write_meta,
    /// used for orphaned meta cleanup).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub video_path: Option<String>,
    /// 当前正在执行的节点的模块内进度（节点未在执行时为 null）。
    /// 前端应优先读取此字段作为实时进度，而非依赖 SSE 进度事件。
    ///
    /// Intra-module progress of the currently executing node (null when no node is running).
    /// Frontend should read this field for real-time progress instead of relying on SSE progress events.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pp_progress: Option<PpNodeProgress>,
}