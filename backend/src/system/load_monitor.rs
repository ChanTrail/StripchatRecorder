//! 系统负载采样 / System Load Sampling
//!
//! 采样 CPU 使用率和内存使用率，供后处理并发度动态调整使用。
//! CPU 使用率需要两次采样取差值（sysinfo 内部已处理），因此采样间隔应 ≥ 200ms。
//!
//! Samples CPU usage and memory usage for dynamic post-processing concurrency adjustment.
//! CPU usage requires two samples to compute a delta (handled internally by sysinfo),
//! so the sampling interval should be ≥ 200ms.

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// 一次系统负载快照 / A single system load snapshot
#[derive(Debug, Clone)]
pub struct LoadSnapshot {
    /// CPU 整体使用率（0.0 ~ 100.0）/ Overall CPU usage (0.0–100.0)
    pub cpu_usage_pct: f32,
    /// 内存使用率（0.0 ~ 100.0）/ Memory usage (0.0–100.0)
    pub mem_usage_pct: f32,
    /// 可用内存字节数 / Available memory in bytes
    pub mem_available_bytes: u64,
    /// 总内存字节数 / Total memory in bytes
    pub mem_total_bytes: u64,
    /// EMA 平滑后的每任务 CPU 占用（百分点）；running == 0 时为 None。
    /// EMA-smoothed per-task CPU usage (percentage points); None when running == 0.
    pub smoothed_per_task_cpu: Option<f32>,
    /// EMA 平滑后的每任务内存占用（字节）；running == 0 或内存信息不可用时为 None。
    /// EMA-smoothed per-task memory usage (bytes); None when running == 0 or memory unavailable.
    pub smoothed_per_task_mem: Option<f64>,
}

/// 系统负载采样器，持有 sysinfo::System 实例以复用内部状态。
///
/// 内置 EMA（指数移动平均）平滑器，消除 running=1 等少任务场景下
/// per_task_cpu / per_task_mem 的瞬时噪声。α=0.3 表示新样本权重 30%，
/// 历史权重 70%，约 3~4 个采样周期（15~20 秒）后收敛到稳态。
/// running 变为 0 时自动重置 EMA，下次有任务时重新建立基准。
///
/// System load sampler; holds a sysinfo::System instance to reuse internal state.
///
/// Contains a built-in EMA (Exponential Moving Average) smoother to eliminate
/// per-sample noise in per_task_cpu / per_task_mem when few tasks are running
/// (e.g. running=1). α=0.3 gives new samples 30% weight and history 70%,
/// converging to steady state in roughly 3–4 cycles (15–20 s). The EMA is
/// reset automatically when running drops to 0, so it rebuilds a fresh baseline
/// next time tasks start.
pub struct LoadSampler {
    sys: System,
    /// EMA 平滑后的每任务 CPU（百分点），None 表示尚未建立基准
    /// EMA-smoothed per-task CPU (pct points); None means no baseline yet
    ema_cpu: Option<f32>,
    /// EMA 平滑后的每任务内存（字节），None 表示尚未建立基准
    /// EMA-smoothed per-task memory (bytes); None means no baseline yet
    ema_mem: Option<f64>,
}

/// EMA 平滑系数：新样本权重 / EMA smoothing factor: weight of the new sample
const EMA_ALPHA: f32 = 0.3;

impl Default for LoadSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadSampler {
    /// 创建采样器并完成第一次 CPU 采样（为差值计算热身）。
    /// Create a sampler and perform the first CPU sample (warm-up for delta computation).
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        // 第一次刷新仅用于建立基准，CPU 使用率在下一次刷新时才有意义
        // First refresh only establishes a baseline; CPU usage becomes meaningful after the next refresh
        sys.refresh_specifics(
            RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        Self { sys, ema_cpu: None, ema_mem: None }
    }

    /// 刷新系统数据，更新 EMA 平滑器，并返回当前负载快照。
    ///
    /// `running`：调用方传入当前实际在跑的任务数，用于计算 per_task 指标。
    /// running == 0 时不更新 EMA（无数据），并将已有 EMA 状态清零，
    /// 下次有任务时重新建立基准，避免用陈旧数据误判。
    ///
    /// Refresh system data, update EMA smoothers, and return the current load snapshot.
    ///
    /// `running`: number of currently running tasks, used to compute per-task metrics.
    /// When running == 0, the EMA is not updated (no data) and existing state is cleared
    /// so a fresh baseline is built next time tasks are active.
    pub fn sample(&mut self, running: usize) -> LoadSnapshot {
        self.sys.refresh_specifics(
            RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );

        // 全局 CPU 使用率（所有逻辑核的平均值）
        // Global CPU usage (average across all logical cores)
        let cpu_usage_pct = self.sys.global_cpu_usage();

        // 内存使用率 / Memory usage
        let total = self.sys.total_memory();
        let available = self.sys.available_memory();
        let used = total.saturating_sub(available);
        let mem_usage_pct = if total > 0 {
            (used as f64 / total as f64 * 100.0) as f32
        } else {
            0.0
        };

        // ── EMA 平滑 / EMA smoothing ──────────────────────────────────────────
        let (smoothed_per_task_cpu, smoothed_per_task_mem) = if running == 0 {
            // 无任务：重置 EMA，下次从零建立基准
            // No tasks: reset EMA so we build a fresh baseline next time
            self.ema_cpu = None;
            self.ema_mem = None;
            (None, None)
        } else {
            // CPU EMA：新样本 = cpu_pct / running / EMA 公式: new = α*x + (1-α)*old
            // CPU EMA: new sample = cpu_pct / running; formula: new = α*x + (1-α)*old
            let raw_cpu = cpu_usage_pct / running as f32;
            let ema_cpu = match self.ema_cpu {
                None => raw_cpu,  // 首个样本直接作为初始值 / First sample: seed directly
                Some(prev) => EMA_ALPHA * raw_cpu + (1.0 - EMA_ALPHA) * prev,
            };
            self.ema_cpu = Some(ema_cpu);

            // 内存 EMA：total == 0 时跳过 / Memory EMA: skip if total is 0
            let ema_mem = if total > 0 {
                let raw_mem = used as f64 / running as f64;
                let v = match self.ema_mem {
                    None => raw_mem,
                    Some(prev) => EMA_ALPHA as f64 * raw_mem + (1.0 - EMA_ALPHA as f64) * prev,
                };
                self.ema_mem = Some(v);
                Some(v)
            } else {
                None
            };

            (Some(ema_cpu), ema_mem)
        };

        LoadSnapshot {
            cpu_usage_pct,
            mem_usage_pct,
            mem_available_bytes: available,
            mem_total_bytes: total,
            smoothed_per_task_cpu,
            smoothed_per_task_mem,
        }
    }
}

/// 根据当前负载快照和正在运行的任务数，实时计算建议的并发度上限。
///
/// 使用 `LoadSnapshot` 中 EMA 平滑后的 `smoothed_per_task_cpu` / `smoothed_per_task_mem`
/// 反推系统容量，消除单次采样的瞬时噪声（尤其是 running=1 时的抖动）。
/// 没有任务在跑时（两个字段均为 None），直接返回 `max_permits`。
///
/// **CPU 约束**：
///   `capacity = floor(100 / smoothed_per_task_cpu)`
///   若 per_task_cpu < 0.1（任务几乎不耗 CPU，如纯网络通知），不以 CPU 为瓶颈。
///
/// **内存约束**：
///   `capacity = floor(total_bytes / smoothed_per_task_mem)`
///   若总内存信息不可用则不限制。
///   若可用内存 < 总内存的 5%（极度紧张），无论如何强制限为 1。
///
/// 两个约束取较小值，再 clamp 到 `[1, max_permits]`。
///
/// Compute the recommended concurrency ceiling using EMA-smoothed per-task metrics
/// from the load snapshot, eliminating instantaneous noise (especially with running=1).
/// Returns `max_permits` when no tasks are running (both smoothed fields are None).
///
/// - CPU:    capacity = floor(100 / smoothed_per_task_cpu)
/// - Memory: capacity = floor(total_bytes / smoothed_per_task_mem)
/// - Critically low memory (avail < 5% of total): hard cap at 1.
/// - Both constraints clamped to [1, max_permits]; minimum of the two is used.
pub fn recommend_permits(
    snapshot: &LoadSnapshot,
    max_permits: usize,
    running: usize,
) -> usize {
    // ── 内存极度紧张：兜底硬限，无论是否有任务 / Critically low memory: hard floor regardless ──
    if snapshot.mem_total_bytes > 0
        && snapshot.mem_available_bytes < snapshot.mem_total_bytes / 20
    {
        return 1;
    }

    // running == 0 / EMA 尚未建立：无实测数据，不做推算，返回上限
    // No running tasks or EMA not yet seeded: no data to infer from, return max
    if running == 0 || snapshot.smoothed_per_task_cpu.is_none() {
        return max_permits;
    }

    // ── CPU 约束 / CPU constraint ────────────────────────────────────────────
    // 用 EMA 平滑后的每任务 CPU 反推系统容量，消除瞬时噪声
    // Use EMA-smoothed per-task CPU to back-calculate capacity, removing spikes
    let cpu_limit = match snapshot.smoothed_per_task_cpu {
        Some(per_task) if per_task > 0.1 => (100.0 / per_task).floor() as usize,
        _ => max_permits, // 每任务 CPU 占用极小（如纯 I/O），不以 CPU 为瓶颈
    };

    // ── 内存约束 / Memory constraint ─────────────────────────────────────────
    // 用 EMA 平滑后的每任务内存反推系统容量
    // Use EMA-smoothed per-task memory to back-calculate capacity
    let mem_limit = match snapshot.smoothed_per_task_mem {
        Some(per_task) if snapshot.mem_total_bytes > 0 => {
            (snapshot.mem_total_bytes as f64 / per_task.max(1.0)).floor() as usize
        }
        _ => max_permits, // 无内存信息，不限制 / No memory info, no limit
    };

    // 取两种约束的较小值，clamp 到 [1, max_permits]
    // Use the more restrictive constraint, clamped to [1, max_permits]
    cpu_limit.min(mem_limit).clamp(1, max_permits)
}
