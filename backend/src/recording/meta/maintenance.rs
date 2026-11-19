//! Meta 定时维护与孤立清理 / Meta Scheduled Maintenance and Orphan Cleanup
//!
//! 提供孤立 meta 文件清理、输出目录维护主流程（[`maintain_output_dir`]）及其
//! 定时调度封装。程序启动时的一次性检查与周期性维护共用同一套逻辑
//! （见 [`maintain_output_dir`] 的调用方式）。
//!
//! Provides orphaned meta file cleanup, the output-directory maintenance main flow
//! ([`maintain_output_dir`]), and its scheduled wrappers. The one-shot startup check
//! and periodic maintenance share identical logic (see how [`maintain_output_dir`] is
//! invoked).
//!
//! 子模块划分 / Submodule layout:
//! - `migration`：旧版 meta 文件的一次性迁移 / one-shot migrations of legacy meta files
//! - `cleanup`：孤立 meta、临时文件、空目录与 tmp 清理 / orphan/temp/empty-dir/tmp cleanup
//! - `output`：输出目录维护主流程与定时调度 / maintenance main flow and scheduling

mod cleanup;
mod migration;
mod output;

pub use cleanup::{cleanup_orphaned_meta_files, cleanup_tmp_on_startup};
pub use migration::{migrate_flat_meta_files, migrate_truncated_stem_meta_files};
pub use output::{maintain_output_dir, schedule_meta_version_check};
