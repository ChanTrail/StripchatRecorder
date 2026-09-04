//! DAG 执行引擎 / DAG Execution Engine

use super::model::{ModuleInfo, ModuleOutput, NodeResult, PipelineConfig, PipelineNode};
use crate::config::app_state::exe_dir;
use crate::recording::meta::PpExecCode;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::core::no_window::NoWindowExt;

/// 节点执行的上下文信息（传给模块的 recording 字段）。
/// Context information for node execution (passed to module as the `recording` field).
#[derive(Debug, Clone, Serialize)]
pub struct RecordingContext {
    pub video_path: String,
    pub started_at: String,
    pub username: String,
}

/// 执行整个 DAG 流水线，支持分叉。
/// Execute the full DAG pipeline with fork support.
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    initial_inputs: &[PathBuf],
    pipeline: &PipelineConfig,
    modules: &[ModuleInfo],
    recording_ctx: &RecordingContext,
    cancel: Option<Arc<AtomicBool>>,
    max_tmp_dir_gb: f64,
    pre_collected: HashMap<String, Vec<(usize, PathBuf)>>,
    on_node_start: &dyn Fn(&str, &str, &[PathBuf]),  // (effective_id, module_id, inputs)
    on_node_done: &dyn Fn(NodeResult),
    on_progress: &dyn Fn(&str, u32, u32, &str),      // (effective_id, done, total, status)
    on_log: &dyn Fn(&str, &str, &str),               // (module_id, stream, line)
) -> Vec<NodeResult> {
    // 队列键统一使用 effective_id / Queue key is always effective_id
    let mut collected: HashMap<String, Vec<(usize, PathBuf)>> = pre_collected;
    let mut all_results: Vec<NodeResult> = Vec::new();
    let mut queue: std::collections::VecDeque<(String, Vec<PathBuf>)> =
        std::collections::VecDeque::new();
    let mut enqueued: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 初始化根节点 / Initialize root nodes
    for root_id in pipeline.root_nodes() {
        if !enqueued.contains(root_id) {
            enqueued.insert(root_id.to_string());
            queue.push_back((root_id.to_string(), initial_inputs.to_vec()));
        }
    }

    // 检查 pre_collected 中已有足够输入的非根节点。
    // 不按 enabled 过滤（原因同 root_nodes/successors 的文档说明）——禁用节点
    // 同样需要进入队列以便被当作"跳过（透传）"节点处理，继续驱动 DAG。
    //
    // Enqueue non-root nodes with sufficient pre-filled inputs.
    // Not filtered by enabled (same reasoning as root_nodes/successors's doc comments) —
    // a disabled node still needs to enter the queue so it can be handled as a
    // "skip (pass-through)" node, keeping the DAG moving.
    for node in pipeline.nodes.iter() {
        let eid = node.effective_id();
        if enqueued.contains(eid) {
            continue;
        }
        let slot = match collected.get(eid) {
            Some(s) => s,
            None => continue,
        };
        let target_module = modules.iter().find(|m| m.id == node.module_id);
        let required_inputs = target_module.map(|m| m.input_types.len()).unwrap_or(1);
        let distinct_ports: std::collections::HashSet<usize> =
            slot.iter().map(|(p, _)| *p).collect();
        if distinct_ports.len() >= required_inputs {
            enqueued.insert(eid.to_string());
            let mut sorted = slot.clone();
            sorted.sort_by_key(|(p, _)| *p);
            let node_inputs: Vec<PathBuf> = sorted.into_iter().map(|(_, p)| p).collect();
            queue.push_back((eid.to_string(), node_inputs));
        }
    }

    while let Some((eid, inputs)) = queue.pop_front() {
        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }

        let node = match pipeline.nodes.iter().find(|n| n.effective_id() == eid) {
            Some(n) => n,
            None => continue,
        };

        // 节点被禁用：本节点不执行任何实际处理，但必须原样把输入透传给下游，
        // 而不是 `continue` 直接从队列中消失——`continue` 会导致该节点的所有
        // 下游（包括分支中该节点之后的全部节点）永远收不到输入，表现为"某条
        // 分支完全不执行"或"流水线在此处莫名中断"。这里把 `inputs` 直接作为
        // `outputs`（视为直通），交由下面与正常节点完全相同的"分发给后继节点"
        // 逻辑处理——跳过节点因此和其他任何成功节点一样能继续驱动 DAG 前进。
        //
        // 不调用 on_node_start/on_node_done，也不写入 all_results：禁用节点
        // 本来就不该出现在 meta.pp_execution 或前端的执行情况列表中——这与
        // 流水线跑完后 meta 的最终形态一致（`postprocess_cmd::merge_with_prev_results`
        // 只遍历 `pipeline.nodes.iter().filter(|n| n.enabled)`，被禁用的节点从来
        // 不会出现在最终写回的 pp_execution 里）。此前的实现调用了这两个回调，
        // 导致运行期间通过 SSE 推送的实时快照（meta 尚未被最终清理前的中间状态）
        // 会短暂包含被禁用节点的记录，页面上出现"跳过的模块也显示在执行情况
        // 列表里"，直到手动刷新页面读取到已清理的最终 meta 才恢复正常。
        //
        // 注：直通按位置一一对应（第 i 个输入 = 第 i 个输出）。对绝大多数
        // 输入端口数=输出端口数的常规模块（1→1 的线性节点）这是唯一合理的
        // 映射；对输入/输出端口数不一致的节点（如内置 unpack：1 输入、2 输出）
        // 禁用后无法产生它本该拆分出的第二个端口，属于该类节点被禁用时固有的
        // 语义空白，不在本次修复范围内。
        //
        // Node is disabled: it performs no actual processing, but its input must still
        // be forwarded downstream unchanged rather than `continue`-ing straight out of
        // the queue — `continue` would starve every downstream node (including the rest
        // of that branch) of input forever, manifesting as "an entire branch never runs"
        // or "the pipeline mysteriously stops here". `inputs` is used directly as
        // `outputs` (treated as pass-through), and handed to the exact same "dispatch to
        // successors" logic used for normal nodes below — so a skipped node keeps
        // driving the DAG forward just like any other successful one.
        //
        // on_node_start/on_node_done are NOT called, and nothing is pushed to
        // all_results: a disabled node shouldn't appear in meta.pp_execution or the
        // frontend's execution list at all — matching the final shape of meta once the
        // pipeline finishes (`postprocess_cmd::merge_with_prev_results` only iterates
        // `pipeline.nodes.iter().filter(|n| n.enabled)`, so a disabled node never ends up
        // in the pp_execution written back at the end). The previous implementation
        // called both callbacks, so the live SSE snapshot pushed during the run (meta's
        // intermediate state before final cleanup) would briefly include the disabled
        // node's record — showing "skipped modules also appear in the execution list" on
        // screen, only fixed after a manual page refresh (which reads the
        // already-cleaned-up final meta).
        //
        // Note: pass-through is positional (i-th input = i-th output). This is the only
        // sensible mapping for the vast majority of regular modules where input port
        // count equals output port count (linear 1-in-1-out nodes); for nodes whose
        // input/output port counts differ (e.g. the built-in unpack: 1 input, 2 outputs),
        // disabling it can't produce the second port it would normally split out — an
        // inherent semantic gap for that class of node when disabled, out of scope here.
        if !node.enabled {
            tracing::debug!("Node {} disabled, passing through {} input(s) unchanged", eid, inputs.len());
            dispatch_to_successors(pipeline, modules, &eid, &inputs, &mut collected, &mut enqueued, &mut queue);
            continue;
        }

        let module = match modules.iter().find(|m| m.id == node.module_id) {
            Some(m) => m,
            None => {
                let result = NodeResult {
                    effective_id: eid.clone(),
                    module_id: node.module_id.clone(),
                    code: PpExecCode::Error,
                    message: format!("模块 '{}' 不存在，请检查 modules/ 目录", node.module_id),
                    outputs: vec![],
                    inputs: inputs.clone(),
                };
                on_node_done(result.clone());
                all_results.push(result);
                continue;
            }
        };

        on_node_start(&eid, &node.module_id, &inputs);

        let result = if node.module_id.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX) {
            // 内置节点：由后端直接处理 / Built-in node: handled directly by backend
            match node.module_id.as_str() {
                crate::postprocess::builtin_nodes::ID_UNPACK => {
                    crate::postprocess::builtin_nodes::run_unpack(&eid, &inputs)
                }
                other => NodeResult {
                    effective_id: eid.clone(),
                    module_id: other.to_string(),
                    code: PpExecCode::Error,
                    message: format!("unknown built-in node: {}", other),
                    outputs: vec![],
                    inputs: inputs.clone(),
                },
            }
        } else {
            run_node(
                module,
                node,
                &eid,
                &inputs,
                recording_ctx,
                cancel.clone(),
                max_tmp_dir_gb,
                &|done, total| on_progress(&eid, done, total, ""),
                &|status| on_progress(&eid, 0, 0, status),
                &|stream, line| on_log(&node.module_id, stream, line),
            )
        };

        let outputs = result.outputs.clone();
        let is_ok = result.is_success();
        let is_terminal = result.is_terminal();

        on_node_done(result.clone());
        all_results.push(result);

        if is_terminal || !is_ok {
            continue;
        }

        dispatch_to_successors(pipeline, modules, &eid, &outputs, &mut collected, &mut enqueued, &mut queue);
    }

    all_results
}

/// 将某节点（无论是正常成功执行、还是被禁用而透传）的输出，沿 DAG 边分发给
/// 所有下游节点：把输出路径写入对应下游节点在 `collected` 中的输入槽位，
/// 一旦某下游节点的所有必需输入端口都已凑齐（`distinct_ports.len() >= required_inputs`），
/// 且尚未入队，就将其加入执行队列。
///
/// 不对下游节点的 `enabled` 做任何判断——分发的职责只是"数据是否送达"，
/// 送达之后如何处理（正常执行 or 视为跳过并继续透传）由主循环在 dequeue
/// 该节点时统一决定，此函数不重复该逻辑。
///
/// 由主循环中两处调用：①正常节点执行成功后 ②节点被禁用而透传时。两者对
/// 后继节点的处理方式完全一致，因此提取为共享函数，避免逻辑分叉导致的
/// 不一致（历史上"仅第一条分支执行"的 bug 之一正是因为禁用节点的分支
/// 未走到这段分发逻辑）。
///
/// Dispatch a node's output (whether from normal successful execution, or a disabled
/// node's pass-through) along DAG edges to all downstream nodes: writes the output path
/// into each downstream node's input slot in `collected`, and once a downstream node's
/// required input ports are all filled (`distinct_ports.len() >= required_inputs`) and
/// it isn't already queued, enqueues it for execution.
///
/// Does not check the downstream node's `enabled` at all — dispatch's only
/// responsibility is "did the data arrive"; what happens once it arrives (execute
/// normally, or be treated as skipped-and-pass-through) is decided uniformly by the
/// main loop when that node is dequeued, and this function doesn't duplicate that logic.
///
/// Called from two places in the main loop: ① after a normal node executes
/// successfully, ② when a disabled node passes through. Both need identical handling of
/// successors, so this is extracted into a shared function to avoid the kind of logic
/// fork that caused inconsistency — one of the historical "only the first branch runs"
/// bugs was exactly this: a disabled node's branch never reached this dispatch logic.
#[allow(clippy::too_many_arguments)]
fn dispatch_to_successors(
    pipeline: &PipelineConfig,
    modules: &[ModuleInfo],
    from_eid: &str,
    outputs: &[PathBuf],
    collected: &mut HashMap<String, Vec<(usize, PathBuf)>>,
    enqueued: &mut std::collections::HashSet<String>,
    queue: &mut std::collections::VecDeque<(String, Vec<PathBuf>)>,
) {
    for (edge, _succ_node) in pipeline.successors(from_eid) {
        let output_for_port = outputs.get(edge.from_port).cloned();
        let path = match output_for_port {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Node {} output port {} not available, skipping edge to {}",
                    from_eid, edge.from_port, edge.to_node_id
                );
                continue;
            }
        };

        let slot = collected.entry(edge.to_node_id.clone()).or_default();
        slot.push((edge.to_port, path));

        let target_node = match pipeline.nodes.iter().find(|n| n.effective_id() == edge.to_node_id) {
            Some(n) => n,
            None => continue,
        };
        let target_module = modules.iter().find(|m| m.id == target_node.module_id);
        let required_inputs = target_module.map(|m| m.input_types.len()).unwrap_or(1);
        let distinct_ports: std::collections::HashSet<usize> =
            slot.iter().map(|(p, _)| *p).collect();

        if distinct_ports.len() >= required_inputs && !enqueued.contains(&edge.to_node_id) {
            enqueued.insert(edge.to_node_id.clone());
            let mut sorted = slot.clone();
            sorted.sort_by_key(|(p, _)| *p);
            let node_inputs: Vec<PathBuf> = sorted.into_iter().map(|(_, p)| p).collect();
            queue.push_back((edge.to_node_id.clone(), node_inputs));
        }
    }
}

// ─── 单节点执行 / Single Node Execution ──────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn run_node(
    module: &ModuleInfo,
    node: &PipelineNode,
    effective_id: &str,
    inputs: &[PathBuf],
    recording_ctx: &RecordingContext,
    cancel: Option<Arc<AtomicBool>>,
    max_tmp_dir_gb: f64,
    on_module_progress: &dyn Fn(u32, u32),
    on_status: &dyn Fn(&str),
    on_log: &dyn Fn(&str, &str),
) -> NodeResult {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    enum StreamEvent {
        StdoutLine(String),
        StderrLine(String),
        StdoutEof,
        StderrEof,
    }

    let max_tmp_mb = (max_tmp_dir_gb * 1024.0) as u64;
    let input_json = serde_json::json!({
        "inputs": inputs.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
        "params": node.params,
        "exe_dir": exe_dir().to_string_lossy(),
        "max_tmp_mb": max_tmp_mb,
        "recording": recording_ctx,
    });

    let mut cmd = std::process::Command::new(&module.exe_path);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .no_window();

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return NodeResult {
                effective_id: effective_id.to_string(),
                module_id: node.module_id.clone(),
                code: PpExecCode::Error,
                message: format!("Failed to spawn: {}", e),
                outputs: vec![],
                inputs: inputs.to_vec(),
            }
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(serde_json::to_string(&input_json).unwrap_or_default().as_bytes());
    }

    let mut last_message = String::new();
    let mut stderr_msg = String::new();
    let mut panic_msg = String::new();
    let mut final_json: Option<ModuleOutput> = None;
    let mut cancelled = false;

    let (tx, rx) = mpsc::channel::<StreamEvent>();
    let mut stdout_done = true;
    let mut stderr_done = true;

    if let Some(stdout) = child.stdout.take() {
        stdout_done = false;
        let tx2 = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx2.send(StreamEvent::StdoutLine(line)).is_err() { return; }
            }
            let _ = tx2.send(StreamEvent::StdoutEof);
        });
    }
    if let Some(stderr) = child.stderr.take() {
        stderr_done = false;
        let tx2 = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx2.send(StreamEvent::StderrLine(line)).is_err() { return; }
            }
            let _ = tx2.send(StreamEvent::StderrEof);
        });
    }
    drop(tx);

    while !(stdout_done && stderr_done) {
        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            #[cfg(target_os = "windows")]
            {
                let pid = child.id();
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/T", "/PID", &pid.to_string()])
                    .stdout(Stdio::null()).stderr(Stdio::null()).no_window().status();
            }
            let _ = child.kill();
            cancelled = true;
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::StdoutLine(line)) => {
                let trimmed = line.trim();
                if trimmed.starts_with('{')
                    && let Ok(out) = serde_json::from_str::<ModuleOutput>(trimmed)
                {
                    final_json = Some(out);
                    continue;
                }
                if let Some(rest) = trimmed.strip_prefix("PROGRESS:") {
                    let mut parts = rest.splitn(2, '/');
                    if let (Some(d), Some(t)) = (parts.next(), parts.next())
                        && let (Ok(done), Ok(total)) = (d.trim().parse::<u32>(), t.trim().parse::<u32>())
                    {
                        on_module_progress(done, total);
                    }
                } else if let Some(st) = trimmed.strip_prefix("STATUS:") {
                    on_log("status", st.trim());
                    on_status(st.trim());
                } else if !trimmed.is_empty() {
                    tracing::info!("[{}] {}", node.module_id, trimmed);
                    on_log("stdout", trimmed);
                    last_message = trimmed.to_string();
                }
            }
            Ok(StreamEvent::StderrLine(line)) => {
                let t = line.trim();
                if t.is_empty() || t.starts_with("note: run with `RUST_BACKTRACE") { continue; }
                if t.contains("panicked at") && panic_msg.is_empty() { panic_msg = t.to_string(); }
                tracing::warn!("[{}] stderr: {}", node.module_id, t);
                on_log("stderr", t);
                stderr_msg = t.to_string();
            }
            Ok(StreamEvent::StdoutEof) => { stdout_done = true; }
            Ok(StreamEvent::StderrEof) => { stderr_done = true; }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => { stdout_done = true; stderr_done = true; }
        }
    }

    if cancelled {
        let _ = child.wait();
        return NodeResult {
            effective_id: effective_id.to_string(),
            module_id: node.module_id.clone(),
            code: PpExecCode::Cancelled,
            message: "cancelled".to_string(),
            outputs: vec![],
            inputs: inputs.to_vec(),
        };
    }

    if !panic_msg.is_empty() { stderr_msg = panic_msg; }

    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            return NodeResult {
                effective_id: effective_id.to_string(),
                module_id: node.module_id.clone(),
                code: PpExecCode::Error,
                message: format!("wait failed: {}", e),
                outputs: vec![],
                inputs: inputs.to_vec(),
            }
        }
    };

    if let Some(out) = final_json {
        let outputs: Vec<PathBuf> = out.outputs
            .unwrap_or_default()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let code = match out.code {
            PpExecCode::Ok if outputs.is_empty() => PpExecCode::Done,
            other => other,
        };
        let message = out.message.unwrap_or_else(|| {
            if last_message.is_empty() { "OK".to_string() } else { last_message.clone() }
        });
        NodeResult { effective_id: effective_id.to_string(), module_id: node.module_id.clone(),
                     code, message, outputs, inputs: inputs.to_vec() }
    } else if status.success() {
        NodeResult {
            effective_id: effective_id.to_string(),
            module_id: node.module_id.clone(),
            code: PpExecCode::Done,
            message: if last_message.is_empty() { "OK".to_string() } else { last_message },
            outputs: vec![],
            inputs: inputs.to_vec(),
        }
    } else {
        let msg = if !stderr_msg.is_empty() { stderr_msg }
                  else if !last_message.is_empty() { last_message }
                  else { format!("exit {}", status) };
        NodeResult {
            effective_id: effective_id.to_string(),
            module_id: node.module_id.clone(),
            code: PpExecCode::Error,
            message: msg,
            outputs: vec![],
            inputs: inputs.to_vec(),
        }
    }
}
