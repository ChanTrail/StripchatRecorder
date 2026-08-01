//! DAG 执行引擎 / DAG Execution Engine
//!
//! 按拓扑顺序执行流水线 DAG（支持分叉/汇合），并负责单个节点的子进程调用
//! （stdin JSON 写入、stdout/stderr 流式解析、取消处理）。不涉及 DAG 数据结构
//! 定义（见 `super::model`）或模块发现（见 `super::discovery`）。
//!
//! Executes the pipeline DAG in topological order (supporting forks/joins), and
//! handles subprocess invocation for individual nodes (stdin JSON, streaming
//! stdout/stderr parsing, cancellation). Does not define DAG data structures (see
//! `super::model`) or perform module discovery (see `super::discovery`).

use super::model::{ModuleInfo, ModuleOutput, NodeResult, PipelineConfig, PipelineNode};
use crate::config::settings::exe_dir;
use crate::recording::meta::PpExecCode;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 节点执行的上下文信息（传给模块的 recording 字段）。
/// Context information for node execution (passed to module as the `recording` field).
#[derive(Debug, Clone, Serialize)]
pub struct RecordingContext {
    /// 视频文件路径（可能在流水线执行过程中变化，如 ts_merge 后才确定）
    /// Video file path (may change during pipeline, e.g. determined after ts_merge)
    pub video_path: String,
    /// 录制开始时间 / Recording start time
    pub started_at: String,
    /// 主播用户名 / Streamer username
    pub username: String,
}

/// 执行整个 DAG 流水线，支持分叉。
///
/// 从所有根节点（无入边的节点）开始，按拓扑顺序执行。
/// 分叉时各分支并发执行（在同一线程内顺序调度，因子进程是阻塞的）。
/// 每个节点执行前调用 `on_node_start`，执行后调用 `on_node_done`。
///
/// Execute the full DAG pipeline with fork support.
/// Starts from all root nodes and executes in topological order.
/// Forks are scheduled sequentially (since subprocesses are blocking).
/// Calls `on_node_start` before each node and `on_node_done` after.
#[allow(clippy::too_many_arguments)]
pub fn run_pipeline(
    // 初始输入路径（根节点的输入，通常是 ts_session_dir 或 video_file）
    // Initial input paths (root node inputs, usually ts_session_dir or video_file)
    initial_inputs: &[PathBuf],
    pipeline: &PipelineConfig,
    modules: &[ModuleInfo],
    recording_ctx: &RecordingContext,
    cancel: Option<Arc<AtomicBool>>,
    max_tmp_dir_gb: f64,
    // 预填的 collected 输入槽（来自上次已成功节点的 outputs）
    // Pre-filled collected input slots (from previously succeeded nodes' outputs)
    pre_collected: HashMap<String, Vec<(usize, PathBuf)>>,
    on_node_start: &dyn Fn(&str, &str, &[PathBuf]),           // (node_id, module_id, inputs)
    on_node_done: &dyn Fn(NodeResult),                        // NodeResult
    on_progress: &dyn Fn(&str, u32, u32, &str),               // (node_id, done, total, status)
    on_log: &dyn Fn(&str, &str, &str),                        // (module_id, stream, line)
) -> Vec<NodeResult> {
    // 构建每个节点已收到的输入槽：node_id → Vec<(port_index, path)>
    // Build collected inputs per node: node_id → Vec<(port_index, path)>
    let mut collected: HashMap<String, Vec<(usize, PathBuf)>> = pre_collected;
    // 已完成节点的结果
    let mut all_results: Vec<NodeResult> = Vec::new();

    // 使用 BFS 队列按拓扑顺序调度
    // Use a BFS queue for topological scheduling
    let mut queue: std::collections::VecDeque<(String, Vec<PathBuf>)> =
        std::collections::VecDeque::new();

    // 跟踪已入队节点，避免同一节点多次执行（分叉汇合时等所有输入就位再执行）
    // Track enqueued nodes to avoid re-execution; for join nodes, wait for all inputs
    let mut enqueued: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 初始化根节点（无任何预填输入的节点从 initial_inputs 启动）
    // Initialize root nodes (nodes with no pre-filled inputs start from initial_inputs)
    for root_id in pipeline.root_nodes() {
        if !enqueued.contains(root_id) {
            enqueued.insert(root_id.to_string());
            queue.push_back((root_id.to_string(), initial_inputs.to_vec()));
        }
    }

    // 检查 pre_collected 中已有足够输入的非根节点，直接入队
    // Check non-root nodes in pre_collected that already have sufficient inputs and enqueue them
    for node in pipeline.nodes.iter().filter(|n| n.enabled) {
        if enqueued.contains(&node.node_id) {
            continue;
        }
        let slot = match collected.get(&node.node_id) {
            Some(s) => s,
            None => continue,
        };
        let target_module = modules.iter().find(|m| m.id == node.module_id);
        let required_inputs = target_module.map(|m| m.input_types.len()).unwrap_or(1);
        let distinct_ports: std::collections::HashSet<usize> =
            slot.iter().map(|(p, _)| *p).collect();
        if distinct_ports.len() >= required_inputs {
            enqueued.insert(node.node_id.clone());
            let mut sorted = slot.clone();
            sorted.sort_by_key(|(p, _)| *p);
            let node_inputs: Vec<PathBuf> = sorted.into_iter().map(|(_, p)| p).collect();
            queue.push_back((node.node_id.clone(), node_inputs));
        }
    }

    while let Some((node_id, inputs)) = queue.pop_front() {
        // 检查取消 / Check cancel
        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            break;
        }

        let node = match pipeline.nodes.iter().find(|n| n.node_id == node_id) {
            Some(n) => n,
            None => continue,
        };
        if !node.enabled {
            continue;
        }

        let module = match modules.iter().find(|m| m.id == node.module_id) {
            Some(m) => m,
            None => {
                // 内置节点在 modules 列表中存在，但 exe_path 为空。
                // 若此处找不到（理论上不应发生），给出清晰错误。
                // Built-in nodes exist in the modules list with an empty exe_path.
                // If somehow not found here, emit a clear error.
                let result = NodeResult {
                    node_id: node_id.clone(),
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

        on_node_start(&node_id, &node.module_id, &inputs);

        let result = if node.module_id.starts_with(crate::postprocess::builtin_nodes::BUILTIN_PREFIX) {
            // 内置节点：由后端直接处理，不调用外部进程
            // Built-in node: handled directly by the backend, no subprocess
            match node.module_id.as_str() {
                crate::postprocess::builtin_nodes::ID_UNPACK => {
                    crate::postprocess::builtin_nodes::run_unpack(&node_id, &inputs)
                }
                other => NodeResult {
                    node_id: node_id.clone(),
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
                &inputs,
                recording_ctx,
                cancel.clone(),
                max_tmp_dir_gb,
                &|done, total| on_progress(&node_id, done, total, ""),
                &|status| on_progress(&node_id, 0, 0, status),
                &|stream, line| on_log(&node.module_id, stream, line),
            )
        };

        let outputs = result.outputs.clone();
        let is_ok = result.is_success();
        let is_terminal = result.is_terminal();

        on_node_done(result.clone());
        all_results.push(result);

        // 若节点终止（done/error/cancelled），不向后继传播
        // If terminal (done/error/cancelled), do not propagate to successors
        if is_terminal || !is_ok {
            continue;
        }

        // 向后继节点分发输出
        // Dispatch outputs to successor nodes
        for (edge, _succ_node) in pipeline.successors(&node_id) {
            // 根据端口索引取对应的输出路径
            // Pick the output path corresponding to the port index
            let output_for_port = outputs.get(edge.from_port).cloned();
            let path = match output_for_port {
                Some(p) => p,
                None => {
                    tracing::warn!(
                        "Node {} output port {} not available, skipping edge to {}",
                        node_id, edge.from_port, edge.to_node_id
                    );
                    continue;
                }
            };

            let slot = collected.entry(edge.to_node_id.clone()).or_default();
            slot.push((edge.to_port, path));

            // 检查目标节点是否已收到所有必要输入
            // Check if the target node has received all required inputs
            let target_node = match pipeline.nodes.iter().find(|n| n.node_id == edge.to_node_id) {
                Some(n) => n,
                None => continue,
            };
            let target_module = modules.iter().find(|m| m.id == target_node.module_id);
            let required_inputs = target_module.map(|m| m.input_types.len()).unwrap_or(1);
            let distinct_ports: std::collections::HashSet<usize> =
                slot.iter().map(|(p, _)| *p).collect();

            if distinct_ports.len() >= required_inputs
                && !enqueued.contains(&edge.to_node_id)
            {
                enqueued.insert(edge.to_node_id.clone());
                // 按端口索引排序构建 inputs 数组
                // Build sorted inputs array by port index
                let mut sorted = slot.clone();
                sorted.sort_by_key(|(p, _)| *p);
                let node_inputs: Vec<PathBuf> = sorted.into_iter().map(|(_, p)| p).collect();
                queue.push_back((edge.to_node_id.clone(), node_inputs));
            }
        }
    }

    all_results
}

// ─── 单节点执行 / Single Node Execution ──────────────────────────────────────

/// 执行单个流水线节点：启动子进程，向 stdin 写入 JSON，读取 stdout/stderr，处理取消。
/// Execute a single pipeline node: spawn subprocess, write JSON to stdin,
/// read stdout/stderr, handle cancellation.
#[allow(clippy::too_many_arguments)]
fn run_node(
    module: &ModuleInfo,
    node: &PipelineNode,
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

    // 构建传给模块的 stdin JSON
    // Build stdin JSON to pass to the module
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
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return NodeResult {
                node_id: node.node_id.clone(),
                module_id: node.module_id.clone(),
                code: PpExecCode::Error,
                message: format!("Failed to spawn: {}", e),
                outputs: vec![],
                inputs: inputs.to_vec(),
            }
        }
    };

    // 写 stdin JSON，忽略错误（模块可能不需要读 stdin）
    // Write stdin JSON; ignore errors (module may not need to read stdin)
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
                    .stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            let _ = child.kill();
            cancelled = true;
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(StreamEvent::StdoutLine(line)) => {
                let trimmed = line.trim();
                // 优先尝试解析为 JSON 返回值（最后一个有效 JSON 行视为模块输出）
                // Try parsing as JSON module output first; last valid JSON line wins
                if trimmed.starts_with('{') {
                    if let Ok(out) = serde_json::from_str::<ModuleOutput>(trimmed) {
                        final_json = Some(out);
                        continue;
                    }
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
            node_id: node.node_id.clone(),
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
                node_id: node.node_id.clone(),
                module_id: node.module_id.clone(),
                code: PpExecCode::Error,
                message: format!("wait failed: {}", e),
                outputs: vec![],
                inputs: inputs.to_vec(),
            }
        }
    };

    // 优先使用 JSON 返回值；若没有则根据退出码推断
    // Prefer JSON return value; if absent, infer from exit code
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
        NodeResult { node_id: node.node_id.clone(), module_id: node.module_id.clone(),
                     code, message, outputs, inputs: inputs.to_vec() }
    } else if status.success() {
        // 旧协议兼容：无 JSON 返回时，已成功但无输出 → done
        // Legacy protocol fallback: succeeded but no JSON output → done
        NodeResult {
            node_id: node.node_id.clone(),
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
            node_id: node.node_id.clone(),
            module_id: node.module_id.clone(),
            code: PpExecCode::Error,
            message: msg,
            outputs: vec![],
            inputs: inputs.to_vec(),
        }
    }
}
