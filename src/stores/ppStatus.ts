/**
 * 后处理任务状态管理 Store / Post-processing Task Status Store
 *
 * 将后处理任务的运行状态、进度和模块输出路径提升为全局 Pinia store，
 * 使各页面（如主播列表页）在删除主播时能够取消并清理对应的后处理任务。
 *
 * Elevates post-processing task status, progress, and module output paths to a
 * global Pinia store so that other views (e.g. the streamer list) can cancel and
 * clean up post-processing tasks when a streamer is removed.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import type { PpStatus, PpProgress } from "@/composables/usePostprocess";
import { call } from "@/lib/api";

export const usePpStatusStore = defineStore("ppStatus", () => {
	/** 各文件路径的后处理状态 / Post-processing status per file path */
	const ppStatus = ref<Record<string, PpStatus>>({});

	/** 各文件路径的后处理进度 / Post-processing progress per file path */
	const ppProgress = ref<Record<string, PpProgress>>({});

	/** 各文件路径的模块输出路径 / Module output paths per file path */
	const moduleOutputs = ref<Record<string, Record<string, string>>>({});

	/**
	 * 清除指定文件的所有后处理状态（文件被删除时调用）。
	 * Clear all post-processing state for a specific file (called when file is deleted).
	 */
	function removeFile(path: string) {
		delete ppStatus.value[path];
		delete ppProgress.value[path];
		delete moduleOutputs.value[path];
	}

	/**
	 * 取消并清除指定主播的所有后处理任务（主播被移除时调用）。
	 * Cancel and clear all post-processing tasks for a specific streamer (called when a streamer is removed).
	 *
	 * 通过路径中倒数第二段判断文件是否属于该主播。
	 * Identifies files belonging to the streamer by the second-to-last path segment.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function cancelAndClearForUsername(username: string) {
		const pathsToRemove = Object.keys(ppStatus.value).filter((path) => {
			const parts = path.split(/[\\/]/).filter(Boolean);
			return parts.slice(-2, -1)[0] === username;
		});

		// 并发取消所有正在运行的任务 / Cancel all running tasks concurrently
		await Promise.all(
			pathsToRemove
				.filter(
					(p) =>
						ppStatus.value[p] === "running" ||
						ppStatus.value[p] === "waiting",
				)
				.map((p) => call("cancel_postprocess", { path: p }).catch(() => {})),
		);

		// 清除前端状态 / Clear frontend state
		for (const path of pathsToRemove) {
			removeFile(path);
		}
	}

	return {
		ppStatus,
		ppProgress,
		moduleOutputs,
		removeFile,
		cancelAndClearForUsername,
	};
});
