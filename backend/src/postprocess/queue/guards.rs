//! RAII 守卫子模块：claim、扫描占位、删除意图与删除执行权的守卫。
//! RAII-guard submodule: claim, scan-reservation, removal-intent and
//! removal-execution guards.

use super::{ActiveEntry, PpQueue};

/// 录制级后处理 claim 的 RAII 守卫：持有期间同一录制的其他触发会被拒绝。
/// drop 时依次清理任务记录、取消标志与 claim（每把锁独立获取，不嵌套）。
///
/// RAII guard for a recording-level post-processing claim: while held, other triggers
/// for the same recording are rejected. On drop it removes the task record, the cancel
/// flag and the claim in turn (each lock is acquired independently, never nested).
pub struct PpClaim<'a> {
    pub(super) queue: &'a PpQueue,
    pub(super) recording_key: String,
    pub(super) path_key: String,
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
    pub(super) queue: &'a PpQueue,
    pub(super) recording_key: String,
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
    pub(super) queue: &'a PpQueue,
    pub(super) recording_key: String,
}

impl Drop for PpRemovalGuard<'_> {
    fn drop(&mut self) {
        use std::collections::hash_map::Entry;
        // 计数减一，归零才移除 / Decrement the count; remove only when it reaches zero
        if let Entry::Occupied(mut o) = self
            .queue
            .removal_requested
            .lock()
            .entry(self.recording_key.clone())
        {
            let n = o.get_mut();
            *n = n.saturating_sub(1);
            if *n == 0 {
                o.remove();
            }
        }
    }
}

/// 同一录制删除执行权的 RAII 守卫（见 [`super::PpQueue::try_begin_removal`]）：持有期间
/// 同一录制的其他删除请求拿不到执行权，drop 时释放。
/// RAII guard for the right to execute a recording's removal (see
/// [`super::PpQueue::try_begin_removal`]): while held, other removal requests for the same
/// recording can't obtain it; released on drop.
pub struct PpRemovalExecution<'a> {
    pub(super) queue: &'a PpQueue,
    pub(super) recording_key: String,
}

impl Drop for PpRemovalExecution<'_> {
    fn drop(&mut self) {
        self.queue
            .removal_executing
            .lock()
            .remove(&self.recording_key);
    }
}
