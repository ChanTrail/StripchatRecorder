//! 后处理任务队列 / Post-processing Task Queue
//!
//! 集中管理后处理任务的运行时状态、取消标志和串行执行锁，
//! 从 `AppState` 中分离出来以保持关注点清晰。
//!
//! This module centralizes post-processing task runtime state, cancel flags,
//! and the serial execution lock, separated from `AppState` to keep concerns clean.
//!
//! ## 设计 / Design
//!
//! - 同一时刻只允许一个后处理任务运行，通过 [`PpQueue::acquire_serial_lock`] 保证。
//! - 其余排队中的任务在 `tasks` 表里标记为 `"waiting"`，直到轮到执行。
//! - `cancel_flags` 允许调用方（如取消按钮）异步请求中止某个正在运行或排队的任务。
//! - [`PpQueue::get_all_tasks`] 合并内存中的运行时状态和 `meta/` 目录中的历史完成记录，
//!   供前端一次性获取完整的任务列表。

use parking_lot::RwLock;
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

/// 后处理任务队列：任务状态表 + 取消标志 + 串行执行锁。
/// Post-processing task queue: task status table + cancel flags + serial execution lock.
pub struct PpQueue {
    /// 任务状态表（文件路径 -> 状态）/ Task status map (file path -> status)
    tasks: RwLock<HashMap<String, PpTaskStatus>>,
    /// 取消标志表（文件路径 -> 原子布尔）/ Cancel flag map (file path -> atomic bool)
    cancel_flags: RwLock<HashMap<String, Arc<AtomicBool>>>,
    /// 串行执行锁，确保同一时刻只有一个后处理任务运行
    /// Serial execution lock ensuring only one post-processing task runs at a time
    serial_lock: std::sync::Mutex<()>,
}

impl Default for PpQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PpQueue {
    /// 创建空队列。
    /// Create an empty queue.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            cancel_flags: RwLock::new(HashMap::new()),
            serial_lock: std::sync::Mutex::new(()),
        }
    }

    /// 获取串行执行锁（阻塞直至轮到当前任务）。
    /// 锁中毒（上一个持有者 panic）时仍能正常获取，不会永久卡死队列。
    ///
    /// Acquire the serial execution lock (blocks until it's this task's turn).
    /// Recovers from a poisoned lock (previous holder panicked) so the queue never
    /// gets permanently stuck.
    pub fn acquire_serial_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.serial_lock.lock().unwrap_or_else(|e| e.into_inner())
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
