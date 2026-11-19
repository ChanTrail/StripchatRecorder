//! 系统负载采样与并发度自适应决策 / System load sampling & adaptive concurrency decisions
//!
//! 采样全局 CPU 使用率与内存占用，为后处理队列提供自适应调节依据。
//! 决策采用闭环 AIMD（Additive Increase / Multiplicative Decrease，加性增、乘性减）
//! 控制：以"全局 CPU 与可用内存"为目标量做反馈调节，无需估算每任务资源占用
//! （全局负载无法按任务归因，开环容量模型必然失真）。
//!
//! - 后处理（弹性负载）：AIMD——压力大时乘性快速退避、宽裕且槽位占满时
//!   加性慢恢复（步长随逻辑核数缩放：≤8 核 +1，核数越多步长越大）。
//!   pp 是系统压力的主要承担者（解码型任务 ~12 路即在 32 核机器上饱和）。
//! - 录制：不设限——实测非密集（每路仅 ~2% 占空比的 `-c copy` 转码突发），
//!   移除录制并发控制后系统压力统一由 pp 侧吸收。
//!
//! Post-processing (elastic): AIMD — multiplicative backoff under pressure,
//! additive reclaim when comfortable and saturated (step scales with logical
//! cores: +1 up to 8 cores, larger steps on bigger machines). pp is the main
//! pressure absorber (decode-type tasks saturate a 32-core box at ~12).
//! Recording: uncapped — measured non-intensive (~2% duty-cycle `-c copy`
//! transcode bursts); with recording control removed, pp absorbs all
// !   concurrency pressure.
//!
//! Samples global CPU and memory usage to feed adaptive concurrency control for
//! the post-processing queue. Decisions use a closed-loop AIMD
//! (additive increase / multiplicative decrease) controller driven by observable
//! targets (global CPU headroom, available memory) rather than per-task cost
//! estimates — global load cannot be attributed per task, so an open-loop
//! capacity model is inherently wrong.
//!
//! - Post-processing (elastic workload): AIMD — multiplicative backoff under
//!   pressure, additive reclaim when comfortable and slots are saturated
//!   (step scales with logical cores).
//! - Recording: uncapped (measured non-intensive); pp absorbs system pressure.

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// 一次系统负载快照 / A single system load snapshot
#[derive(Debug, Clone)]
pub struct LoadSnapshot {
    /// 全局 CPU 使用率瞬时值（0.0 ~ 100.0）/ Instantaneous global CPU usage (0.0–100.0)
    pub cpu_usage_pct: f32,
    /// EMA 平滑后的全局 CPU 使用率（决策输入，抑制瞬时尖峰）/
    /// EMA-smoothed global CPU usage (the decision input; suppresses transient spikes)
    pub smoothed_cpu_pct: f32,
    /// 内存使用率（0.0 ~ 100.0）/ Memory usage (0.0–100.0)
    pub mem_usage_pct: f32,
    /// 可用内存字节数 / Available memory in bytes
    pub mem_available_bytes: u64,
    /// 总内存字节数 / Total memory in bytes
    pub mem_total_bytes: u64,
}

/// 系统压力等级，供 AIMD/冻结决策使用（含迟滞带防震荡）/
/// System pressure level for AIMD/freeze decisions (with a hysteresis band to prevent oscillation)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureLevel {
    /// 宽裕：允许加性增长 / Comfortable: additive growth allowed
    Comfortable,
    /// 正常：迟滞带内，保持现状 / Normal: inside the hysteresis band, hold
    Normal,
    /// 高压：乘性退避 / 冻结 / High: multiplicative backoff / freeze
    High,
    /// 危急：硬下限 / Critical: hard floor
    Critical,
}

/// 判定阈值常量 / Assessment thresholds
/// CPU 危急线：达到即视为系统饱和 / CPU critical line: system saturated
const CPU_CRITICAL: f32 = 95.0;
/// 后处理 CPU 高压线：达到即乘性退避（刻意收紧——pp 是弹性负载，优先给
/// 录制与其他程序留余量）/ PP CPU high line: multiplicative backoff
/// (deliberately tight — pp is elastic and yields headroom to recordings and other apps)
const CPU_HIGH_PP: f32 = 75.0;
/// 后处理 CPU 宽裕线：低于才允许加性增长（55%~75% 迟滞带防抖）/
/// PP CPU comfortable line: additive growth only below this (55–75% hysteresis band prevents oscillation)
const CPU_LOW_PP: f32 = 55.0;
/// 内存危急比：可用/总内存低于此值强制最小并发 / Critical memory ratio: force minimum concurrency
const MEM_CRITICAL_RATIO: f64 = 0.05;
/// 内存高压比：可用内存低于此比例视为紧张 / High memory ratio: available below this counts as tight
const MEM_HIGH_RATIO: f64 = 0.15;
/// 内存宽裕比：可用内存高于此比例才允许增长 / Comfortable memory ratio: growth only when available exceeds this
const MEM_COMFORTABLE_RATIO: f64 = 0.25;

/// 按给定 CPU 阈值评估快照的压力等级。
/// 所有判定使用 EMA 平滑后的 CPU，避免瞬时尖峰触发误调。
///
/// Assess a snapshot's pressure level against the given CPU thresholds.
/// All checks use the EMA-smoothed CPU so transient spikes don't trigger
/// false adjustments.
fn assess_with(s: &LoadSnapshot, cpu_high: f32, cpu_low: f32) -> PressureLevel {
    let avail_ratio = if s.mem_total_bytes > 0 {
        s.mem_available_bytes as f64 / s.mem_total_bytes as f64
    } else {
        // 无内存信息：视为宽裕（不做内存约束）/ No memory info: treat as ample (no memory constraint)
        1.0
    };
    if avail_ratio < MEM_CRITICAL_RATIO || s.smoothed_cpu_pct >= CPU_CRITICAL {
        PressureLevel::Critical
    } else if avail_ratio < MEM_HIGH_RATIO || s.smoothed_cpu_pct >= cpu_high {
        PressureLevel::High
    } else if avail_ratio >= MEM_COMFORTABLE_RATIO && s.smoothed_cpu_pct <= cpu_low {
        PressureLevel::Comfortable
    } else {
        PressureLevel::Normal
    }
}

/// 后处理负载的压力等级（阈值 75% 即退避、55% 以下才增长）。
/// pp 刻意更早让路、更晚恢复：为录制（不设限）与其他程序保留余量。
///
/// Pressure level for the post-processing workload (back off at 75%, grow
/// only below 55%). pp deliberately yields early and recovers late, keeping
/// headroom for recordings (uncapped) and other programs.
pub fn assess_pp(s: &LoadSnapshot) -> PressureLevel {
    assess_with(s, CPU_HIGH_PP, CPU_LOW_PP)
}

/// 后处理并发度的下一步值（AIMD）。
///
/// - `Critical`：直接降到 1（最优先保命）。
/// - `High`：乘性退避 `current - max(1, current/3)`，约每轮退 1/3，快速让位。
/// - `Comfortable` 且 `running >= current`（槽位占满证明有排队需求）：
///   `+step` 加性恢复。步长 `step = max(1, max/8)` 随天花板（= 逻辑核数）缩放：
///   ≤8 核保持保守 +1，16 核 +2、32 核 +4、128 核 +16——大机器不至于
///   每轮 +1 爬 ~10 分钟，小机器行为不变。
/// - 其余（`Normal` 迟滞带、或 `Comfortable` 但槽位未占满即无新增需求）：保持。
///
/// Next post-processing permit count (AIMD).
///
/// - `Critical`: drop straight to 1 (survival first).
/// - `High`: multiplicative backoff `current - max(1, current/3)` — roughly a
///   third per round, yielding quickly.
/// - `Comfortable` with `running >= current` (saturated slots prove queued
///   demand): `+step` additive reclaim. The step `max(1, max/8)` scales with
///   the ceiling (= logical cores): ≤8 cores keep the conservative +1, while
///   16 → +2, 32 → +4, 128 → +16 so large machines don't spend ~10 minutes
///   climbing at +1 per round.
/// - Otherwise (`Normal` hysteresis band, or `Comfortable` with idle slots,
///   i.e. no extra demand): hold.
pub fn next_pp_permits(level: PressureLevel, current: usize, running: usize, max: usize) -> usize {
    // 加性步长随天花板缩放；乘性退避本身已随 current 缩放无需步长
    // Additive step scales with the ceiling; multiplicative backoff already
    // scales via `current` and needs no step
    let step = (max / 8).max(1);
    let next = match level {
        PressureLevel::Critical => 1,
        PressureLevel::High => current.saturating_sub((current / 3).max(1)).max(1),
        PressureLevel::Comfortable if running >= current => current.saturating_add(step),
        _ => current,
    };
    next.clamp(1, max.max(1))
}

/// EMA 平滑系数：新样本权重 / EMA smoothing factor: weight of the new sample
const EMA_ALPHA: f32 = 0.3;

/// 系统负载采样器，持有 sysinfo::System 实例以复用内部状态。
///
/// 对全局 CPU 使用率做 EMA（指数移动平均）平滑：α=0.3 表示新样本权重 30%、
/// 历史权重 70%，约 3~4 个采样周期（15~20 秒）收敛到稳态，避免瞬时尖峰触发误调。
/// 全局 CPU 任何时候都有意义，无需像 per-task 指标那样在 running==0 时重置。
///
/// System load sampler; holds a sysinfo::System instance to reuse internal state.
///
/// Applies EMA (exponential moving average) smoothing to global CPU usage:
/// α=0.3 weights the new sample 30% and history 70%, converging to steady state
/// in roughly 3–4 cycles (15–20 s) so transient spikes don't cause false
/// adjustments. Global CPU is always meaningful, so unlike per-task metrics it
/// never needs resetting when nothing is running.
pub struct LoadSampler {
    sys: System,
    /// EMA 平滑后的全局 CPU（百分点），None 表示尚未采样
    /// EMA-smoothed global CPU (pct points); None before the first sample
    ema_cpu: Option<f32>,
}

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
        Self { sys, ema_cpu: None }
    }

    /// 刷新系统数据，更新全局 CPU 的 EMA 平滑器，并返回当前负载快照。
    ///
    /// Refresh system data, update the global-CPU EMA smoother, and return the
    /// current load snapshot.
    pub fn sample(&mut self) -> LoadSnapshot {
        self.sys.refresh_specifics(
            RefreshKind::nothing()
                .with_cpu(sysinfo::CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        );

        // 全局 CPU 使用率（所有逻辑核的平均值）/ Global CPU usage (average across all logical cores)
        let cpu_usage_pct = self.sys.global_cpu_usage();

        // 全局 CPU EMA：新样本 = 本次全局值 / Global CPU EMA: new sample = this global value
        let smoothed_cpu_pct = match self.ema_cpu {
            None => cpu_usage_pct, // 首个样本直接作为初始值 / First sample: seed directly
            Some(prev) => EMA_ALPHA * cpu_usage_pct + (1.0 - EMA_ALPHA) * prev,
        };
        self.ema_cpu = Some(smoothed_cpu_pct);

        // 内存使用率 / Memory usage
        let total = self.sys.total_memory();
        let available = self.sys.available_memory();
        let used = total.saturating_sub(available);
        let mem_usage_pct = if total > 0 {
            (used as f64 / total as f64 * 100.0) as f32
        } else {
            0.0
        };

        LoadSnapshot {
            cpu_usage_pct,
            smoothed_cpu_pct,
            mem_usage_pct,
            mem_available_bytes: available,
            mem_total_bytes: total,
        }
    }
}
