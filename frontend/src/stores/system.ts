/**
 * 系统信息 Store / System Info Store
 *
 * 存储只读系统信息（如 CPU 核心数），由首个需要它的页面拉取后全局共享。
 * 目前用于动态限制后处理并发数和录制并发数的上限。
 *
 * Stores read-only system info (e.g. CPU count), fetched by the first page that
 * needs it and shared globally. Currently used to dynamically cap the concurrency
 * inputs for post-processing and recording.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { call } from "@/lib/api";

export const useSystemStore = defineStore("system", () => {
	/** CPU 逻辑核心数（0 = 尚未加载）/ Logical CPU count (0 = not yet loaded) */
	const cpuCount = ref(0);
	/** 后处理并发数的输入上限（= cpu * 4）/ Input cap for post-processing concurrency */
	const maxPpConcurrentCap = ref(0);
	/** 录制并发数的输入上限（= cpu * 4）/ Input cap for recording concurrency */
	const maxConcurrentCap = ref(0);

	/**
	 * 从后端拉取系统信息（幂等：已加载则跳过）。
	 * Fetch system info from backend (idempotent: skips if already loaded).
	 */
	async function fetchSystemInfo() {
		if (cpuCount.value > 0) return;
		try {
			const info = await call<{ cpuCount: number; maxPpConcurrentCap: number; maxConcurrentCap: number }>("get_system_info");
			cpuCount.value = info.cpuCount ?? 1;
			maxPpConcurrentCap.value = info.maxPpConcurrentCap ?? cpuCount.value;
			maxConcurrentCap.value = info.maxConcurrentCap ?? cpuCount.value;
		} catch {
			cpuCount.value = 1;
			maxPpConcurrentCap.value = 1;
			maxConcurrentCap.value = 1;
		}
	}

	return { cpuCount, maxPpConcurrentCap, maxConcurrentCap, fetchSystemInfo };
});
