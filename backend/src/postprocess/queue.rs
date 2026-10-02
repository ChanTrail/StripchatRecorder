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
//! - 0 = 自动（= CPU 逻辑核心数 × 2）；≥1 = 手动固定并发数，上限同为 CPU × 2。
//!   手动设置的意义在于主动限制到低于自动值。
//! - 并发控制使用纯同步原语（`Mutex<SemState>` + `Condvar`）实现计数信号量，
//!   不依赖 tokio async，避免在 `spawn_blocking` 栈上调用 `block_on` 导致栈溢出。
//!   信号量分别记录上限 `limit` 与占用数 `in_use`，动态调整只修改上限，
//!   可用许可始终为 `limit - in_use`，不会因反复调整而超发。
//! - `cancel_flags` 允许调用方（如取消按钮）异步请求中止某个正在运行或排队的任务。
//! - [`PpQueue::get_all_tasks`] 合并内存中的运行时状态和 `meta/` 目录中的历史完成记录，
//!   供前端一次性获取完整的任务列表。
//!
//! Concurrency is determined by the `max_pp_concurrent` user setting,
//! updated dynamically via [`PpQueue::set_concurrency`].
//! 0 = auto (= logical CPU count × 2); ≥1 = fixed count, capped at the same CPU × 2.
//! Manual values are useful for deliberately going below the automatic default.
//! Concurrency control uses a pure sync counting semaphore (`Mutex<SemState>` + `Condvar`)
//! to avoid calling `block_on` on a `spawn_blocking` stack (which causes stack overflow).
//! The semaphore tracks the ceiling `limit` and the held count `in_use` separately; dynamic
//! adjustments only change the ceiling, so available permits are always `limit - in_use`
//! and repeated adjustments can never over-issue permits.

use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 计算录制身份键：同一录制的 session_dir（ts_fragment/{user}/{stem}）与合并后的视频文件
/// （recordings/{user}/{stem}.mp4）共用同一个 meta 文件，因此以 meta 文件路径作为身份。
/// 通过 [`crate::recording::meta::resolve_meta_path`] 解析：派生 meta 缺失时（如
/// split_by_streamer=false 的扁平目录合并文件）返回 video_path 指向该路径的归属 meta，
/// 与 read/write/delete_meta 使用同一身份。无法推断 meta 路径时回退为路径字符串本身。
///
/// Compute the recording identity key: a recording's session_dir (ts_fragment/{user}/{stem})
/// and its merged video file (recordings/{user}/{stem}.mp4) share the same meta file, so the
/// meta file path is used as the identity. Resolved via
/// [`crate::recording::meta::resolve_meta_path`]: when the derived meta is missing (e.g. a
/// merged file in a flat directory with split_by_streamer=false), the owning meta whose
/// video_path points at this path is returned — the same identity used by
/// read/write/delete_meta. Falls back to the path string itself when the meta path cannot
/// be derived.
pub fn recording_key(path: &Path) -> String {
    crate::recording::meta::resolve_meta_path(path)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// `active` 表中的条目：流水线 claim（附队列路径键）或维护扫描的短暂占位。
/// Entry in the `active` map: a pipeline claim (with its queue path key) or a short-lived
/// maintenance-scan reservation.
enum ActiveEntry {
    /// 已被流水线 claim（队列路径键）/ Claimed by a pipeline (queue path key)
    Claimed(String),
    /// 维护扫描占位 / Maintenance-scan reservation
    Reserved,
}

/// [`PpQueue::try_claim`] 被拒绝的原因。
/// Reason a [`PpQueue::try_claim`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRejected {
    /// 同一录制已有任务在排队或执行 / A task for the same recording is already queued or running
    AlreadyClaimed,
    /// 维护扫描正短暂占位该录制 / A maintenance scan briefly holds a reservation on the recording
    Reserved,
    /// 该录制正在被删除 / The recording is being deleted
    RemovalRequested,
}

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
    /// 许可上限与当前占用数 / Permit ceiling and current held count
    state: Mutex<SemState>,
    /// 许可释放 / 上限变更通知 / Permit-release / ceiling-change notification
    condvar: Condvar,
}

/// 信号量内部状态：上限与占用数分开记录，调整上限不会影响已发出的许可。
/// Semaphore internal state: ceiling and held count are tracked separately, so changing
/// the ceiling never affects permits already handed out.
struct SemState {
    /// 许可上限 / Permit ceiling
    limit: usize,
    /// 已发出且尚未归还的许可数 / Permits handed out and not yet returned
    in_use: usize,
}

impl SyncSemaphore {
    fn new(permits: usize) -> Self {
        Self {
            state: Mutex::new(SemState { limit: permits, in_use: 0 }),
            condvar: Condvar::new(),
        }
    }

    /// 阻塞等待并获取一个许可（RAII guard 在 drop 时自动归还并递减运行计数），等待期间可取消。
    /// 占用数达到上限时等待；上限被调低时，已运行的任务不受影响，新任务等到占用数回落。
    /// 每轮先检查 `cancel`，已取消返回 `None`；等待以 200 ms 为上限，配合 [`Self::wake_all`]
    /// 让取消能及时生效。
    ///
    /// Block until a permit is available, then acquire one; the wait can be cancelled.
    /// The returned guard returns the permit and decrements the running count on drop.
    /// Waits while the held count is at the ceiling; when the ceiling is lowered, running
    /// tasks are unaffected and new tasks wait until the held count drops below it.
    /// `cancel` is checked on every round and `None` is returned once it is set; each wait
    /// is capped at 200 ms and combined with [`Self::wake_all`] so cancellation takes effect
    /// promptly.
    fn acquire_cancellable(
        self: &Arc<Self>,
        running: &Arc<AtomicUsize>,
        cancel: &AtomicBool,
    ) -> Option<SyncPermit> {
        let mut st = self.state.lock();
        loop {
            if cancel.load(Ordering::Relaxed) {
                // 可能消耗了一次 notify_one，转交给下一个等待者
                // We may have consumed a notify_one; pass it on to the next waiter
                self.condvar.notify_one();
                return None;
            }
            if st.in_use < st.limit {
                break;
            }
            self.condvar.wait_for(&mut st, Duration::from_millis(200));
        }
        st.in_use += 1;
        running.fetch_add(1, Ordering::Relaxed);
        Some(SyncPermit { sem: Arc::clone(self), running: Arc::clone(running) })
    }

    /// 唤醒所有等待者重新检查（取消请求后调用）。
    /// Wake all waiters to re-check (called after a cancel request).
    fn wake_all(&self) {
        self.condvar.notify_all();
    }

    /// 归还一个许可并唤醒一个等待者。
    /// Return a permit and wake one waiter.
    fn release(&self) {
        let mut st = self.state.lock();
        st.in_use = st.in_use.saturating_sub(1);
        self.condvar.notify_one();
    }

    /// 设置许可上限（动态调整并发度时调用）。只修改上限、不改占用数，
    /// 因此反复调用不会让可用许可超过 `limit - in_use`。
    ///
    /// Set the permit ceiling (called when updating concurrency). Only the ceiling changes,
    /// not the held count, so repeated calls never push available permits above
    /// `limit - in_use`.
    fn set_permits(&self, permits: usize) {
        let mut st = self.state.lock();
        st.limit = permits;
        // 唤醒所有等待者重新检查，上限调高时新增的许可能被立即消费
        // Wake all waiters to re-check; newly available permits after raising the ceiling get consumed
        self.condvar.notify_all();
    }

    /// 返回当前可用许可数快照（= limit - in_use，瞬时值，仅供监控/调试使用）。
    /// Returns a snapshot of the current available permit count
    /// (= limit - in_use, instantaneous, for monitoring/debug only).
    fn current_permits(&self) -> usize {
        let st = self.state.lock();
        st.limit.saturating_sub(st.in_use)
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
    /// 已被 claim 或扫描占位的录制（录制身份 -> 条目），保证同一录制同一时刻只有一条流水线。
    /// Claimed or scan-reserved recordings (recording identity -> entry); ensures only one
    /// pipeline runs per recording at any given time.
    active: Mutex<HashMap<String, ActiveEntry>>,
    /// 正在被删除的录制身份（录制身份 -> 未撤销的删除意图数）：期间拒绝新的 claim 与占位，
    /// 流水线跳过失败通知。按计数而非集合记录，同一录制的并发删除请求（如双击删除）中
    /// 先返回的一方不会提前撤销另一方的意图；计数归零时移除条目。
    /// Recording identities being deleted (recording identity -> number of outstanding removal
    /// intents): new claims and reservations are rejected meanwhile, and the pipeline skips
    /// the failure notification. Counted rather than kept in a set, so among concurrent
    /// removal requests for the same recording (e.g. a double-clicked delete) the one that
    /// returns first doesn't revoke the other's intent early; the entry is removed at zero.
    removal_requested: Mutex<HashMap<String, usize>>,
    /// 已通过等待阶段、正在删除文件与 meta 的录制身份：同一录制同一时刻只有一个删除请求
    /// 在执行，后到的请求等它结束后再复查文件与 meta 是否已不存在（D3）。
    /// Recording identities that passed the wait phase and are deleting their files and meta:
    /// only one removal request executes per recording at a time; later requests wait for it
    /// to finish and then re-check whether the files and meta are already gone (D3).
    removal_executing: Mutex<HashSet<String>>,
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

/// 录制级后处理 claim 的 RAII 守卫：持有期间同一录制的其他触发会被拒绝。
/// drop 时依次清理任务记录、取消标志与 claim（每把锁独立获取，不嵌套）。
///
/// RAII guard for a recording-level post-processing claim: while held, other triggers
/// for the same recording are rejected. On drop it removes the task record, the cancel
/// flag and the claim in turn (each lock is acquired independently, never nested).
pub struct PpClaim<'a> {
    queue: &'a PpQueue,
    recording_key: String,
    path_key: String,
}

impl Drop for PpClaim<'_> {
    fn drop(&mut self) {
        self.queue.tasks.write().remove(&self.path_key);
        self.queue.cancel_flags.write().remove(&self.path_key);
        self.queue.active.lock().remove(&self.recording_key);
    }
}

/// 维护扫描对某个录制的短暂占位（RAII）：持有期间 `try_claim` 返回 `Err(Reserved)`。
/// drop 时只在该键仍是占位时移除它，不触碰任务记录与取消标志。
///
/// Short-lived maintenance-scan reservation on a recording (RAII): while held, `try_claim`
/// returns `Err(Reserved)`. On drop it removes the key only if it is still a reservation,
/// and never touches task records or cancel flags.
pub struct PpReservation<'a> {
    queue: &'a PpQueue,
    recording_key: String,
}

impl Drop for PpReservation<'_> {
    fn drop(&mut self) {
        let mut active = self.queue.active.lock();
        if matches!(active.get(&self.recording_key), Some(ActiveEntry::Reserved)) {
            active.remove(&self.recording_key);
        }
    }
}

/// 删除录制意图的 RAII 守卫：持有期间拒绝该录制新的 claim 与占位，drop 时撤销本守卫
/// 登记的那一次意图（同一录制的其他守卫仍持有时，删除意图继续生效）。
/// RAII guard for a recording-removal intent: while held, new claims and reservations for
/// the recording are rejected; on drop it revokes only the intent it registered (the removal
/// intent stays in effect while other guards for the same recording are still held).
pub struct PpRemovalGuard<'a> {
    queue: &'a PpQueue,
    recording_key: String,
}

impl Drop for PpRemovalGuard<'_> {
    fn drop(&mut self) {
        use std::collections::hash_map::Entry;
        // 计数减一，归零才移除 / Decrement the count; remove only when it reaches zero
        if let Entry::Occupied(mut o) =
            self.queue.removal_requested.lock().entry(self.recording_key.clone())
        {
            let n = o.get_mut();
            *n = n.saturating_sub(1);
            if *n == 0 {
                o.remove();
            }
        }
    }
}

/// 同一录制删除执行权的 RAII 守卫（见 [`PpQueue::try_begin_removal`]）：持有期间同一录制
/// 的其他删除请求拿不到执行权，drop 时释放。
/// RAII guard for the right to execute a recording's removal (see
/// [`PpQueue::try_begin_removal`]): while held, other removal requests for the same recording
/// can't obtain it; released on drop.
pub struct PpRemovalExecution<'a> {
    queue: &'a PpQueue,
    recording_key: String,
}

impl Drop for PpRemovalExecution<'_> {
    fn drop(&mut self) {
        self.queue.removal_executing.lock().remove(&self.recording_key);
    }
}

impl Default for PpQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 将用户配置的并发数（0=自动）解析为实际许可数。
///
/// 后处理任务以磁盘 I/O（ts_merge）为主，每个任务都会驱动一个 ffmpeg 进程。
/// 自动模式取 `cpu × 2` 作为默认值；用户手动设置时上限同样为 `cpu × 2`，
/// 两者一致，手动设置的意义在于可以低于自动值以主动限制并发。
///
/// Resolve the configured concurrency (0 = auto) to the actual permit count.
///
/// Post-processing tasks are primarily disk-I/O-bound (ts_merge drives one ffmpeg
/// process per task). Auto mode uses `cpu × 2`; the hard cap for manually-set
/// values is also `cpu × 2` — the purpose of manual setting is to go *below*
/// the automatic value to limit concurrency intentionally.
pub fn resolve_concurrency(n: usize) -> usize {
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // 上限为 cpu * 2 / Hard cap at cpu * 2
    let cap = (cpu * 2).max(1);
    if n == 0 {
        // 自动：取 cpu × 2 / Auto: twice the core count
        cap
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
            active: Mutex::new(HashMap::new()),
            removal_requested: Mutex::new(HashMap::new()),
            removal_executing: Mutex::new(HashSet::new()),
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
    /// `n = 0` 表示自动（由 `resolve_concurrency` 映射为 CPU 逻辑核心数 × 2）。
    ///
    /// Dynamically update concurrency (from user config change).
    ///
    /// Also updates the theoretical upper bound (`max_permits`); subsequent
    /// `adjust_for_load` calls will never exceed this value.
    ///
    /// `n = 0` means auto (= logical CPU count × 2); manually set values are capped at the same CPU × 2 by `resolve_concurrency`.
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

    /// 获取并发执行许可（阻塞直至有空闲槽位），并递增运行中计数。
    /// 等待期间 `cancel` 被置位（[`cancel`](Self::cancel) 会同时唤醒等待者）时返回 `None`，
    /// 排队中的任务因此能在取消后及时退出，而不必等到拿到许可。
    ///
    /// Acquire a concurrency permit (blocks until a slot is free) and increment the running
    /// count. Returns `None` if `cancel` gets set while waiting ([`cancel`](Self::cancel)
    /// also wakes the waiters), so a queued task exits promptly after cancellation instead
    /// of waiting until it obtains a permit.
    pub fn acquire_concurrency_permit(&self, cancel: &AtomicBool) -> Option<SyncPermit> {
        self.semaphore.acquire_cancellable(&self.running_count, cancel)
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

    /// 尝试为某个录制原子地取得后处理 claim。
    /// 该录制正在被删除时返回 `Err(RemovalRequested)`；已有任务在排队或执行中返回
    /// `Err(AlreadyClaimed)`；维护扫描正占位时返回 `Err(Reserved)`（调用方应短暂等待后重试）；
    /// 否则登记并返回 RAII 守卫。两把锁依次获取，不嵌套。
    ///
    /// Atomically try to claim post-processing for a recording.
    /// Returns `Err(RemovalRequested)` while the recording is being deleted,
    /// `Err(AlreadyClaimed)` if a task for it is already queued or running, and
    /// `Err(Reserved)` while a maintenance scan holds a reservation (callers should wait
    /// briefly and retry); otherwise registers the claim and returns an RAII guard. The two
    /// locks are taken one after another, never nested.
    ///
    /// 登记后会再复查一次删除意图（见 [`Self::recheck_removal_after_insert`]），保证与
    /// `request_removal` + `is_recording_active` 并发时两者至少有一方能看到对方。
    /// After registering, the removal intent is re-checked once more (see
    /// [`Self::recheck_removal_after_insert`]), so that when racing `request_removal` +
    /// `is_recording_active`, at least one side always sees the other.
    pub fn try_claim(&self, recording_key: &str, path_key: &str) -> Result<PpClaim<'_>, ClaimRejected> {
        use std::collections::hash_map::Entry;
        if self.is_removal_requested(recording_key) {
            return Err(ClaimRejected::RemovalRequested);
        }
        match self.active.lock().entry(recording_key.to_string()) {
            Entry::Occupied(o) => {
                return Err(match o.get() {
                    ActiveEntry::Claimed(_) => ClaimRejected::AlreadyClaimed,
                    ActiveEntry::Reserved => ClaimRejected::Reserved,
                });
            }
            Entry::Vacant(v) => {
                v.insert(ActiveEntry::Claimed(path_key.to_string()));
            }
        }
        if self.recheck_removal_after_insert(recording_key) {
            // 条目是本次刚插入的（其他调用方见到已占用只会返回错误），直接移除，不触碰任务记录
            // The entry was inserted just now (other callers only get an error when they see
            // it occupied), so remove it directly without touching task records
            self.active.lock().remove(recording_key);
            return Err(ClaimRejected::RemovalRequested);
        }
        Ok(PpClaim {
            queue: self,
            recording_key: recording_key.to_string(),
            path_key: path_key.to_string(),
        })
    }

    /// 在 `active` 中登记 claim/占位之后复查删除意图，返回 true 表示应撤销刚才的登记。
    ///
    /// 入口处的检查与插入分属两把锁，单靠它会漏掉这样的交错：调用方检查（无删除意图）→
    /// 删除方登记意图 → 删除方检查 `active`（为空，开始删文件）→ 调用方插入。插入后再查一次：
    /// 若复查在删除方登记之前完成，则插入也早于删除方对 `active` 的检查，删除方会看到并等待；
    /// 否则复查必然看到删除意图。两种情况下 claim/占位都不会与删除同时进行。
    ///
    /// Re-check the removal intent after registering a claim/reservation in `active`; returns
    /// true if that registration should be rolled back.
    ///
    /// The entry check and the insert use two separate locks, which alone miss this
    /// interleaving: caller checks (no intent) → remover registers its intent → remover checks
    /// `active` (empty, starts deleting files) → caller inserts. Checking again after the
    /// insert closes it: if the re-check completes before the remover registers, the insert
    /// also precedes the remover's `active` check, so the remover sees it and waits; otherwise
    /// the re-check is guaranteed to see the intent. Either way a claim/reservation never
    /// overlaps the deletion.
    fn recheck_removal_after_insert(&self, recording_key: &str) -> bool {
        self.is_removal_requested(recording_key)
    }

    /// 维护扫描在读写某录制 meta 前短暂占位，消除"检查 claim"与 `write_meta` 之间的
    /// TOCTOU（A3）：占位期间 `try_claim` 返回 `Err(Reserved)`。该录制正在被删除或已被
    /// claim/占位时返回 `None`。
    ///
    /// Briefly reserve a recording before a maintenance scan reads/writes its meta, closing
    /// the TOCTOU window between "check claim" and `write_meta` (A3): while reserved,
    /// `try_claim` returns `Err(Reserved)`. Returns `None` if the recording is being deleted
    /// or is already claimed/reserved.
    pub fn try_reserve(&self, recording_key: &str) -> Option<PpReservation<'_>> {
        use std::collections::hash_map::Entry;
        if self.is_removal_requested(recording_key) {
            return None;
        }
        match self.active.lock().entry(recording_key.to_string()) {
            Entry::Occupied(_) => return None,
            Entry::Vacant(v) => {
                v.insert(ActiveEntry::Reserved);
            }
        }
        let reservation = PpReservation { queue: self, recording_key: recording_key.to_string() };
        // 插入后复查删除意图（同 try_claim）；失败时 drop 守卫撤销刚插入的占位
        // Re-check the removal intent after inserting (same as try_claim); on failure the
        // guard is dropped, rolling back the reservation just inserted
        if self.recheck_removal_after_insert(recording_key) {
            return None;
        }
        Some(reservation)
    }

    /// 登记删除录制的意图：守卫持有期间拒绝该录制新的 claim 与占位，并让流水线跳过
    /// 失败通知（B13）；守卫 drop 时撤销。按计数登记，同一录制可有多个守卫并存，
    /// 全部 drop 后删除意图才撤销。
    ///
    /// Register an intent to delete a recording: while the guard is held, new claims and
    /// reservations for it are rejected and the pipeline skips its failure notification
    /// (B13); revoked when the guard drops. Intents are counted, so several guards for the same
    /// recording may coexist and the intent is revoked only after all of them drop.
    pub fn request_removal(&self, recording_key: &str) -> PpRemovalGuard<'_> {
        *self
            .removal_requested
            .lock()
            .entry(recording_key.to_string())
            .or_insert(0) += 1;
        PpRemovalGuard { queue: self, recording_key: recording_key.to_string() }
    }

    /// 判断某个录制当前是否正在被删除（尚有未撤销的删除意图）。
    /// Check whether a recording is currently being deleted (has outstanding removal intents).
    pub fn is_removal_requested(&self, recording_key: &str) -> bool {
        self.removal_requested
            .lock()
            .get(recording_key)
            .is_some_and(|&n| n > 0)
    }

    /// 尝试获取某个录制的删除执行权：同一录制已有删除请求在执行时返回 `None`。
    /// 只取 `removal_executing` 一把锁，不嵌套其他锁。执行权不影响 claim 与占位
    /// （拒绝它们靠的是删除意图），调用方应先 [`request_removal`](Self::request_removal)
    /// 再获取执行权。
    ///
    /// Try to obtain the right to execute a recording's removal: returns `None` while another
    /// removal request for the same recording is executing. Only takes the
    /// `removal_executing` lock, never nested with others. The execution right doesn't affect
    /// claims or reservations (those are rejected by the removal intent), so callers should
    /// call [`request_removal`](Self::request_removal) first.
    pub fn try_begin_removal(&self, recording_key: &str) -> Option<PpRemovalExecution<'_>> {
        if !self.removal_executing.lock().insert(recording_key.to_string()) {
            return None;
        }
        Some(PpRemovalExecution { queue: self, recording_key: recording_key.to_string() })
    }

    /// 判断某个录制当前是否已被 claim（正在排队或执行后处理）或被维护扫描占位。
    /// Check whether a recording is currently claimed (queued or running post-processing)
    /// or reserved by a maintenance scan.
    pub fn is_recording_active(&self, recording_key: &str) -> bool {
        self.active.lock().contains_key(recording_key)
    }

    /// 判断是否有已被 claim 的录制与给定文件名 stem 相同（录制身份键是 `.../{stem}.json`）。
    /// 只统计流水线 claim，不统计扫描占位（扫描自身的占位不能挡住自己）。
    /// 用于 split_by_streamer=false 时：合并文件落在扁平目录、推导出的身份键不同，
    /// 且 ts_merge 尚未把 meta.video_path 切换过去，只能按 stem 识别出它属于执行中的录制。
    ///
    /// Check whether any claimed recording has the given file stem (identity keys are
    /// `.../{stem}.json`). Only pipeline claims count, not scan reservations (a scan's own
    /// reservation must not block itself). Used when split_by_streamer=false: the merged
    /// file lands in a flat directory with a different derived identity key and ts_merge
    /// hasn't switched meta.video_path yet, so only the stem can tie it to the running
    /// recording.
    pub fn is_stem_active(&self, stem: &std::ffi::OsStr) -> bool {
        self.active
            .lock()
            .iter()
            .any(|(k, e)| matches!(e, ActiveEntry::Claimed(_)) && Path::new(k).file_stem() == Some(stem))
    }

    /// 请求取消指定任务。
    /// 除按 `path` 直接设置标志外，还会按录制身份解析实际的队列键：
    /// 例如传入合并后的视频路径，也能取消以 session_dir 为键运行中的同一录制任务。
    ///
    /// Request cancellation of a task.
    /// Besides setting the flag for `path` directly, it also resolves the actual queue key
    /// via the recording identity: e.g. passing the merged video path also cancels the same
    /// recording's task running under its session_dir key.
    pub fn cancel(&self, path: &str) {
        if let Some(flag) = self.cancel_flags.read().get(path) {
            flag.store(true, Ordering::Relaxed);
        }
        // 先克隆队列键并释放 active 锁，再获取 cancel_flags 锁，避免锁嵌套
        // Clone the queue key and release the active lock before taking cancel_flags (no nesting)
        let key = recording_key(Path::new(path));
        let queued_path = match self.active.lock().get(&key) {
            Some(ActiveEntry::Claimed(p)) => Some(p.clone()),
            _ => None,
        };
        if let Some(queued_path) = queued_path
            && queued_path != path
            && let Some(flag) = self.cancel_flags.read().get(&queued_path)
        {
            flag.store(true, Ordering::Relaxed);
        }
        // 唤醒等待并发许可的任务，让被取消者及时退出
        // Wake tasks waiting for a concurrency permit so cancelled ones exit promptly
        self.semaphore.wake_all();
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

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_DIR: &str = "X:/out/ts_fragment/alice/alice_20240101_120000";
    const MERGED_FILE: &str = "X:/out/recordings/alice/alice_20240101_120000.mp4";

    /// 同一录制第二次 claim 被拒绝，守卫 drop 后可再次 claim。
    /// A second claim for the same recording is rejected; re-claim works after the guard drops.
    #[test]
    fn claim_is_exclusive_until_dropped() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let first = q.try_claim(&key, SESSION_DIR);
        assert!(first.is_ok());
        assert_eq!(q.try_claim(&key, MERGED_FILE).err(), Some(ClaimRejected::AlreadyClaimed));
        drop(first);
        assert!(q.try_claim(&key, MERGED_FILE).is_ok());
    }

    /// session_dir 与其合并文件解析为同一录制身份。
    /// A session_dir and its merged file resolve to the same recording identity.
    #[test]
    fn session_dir_and_merged_file_share_key() {
        assert_eq!(
            recording_key(Path::new(SESSION_DIR)),
            recording_key(Path::new(MERGED_FILE))
        );
    }

    /// 按合并文件路径取消能命中以 session_dir 为键的任务。
    /// Cancelling by the merged file path hits the task keyed by session_dir.
    #[test]
    fn cancel_by_merged_path_resolves_session_key() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let _claim = q.try_claim(&key, SESSION_DIR).expect("claim");
        q.enqueue(SESSION_DIR);
        assert!(!q.is_cancelled(SESSION_DIR));
        q.cancel(MERGED_FILE);
        assert!(q.is_cancelled(SESSION_DIR));
    }

    /// 守卫 drop 后任务记录与 claim 均被清理。
    /// After the guard drops, both the task record and the claim are cleared.
    #[test]
    fn drop_clears_task_and_claim() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let claim = q.try_claim(&key, SESSION_DIR).expect("claim");
        q.enqueue(SESSION_DIR);
        assert!(q.is_tracked(SESSION_DIR));
        assert!(q.is_recording_active(&key));
        drop(claim);
        assert!(!q.is_tracked(SESSION_DIR));
        assert!(!q.is_recording_active(&key));
    }

    /// 扁平目录下的合并文件（身份键不同）能按 stem 识别为执行中的录制，drop 后不再命中。
    /// A merged file in a flat dir (different identity key) is recognized by stem as belonging
    /// to the running recording, and no longer matches after the guard drops.
    #[test]
    fn stem_active_matches_flat_merged_file() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let flat = Path::new("X:/custom/alice_20240101_120000.mp4");
        assert_ne!(recording_key(flat), key);
        let stem = flat.file_stem().expect("stem");
        let claim = q.try_claim(&key, SESSION_DIR).expect("claim");
        assert!(q.is_stem_active(stem));
        assert!(!q.is_stem_active(std::ffi::OsStr::new("bob_20240101_120000")));
        drop(claim);
        assert!(!q.is_stem_active(stem));
    }

    /// set_permits 只调整上限，不会让可用许可超过 limit − in_use。
    /// set_permits only changes the ceiling; available permits never exceed limit − in_use.
    #[test]
    fn set_permits_does_not_over_issue() {
        use std::sync::atomic::AtomicUsize;
        let sem = Arc::new(SyncSemaphore::new(2));
        let running = Arc::new(AtomicUsize::new(0));
        let p1 = sem.acquire_cancellable(&running, &AtomicBool::new(false)).unwrap();
        let p2 = sem.acquire_cancellable(&running, &AtomicBool::new(false)).unwrap();
        assert_eq!(sem.current_permits(), 0);
        // 旧实现会把可用许可重置为 2 / The old implementation would reset available permits to 2
        sem.set_permits(2);
        assert_eq!(sem.current_permits(), 0);
        drop(p1);
        assert_eq!(sem.current_permits(), 1);
        sem.set_permits(1);
        assert_eq!(sem.current_permits(), 0);
        drop(p2);
        assert_eq!(sem.current_permits(), 1);
        assert_eq!(running.load(Ordering::Relaxed), 0);
    }

    /// 扫描占位期间 try_claim 返回 Err(Reserved)，占位 drop 后可 claim；占位不影响任务记录。
    /// While reserved, try_claim returns Err(Reserved); claim works after the reservation drops;
    /// the reservation leaves task records alone.
    #[test]
    fn reservation_blocks_claim_until_dropped() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        q.enqueue(SESSION_DIR);
        let r = q.try_reserve(&key).expect("reserve");
        assert!(q.is_recording_active(&key));
        assert_eq!(q.try_claim(&key, SESSION_DIR).err(), Some(ClaimRejected::Reserved));
        drop(r);
        assert!(q.is_tracked(SESSION_DIR));
        assert!(!q.is_recording_active(&key));
        assert!(q.try_claim(&key, SESSION_DIR).is_ok());
    }

    /// 扫描占位不被 is_stem_active 统计。
    /// Scan reservations are not counted by is_stem_active.
    #[test]
    fn reservation_not_counted_by_stem_active() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let _r = q.try_reserve(&key).expect("reserve");
        assert!(!q.is_stem_active(std::ffi::OsStr::new("alice_20240101_120000")));
    }

    /// 已被 claim 的录制不能再被占位；claim drop 后可占位。
    /// A claimed recording cannot be reserved; reservation works after the claim drops.
    #[test]
    fn claim_blocks_reservation() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let c = q.try_claim(&key, SESSION_DIR).expect("claim");
        assert!(q.try_reserve(&key).is_none());
        drop(c);
        assert!(q.try_reserve(&key).is_some());
    }

    /// 删除意图期间 claim 与占位都被拒绝，守卫 drop 后恢复。
    /// While a removal is requested both claims and reservations are rejected; restored after drop.
    #[test]
    fn removal_request_blocks_claim_and_reservation() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let g = q.request_removal(&key);
        assert!(q.is_removal_requested(&key));
        assert_eq!(q.try_claim(&key, SESSION_DIR).err(), Some(ClaimRejected::RemovalRequested));
        assert!(q.try_reserve(&key).is_none());
        drop(g);
        assert!(!q.is_removal_requested(&key));
        assert!(q.try_reserve(&key).is_some());
        assert!(q.try_claim(&key, SESSION_DIR).is_ok());
    }

    /// 同一录制的两个删除守卫并存时，drop 其中一个后删除意图仍生效（仍拒绝 claim 与占位，
    /// 且被拒绝的调用不在 active 中留下条目）；两个都 drop 后恢复。
    /// With two removal guards for the same recording, dropping one keeps the intent in effect
    /// (claims and reservations are still rejected, and rejected calls leave no entry in
    /// `active`); everything is restored once both drop.
    #[test]
    fn removal_intent_is_counted_across_guards() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let g1 = q.request_removal(&key);
        let g2 = q.request_removal(&key);
        drop(g1);
        assert!(q.is_removal_requested(&key));
        assert_eq!(q.try_claim(&key, SESSION_DIR).err(), Some(ClaimRejected::RemovalRequested));
        assert!(q.try_reserve(&key).is_none());
        assert!(!q.is_recording_active(&key));
        drop(g2);
        assert!(!q.is_removal_requested(&key));
        assert!(q.try_reserve(&key).is_some());
        assert!(q.try_claim(&key, SESSION_DIR).is_ok());
    }

    /// 删除意图按录制身份分别计数，不影响其他录制。
    /// Removal intents are counted per recording identity and don't affect other recordings.
    #[test]
    fn removal_intent_is_per_recording() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let other = recording_key(Path::new("X:/out/ts_fragment/bob/bob_20240101_120000"));
        assert_ne!(key, other);
        let _g = q.request_removal(&key);
        assert!(!q.is_removal_requested(&other));
        assert!(q.try_claim(&other, "X:/out/ts_fragment/bob/bob_20240101_120000").is_ok());
    }

    /// 删除执行权按录制身份互斥：同一录制第二次获取失败，其他录制不受影响，drop 后可再次
    /// 获取；全程不触碰 active（不影响 claim/占位判断）。
    /// The removal execution right is exclusive per recording identity: a second acquire for
    /// the same recording fails, other recordings are unaffected, and it can be re-acquired
    /// after drop; `active` is never touched (claims/reservations are unaffected).
    #[test]
    fn removal_execution_is_exclusive_per_recording() {
        let q = PpQueue::new();
        let key = recording_key(Path::new(SESSION_DIR));
        let other = recording_key(Path::new("X:/out/ts_fragment/bob/bob_20240101_120000"));
        let g = q.try_begin_removal(&key);
        assert!(g.is_some());
        assert!(q.try_begin_removal(&key).is_none());
        assert!(!q.is_recording_active(&key));
        let g_other = q.try_begin_removal(&other);
        assert!(g_other.is_some());
        drop(g);
        assert!(!q.is_recording_active(&key));
        let again = q.try_begin_removal(&key);
        assert!(again.is_some());
        assert!(!q.is_recording_active(&key));
    }

    /// 排队等待许可的任务在取消标志置位并 wake_all 后 1 秒内返回 None。
    /// A task waiting for a permit returns None within 1 s after its cancel flag is set and wake_all.
    #[test]
    fn cancellable_acquire_returns_none_when_cancelled() {
        use std::sync::atomic::AtomicUsize;
        use std::time::{Duration, Instant};
        let sem = Arc::new(SyncSemaphore::new(1));
        let running = Arc::new(AtomicUsize::new(0));
        let held = sem.acquire_cancellable(&running, &AtomicBool::new(false)).expect("permit");
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = {
            let sem = Arc::clone(&sem);
            let running = Arc::clone(&running);
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || {
                let got = sem.acquire_cancellable(&running, &cancel);
                tx.send(got.is_some()).expect("send");
            })
        };
        // 确认等待者仍在阻塞 / Confirm the waiter is still blocked
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        let start = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        sem.wake_all();
        let got = rx.recv_timeout(Duration::from_secs(1)).expect("waiter returned within 1 s");
        assert!(!got);
        assert!(start.elapsed() < Duration::from_secs(1));
        handle.join().expect("join");
        assert_eq!(running.load(Ordering::Relaxed), 1);
        drop(held);
        assert_eq!(running.load(Ordering::Relaxed), 0);
    }
}
