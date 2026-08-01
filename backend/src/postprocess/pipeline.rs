//! 后处理流水线引擎 / Post-processing Pipeline Engine
//!
//! ## 模块协议 / Module Protocol
//!
//! ### `--describe` 输出 / `--describe` output
//! 模块可执行文件以 `--describe` 参数调用时，向 stdout 输出 JSON 格式的模块元数据。
//! Module executables output JSON metadata to stdout when called with `--describe`.
//!
//! ### 运行时调用 / Runtime invocation
//! 主程序将 JSON 输入通过 stdin 传给模块，模块向 stdout 输出进度行和最终结果 JSON。
//! The host passes JSON input via stdin; the module writes progress lines and a final JSON result to stdout.
//!
//! **stdin 格式 / stdin format:**
//! ```json
//! {
//!   "inputs": ["/path/to/file_or_dir"],
//!   "params": { "key": "value" },
//!   "exe_dir": "/path/to/modules/",
//!   "max_tmp_mb": 51200,
//!   "recording": { "video_path": "...", "started_at": "...", "username": "..." }
//! }
//! ```
//!
//! **stdout 非 JSON 行 / Non-JSON stdout lines:**
//! - `PROGRESS:{done}/{total}` — 进度上报 / Progress reporting
//! - `STATUS:{text}` — 状态文字（如上传速度）/ Status text (e.g. upload speed)
//!
//! **stdout 最后一行 JSON（模块返回值）/ Last JSON line on stdout (module return value):**
//! ```json
//! {
//!   "code": "ok" | "done" | "skipped" | "error" | "cancelled",
//!   "message": "optional log/error message",
//!   "outputs": ["/path/to/output"]
//! }
//! ```
//! - `outputs` 为空数组或缺失 → `code: "done"` → 流水线终止
//! - `outputs` 非空 → 传递给下游节点
//!
//! ## 子模块划分 / Submodule breakdown
//!
//! - [`model`]：端口类型系统、模块描述、DAG 数据结构（节点/边/配置）、节点执行结果
//! - [`discovery`]：扫描 `modules/` 目录并调用 `--describe` 获取模块元数据
//! - [`exec`]：DAG 拓扑执行调度、单节点子进程调用
//!
//! - [`model`]: port type system, module description, DAG data structures (nodes/edges/config),
//!   node execution results
//! - [`discovery`]: scans the `modules/` directory and invokes `--describe` for module metadata
//! - [`exec`]: DAG topological execution scheduling, single-node subprocess invocation

mod discovery;
mod exec;
mod model;

pub use discovery::{discover_modules, modules_dir};
pub use exec::{RecordingContext, run_pipeline};
pub use model::{
    ModuleInfo, ModuleOutput, NodeInputRef, NodeResult, ParamDef, PipelineConfig, PipelineEdge,
    PipelineNode, PortType,
};
