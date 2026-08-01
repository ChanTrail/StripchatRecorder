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
    /// 模块可执行文件路径（不序列化，运行时填充）/ Executable path (not serialized, filled at runtime)
    #[serde(skip)]
    pub exe_path: PathBuf,
}

// ─── DAG 数据结构 / DAG Data Structures ──────────────────────────────────────

/// 流水线节点（模块实例）/ Pipeline node (module instance)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipelineNode {
    /// 节点唯一 ID（UUID）/ Node unique ID (UUID)
    pub node_id: String,
    /// 对应的模块 ID / Corresponding module ID
    pub module_id: String,
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
    /// 上游节点 ID（"0" 表示录制输入节点）/ Upstream node ID ("0" = recording input node)
    pub node_id: String,
    /// 上游节点的输出端口索引 / Upstream output port index
    pub port: usize,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PipelineConfig {
    /// 节点列表 / Node list
    pub nodes: Vec<PipelineNode>,
    /// 有向边列表 / Directed edge list
    #[serde(default)]
    pub edges: Vec<PipelineEdge>,
    /// 虚拟录制输入节点在画布中的位置（仅前端使用，后端透传保存）
    /// Virtual recording input node position on canvas (frontend-only, backend stores as-is)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_node_position: Option<serde_json::Value>,
}

impl PipelineConfig {
    /// 合并 nodes[].inputs 和顶层 edges，返回完整的有向边列表。
    /// nodes[].inputs 中 node_id="0" 的连接表示来自录制输入节点，执行时作为初始输入，
    /// 不产生实际的 PipelineEdge（run_pipeline 通过 root_nodes 处理）。
    ///
    /// Merges nodes[].inputs and top-level edges into a complete directed edge list.
    /// Connections with node_id="0" in nodes[].inputs represent the recording input node;
    /// they don't produce actual PipelineEdges (handled by root_nodes via run_pipeline).
    pub fn resolved_edges(&self) -> Vec<PipelineEdge> {
        let mut edges: Vec<PipelineEdge> = Vec::new();

        // 从每个节点的 inputs 字段生成边（排除来自录制输入节点 "0" 的连接）
        // Generate edges from each node's inputs field (skip connections from recording input node "0")
        for node in &self.nodes {
            for (to_port, input_ref) in &node.inputs {
                if input_ref.node_id == "0" {
                    // 来自录制输入节点，不生成 edge，由 root_nodes() 处理
                    // From recording input node — not an edge, handled by root_nodes()
                    continue;
                }
                edges.push(PipelineEdge {
                    from_node_id: input_ref.node_id.clone(),
                    from_port: input_ref.port,
                    to_node_id: node.node_id.clone(),
                    to_port: *to_port,
                });
            }
        }

        // 合并顶层 edges（后向兼容旧格式）去重
        // Merge top-level edges (backward compat with old format), dedup
        for e in &self.edges {
            let already = edges.iter().any(|x| {
                x.from_node_id == e.from_node_id
                    && x.from_port == e.from_port
                    && x.to_node_id == e.to_node_id
                    && x.to_port == e.to_port
            });
            if !already {
                edges.push(e.clone());
            }
        }

        edges
    }

    /// 返回没有入边但有出边、或者其 inputs 中有来自录制输入节点("0")的连接的节点 ID 列表。
    /// 孤立节点（无任何入边且无出边且 inputs 为空）不作为根节点，不会被执行。
    ///
    /// Returns node IDs that are connected to the recording input node ("0") via their inputs field,
    /// OR that have outgoing edges but no incoming edges in the resolved edge list.
    /// Isolated nodes are NOT treated as roots and will not run.
    pub fn root_nodes(&self) -> Vec<&str> {
        let edges = self.resolved_edges();
        let has_incoming: std::collections::HashSet<&str> =
            edges.iter().map(|e| e.to_node_id.as_str()).collect();
        let has_outgoing: std::collections::HashSet<&str> =
            edges.iter().map(|e| e.from_node_id.as_str()).collect();

        self.nodes
            .iter()
            .filter(|n| {
                if !n.enabled {
                    return false;
                }
                // 直接连接到录制输入节点 "0" 的节点是根节点
                // Nodes directly connected to recording input node "0" are roots
                let connected_to_input = n.inputs.values().any(|r| r.node_id == "0");
                if connected_to_input {
                    return true;
                }
                // 无入边但有出边的节点也是根节点（后向兼容）
                // No incoming edges but has outgoing edges (backward compat)
                !has_incoming.contains(n.node_id.as_str())
                    && has_outgoing.contains(n.node_id.as_str())
            })
            .map(|n| n.node_id.as_str())
            .collect()
    }

    /// 返回节点的直接后继节点信息（边 + 目标节点）。
    /// Returns direct successors of a node (edges + target nodes).
    pub fn successors(&self, node_id: &str) -> Vec<(PipelineEdge, &PipelineNode)> {
        let edges = self.resolved_edges();
        edges
            .into_iter()
            .filter(|e| e.from_node_id == node_id)
            .filter_map(|e| {
                self.nodes
                    .iter()
                    .find(|n| n.node_id == e.to_node_id && n.enabled)
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
    /// 节点 ID / Node ID
    pub node_id: String,
    /// 模块 ID / Module ID
    pub module_id: String,
    /// 执行结果码 / Result code
    pub code: PpExecCode,
    /// 结果消息 / Result message
    pub message: String,
    /// 输出路径列表（传递给后继节点）/ Output paths (passed to successor nodes)
    pub outputs: Vec<PathBuf>,
    /// 本次节点实际使用的输入路径 / Actual input paths used by this node
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
