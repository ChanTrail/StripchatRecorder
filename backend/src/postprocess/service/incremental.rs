//! 增量后处理辅助子模块：脏节点计算、预填输入、历史结果合并与路径分组。
//! Incremental post-processing helpers: dirty-node computation, pre-filled
//! inputs, merging with previous results and path (un)grouping.

use crate::postprocess::pipeline::{NodeResult, PipelineConfig, PipelineNode};
use crate::recording::meta::{PpExecCode, PpExecutionEntry};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

/// 根据 video_path 推断后处理流水线的初始输入路径。
///
/// 规则：若与 video_path 同级、同名的目录（session_dir）存在，说明分片尚未被
/// segment_merge 合并，此时首节点（通常是 segment_merge）需要以 session_dir 作为输入。
/// 否则（session_dir 已删，或从未存在），直接用 video_path 作为输入。
///
/// Infer the initial input path for the post-processing pipeline from video_path.
///
/// Rule: if a directory with the same stem exists alongside video_path (session_dir),
/// the segments haven't been merged yet, so the first node (typically segment_merge)
/// must receive session_dir as its input.
/// Otherwise (session_dir already deleted or never existed), use video_path directly.
pub fn infer_initial_path(video_path: &std::path::Path) -> std::path::PathBuf {
    if let (Some(parent), Some(stem)) = (
        video_path.parent(),
        video_path.file_stem().and_then(|s| s.to_str()),
    ) {
        let session_dir = parent.join(stem);
        if session_dir.is_dir() {
            return session_dir;
        }
    }
    video_path.to_path_buf()
}

/// 计算节点当前参数 + 连线的指纹（sha256 十六进制字符串），用于与上次执行记录比较
/// 是否发生变更。不直接比较/存储原始 params/wiring——那些已是 `pipeline.json` 的
/// 权威数据，重复保存到 meta 属于冗余；这里只保留一个用于变更检测的哈希摘要。
/// BTreeMap 保证键顺序稳定，使指纹不受 HashMap 迭代顺序影响。
///
/// Compute a fingerprint (sha256 hex string) of a node's current params + wiring, for
/// comparison against the previous execution record to detect changes. Does not compare/
/// store the raw params/wiring directly — those are already authoritative in
/// `pipeline.json`; duplicating them in meta would be redundant. Only a hash digest for
/// change detection is kept. BTreeMap guarantees stable key ordering so the fingerprint
/// doesn't depend on HashMap iteration order.
pub(super) fn node_config_fingerprint(node: &PipelineNode) -> String {
    let params: BTreeMap<&String, &serde_json::Value> = node.params.iter().collect();
    let wiring: BTreeMap<usize, (String, usize)> = node
        .inputs
        .iter()
        .map(|(port, input_ref)| (*port, (input_ref.node_id.clone(), input_ref.port)))
        .collect();
    // enabled 纳入指纹：禁用节点现在会被执行引擎当作"跳过并透传"处理（见
    // exec.rs 的文档说明），这是一种与"正常执行"截然不同的行为，因此启用/禁用
    // 状态的切换必须被视为"自身配置变更"，强制重新评估——否则在启用状态切换后
    // 重新触发后处理时，指纹不变会导致该节点被误判为"未变更"而跳过重新执行，
    // 继续复用切换前的旧结果（如把"已禁用透传"的旧输出当作"现在应该真正执行"
    // 的结果直接复用）。
    //
    // enabled is included in the fingerprint: a disabled node is now treated by the
    // execution engine as "skip and pass through" (see exec.rs's doc comments), a
    // fundamentally different behavior from "run normally" — so toggling enabled must
    // count as a "self config change" and force re-evaluation. Otherwise, re-triggering
    // post-processing after flipping enabled would see an unchanged fingerprint and
    // wrongly treat the node as "unchanged", skipping re-execution and continuing to
    // reuse the stale pre-toggle result (e.g. reusing an old "disabled pass-through"
    // output as if it were the result of "now actually running").
    let combined =
        serde_json::json!({ "params": params, "wiring": wiring, "enabled": node.enabled });
    let serialized = serde_json::to_string(&combined).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// 判断某节点相对于上次执行记录是否"自身发生变更"（模块 ID、参数、连线任意
/// 一项不同，或上次记录的输出文件已不再存在于磁盘上）。
/// 找不到上次记录（新增节点）或上次未成功，也视为自身变更。
///
/// 关于"输出文件已不再存在"这一判断依据：不能仅凭 meta 中记录过某个输出路径
/// 就认为该输出现在依然可用——文件可能在两次后处理之间被 cleanup 模块删除、
/// 被用户手动删除、或被外部磁盘清理工具误删。若该节点因"配置未变"被跳过
/// （见 `build_effective_pipeline`），它在 meta 中记录的旧输出路径会被
/// `build_pre_collected` 原样当作真实数据传给下游节点——下游节点收到一个根本
/// 不存在的路径，读取时才会失败，报错信息与真实原因（"上游产物已丢失"）完全
/// 脱节。将这种情况纳入"自身变更"判断，会让该节点被重新执行（重新生成缺失的
/// 输出），而不是假装它仍然成功过。
///
/// Determine whether a node has "changed on its own" relative to its previous execution
/// record (module ID, params, or wiring differ, OR the previously recorded output
/// file(s) no longer exist on disk). A missing previous record (newly added node) or a
/// non-successful previous run also counts as self-changed.
///
/// On the "output files no longer exist" criterion: a path having been recorded in meta
/// doesn't mean that output is still available now — the file could have been deleted by
/// the cleanup module, by the user manually, or by an external disk-cleanup tool between
/// two post-processing runs. If this node were skipped because its "config is unchanged"
/// (see `build_effective_pipeline`), its stale output path recorded in meta would be
/// handed to downstream nodes as real data by `build_pre_collected` — the downstream node
/// receives a path that simply doesn't exist, failing only once it tries to read it, with
/// an error message completely disconnected from the real cause ("the upstream artifact
/// is gone"). Folding this into "self-changed" makes the node re-execute (regenerating
/// the missing output) instead of pretending it's still successful.
fn node_self_changed(node: &PipelineNode, prev_execution: &[PpExecutionEntry]) -> bool {
    // effective_id 缺省即 module_id：按它先找到同一节点的上次执行记录。
    // effective_id defaults to module_id: look up the node's previous
    // execution record by it first.
    let eid = node.effective_id();
    let Some(prev) = prev_execution.iter().find(|e| e.effective_id() == eid) else {
        return true;
    };
    let was_success = matches!(
        prev.result.as_ref().map(|r| &r.code),
        Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
    );
    if !was_success {
        return true;
    }
    // 模块 ID 一致才继续比较参数，不同模块一律视为"配置变更"。
    // Only compare params when the module ID matches; a different module
    // always counts as a "config change".
    if prev.module_id != node.module_id {
        return true;
    }
    if prev.config_fingerprint != node_config_fingerprint(node) {
        return true;
    }
    if !all_recorded_outputs_exist(&prev.outputs) {
        return true;
    }
    false
}

/// 检查 meta 中按端口分组记录的输出路径（`Vec<Vec<String>>`，见 [`PpExecutionEntry::outputs`]
/// 的文档说明）是否每一个都仍然真实存在于磁盘上。空列表（节点无输出）视为通过。
///
/// Check whether every output path recorded in meta, grouped by port
/// (`Vec<Vec<String>>`, see [`PpExecutionEntry::outputs`]'s doc comment), still actually
/// exists on disk. An empty list (node produced no output) is considered passing.
fn all_recorded_outputs_exist(outputs: &[Vec<String>]) -> bool {
    outputs
        .iter()
        .flatten()
        .all(|p| std::path::Path::new(p).exists())
}

/// 将执行引擎按端口顺序返回的扁平路径列表（`inputs`/`outputs`）转换为 meta 存储格式：
/// 按端口分组的二维数组，每个端口内部按 `BUNDLE_SEP`（`\n`）拆分为多个路径字符串。
///
/// 流水线执行时的实际连线格式完全不受影响——`MediaBundle` 端口传递的仍然是
/// "视频路径\n图片路径" 的单一 PathBuf；这里只是让 meta JSON 中的表达更清晰，
/// 调用方可以直接按数组下标取路径，无需各自重复实现按 `\n` 拆分的逻辑。
///
/// Convert the flat, port-ordered path list returned by the execution engine
/// (`inputs`/`outputs`) into the meta storage format: a 2D array grouped by port, with
/// each port's single string split on `BUNDLE_SEP` (`\n`) into multiple path strings.
///
/// The actual pipeline wiring format is completely unaffected — a `MediaBundle` port
/// still carries a single "video_path\nimage_path" PathBuf during execution; this only
/// makes the meta JSON representation clearer so callers can index into the array
/// directly instead of each re-implementing `\n`-splitting themselves.
pub(super) fn group_paths_by_bundle(paths: &[std::path::PathBuf]) -> Vec<Vec<String>> {
    paths
        .iter()
        .map(|p| {
            p.to_string_lossy()
                .split(crate::postprocess::builtin_nodes::BUNDLE_SEP)
                .map(|s| s.to_string())
                .collect()
        })
        .collect()
}

/// `group_paths_by_bundle` 的逆操作：将 meta 中按端口分组的二维数组重新合并为
/// 扁平的单路径-per-port 列表（同端口内的多个字符串重新用 `BUNDLE_SEP` 拼接），
/// 用于将历史 meta 记录重新接入执行引擎（如 `build_pre_collected`/`merge_with_prev_results`）。
///
/// Inverse of `group_paths_by_bundle`: re-joins the meta's per-port grouped arrays back
/// into a flat single-path-per-port list (strings within the same port are re-joined with
/// `BUNDLE_SEP`), for feeding historical meta records back into the execution engine
/// (e.g. `build_pre_collected`/`merge_with_prev_results`).
fn ungroup_paths_from_bundle(grouped: &[Vec<String>]) -> Vec<std::path::PathBuf> {
    let sep = crate::postprocess::builtin_nodes::BUNDLE_SEP.to_string();
    grouped
        .iter()
        .map(|port_paths| std::path::PathBuf::from(port_paths.join(&sep)))
        .collect()
}

/// 计算本次需要（重新）执行的"脏"节点集合：自身配置变更的节点，加上所有
/// 直接或间接依赖于脏节点输出的下游节点（沿 DAG 边传播，因为它们的实际输入
/// 也会随之改变，即使这些下游节点自身参数未变）。
///
/// Compute the set of "dirty" nodes that must (re-)run this time: nodes whose own
/// config changed, plus all nodes directly or indirectly downstream of a dirty node
/// (propagated along DAG edges, since their actual input also changes even if their
/// own params didn't).
pub(super) fn compute_dirty_nodes(
    pipeline: &PipelineConfig,
    prev_execution: &[PpExecutionEntry],
) -> HashSet<String> {
    let edges = pipeline.resolved_edges();
    let mut dirty: HashSet<String> = pipeline
        .nodes
        .iter()
        .filter(|n| n.enabled && node_self_changed(n, prev_execution))
        .map(|n| n.effective_id().to_string())
        .collect();

    loop {
        let mut added = false;
        for edge in &edges {
            if dirty.contains(&edge.from_node_id) && !dirty.contains(&edge.to_node_id) {
                dirty.insert(edge.to_node_id.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    dirty
}

/// 构建本次实际要执行的流水线副本：只保留禁用节点（透传）与脏节点，
/// 其余已成功且未变更的启用节点被剔除（其输出经 `build_pre_collected` 预填）。
///
/// Build the pipeline to actually execute this run: keeps disabled nodes
/// (pass-through) and dirty nodes; enabled nodes that already succeeded with
/// unchanged config are dropped (their outputs are pre-filled via
/// `build_pre_collected`).
pub(super) fn build_effective_pipeline(
    pipeline: &PipelineConfig,
    dirty_nodes: &HashSet<String>,
) -> PipelineConfig {
    let mut p = pipeline.clone();
    p.nodes.retain(|n| {
        if !n.enabled {
            return true;
        }
        let skip = !dirty_nodes.contains(n.effective_id());
        if skip {
            tracing::info!(
                "{}",
                crate::tl!(
                    "postprocess.serviceSkipNode",
                    id = n.effective_id(),
                    module = n.module_id
                )
            );
        }
        !skip
    });
    p
}

/// 构建预填输入槽：将上次已成功且未变更节点的输出沿边传给下游节点，
/// 使本次新增/受影响的节点仍能拿到正确输入。
///
/// Build pre-filled input slots: propagate outputs of previously succeeded,
/// unchanged nodes along edges to downstream nodes, so newly added or
/// affected nodes still receive the correct inputs.
pub(super) fn build_pre_collected(
    pipeline: &PipelineConfig,
    prev_execution: &[PpExecutionEntry],
    dirty_nodes: &HashSet<String>,
) -> HashMap<String, Vec<(usize, std::path::PathBuf)>> {
    let mut pre: HashMap<String, Vec<(usize, std::path::PathBuf)>> = HashMap::new();
    let edges = pipeline.resolved_edges();

    for entry in prev_execution {
        if dirty_nodes.contains(entry.effective_id()) {
            continue;
        }
        let is_success = matches!(
            entry.result.as_ref().map(|r| &r.code),
            Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
        );
        if !is_success {
            continue;
        }
        if entry.outputs.is_empty() {
            continue;
        }
        // 二次防御：只信任"仍然存在于磁盘上"的历史输出路径，不能仅凭 meta 中
        // 记录过就当作现在依然有效——`node_self_changed` 已经会把输出缺失的
        // 节点标记为脏节点并从这里排除（见其文档说明），此处是防止未来改动
        // 绕过那层判断而重新引入同一个 bug 的保险。
        //
        // Defense in depth: only trust historical output paths that "still exist on
        // disk" — meta having recorded a path once doesn't mean it's still valid now.
        // `node_self_changed` already marks nodes with missing outputs as dirty and
        // excludes them from reaching here (see its doc comment); this is a safety net
        // against a future change accidentally bypassing that check and reintroducing
        // the same bug.
        if !all_recorded_outputs_exist(&entry.outputs) {
            tracing::warn!(
                "{}",
                crate::tl!(
                    "postprocess.serviceStalePreFill",
                    id = entry.effective_id(),
                    module = entry.module_id
                )
            );
            continue;
        }
        let outputs = ungroup_paths_from_bundle(&entry.outputs);
        for edge in edges
            .iter()
            .filter(|e| e.from_node_id == entry.effective_id())
        {
            if let Some(output_path) = outputs.get(edge.from_port) {
                pre.entry(edge.to_node_id.clone())
                    .or_default()
                    .push((edge.to_port, output_path.clone()));
            }
        }
    }
    pre
}

/// 将本次执行结果与上次已成功且未变更的历史结果按拓扑序合并，
/// 得到写回 meta 的最终结果集。
///
/// Merge this run's results with previously succeeded, unchanged results in
/// topological order, producing the final result set to write back to meta.
pub(super) fn merge_with_prev_results(
    new_results: Vec<NodeResult>,
    prev_execution: &[PpExecutionEntry],
    pipeline: &PipelineConfig,
    dirty_nodes: &HashSet<String>,
) -> Vec<NodeResult> {
    let mut merged: Vec<NodeResult> = Vec::new();
    // 按连线拓扑顺序遍历（由 model 层的 topological_order 保证，顺序由连线决定
    // 而非节点存储顺序），使 pp_execution 记录顺序与实际执行顺序一致。
    // 过滤掉禁用节点——它们不应出现在 meta.pp_execution 中。
    //
    // Iterate in wiring-defined topological order (guaranteed by model's topological_order,
    // which is determined by wiring, not node storage order) so pp_execution entries are
    // recorded in actual execution order. Filter out disabled nodes — they must not appear
    // in meta.pp_execution.
    let enabled_ids: HashSet<&str> = pipeline
        .nodes
        .iter()
        .filter(|n| n.enabled)
        .map(|n| n.effective_id())
        .collect();
    let topo: Vec<String> = pipeline
        .topological_order()
        .into_iter()
        .filter(|id| enabled_ids.contains(id.as_str()))
        .collect();
    for eid in &topo {
        if let Some(r) = new_results.iter().find(|r| &r.effective_id == eid) {
            merged.push(r.clone());
            continue;
        }
        if dirty_nodes.contains(eid.as_str()) {
            continue;
        }
        if let Some(prev) = prev_execution.iter().find(|e| {
            e.effective_id() == eid.as_str()
                && matches!(
                    e.result.as_ref().map(|r| &r.code),
                    Some(PpExecCode::Ok | PpExecCode::Done | PpExecCode::Skipped)
                )
        }) {
            // 二次防御：与 build_pre_collected 的同名检查一样——`node_self_changed`
            // 已经会把输出缺失的节点标记为脏并从这里排除（`dirty_nodes.contains`
            // 判断在上面已经 `continue` 掉了），这里只是防止未来改动绕过那层
            // 判断后，最终写回 meta 的 pp_execution 里出现一条"结果码是 ok，但
            // 输出文件其实已经不存在"的记录。
            //
            // Defense in depth, mirroring the same check in build_pre_collected —
            // `node_self_changed` already marks nodes with missing outputs as dirty and
            // excludes them from reaching here (the `dirty_nodes.contains` check above
            // already `continue`s past them); this only guards against a future change
            // bypassing that check and letting a "result code is ok, but the output file
            // no longer actually exists" record end up in the final pp_execution
            // written back to meta.
            if !all_recorded_outputs_exist(&prev.outputs) {
                tracing::warn!(
                    "{}",
                    crate::tl!(
                        "postprocess.serviceStaleMerge",
                        id = eid,
                        module = prev.module_id
                    )
                );
                continue;
            }
            merged.push(NodeResult {
                effective_id: prev.effective_id().to_string(),
                module_id: prev.module_id.clone(),
                code: prev
                    .result
                    .as_ref()
                    .map(|r| r.code.clone())
                    .unwrap_or(PpExecCode::Ok),
                message: prev
                    .result
                    .as_ref()
                    .and_then(|r| r.message.clone())
                    .unwrap_or_default(),
                outputs: ungroup_paths_from_bundle(&prev.outputs),
                inputs: ungroup_paths_from_bundle(&prev.inputs),
            });
        }
    }
    merged
}
