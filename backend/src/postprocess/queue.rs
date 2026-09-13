//! 后处理任务队列 / Post-processing Task Queue
//!
//! 集中管理后处理任务的运行时状态、取消标志和并发执行控制，
//! 从 `AppState` 中分离出来以保持关注点清晰。
//!
//! This module centralizes post-processing task runtime state, cancel flags,
//! and concurrency execution control, separated from `AppState` to keep concerns clean.
//!
//! ## 设计 / Design
//!
//! - 并发度由用户设置中的 `max_pp_concurrent` 决定，通过 [`PpQueue::set_concurrency`] 动态更新。
//! - 0 = 自动（CPU 逻辑核心数 × 2）；≥1 = 固定并发数。
//! - 并发控制使用纯同步原语（`Mutex<usize>` + `Condvar`）实现计数信号量，
//!   不依赖 tokio async，避免在 `spawn_blocking` 栈上调用 `block_on` 导致栈溢出。
//! - `cancel_flags` 允许调用方（如取消按钮）异步请求中止某个正在运行或排队的任务。
//! - [`PpQueue::get_all_tasks`] 合并内存中的运行时状态和 `meta/` 目录中的历史完成记录，
//!   供前端一次性获取完整的任务列表。
//!
//! Concurrency is determined by the `max_pp_concurrent` user setting,
//! updated dynamically via [`PpQueue::set_concurrency`].
//! 0 = auto (logical CPU count × 2); ≥1 = fixed count.
//! Concurrency control uses a pure sync counting semaphore (`Mutex<usize>` + `Condvar`)
//! to avoid calling `block_on` on a `spawn_blocking` stack (which causes stack overflow).

use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 后处理任务状态快照（序列化后发送给前端）。
/// Post-processing task status snapshot (serialized and sent to the frontend).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PpTaskStatus {
    /// 视频文件路径 / Video file path
    pub path: String,
    /// 整体进度百分比（0.0 - 100.0）/ Overall progress percentage (0.0 - 100.0)
    pub pct: f64,
    /// 当前模块已完成进度值 / Current module done progress value
    pub mod_done: u32,
    /// 当前模块名称 / Current module name
    pub module_name: String,
    /// 已完成的节点数 / Number of completed nodes
    pub done: usize,
    /// 总节点数 / Total number of nodes
    pub total: usize,
    /// 任务状态字符串（"waiting" / "running" / "done" / "error"）/ Task status string
    pub status: String,
    /// 是否来自内存（true = 运行中任务，false = 持久化结果）/ Whether from memory (true = in-progress, false = persisted result)
    pub from_memory: bool,
}

/// 纯同步计数信号量，基于 `parking_lot::Mutex` + `Condvar` 实现。
///
/// 不依赖 tokio async，可在 `spawn_blocking` 线程（栈较小）上安全调用，
/// 避免 `block_on` 嵌套导致的栈溢出。
///
/// Pure sync counting semaphore backed by `parking_lot::Mutex` + `Condvar`.
///
/// Does not depend on tokio async; safe to call from `spawn_blocking` threads
/// (which have smaller stacks), avoiding stack overflow from nested `block_on`.
struct SyncSemaphore {
    /// 当前可用许可数 / Available permit count
    count: Mutex<usize>,
    /// 许可释放通知 / Permit-release notification
    condvar: Condvar,
}

impl SyncSemaphore {
    fn new(permits: usize) -> Self {
        Self {
            count: Mutex::new(permits),
            condvar: Condvar::new(),
        }
    }

    /// 阻塞等待并获取一个许可（RAII guard 在 drop 时自动归还并递减运行计数）。
    /// Block until a permit is available, then acquire one.
    /// The returned guard returns the permit and decrements the running count on drop.
    fn acquire(self: &Arc<Self>, running: &Arc<std::sync::atomic::AtomicUsize>) -> SyncPermit {
        let mut count = self.count.lock();
        while *count == 0 {
            self.condvar.wait(&mut count);
        }
        *count -= 1;
        running.fetch_add(1, Ordering::Relaxed);
        SyncPermit { sem: Arc::clone(self), running: Arc::clone(running) }
    }

    /// 归还一个许可并唤醒一个等待者。
    /// Return a permit and wake one waiter.
    fn release(&self) {
        let mut count = self.count.lock();
        *count += 1;
        self.condvar.notify_one();
    }

    /// 替换可用许可数（动态调整并发度时调用）。
    /// Replace the available permit count (called when updating concurrency).
    fn set_permits(&self, permits: usize) {
        let mut count = self.count.lock();
        *count = permits;
        // 唤醒所有等待者重新竞争，避免新许可永远无人消费
        // Wake all waiters to re-compete; avoids new permits going permanently unconsumed
        self.condvar.notify_all();
    }

    /// 返回当前可用许可数快照（瞬时值，仅供监控/调试使用）。
    /// Returns a snapshot of the current available permit count (instantaneous, for monitoring/debug only).
    fn current_permits(&self) -> usize {
        *self.count.lock()
    }
}

/// `SyncSemaphore` 的 RAII 许可守卫，drop 时自动归还许可并递减运行中计数。
/// RAII permit guard for `SyncSemaphore`; returns the permit and decrements the running count on drop.
pub struct SyncPermit {
    sem: Arc<SyncSemaphore>,
    running: Arc<std::sync::atomic::AtomicUsize>,
}

impl Drop for SyncPermit {
    fn drop(&mut self) {
        self.sem.release();
        self.running.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 后处理任务队列：任务状态表 + 取消标志 + 并发执行信号量。
/// Post-processing task queue: task status map + cancel flags + concurrency semaphore.
pub struct PpQueue {
    /// 任务状态表（文件路径 -> 状态）/ Task status map (file path -> status)
    tasks: RwLock<HashMap<String, PpTaskStatus>>,
    /// 取消标志表（文件路径 -> 原子布尔）/ Cancel flag map (file path -> atomic bool)
    cancel_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
    /// 同步计数信号量，控制最大并发后处理任务数。
    /// Sync counting semaphore controlling max concurrent post-processing tasks.
    semaphore: Arc<SyncSemaphore>,
    /// 用户配置换算后的理论上限（CPU×公式），动态调整不超过此值。
    /// Theoretical upper bound derived from user config (CPU × formula); dynamic adjustments never exceed this.
    max_permits: std::sync::atomic::AtomicUsize,
    /// 负载自适应后的当前许可上限（介于 1 和 max_permits 之间）。
    /// Current permit ceiling after load-adaptive adjustment (between 1 and max_permits).
    adjusted_permits: Arc<std::sync::atomic::AtomicUsize>,
    /// 当前持有许可正在运行的任务数（acquire +1，permit drop -1）。
    /// Number of tasks currently holding a permit and running (acquire +1, permit drop -1).
    running_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl Default for PpQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 将用户配置的并发数（0=自动）解析为实际许可数。
///
/// 后处理任务以磁盘 I/O（ts_merge）为主，每个任务都会驱动一个 ffmpeg 进程，
/// 过高并发会导致磁盘争抢和 CPU 过载反而降速。自动模式取 `cpu` 作为默认值；
/// 用户手动设置时上限为 `cpu × 2`，防止过度并发。
///
/// Resolve the configured concurrency (0 = auto) to the actual permit count.
///
/// Post-processing tasks are primarily disk-I/O-bound (ts_merge drives one ffmpeg
/// process per task); excessive concurrency causes disk contention and CPU saturation
/// that hurts rather than helps throughput. Auto mode uses `cpu` as the default;
/// the hard cap for manually-set values is `cpu × 2`.
pub fn resolve_concurrency(n: usize) -> usize {
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // 上限为 cpu * 2 / Hard cap at cpu * 2
    let cap = (cpu * 2).max(1);
    if n == 0 {
        // 自动：取 cpu（每核一个任务）/ Auto: one task per core
        cpu.min(cap)
    } else {
        n.min(cap)
    }
}

impl PpQueue {
    /// 创建空队列，初始并发度为 1（串行）。
    /// 调用方应在流水线加载后立即调用 [`set_concurrency`] 更新为实际值。
    ///
    /// Create an empty queue with initial concurrency of 1 (serial).
    /// Caller should call [`set_concurrency`] immediately after pipeline is loaded.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            cancel_flags: RwLock::new(HashMap::new()),
            semaphore: Arc::new(SyncSemaphore::new(1)),
            max_permits: std::sync::atomic::AtomicUsize::new(1),
            adjusted_permits: Arc::new(std::sync::atomic::AtomicUsize::new(1)),
            running_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// 动态更新并发度（来自用户配置变更）。
    ///
    /// 同时更新理论上限（`max_permits`），后续的 `adjust_for_load` 调用不会超过此值。
    /// 调用后正在等待的任务会立即以新的许可数重新竞争。
    /// 已持有许可正在运行的任务不受影响，继续运行直至完成。
    ///
    /// `n = 0` 表示自动（由 `resolve_concurrency` 映射为 CPU × 2）。
    ///
    /// Dynamically update concurrency (from user config change).
    ///
    /// Also updates the theoretical upper bound (`max_permits`); subsequent
    /// `adjust_for_load` calls will never exceed this value.
    ///
    /// `n = 0` means auto (mapped to CPU × 2 by `resolve_concurrency`).
    pub fn set_concurrency(&self, n: usize) {
        let permits = resolve_concurrency(n);
        self.max_permits.store(permits, Ordering::Relaxed);
        self.adjusted_permits.store(permits, Ordering::Relaxed);
        self.semaphore.set_permits(permits);
        tracing::debug!("{}", crate::tl!("postprocess.concurrencySet", n = permits));
    }

    /// 根据实时系统负载动态调整当前信号量许可数。
    ///
    /// 调用 `recommend_permits` 根据实测每任务资源占用实时推算建议并发数，
    /// 确保结果在 `[1, max_permits]` 范围内，再更新信号量。
    /// 此方法由后台负载监控定时器调用，不影响 `max_permits`（理论上限）。
    ///
    /// Dynamically adjust the current semaphore permit count based on real-time system load.
    ///
    /// Uses `recommend_permits` to back-calculate a suggested value from measured
    /// per-task resource usage, clamps it to `[1, max_permits]`, then updates the
    /// semaphore. Called by the background load monitor timer; does not modify
    /// `max_permits` (the theoretical ceiling).
    pub fn adjust_for_load(&self, snapshot: &crate::system::load_monitor::LoadSnapshot) {
        let max = self.max_permits.load(Ordering::Relaxed);
        let running = self.running_count.load(Ordering::Relaxed);
        let recommended = crate::system::load_monitor::recommend_permits(
            snapshot,
            max,
            running,
        );
        let new_permits = recommended.clamp(1, max);
        self.adjusted_permits.store(new_permits, Ordering::Relaxed);
        self.semaphore.set_permits(new_permits);
        tracing::debug!(
            "{}",
            crate::tl!(
                "postprocess.ppQueueAdjusted",
                permits = new_permits,
                max = max,
                running = running,
                cpu = format!("{:.1}", snapshot.cpu_usage_pct),
                mem = format!("{:.1}", snapshot.mem_usage_pct),
                avail = snapshot.mem_available_bytes / 1024 / 1024
            )
        );
    }

    /// 获取当前并发执行许可（阻塞直至有空闲槽位），并递增运行中计数。
    /// Acquire a concurrency permit (blocks until a slot is free) and increment the running count.
    pub fn acquire_concurrency_permit(&self) -> SyncPermit {
        self.semaphore.acquire(&self.running_count)
    }

    /// 当前实际正在运行（持有许可）的任务数。精确计数：acquire +1，permit drop -1。
    /// Number of tasks currently running (holding a permit). Exact: acquire +1, drop -1.
    pub fn running_count(&self) -> usize {
        self.running_count.load(Ordering::Relaxed)
    }

    /// 返回 running_count 的 Arc 克隆，供需要跨线程实时读取任务数的场合使用。
    /// Returns a cloned Arc of the running_count for cross-thread real-time reads.
    pub fn running_count_arc(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::clone(&self.running_count)
    }

    /// 负载自适应后的当前许可上限（受 CPU/内存压力影响，≤ max_permits）。
    /// Current permit ceiling after load-adaptive adjustment (≤ max_permits).
    pub fn adjusted_permits(&self) -> usize {
        self.adjusted_permits.load(Ordering::Relaxed)
    }

    /// 理论上限（用户配置换算后，不受实时负载影响）。
    /// Theoretical upper bound (from user config, unaffected by real-time load).
    pub fn max_permits(&self) -> usize {
        self.max_permits.load(Ordering::Relaxed)
    }

    /// 当前可用许可数（瞬时值，= adjusted_permits - running_count）。
    /// Current available permits (instantaneous, = adjusted_permits - running_count).
    pub fn current_permits(&self) -> usize {
        self.semaphore.current_permits()
    }

    /// 将任务加入等待队列（状态设为 `"waiting"`），并确保取消标志存在（不覆盖已有值）。
    /// Enqueue a task (status `"waiting"`), ensuring a cancel flag exists (without overwriting).
    pub fn enqueue(&self, path: &str) {
        self.tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                module_name: String::new(),
                done: 0,
                total: 0,
                status: "waiting".to_string(),
                from_memory: true,
            },
        );
        self.cancel_flags
            .write()
            .entry(path.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)));
    }

    /// 将任务标记为运行中（状态设为 `"running"`）。
    /// Mark a task as running (status `"running"`).
    pub fn start(&self, path: &str, total: usize) {
        self.tasks.write().insert(
            path.to_string(),
            PpTaskStatus {
                path: path.to_string(),
                pct: 0.0,
                mod_done: 0,
                module_name: String::new(),
                done: 0,
                total,
                status: "running".to_string(),
                from_memory: true,
            },
        );
    }

    /// 获取或创建指定任务的取消标志。
    /// Get or create the cancel flag for a task.
    pub fn make_cancel_flag(&self, path: &str) -> Arc<AtomicBool> {
        let mut flags = self.cancel_flags.write();
        if let Some(existing) = flags.get(path) {
            return Arc::clone(existing);
        }
        let flag = Arc::new(AtomicBool::new(false));
        flags.insert(path.to_string(), Arc::clone(&flag));
        flag
    }

    /// 请求取消指定任务。
    /// Request cancellation of a task.
    pub fn cancel(&self, path: &str) {
        if let Some(flag) = self.cancel_flags.read().get(path) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// 判断指定任务是否已被请求取消。
    /// Check whether a task has been requested to cancel.
    pub fn is_cancelled(&self, path: &str) -> bool {
        self.cancel_flags
            .read()
            .get(path)
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// 清除指定任务的取消标志（任务结束后调用）。
    /// Clear the cancel flag for a task (called after the task ends).
    pub fn clear_cancel_flag(&self, path: &str) {
        self.cancel_flags.write().remove(path);
    }

    /// 更新指定任务的进度信息。
    /// Update progress information for a task.
    #[allow(clippy::too_many_arguments)]
    pub fn progress(
        &self,
        path: &str,
        pct: f64,
        mod_done: u32,
        module_name: &str,
        done: usize,
        total: usize,
    ) {
        if let Some(t) = self.tasks.write().get_mut(path) {
            t.pct = pct;
            t.mod_done = mod_done;
            t.module_name = module_name.to_string();
            t.done = done;
            t.total = total;
        }
    }

    /// 将任务标记为完成或失败，并从内存队列中移除。
    ///
    /// 完成后不再需要在内存队列中保留记录——[`get_all_tasks`] 会通过扫描
    /// `meta/` 目录得到权威的最终状态（`finish` / `pp_error`）。
    /// 若这里保留记录，[`get_status`] 会返回过期的 `"done"`/`"error"` 字符串，
    /// 覆盖 meta 文件中真正的状态值，导致依赖状态字符串匹配的调用方
    /// （如 `list_recordings`）读到错误的值。
    ///
    /// Mark a task as done or failed, and remove it from the in-memory queue.
    ///
    /// Once finished, the task no longer needs to live in the in-memory queue —
    /// [`get_all_tasks`] derives the authoritative final state (`finish` / `pp_error`)
    /// by scanning the `meta/` directory. Leaving the record here would cause
    /// [`get_status`] to return a stale `"done"`/`"error"` string that shadows the
    /// real status value in the meta file, breaking callers that match on the
    /// status string (e.g. `list_recordings`).
    pub fn finish(&self, path: &str, _success: bool) {
        self.tasks.write().remove(path);
    }

    /// 从队列表中移除任务记录（不影响磁盘上的 meta 文件）。
    /// Remove a task record from the queue table (does not affect the meta file on disk).
    pub fn remove(&self, path: &str) {
        self.tasks.write().remove(path);
    }

    /// 获取指定任务当前的状态字符串（若存在于内存队列中）。
    /// Get the current status string of a task (if present in the in-memory queue).
    pub fn get_status(&self, path: &str) -> Option<String> {
        self.tasks.read().get(path).map(|t| t.status.clone())
    }

    /// 判断指定路径当前是否被本进程的内存队列追踪（即真的在排队或运行）。
    ///
    /// 用于区分"meta 中记录的 pp_waiting/pp_running"是真实活跃状态，
    /// 还是进程重启前遗留的陈旧状态（上次异常退出时未能写回 finish/pp_error）。
    /// 后者在重启扫描时应被视为需要重新触发后处理，而非继续等待。
    ///
    /// Check whether a path is currently tracked by this process's in-memory queue
    /// (i.e. actually queued or running).
    ///
    /// Used to distinguish a genuinely active `pp_waiting`/`pp_running` meta status
    /// from a stale one left over from a previous abnormal exit (which never got
    /// written back to `finish`/`pp_error`). The latter should be re-triggered on
    /// restart scans rather than treated as still in progress.
    pub fn is_tracked(&self, path: &str) -> bool {
        self.tasks.read().contains_key(path)
    }

    /// 获取所有后处理任务状态的列表，合并内存中的运行时状态和 `meta/` 目录中的历史记录。
    /// 历史记录直接从 `meta/` 目录扫描获取，无需额外持久化文件。
    ///
    /// Get a list of all post-processing task statuses, merging in-memory runtime state
    /// with historical records scanned from the `meta/` directory.
    pub fn get_all_tasks(&self) -> Vec<PpTaskStatus> {
        let mut tasks: HashMap<String, PpTaskStatus> = self.tasks.read().clone();

        // 扫描 meta/ 目录（含所有主播子目录），补充历史后处理记录（status 为 finish 或 pp_error）
        // Scan meta/ directory (including all per-streamer subdirectories) to supplement
        // historical post-processing records
        for path in crate::recording::meta::list_all_meta_paths() {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta: crate::recording::meta::VideoMeta = match serde_json::from_str(&content) {
                Ok(m) => m,
                Err(_) => continue,
            };

            // 只处理已完成的后处理记录 / Only include completed post-processing records
            if !matches!(meta.status.as_str(), "finish" | "pp_error") {
                continue;
            }

            // video_path 是前端用的 key / video_path is the key used by the frontend
            let key = match meta.video_path.as_deref() {
                Some(p) => p.to_string(),
                None => continue,
            };

            if tasks.contains_key(&key) {
                continue;
            }

            let success = meta.status == "finish";
            tasks.insert(
                key.clone(),
                PpTaskStatus {
                    path: key,
                    pct: if success { 100.0 } else { 0.0 },
                    mod_done: 0,
                    module_name: String::new(),
                    done: 0,
                    total: 0,
                    status: if success { "done" } else { "error" }.to_string(),
                    from_memory: false,
                },
            );
        }

        tasks.into_values().collect()
    }
}
