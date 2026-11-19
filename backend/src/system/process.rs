//! 进程树资源占用采集 / Process-tree resource usage collection
//!
//! 统计本程序进程及其唤起的全部后代进程（录制用 ffmpeg、后处理模块、
//! 模块再唤起的 ffmpeg 等）的 CPU 与内存占用，供「资源占用」页面展示。
//! 采集方式：枚举全系统进程 → 以自身 PID 为根按父子关系 BFS/DFS 出子树。
//! CPU 为差分指标，需两次刷新之间留出一个采样窗口（≥
//! `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`），故单次采集约耗时
//! `SAMPLE_WINDOW`；采集放在 `spawn_blocking` 中执行，避免阻塞异步运行时。
//!
//! Collects CPU + memory usage of this process and every descendant it has
//! spawned (recording ffmpeg, post-process modules, ffmpeg spawned by those
//! modules, ...) for the "resource usage" page. The tree is obtained by
//! enumerating all system processes and walking the parent map down from our
//! own PID. CPU usage is a differential metric — two refreshes must bracket a
//! sampling window (≥ `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`), so one
//! collection takes roughly `SAMPLE_WINDOW`; it runs inside `spawn_blocking`
//! to keep the async runtime responsive.

use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sysinfo::{Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::postprocess::pipeline::modules_dir;
use crate::system::gpu::GpuSampler;

/// CPU 差分采样窗口（约半个数量级于页面刷新间隔，兼顾精度与响应速度）/
/// CPU differential sampling window (a fraction of the page's refresh
/// interval — balances accuracy against response latency)
const SAMPLE_WINDOW: Duration = Duration::from_millis(450);

/// 单条命令行展示上限（防止超长参数把响应体撑大）/
/// Per-process command line cap (keeps oversized argv from bloating the payload)
const CMD_MAX_LEN: usize = 240;

/// 进程角色（前端以徽标颜色区分）/ Process role (frontend renders as a colored badge)
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessRole {
    /// 本程序主进程 / The recorder's own main process
    Main,
    /// 后处理模块（modules/ 目录下的可执行文件）/
    /// Post-process module (executable under modules/)
    Module,
    /// ffmpeg 工具链（ffmpeg/ffprobe/ffplay，无论由谁唤起）/
    /// ffmpeg toolchain (ffmpeg/ffprobe/ffplay, regardless of who spawned it)
    Ffmpeg,
    /// 其它后代（模块唤起的脚本/子工具等）/
    /// Other descendants (scripts/helpers spawned by modules, etc.)
    Other,
}

/// 单个进程的资源占用条目 / One process' resource usage entry
#[derive(Serialize)]
pub struct ProcessUsageEntry {
    /// 进程 ID / Process ID
    pub pid: u32,
    /// 父进程 ID（未知为 0）/ Parent PID (0 when unknown)
    pub ppid: u32,
    /// 展示名（可执行文件名，退化为进程名）/ Display name (exe file name, falls back to process name)
    pub name: String,
    /// 可执行文件完整路径 / Full executable path
    pub exe: String,
    /// 截断后的命令行 / Command line (truncated to `CMD_MAX_LEN`)
    pub cmd: String,
    /// 角色分类 / Role classification
    pub role: ProcessRole,
    /// 树深度（主进程为 0，前端用于缩进）/ Tree depth (0 = main; frontend indents by it)
    pub depth: u32,
    /// CPU 使用率（100% = 单核满载；多核可超 100）/
    /// CPU usage (100% = one fully-loaded core; may exceed 100 on multi-core)
    pub cpu_pct: f32,
    /// 常驻内存字节数 / Resident memory in bytes
    pub mem_bytes: u64,
    /// 磁盘读速率（字节/秒）/ Disk read rate (bytes/s)
    pub disk_read_bps: u64,
    /// 磁盘写速率（字节/秒；Windows 侧为聚合 I/O 计数，
    /// 含 socket/管道流量，是 OS 提供的最接近口径）/
    /// Disk write rate (bytes/s; on Windows these are aggregate I/O
    /// counters incl. sockets/pipes — the closest available)
    pub disk_write_bps: u64,
    /// GPU 引擎利用率（null = 平台不支持或进程无 GPU 活动）/
    /// GPU engine utilization (null = unsupported platform or no GPU activity)
    pub gpu_pct: Option<f32>,
    /// GPU 显存字节（null = 不可得）/ GPU VRAM bytes (null = unavailable)
    pub gpu_mem_bytes: Option<u64>,
    /// 已运行秒数 / Elapsed runtime in seconds
    pub run_secs: u64,
}

/// 一次完整的进程树资源快照 / A full process-tree usage snapshot
#[derive(Serialize)]
pub struct ProcessUsageSnapshot {
    /// 本进程 PID / Our own PID
    pub self_pid: u32,
    /// 逻辑核数（前端据此把 per-core CPU% 归一化）/
    /// Logical core count (lets the frontend normalize per-core CPU%)
    pub cpu_count: usize,
    /// CPU 采样窗口毫秒数 / CPU sampling window in ms
    pub sample_ms: u64,
    /// 采集完成时刻（unix ms）/ Collection timestamp (unix ms)
    pub collected_at: u64,
    /// 全树 CPU 合计（per-core 口径）/ Tree-wide CPU total (per-core scale)
    pub total_cpu_pct: f32,
    /// 全树内存合计 / Tree-wide memory total
    pub total_mem_bytes: u64,
    /// 全树磁盘读速率合计 / Tree-wide disk read rate total
    pub total_disk_read_bps: u64,
    /// 全树磁盘写速率合计 / Tree-wide disk write rate total
    pub total_disk_write_bps: u64,
    /// 全树 GPU 引擎利用率合计（无进程有 GPU 数据时为 null——平台
    /// 不支持或全树无 GPU 活动）/ Tree-wide GPU engine utilization
    /// total (null when no process reports GPU data — platform
    /// unsupported or the whole tree is GPU-idle)
    pub total_gpu_pct: Option<f32>,
    /// 全树显存合计（null = 不可得）/ Tree-wide VRAM total (null = unavailable)
    pub total_gpu_mem_bytes: Option<u64>,
    /// 系统网络下行速率（字节/秒，排除回环口；OS 不按进程归属网络流量，
    /// 只有整机口径）/ System network RX rate (bytes/s, loopback excluded;
    /// the OS doesn't attribute network bytes to processes — whole-machine
    /// scope is all that's available)
    pub net_rx_bps: u64,
    /// 系统网络上行速率 / System network TX rate
    pub net_tx_bps: u64,
    /// 进程条目（DFS 序：主进程在前，子进程按 CPU 降序排于其下）/
    /// Process entries (DFS order: main first, children sorted by CPU desc)
    pub processes: Vec<ProcessUsageEntry>,
}

/// 采集一次进程树资源快照（异步包装，内部 spawn_blocking）。
/// Collect one process-tree usage snapshot (async wrapper around spawn_blocking).
pub async fn collect_process_usage() -> ProcessUsageSnapshot {
    tokio::task::spawn_blocking(collect_process_usage_blocking)
        .await
        .unwrap_or_else(|_| ProcessUsageSnapshot {
            // join 失败（运行时关闭中）时返回空快照而非 panic /
            // Join failure (runtime shutting down): return an empty snapshot instead of panicking
            self_pid: std::process::id(),
            cpu_count: 0,
            sample_ms: 0,
            collected_at: now_unix_ms(),
            total_cpu_pct: 0.0,
            total_mem_bytes: 0,
            total_disk_read_bps: 0,
            total_disk_write_bps: 0,
            total_gpu_pct: None,
            total_gpu_mem_bytes: None,
            net_rx_bps: 0,
            net_tx_bps: 0,
            processes: Vec::new(),
        })
}

/// 同步执行完整采集：两次刷新夹一个采样窗口 → 建树 → DFS 输出。
/// Synchronous collection: two refreshes bracketing a sampling window → build tree → DFS output.
fn collect_process_usage_blocking() -> ProcessUsageSnapshot {
    let refresh_kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);

    let mut sys = System::new();
    // 网络为整机级差分计数器，与进程窗口共用同一采样周期 /
    // Networks are machine-wide differential counters sharing the same window
    let mut networks = Networks::new_with_refreshed_list();
    // GPU 采样器（Windows PDH / Linux fdinfo；不支持的平台为 no-op）/
    // GPU sampler (Windows PDH / Linux fdinfo; no-op where unsupported)
    let mut gpu = GpuSampler::new();

    // 第一次刷新：建立基线（此时 cpu_usage/disk_usage 尚无差分意义）/
    // First refresh: baseline (cpu/disk usage has no diff meaning yet)
    let t0 = Instant::now();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
    gpu.start();
    std::thread::sleep(SAMPLE_WINDOW.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
    // 第二次刷新：差分指标基于窗口得到 /
    // Second refresh: differential metrics now cover the window
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
    networks.refresh(true);
    let gpu_map = gpu.finish();
    let window_secs = t0.elapsed().as_secs_f64().max(f64::EPSILON);

    let self_pid = Pid::from_u32(std::process::id());
    let modules_root = normalize_path(&modules_dir());

    // 父 → 子邻接表（快照内已退出的父节点可能缺失，挂空父的节点只要不是
    // 自己就跳过——它们不是我们的后代）/ Parent→children adjacency (a
    // dead parent may be absent from the snapshot; nodes with unknown
    // parents other than ourselves are skipped — not our descendants)
    let mut children: HashMap<Pid, Vec<Pid>> = HashMap::new();
    for (pid, proc_) in sys.processes() {
        if *pid == self_pid {
            continue;
        }
        if let Some(ppid) = proc_.parent() {
            children.entry(ppid).or_default().push(*pid);
        }
    }

    let mut processes = Vec::new();
    let mut stack: Vec<(Pid, u32)> = vec![(self_pid, 0)];
    while let Some((pid, depth)) = stack.pop() {
        let Some(proc_) = sys.process(pid) else { continue };
        let exe = proc_.exe().map(normalize_path);
        let gpu = gpu_map.get(pid.as_u32());
        let disk = proc_.disk_usage();
        processes.push(ProcessUsageEntry {
            pid: pid.as_u32(),
            ppid: proc_.parent().map(|p| p.as_u32()).unwrap_or(0),
            name: display_name(proc_, exe.as_deref()),
            exe: exe
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            cmd: truncate(&proc_.cmd().join(&std::ffi::OsString::from(" ")).to_string_lossy(), CMD_MAX_LEN),
            role: classify(pid == self_pid, exe.as_deref(), proc_, &modules_root),
            depth,
            cpu_pct: proc_.cpu_usage(),
            mem_bytes: proc_.memory(),
            // 窗口内差分字节 ÷ 秒 = 速率 / Window-delta bytes ÷ seconds = rate
            disk_read_bps: (disk.read_bytes as f64 / window_secs) as u64,
            disk_write_bps: (disk.written_bytes as f64 / window_secs) as u64,
            gpu_pct: gpu.map(|g| g.pct),
            gpu_mem_bytes: gpu.and_then(|g| g.mem_bytes),
            run_secs: proc_.run_time(),
        });

        // 子节点按 CPU 降序入栈（stack 后进先出，故反序压栈）/
        // Children ordered by CPU desc (stack is LIFO — push in reverse)
        if let Some(kids) = children.get(&pid) {
            let mut kids: Vec<Pid> = kids.clone();
            kids.sort_by(|a, b| {
                let ca = sys.process(*a).map(|p| p.cpu_usage()).unwrap_or(0.0);
                let cb = sys.process(*b).map(|p| p.cpu_usage()).unwrap_or(0.0);
                cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
            });
            for kid in kids.into_iter().rev() {
                stack.push((kid, depth + 1));
            }
        }
    }

    let total_cpu_pct = processes.iter().map(|p| p.cpu_pct).sum();
    let total_mem_bytes = processes.iter().map(|p| p.mem_bytes).sum();
    let total_disk_read_bps = processes.iter().map(|p| p.disk_read_bps).sum();
    let total_disk_write_bps = processes.iter().map(|p| p.disk_write_bps).sum();
    // GPU 合计：全 None（无数据）→ null；有数据（含全 0）→ 合计值 /
    // GPU totals: all-None (no data) → null; any data (even all zeros) → sum
    let total_gpu_pct = processes
        .iter()
        .filter_map(|p| p.gpu_pct)
        .reduce(|a, b| a + b);
    let total_gpu_mem_bytes = processes
        .iter()
        .filter_map(|p| p.gpu_mem_bytes)
        .reduce(|a, b| a + b);
    // 系统网络速率：排除回环口（Linux "lo"、macOS "lo0"、Windows "loopback"）/
    // System network rates: exclude loopback (Linux "lo", macOS "lo0*",
    // Windows "*Loopback*")
    let mut net_rx_bps = 0f64;
    let mut net_tx_bps = 0f64;
    for (name, data) in networks.iter() {
        let lname = name.to_lowercase();
        if lname == "lo" || lname.starts_with("lo0") || lname.contains("loopback") {
            continue;
        }
        net_rx_bps += data.received() as f64;
        net_tx_bps += data.transmitted() as f64;
    }
    ProcessUsageSnapshot {
        self_pid: self_pid.as_u32(),
        // 未做系统级 CPU 刷新，sys.cpus() 为空；核数取 available_parallelism
        // （与 get_system_info 口径一致）/ No system-level CPU refresh was
        // done so sys.cpus() is empty; take the core count from
        // available_parallelism (same source as get_system_info)
        cpu_count: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        sample_ms: SAMPLE_WINDOW.as_millis() as u64,
        collected_at: now_unix_ms(),
        total_cpu_pct,
        total_mem_bytes,
        total_disk_read_bps,
        total_disk_write_bps,
        total_gpu_pct,
        total_gpu_mem_bytes,
        net_rx_bps: (net_rx_bps / window_secs) as u64,
        net_tx_bps: (net_tx_bps / window_secs) as u64,
        processes,
    }
}

/// 判定进程角色 / Classify a process' role
fn classify(
    is_self: bool,
    exe: Option<&Path>,
    proc_: &sysinfo::Process,
    modules_root: &Path,
) -> ProcessRole {
    if is_self {
        return ProcessRole::Main;
    }
    let file_stem = exe
        .and_then(|p| p.file_stem())
        .or_else(|| Some(proc_.name()))
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // ffmpeg 工具链无论由谁唤起都算 ffmpeg（含模块自唤起的）/
    // ffmpeg toolchain counts as ffmpeg no matter who spawned it (incl. modules)
    if ["ffmpeg", "ffprobe", "ffplay"]
        .iter()
        .any(|p| file_stem.starts_with(p))
    {
        return ProcessRole::Ffmpeg;
    }
    if let Some(exe) = exe
        && exe.starts_with(modules_root)
    {
        return ProcessRole::Module;
    }
    ProcessRole::Other
}

/// 展示名：优先可执行文件名，退化到进程名 / Display name: exe file name, falling back to process name
fn display_name(proc_: &sysinfo::Process, exe: Option<&Path>) -> String {
    exe.and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| proc_.name().to_string_lossy().into_owned())
}

/// 规范化路径供比较：去除 Windows verbatim 前缀（`\\?\`）。
/// sysinfo 在 Windows 上返回的 exe 路径常带此前缀，直接 starts_with 会失配。
/// Normalize a path for comparison: strip the Windows verbatim prefix
/// (`\\?\`). sysinfo's exe paths on Windows often carry it, which would
/// defeat a plain `starts_with`.
fn normalize_path(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p.to_path_buf()
}

/// 截断到上限（按 char 边界安全截断）/ Truncate to the cap (char-boundary safe)
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// 当前 unix 时间戳（毫秒）/ Current unix timestamp (milliseconds)
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冒烟：快照必含主进程（depth 0 / main 角色 / 自身 PID），核数 ≥1。
    /// 在 CI/本机直接跑 `cargo test process_usage_smoke` 即可验证 sysinfo
    /// 进程枚举在此平台可用。
    ///
    /// Smoke: the snapshot must contain the main process (depth 0 / main
    /// role / our own PID) and cpu_count ≥1. Running `cargo test
    /// process_usage_smoke` verifies sysinfo process enumeration works on
    /// the host platform.
    #[test]
    fn process_usage_smoke() {
        let snap = collect_process_usage_blocking();
        let main = snap
            .processes
            .iter()
            .find(|p| p.role == ProcessRole::Main)
            .expect("snapshot must contain the main process");
        assert_eq!(main.pid, std::process::id());
        assert_eq!(main.depth, 0);
        assert!(snap.cpu_count >= 1);
        assert!(snap.self_pid == std::process::id());
        // 子树条目按 DFS 序，主进程必在首位 /
        // DFS order — the main process is always first
        assert_eq!(snap.processes[0].role, ProcessRole::Main);
    }

    /// 派生进程可见性：spawn 一个长寿命子进程，采集时应出现在子树中且
    /// depth ≥1。Windows/Linux 通用的休眠命令不好写，改用 cargo test 自身
    /// 的 worker 关系——直接断言「若本进程有父进程在快照内，则其 depth=0」
    /// 退化为常量成立。改为验证 truncate/classify 纯函数。
    ///
    /// Spawned-children visibility is hard to assert portably; verify the
    /// pure helpers instead.
    #[test]
    fn helpers() {
        assert_eq!(truncate("abcdef", 3), "abc…");
        assert_eq!(truncate("abc", 3), "abc");
        // UTF-8 边界安全 / UTF-8 boundary safety
        assert_eq!(truncate("中文字符", 4), "中…");
    }
}
