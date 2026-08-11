//! 流水线数据模型：端口类型、模块描述、DAG 结构、节点执行结果
//! Pipeline Data Model: Port Types, Module Description, DAG Structure, Node Execution Result
//!
//! 定义流水线的静态数据结构，不涉及模块发现（见 `super::discovery`）或
//! DAG 执行调度（见 `super::exec`）。
//!
//! Defines the pipeline's static data structures. Does not perform module discovery
//! (see `super::discovery`) or DAG execution scheduling (see `super::exec`).

use crate::recording::meta::PpExecCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ─── 端口类型系统 / Port Type System ─────────────────────────────────────────

/// 模块端口的数据类型 / Data type for a module port
///
/// 连接两个节点时，上游节点的输出类型必须与下游节点对应输入类型兼容。
/// When connecting two nodes, the upstream output type must be compatible with the downstream input type.
///
/// 兼容规则 / Compatibility rules:
/// - 完全相同的类型可以连接 / Identical types can be connected
/// - 任意类型可以连接到 `AnyFile` 或 `AnyDir` / Any type can connect to `AnyFile` or `AnyDir`
/// - `MediaBundle` 可以连接到 `AnyFile`（视为单一文件句柄传递）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// TS 分片录制目录（主程序产生，作为 ts_merge 的输入）
    /// TS segment recording directory (produced by the host, input to ts_merge)
    TsSessionDir,
    /// 单个视频文件（mp4/mkv/ts 等）/ Single video file (mp4/mkv/ts etc.)
    VideoFile,
    /// 单个图片文件 / Single image file
    ImageFile,
    /// 媒体包（视频文件路径 + 图片文件路径，以 `\n` 分隔，单一路径字符串传递）。
    /// 仅当视频和图片都存在时才输出此类型；下游节点负责按换行符拆分。
    ///
    /// Media bundle (video path + image path separated by `\n`, passed as a single path string).
    /// Only emitted when both video and image exist; downstream nodes split on newline.
    MediaBundle,
    /// 任意文件（可接受上游任何文件类型）/ Any file (accepts any upstream file type)
    AnyFile,
    /// 任意目录（可接受上游任何目录类型）/ Any directory (accepts any upstream directory type)
    AnyDir,
}

impl PortType {
    /// 判断 `self`（上游输出类型）是否可以连接到 `target`（下游输入类型）。
    /// Check whether `self` (upstream output type) is compatible with `target` (downstream input type).
    pub fn is_compatible_with(&self, target: &PortType) -> bool {
        if self == target {
            return true;
        }
        match (self, target) {
            // 任何文件类型（含 MediaBundle）都可以接入 AnyFile
            // Any file type (including MediaBundle) can connect to AnyFile
            (PortType::VideoFile | PortType::ImageFile | PortType::MediaBundle, PortType::AnyFile) => true,
            // 任何目录类型都可以接入 AnyDir / Any directory type can connect to AnyDir
            (PortType::TsSessionDir, PortType::AnyDir) => true,
            _ => false,
        }
    }
}

// ─── 模块描述 / Module Description ───────────────────────────────────────────

/// 模块参数定义（从 `--describe` 输出中反序列化）/ Module parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamDef {
    pub key: String,
    pub label: String,
    pub r#type: String,
    pub default: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// 后处理模块信息（从 `--describe` 输出中反序列化）/ Post-processing module info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleInfo {
    /// 模块唯一 ID / Module unique ID
    pub id: String,
    /// 模块显示名称 / Module display name
    pub name: String,
    /// 模块版本号（从模块自身 Cargo.toml 的 `version` 字段读取，见 `pp_utils::describe_with_version`；
    /// 内置节点使用 backend 自身的 `CARGO_PKG_VERSION`）。旧版模块若未提供此字段，默认空字符串。
    /// Module version (read from the module's own Cargo.toml `version` field, see
    /// `pp_utils::describe_with_version`; built-in nodes use the backend's own
    /// `CARGO_PKG_VERSION`). Defaults to an empty string if an older module omits this field.
    #[serde(default)]
    pub version: String,
    /// 模块功能描述 / Module description
    pub description: String,
    /// 输入端口类型列表（按顺序对应 inputs 数组各元素）/ Input port types (indexed with inputs array)
    pub input_types: Vec<PortType>,
    /// 输出端口类型列表（按顺序对应 outputs 数组各元素）/ Output port types (indexed with outputs array)
    pub output_types: Vec<PortType>,
    /// 参数定义列表 / Parameter definitions
    pub params: Vec<ParamDef>,
    /// 多语言翻译（可选）/ i18n translations (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i18n: Option<serde_json::Value>,
    /// 是否为官方模块（在 UI 中显示提示，建议置于 ts_merge 之后）
    /// Whether this is an official module (UI shows a hint to place it after ts_merge)
    #[serde(default)]
    pub official: bool,
    /// 是否为可复用内置节点（可在流水线中放置多次，需要 node_id 区分实例）
    /// Whether this is a reusable built-in node (can appear multiple times in pipeline; needs node_id per instance)
    #[serde(default)]
    pub reusable: bool,
    /// 模块可执行文件路径（不序列化，运行时填充）/ Executable path (not serialized, filled at runtime)
    #[serde(skip)]
    pub exe_path: PathBuf,
}

// ─── DAG 数据结构 / DAG Data Structures ──────────────────────────────────────

/// 流水线节点（模块实例）/ Pipeline node (module instance)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineNode {
    /// 模块 ID，同时也是普通节点的唯一标识 / Module ID, also serves as unique identifier for regular nodes
    pub module_id: String,
    /// 节点实例 ID（仅可复用内置节点的多个实例需要，普通节点不填）
    /// Node instance ID (only needed for multiple instances of reusable built-in nodes; omitted for regular nodes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// 节点参数值 / Node parameter values
    pub params: HashMap<String, serde_json::Value>,
    /// 是否启用此节点 / Whether this node is enabled
    pub enabled: bool,
    /// 节点在画布中的位置（仅前端使用，后端透传保存）/ Node position on canvas (frontend-only, backend stores as-is)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<serde_json::Value>,
    /// 输入端口的连接来源：端口索引 → (上游节点 ID, 上游端口索引)。
    /// 上游为录制输入节点时，节点 ID 固定为 "0"。
    /// 此字段是连线信息的权威来源，优先于顶层 edges 字段。
    ///
    /// Input port wiring: port index → (upstream node ID, upstream port index).
    /// Upstream node ID "0" means the recording input node.
    /// This field is the authoritative source for wiring; takes precedence over top-level edges.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub inputs: HashMap<usize, NodeInputRef>,
}

/// 单个输入端口的连接来源 / Wiring source for a single input port
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInputRef {
    /// 上游节点标识（"0" 表示录制输入节点）/ Upstream node identifier ("0" = recording input node)
    pub node_id: String,
    /// 上游节点的输出端口索引 / Upstream output port index
    pub port: usize,
}

impl PipelineNode {
    /// 返回节点的有效唯一标识：有 node_id 时用 node_id，否则用 module_id。
    /// Returns the effective unique identifier: node_id if present, otherwise module_id.
    pub fn effective_id(&self) -> &str {
        self.node_id.as_deref().unwrap_or(&self.module_id)
    }
}

/// DAG 中的一条有向边，表示上游节点的某个输出连接到下游节点的某个输入。
/// A directed edge in the DAG, connecting an upstream output port to a downstream input port.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineEdge {
    /// 上游节点 ID / Upstream node ID
    pub from_node_id: String,
    /// 上游节点的输出端口索引（对应 ModuleInfo.output_types）/ Upstream output port index
    pub from_port: usize,
    /// 下游节点 ID / Downstream node ID
    pub to_node_id: String,
    /// 下游节点的输入端口索引（对应 ModuleInfo.input_types）/ Downstream input port index
    pub to_port: usize,
}

/// 流水线 DAG 配置 / Pipeline DAG configuration
///
/// 连线信息唯一存储在 `nodes[].inputs` 中，不再有独立的顶层 `edges` 字段
/// （历史上曾同时维护两者，导致状态可能不一致）。需要边列表形式时调用
/// `resolved_edges()` 从 `nodes[].inputs` 实时派生。
///
/// Wiring is stored exclusively in `nodes[].inputs`; there is no separate top-level
/// `edges` field (previously both were maintained in parallel, risking desync).
/// Call `resolved_edges()` to derive an edge-list view from `nodes[].inputs` on demand.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {
    /// 配置格式版本号（用于向后兼容和更新检测）
    /// Configuration format version (for backward compatibility and update detection)
    #[serde(default = "default_pipeline_version")]
    pub version: String,
    /// 节点列表 / Node list
    pub nodes: Vec<PipelineNode>,
    /// 虚拟录制输入节点在画布中的位置（仅前端使用，后端透传保存）
    /// Virtual recording input node position on canvas (frontend-only, backend stores as-is)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_node_position: Option<serde_json::Value>,
}

/// 默认的流水线配置版本号 / Default pipeline configuration version
fn default_pipeline_version() -> String {
    "1".to_string()
}

impl PipelineConfig {
    /// 从 nodes[].inputs 派生完整的有向边列表（唯一的连线数据来源）。
    /// node_id="0" 的连接表示来自录制输入节点，执行时作为初始输入，
    /// 不产生实际的 PipelineEdge（run_pipeline 通过 root_nodes 处理）。
    ///
    /// Derive the directed edge list from nodes[].inputs (the sole source of wiring data).
    /// Connections with node_id="0" represent the recording input node; they don't produce
    /// actual PipelineEdges (handled by root_nodes via run_pipeline).
    pub fn resolved_edges(&self) -> Vec<PipelineEdge> {
        let mut edges: Vec<PipelineEdge> = Vec::new();

        for node in &self.nodes {
            for (to_port, input_ref) in &node.inputs {
                if input_ref.node_id == "0" {
                    continue;
                }
                edges.push(PipelineEdge {
                    from_node_id: input_ref.node_id.clone(),
                    from_port: input_ref.port,
                    to_node_id: node.effective_id().to_string(),
                    to_port: *to_port,
                });
            }
        }

        edges
    }

    /// 返回没有入边但有出边、或者其 inputs 中有来自录制输入节点("0")的连接的节点 ID 列表。
    /// 孤立节点（无任何入边且无出边且 inputs 为空）不作为根节点，不会被执行。
    ///
    /// 不按 `enabled` 过滤：被禁用的根节点仍需进入执行队列，由 `exec::run_pipeline`
    /// 将其作为"跳过（透传）"节点处理——原样把输入转发给下游，而不是从图中整个
    /// 消失导致下游永远收不到输入（见 `run_pipeline` 主循环中对 `!node.enabled`
    /// 的处理逻辑及其文档说明）。
    ///
    /// Returns node IDs that are connected to the recording input node ("0") via their inputs field,
    /// OR that have outgoing edges but no incoming edges in the resolved edge list.
    /// Isolated nodes are NOT treated as roots and will not run.
    ///
    /// Does NOT filter by `enabled`: a disabled root node still needs to enter the
    /// execution queue, where `exec::run_pipeline` treats it as a "skip (pass-through)"
    /// node — forwarding its input to downstream nodes unchanged, rather than vanishing
    /// from the graph entirely and leaving downstream nodes permanently starved of input
    /// (see the handling of `!node.enabled` in `run_pipeline`'s main loop and its doc comment).
    pub fn root_nodes(&self) -> Vec<&str> {
        let edges = self.resolved_edges();
        let has_incoming: std::collections::HashSet<&str> =
            edges.iter().map(|e| e.to_node_id.as_str()).collect();
        let has_outgoing: std::collections::HashSet<&str> =
            edges.iter().map(|e| e.from_node_id.as_str()).collect();

        self.nodes
            .iter()
            .filter(|n| {
                let connected_to_input = n.inputs.values().any(|r| r.node_id == "0");
                if connected_to_input {
                    return true;
                }
                let eid = n.effective_id();
                !has_incoming.contains(eid) && has_outgoing.contains(eid)
            })
            .map(|n| n.effective_id())
            .collect()
    }

    /// 返回从 `node_id` 出发的所有下游边及其目标节点。
    ///
    /// 不按目标节点的 `enabled` 过滤：被禁用的下游节点仍需被 `exec::run_pipeline`
    /// 当作有效的分发目标接收输入（原因同 [`Self::root_nodes`] 的文档说明）——
    /// 否则任何一条分支上出现禁用节点，都会导致该分支自身以及其后所有节点被整个
    /// 从执行队列中排除，而不仅仅是禁用节点自身不运行。
    ///
    /// Returns all outgoing edges from `node_id` and their target nodes.
    ///
    /// Does NOT filter by the target node's `enabled`: a disabled downstream node still
    /// needs to be a valid dispatch target for `exec::run_pipeline` to receive input
    /// (same reasoning as [`Self::root_nodes`]'s doc comment) — otherwise a disabled
    /// node anywhere along a branch would exclude that entire branch (and everything
    /// after it) from the execution queue, not just the disabled node itself.
    pub fn successors(&self, node_id: &str) -> Vec<(PipelineEdge, &PipelineNode)> {
        let edges = self.resolved_edges();
        edges
            .into_iter()
            .filter(|e| e.from_node_id == node_id)
            .filter_map(|e| {
                self.nodes
                    .iter()
                    .find(|n| n.effective_id() == e.to_node_id)
                    .map(|n| (e, n))
            })
            .collect()
    }
}

// ─── 节点执行结果 / Node Execution Result ────────────────────────────────────

/// 模块通过 stdout 最后一行返回的 JSON 结构 / JSON structure returned by module as last stdout line
#[derive(Debug, Clone, Deserialize)]
pub struct ModuleOutput {
    /// 结果码 / Result code
    pub code: PpExecCode,
    /// 可选日志消息 / Optional log message
    #[serde(default)]
    pub message: Option<String>,
    /// 输出路径列表；空或缺失表示流水线在此终止 / Output paths; empty or missing terminates pipeline
    #[serde(default)]
    pub outputs: Option<Vec<String>>,
}

/// 单个节点的执行结果（供调用方聚合）/ Execution result of a single node (for aggregation by caller)
#[derive(Debug, Clone)]
pub struct NodeResult {
    /// 节点有效标识（effective_id：普通节点=module_id，可复用内置节点=node_id）
    /// Effective node identifier (module_id for regular nodes, node_id for reusable built-ins)
    pub effective_id: String,
    /// 模块 ID / Module ID
    pub module_id: String,
    pub code: PpExecCode,
    pub message: String,
    pub outputs: Vec<PathBuf>,
    pub inputs: Vec<PathBuf>,
}

impl NodeResult {
    /// 判断执行是否成功（ok 或 done 或 skipped）。
    /// Whether execution succeeded (ok, done, or skipped).
    pub fn is_success(&self) -> bool {
        matches!(self.code, PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
    }

    /// 判断流水线是否应在此节点终止（done/error/cancelled）。
    /// Whether the pipeline should terminate at this node (done/error/cancelled).
    pub fn is_terminal(&self) -> bool {
        matches!(self.code, PpExecCode::Done | PpExecCode::Error | PpExecCode::Cancelled)
    }
}
