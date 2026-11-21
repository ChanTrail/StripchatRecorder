//! 纯同步计数信号量子模块：基于 `parking_lot::Mutex` + `Condvar` 实现。
//! Pure sync counting-semaphore submodule backed by `parking_lot::Mutex` +
//! `Condvar`.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// 纯同步计数信号量，基于 `parking_lot::Mutex` + `Condvar` 实现。
///
/// 不依赖 tokio async，可在 `spawn_blocking` 线程（栈较小）上安全调用，
/// 避免 `block_on` 嵌套导致的栈溢出。
///
/// Pure sync counting semaphore backed by `parking_lot::Mutex` + `Condvar`.
///
/// Does not depend on tokio async; safe to call from `spawn_blocking` threads
/// (which have smaller stacks), avoiding stack overflow from nested `block_on`.
pub(super) struct SyncSemaphore {
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
	/// 创建信号量 / Create a semaphore with the given permit ceiling
	pub(super) fn new(permits: usize) -> Self {
		Self {
			state: Mutex::new(SemState {
				limit: permits,
				in_use: 0,
			}),
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
	pub(super) fn acquire_cancellable(
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
		Some(SyncPermit {
			sem: Arc::clone(self),
			running: Arc::clone(running),
		})
	}

	/// 唤醒所有等待者重新检查（取消请求后调用）。
	/// Wake all waiters to re-check (called after a cancel request).
	pub(super) fn wake_all(&self) {
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
	pub(super) fn set_permits(&self, permits: usize) {
		let mut st = self.state.lock();
		st.limit = permits;
		// 唤醒所有等待者重新检查，上限调高时新增的许可能被立即消费
		// Wake all waiters to re-check; newly available permits after raising the ceiling get consumed
		self.condvar.notify_all();
	}

	/// 返回当前可用许可数快照（= limit - in_use，瞬时值，仅供监控/调试使用）。
	/// Returns a snapshot of the current available permit count
	/// (= limit - in_use, instantaneous, for monitoring/debug only).
	pub(super) fn current_permits(&self) -> usize {
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
