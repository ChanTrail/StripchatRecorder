//! 每进程 GPU 占用采样（按平台实现）/ Per-process GPU usage sampling (per-platform impls)
//!
//! sysinfo 不提供 GPU 指标，这里按平台各取最便宜的可用来源：
//! - **Windows**：PDH 性能计数器 `\GPU Engine(pid_*)\Utilization Percentage`
//!   （引擎忙时比率，差分计数器 → start/finish 两次采集）与
//!   `\GPU Process Memory(pid_*)\Local Usage`（显存字节，瞬时值）。
//!   手写 FFI 调 pdh.dll，避免引入 windows crate。
//! - **Linux**：`/proc/<pid>/fdinfo/` 中指向 `/dev/dri/*` 的 fd 附带的
//!   `drm-engine-*` 引擎时间（纳秒累计值），窗口内差分 ÷ 墙钟 = 占用率。
//! - **其它平台**（macOS 等）：不支持，统一返回空映射 → 前端显示 —。
//!
//! sysinfo exposes no GPU metrics, so we use the cheapest usable source per
//! platform:
//! - **Windows**: PDH counters `\GPU Engine(pid_*)\Utilization Percentage`
//!   (engine-busy ratio, a differential counter → collected at start/finish)
//!   and `\GPU Process Memory(pid_*)\Local Usage` (VRAM bytes, instantaneous).
//!   Hand-rolled FFI against pdh.dll keeps the `windows` crate out of the tree.
//! - **Linux**: `drm-engine-*` engine-time nanoseconds attached to fds
//!   pointing at `/dev/dri/*` in `/proc/<pid>/fdinfo/`; window delta ÷
//!   wall-clock = utilization.
//! - **Other platforms** (macOS etc.): unsupported — empty map → UI shows —.

use std::collections::HashMap;

/// 单个进程的 GPU 指标 / One process' GPU metrics
#[derive(Debug, Clone, Copy)]
pub struct GpuUsage {
	/// 引擎利用率（各引擎求和，多引擎可超 100，与 CPU 口径一致）/
	/// Engine utilization (summed across engines — may exceed 100 on
	/// multi-engine use, same convention as CPU)
	pub pct: f32,
	/// 显存字节（仅 Windows 可得）/ VRAM bytes (Windows only)
	pub mem_bytes: Option<u64>,
}

/// pid → GPU 指标 / pid → GPU metrics map
#[derive(Debug, Default)]
pub struct GpuUsageMap {
	map: HashMap<u32, GpuUsage>,
}

impl GpuUsageMap {
	/// 查询某 pid 的 GPU 指标 / Look up GPU metrics for a pid
	pub fn get(&self, pid: u32) -> Option<GpuUsage> {
		self.map.get(&pid).copied()
	}
}

/// 跨采样窗口的 GPU 采样器：`new` 建立 → `start` 记基线 → 睡过窗口 →
/// `finish` 出结果。平台不支持时各步均为 no-op。
/// GPU sampler spanning the collection window: `new` sets it up → `start`
/// records the baseline → sleep across the window → `finish` produces the
/// result. Every step is a no-op on unsupported platforms.
pub struct GpuSampler {
	inner: imp::Sampler,
}

impl Default for GpuSampler {
	fn default() -> Self {
		Self::new()
	}
}

impl GpuSampler {
	pub fn new() -> Self {
		Self {
			inner: imp::Sampler::new(),
		}
	}
	pub fn start(&mut self) {
		self.inner.start();
	}
	pub fn finish(self) -> GpuUsageMap {
		self.inner.finish()
	}
}

// ─── Windows：PDH 计数器 / Windows: PDH counters ──────────────────────────────
#[cfg(windows)]
mod imp {
	use super::{GpuUsage, GpuUsageMap};
	use std::collections::HashMap;
	use std::ffi::OsStr;
	use std::os::windows::ffi::OsStrExt;

	/// PDH 浮点格式 / PDH double format
	const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
	/// 缓冲区不足 / Buffer too small
	const PDH_MORE_DATA: i32 = 0x8000_07D2_u32 as i32;
	/// 计数器值有效 / Counter value valid
	const PDH_CSTATUS_VALID_DATA: i32 = 0;
	const PDH_CSTATUS_NEW_DATA: i32 = 1;

	/// `Utilization Percentage` 值联合体（只需 doubleValue 成员）/
	/// `Utilization Percentage` value union (only doubleValue needed)
	#[repr(C)]
	union PdhFmtValue {
		double_value: f64,
	}

	/// PDH_FMT_COUNTERVALUE：LONG CStatus + union（8 字节对齐）/
	/// PDH_FMT_COUNTERVALUE: LONG CStatus + union (8-byte aligned)
	#[repr(C)]
	struct PdhFmtCountervalue {
		c_status: i32,
		_pad: i32,
		value: PdhFmtValue,
	}

	/// PDH_FMT_COUNTERVALUE_ITEM_W（宽字符版）/ PDH_FMT_COUNTERVALUE_ITEM_W
	#[repr(C)]
	struct PdhFmtItemW {
		sz_name: *const u16,
		fmt_value: PdhFmtCountervalue,
	}

	#[link(name = "pdh")]
	unsafe extern "system" {
		fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut isize) -> i32;
		fn PdhAddEnglishCounterW(
			query: isize,
			full_counter_path: *const u16,
			user_data: usize,
			counter: *mut isize,
		) -> i32;
		fn PdhCollectQueryData(query: isize) -> i32;
		fn PdhGetFormattedCounterArrayW(
			counter: isize,
			format: u32,
			buffer_size: *mut u32,
			item_count: *mut u32,
			buffer: *mut u8,
		) -> i32;
		fn PdhCloseQuery(query: isize) -> i32;
	}

	/// 引擎利用率计数器路径（英文名，PdhAddEnglishCounterW 与系统语言无关）/
	/// Engine utilization counter path (English name — PdhAddEnglishCounterW
	/// is locale-independent)
	const ENG_PATH: &str = r"\GPU Engine(*)\Utilization Percentage";
	/// 显存计数器路径 / VRAM counter path
	const MEM_PATH: &str = r"\GPU Process Memory(*)\Local Usage";

	/// UTF-16 宽字符串（含 NUL 结尾）/ UTF-16 wide string (NUL-terminated)
	fn wide(s: &str) -> Vec<u16> {
		OsStr::new(s)
			.encode_wide()
			.chain(std::iter::once(0))
			.collect()
	}

	pub struct Sampler {
		/// PDH 查询句柄（0 = 打开失败）/ PDH query handle (0 = open failed)
		pub(super) query: isize,
		/// 引擎利用率计数器（None = 计数器不可用）/
		/// Engine utilization counter (None = counter unavailable)
		pub(super) eng_counter: Option<isize>,
		/// 显存计数器 / VRAM counter
		pub(super) mem_counter: Option<isize>,
	}

	impl Sampler {
		pub fn new() -> Self {
			let mut s = Self {
				query: 0,
				eng_counter: None,
				mem_counter: None,
			};
			unsafe {
				if PdhOpenQueryW(std::ptr::null(), 0, &mut s.query) != 0 {
					s.query = 0;
					return s;
				}
				let mut c = 0isize;
				if PdhAddEnglishCounterW(s.query, wide(ENG_PATH).as_ptr(), 0, &mut c) == 0 {
					s.eng_counter = Some(c);
				}
				let mut c = 0isize;
				if PdhAddEnglishCounterW(s.query, wide(MEM_PATH).as_ptr(), 0, &mut c) == 0 {
					s.mem_counter = Some(c);
				}
			}
			s
		}

		pub fn start(&mut self) {
			if self.query != 0 {
				// 第一次采集：为差分型计数器（Utilization Percentage）记基线 /
				// First collect: baseline for the differential counter
				unsafe {
					PdhCollectQueryData(self.query);
				}
			}
		}

		/// 窗口结束：再次采集 → 读格式化数组 → 按实例名 pid_ 前缀归并 /
		/// End of window: collect again → read formatted arrays → merge
		/// per pid_ prefix in instance names
		pub fn finish(self) -> GpuUsageMap {
			let mut out = GpuUsageMap::default();
			if self.query == 0 {
				return out;
			}
			unsafe {
				PdhCollectQueryData(self.query);
			}
			let eng = unsafe { self.read_counter(self.eng_counter) };
			let mem = unsafe { self.read_counter(self.mem_counter) };
			for (pid, v) in eng {
				out.map
					.entry(pid)
					.or_insert(GpuUsage {
						pct: 0.0,
						mem_bytes: None,
					})
					.pct = v as f32;
			}
			for (pid, v) in mem {
				out.map
					.entry(pid)
					.or_insert(GpuUsage {
						pct: 0.0,
						mem_bytes: None,
					})
					.mem_bytes = Some(v as u64);
			}
			out
		}

		/// 读取一个计数器的全部实例（pid → 值合计）/
		/// Read all instances of one counter (pid → summed value)
		unsafe fn read_counter(&self, counter: Option<isize>) -> HashMap<u32, f64> {
			let mut sums = HashMap::new();
			let Some(counter) = counter else { return sums };
			unsafe {
				// 两阶段调用：先拿缓冲区大小 / Two-call idiom: size first
				let mut buf_size = 0u32;
				let mut item_count = 0u32;
				let status = PdhGetFormattedCounterArrayW(
					counter,
					PDH_FMT_DOUBLE,
					&mut buf_size,
					&mut item_count,
					std::ptr::null_mut(),
				);
				if status != PDH_MORE_DATA || buf_size == 0 {
					return sums;
				}
				let mut buf = vec![0u8; buf_size as usize];
				let status = PdhGetFormattedCounterArrayW(
					counter,
					PDH_FMT_DOUBLE,
					&mut buf_size,
					&mut item_count,
					buf.as_mut_ptr(),
				);
				if status != 0 {
					return sums;
				}
				let items = std::slice::from_raw_parts(
					buf.as_ptr() as *const PdhFmtItemW,
					item_count as usize,
				);
				for item in items {
					if item.fmt_value.c_status != PDH_CSTATUS_VALID_DATA
						&& item.fmt_value.c_status != PDH_CSTATUS_NEW_DATA
					{
						continue;
					}
					let name = wstr(item.sz_name);
					// 实例名形如 pid_1234_luid_..._engtype_3D /
					// Instance names look like pid_1234_luid_..._engtype_3D
					if let Some(pid) = parse_pid_instance(&name) {
						*sums.entry(pid).or_insert(0.0) += item.fmt_value.value.double_value;
					}
				}
			}
			sums
		}
	}

	impl Drop for Sampler {
		fn drop(&mut self) {
			if self.query != 0 {
				unsafe {
					PdhCloseQuery(self.query);
				}
			}
		}
	}

	/// NUL 结尾 UTF-16 → String / NUL-terminated UTF-16 → String
	unsafe fn wstr(p: *const u16) -> String {
		unsafe {
			let mut len = 0;
			while *p.add(len) != 0 {
				len += 1;
			}
			String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
		}
	}

	/// 从实例名解析 pid（`pid_1234_...` → 1234）/ Parse pid from instance name
	fn parse_pid_instance(name: &str) -> Option<u32> {
		let rest = name.strip_prefix("pid_")?;
		let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
		digits.parse().ok()
	}
}

// ─── Linux：fdinfo DRM 引擎时间 / Linux: DRM engine time via fdinfo ───────────
#[cfg(target_os = "linux")]
mod imp {
	use super::{GpuUsage, GpuUsageMap};
	use std::collections::HashMap;
	use std::time::Instant;

	/// 扫描 /proc，汇总各 pid 的 DRM 引擎累计纳秒 /
	/// Scan /proc, summing cumulative DRM engine nanoseconds per pid
	fn scan_engine_ns() -> HashMap<u32, u64> {
		let mut out = HashMap::new();
		let Ok(proc_dir) = std::fs::read_dir("/proc") else {
			return out;
		};
		for entry in proc_dir.flatten() {
			let name = entry.file_name();
			let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
				continue;
			};
			let fd_dir = entry.path().join("fd");
			let Ok(fds) = std::fs::read_dir(&fd_dir) else {
				continue;
			};
			let mut ns: u64 = 0;
			for fd in fds.flatten() {
				// 只统计指向 DRM 设备的 fd / Only fds pointing at DRM devices
				let Ok(target) = std::fs::read_link(fd.path()) else {
					continue;
				};
				if !target.to_string_lossy().contains("/dev/dri/") {
					continue;
				}
				let fdinfo = entry.path().join("fdinfo").join(fd.file_name());
				let Ok(text) = std::fs::read_to_string(fdinfo) else {
					continue;
				};
				for line in text.lines() {
					// 形如 drm-engine-render: 123456 ns /
					// Lines like `drm-engine-render: 123456 ns`
					if let Some(v) = line
						.strip_prefix("drm-engine-")
						.and_then(|l| l.split_whitespace().nth(1))
						.and_then(|n| n.parse::<u64>().ok())
					{
						ns += v;
					}
				}
			}
			if ns > 0 {
				out.insert(pid, ns);
			}
		}
		out
	}

	pub struct Sampler {
		base: Option<(Instant, HashMap<u32, u64>)>,
	}

	impl Sampler {
		pub fn new() -> Self {
			Self { base: None }
		}
		pub fn start(&mut self) {
			self.base = Some((Instant::now(), scan_engine_ns()));
		}
		pub fn finish(self) -> GpuUsageMap {
			let mut out = GpuUsageMap::default();
			let Some((t0, prev)) = self.base else {
				return out;
			};
			let now = scan_engine_ns();
			let elapsed = t0.elapsed().as_nanos() as f64;
			if elapsed <= 0.0 {
				return out;
			}
			for (pid, cur) in now {
				let delta = cur.saturating_sub(prev.get(&pid).copied().unwrap_or(0));
				if delta > 0 {
					out.map.insert(
						pid,
						GpuUsage {
							pct: (delta as f64 / elapsed * 100.0) as f32,
							mem_bytes: None,
						},
					);
				}
			}
			out
		}
	}
}

// ─── 其它平台：不支持 / Other platforms: unsupported ──────────────────────────
#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
	use super::GpuUsageMap;
	pub struct Sampler;
	impl Sampler {
		pub fn new() -> Self {
			Self
		}
		pub fn start(&mut self) {}
		pub fn finish(self) -> GpuUsageMap {
			GpuUsageMap::default()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// PDH 管道冒烟：查询句柄必须打开、引擎计数器必须挂上（验证手写 FFI
	/// 的结构体布局与 API 调用在本机可行）；finish 返回映射（可能为空——
	/// 无 GPU 活动的进程没有实例，不属失败）。
	///
	/// PDH plumbing smoke: the query handle must open and the engine
	/// counter must attach (validates the hand-rolled FFI layouts on this
	/// host); finish returns a map (possibly empty — processes without GPU
	/// activity have no instances, that's not a failure).
	#[test]
	#[cfg(windows)]
	fn pdh_query_smoke() {
		let mut s = imp::Sampler::new();
		assert_ne!(s.query, 0, "PdhOpenQueryW failed");
		assert!(s.eng_counter.is_some(), "GPU Engine counter unavailable");
		s.start();
		std::thread::sleep(std::time::Duration::from_millis(300));
		let map = s.finish();
		// 两次采集后格式化数组应能读出实例（本机有 GPU 活动进程）/
		// After two collects the formatted array should yield instances
		// (this host has GPU-active processes)
		assert!(
			!map.map.is_empty(),
			"GPU Engine counter returned no instances"
		);
	}

	/// Linux fdinfo 扫描冒烟：不 panic 即过。/ Linux fdinfo scan smoke.
	#[test]
	#[cfg(target_os = "linux")]
	fn fdinfo_scan_smoke() {
		let mut s = imp::Sampler::new();
		s.start();
		std::thread::sleep(std::time::Duration::from_millis(100));
		let _map = s.finish();
	}
}
