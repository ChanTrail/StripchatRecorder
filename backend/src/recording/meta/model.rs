//! Meta 数据结构与路径计算 / Meta Data Structures and Path Resolution
//!
//! 定义 `VideoMeta` 及其嵌套的后处理执行记录/进度类型，以及 meta 文件路径的计算规则。
//! 不涉及文件读写（见 `super::store`）或扫描重建逻辑（见 `super::scan`）。
//!
//! Defines `VideoMeta` and its nested post-processing execution/progress types, plus
//! meta file path resolution rules. Does not perform file I/O (see `super::store`) or
//! scan/rebuild logic (see `super::scan`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 当前 meta 文件格式版本。
/// Current meta file format version.
pub const META_VERSION: u32 = 3;

/// 返回集中存放 meta 文件的目录（exe_dir/meta/）。
/// Returns the centralized meta storage directory (exe_dir/meta/).
pub fn meta_dir() -> PathBuf {
    crate::config::settings::exe_dir().join("meta")
}

/// 根据视频文件路径（或 session_dir）计算对应的元数据文件路径。
/// 文件名格式：`{stem}.json`，存储在 meta_dir() 下。
///
/// Compute the metadata file path for a given video file path or session_dir.
/// Format: `{stem}.json` stored under meta_dir().
pub fn meta_path_for(video_path: &Path) -> Option<PathBuf> {
    let stem = video_path.file_stem()?.to_str()?;
    Some(meta_dir().join(format!("{}.json", stem)))
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

/// 节点在某次执行时的单个输入端口连线快照（上游节点 ID + 端口索引）。
/// 用于检测流水线重新连线后，该节点是否需要重新执行（即使模块本身参数未变）。
///
/// Snapshot of a single input port's wiring (upstream node ID + port index) at the
/// time of a given execution. Used to detect whether a node must be re-run after
/// pipeline rewiring, even if the module's own params are unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PpNodeInputSnapshot {
    pub node_id: String,
    pub port: usize,
}

/// 后处理流水线中单个节点的执行记录
/// Execution record for a single node in the post-processing pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpExecutionEntry {
    pub node_id: String,
    pub module_id: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<PpExecResult>,
    pub inputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<String>>,
    /// 本次执行时该节点的参数快照（用于判断重新触发时流水线是否已变更）。
    /// 旧版 meta 文件缺少此字段时默认为空表，比较时会被视为"配置已变化"，
    /// 从而保守地触发一次重新执行（而非静默沿用可能已过期的结果）。
    ///
    /// Snapshot of the node's params at the time of this execution (used to detect
    /// pipeline changes on re-trigger). Missing on old meta files, defaulting to an
    /// empty map — comparisons against an empty map are treated as "config changed",
    /// conservatively forcing a re-run rather than silently reusing a possibly-stale result.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub params: HashMap<String, serde_json::Value>,
    /// 本次执行时该节点的输入端口连线快照（端口索引 → 上游来源）。
    /// Snapshot of the node's input port wiring at the time of this execution
    /// (port index → upstream source).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub wiring: HashMap<usize, PpNodeInputSnapshot>,
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
    /// 正在执行的节点 ID / Currently executing node ID
    pub node_id: String,
    /// 正在执行的模块 ID / Currently executing module ID
    pub module_id: String,
    /// 模块内已完成进度值（对应 PROGRESS:{done}/{total} 中的 done）
    /// Intra-module done value (the `done` in PROGRESS:{done}/{total})
    pub mod_done: u32,
    /// 模块内总进度值（对应 PROGRESS:{done}/{total} 中的 total）
    /// Intra-module total value (the `total` in PROGRESS:{done}/{total})
    pub mod_total: u32,
    /// 整体已完成节点数（到此节点开始时为止）/ Overall completed node count at node start
    pub overall_done: usize,
    /// 整体总节点数 / Overall total node count
    pub overall_total: usize,
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
