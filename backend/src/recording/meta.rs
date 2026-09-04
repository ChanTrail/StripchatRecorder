//! 视频元数据文件管理 / Video Metadata File Management
//!
//! 每个录制对应一个 JSON 元数据文件，按主播分子目录存储在
//! `exe_dir()/meta/{username}/` 下。文件名为 `{stem}.json`，与视频文件
//! （或 session_dir）的 stem 相同；`{username}` 从视频路径的直接父目录名推断
//! （见 [`model::username_from_path`]），与 session_dir/合并输出路径的
//! `{output_dir}/{username}/...` 结构保持一致。
//!
//! Each recording has a JSON metadata file stored in a per-streamer subdirectory
//! under `exe_dir()/meta/{username}/`. The filename is `{stem}.json`, matching the
//! stem of the video file or session_dir; `{username}` is inferred from the video
//! path's direct parent directory name (see [`model::username_from_path`]), matching
//! the `{output_dir}/{username}/...` structure used by session_dirs and merged output
//! paths.
//!
//! ## Status 状态流转 / Status lifecycle
//!
//! ```
//! recording → pp_waiting → pp_running → finish
//!                                     ↘ pp_error
//! ```
//!
//! 注：TS 分片合并现在是后处理流水线的第一个节点（ts_merge 模块），
//! 不再有单独的 merging_waiting / merging 状态。
//! Note: TS segment merging is now the first pipeline node (ts_merge module);
//! the separate merging_waiting / merging statuses no longer exist.
//!
//! ## 子模块划分 / Submodule breakdown
//!
//! - [`model`]：数据结构（`VideoMeta` 及嵌套类型）与路径计算
//! - [`store`]：meta 文件的读/写/删除及字段级增量更新
//! - [`scan`]：启动/定时扫描，创建、修复或重建缺失的 meta
//! - [`maintenance`]：孤立 meta 清理与输出目录维护的定时调度
//!
//! - [`model`]: data structures (`VideoMeta` and nested types) and path resolution
//! - [`store`]: meta file read/write/delete and field-level incremental updates
//! - [`scan`]: startup/periodic scanning to create, repair, or rebuild missing meta
//! - [`maintenance`]: orphaned meta cleanup and output-directory maintenance scheduling

mod maintenance;
mod model;
mod scan;
mod store;

pub use maintenance::{cleanup_orphaned_meta_files, maintain_output_dir, migrate_flat_meta_files, schedule_meta_version_check};
pub use model::{
    META_VERSION, PpExecCode, PpExecResult, PpExecutionEntry, PpModuleResult,
    PpNodeProgress, VideoMeta, extract_verified_module_outputs, list_all_meta_paths,
    meta_dir, meta_dir_for, meta_path_for, parse_timestamp_from_stem, username_from_path,
};
pub use scan::{ensure_meta_files, ts_merge_output_dir};
pub use store::{
    clear_pp_progress, delete_meta, ensure_meta, pp_execution_finish, pp_execution_start,
    read_meta, set_pp_done, set_pp_progress, set_segment_stats, set_status, write_meta,
};
