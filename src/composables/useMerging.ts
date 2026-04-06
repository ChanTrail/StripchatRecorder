/**
 * 视频合并状态管理 Composable / Video Merge State Management Composable
 *
 * 跟踪正在合并和等待合并的录制片段目录，提供合并进度查询和状态管理功能。
 * 录制结束后，多个 TS 片段会被合并为单个 MP4/MKV 文件，此 composable 管理该过程的状态。
 *
 * Tracks session directories that are merging or waiting to merge, provides
 * merge progress queries and state management.
 * After recording ends, multiple TS segments are merged into a single MP4/MKV file;
 * this composable manages the state of that process.
 */

import { ref, computed } from "vue";
import { call } from "@/lib/api";

/**
 * 视频合并状态与操作。
 * Video merge state and operations.
 */
export function useMerging() {
	/** 正在合并的会话目录 -> 目标文件路径 映射 / Map of merging session dir -> target file path */
	const mergingDirs = ref<Map<string, string>>(new Map());

	/** 各会话目录的合并进度（已写入字节 / 总字节）/ Merge progress per session dir (written / total bytes) */
	const mergeProgress = ref<
		Record<string, { out_bytes: number; total_bytes: number }>
	>({});

	/** 等待合并（排队中）的会话目录 -> 目标文件路径 映射 / Map of waiting-to-merge session dir -> target file path */
	const waitingMergeDirs = ref<Map<string, string>>(new Map());

	/** 正在合并的目标文件路径集合（路径统一为正斜杠）/ Set of target paths currently merging (normalized to forward slashes) */
	const mergingTargetPaths = computed(
		() =>
			new Set(
				[...mergingDirs.value.values()].map((p) => p.replace(/\\/g, "/")),
			),
	);

	/** 等待合并的目标文件路径集合 / Set of target paths waiting to merge */
	const waitingMergeTargetPaths = computed(
		() =>
			new Set(
				[...waitingMergeDirs.value.values()].map((p) => p.replace(/\\/g, "/")),
			),
	);

	/**
	 * 判断指定路径的文件是否正在合并（包括等待中）。
	 * Check if the file at the given path is currently merging (including waiting).
	 *
	 * @param path - 文件路径 / File path
	 */
	function isMerging(path: string): boolean {
		const norm = path.replace(/\\/g, "/");
		return (
			mergingTargetPaths.value.has(norm) ||
			waitingMergeTargetPaths.value.has(norm)
		);
	}

	/**
	 * 判断指定路径的文件是否在等待合并队列中。
	 * Check if the file at the given path is in the waiting-to-merge queue.
	 *
	 * @param path - 文件路径 / File path
	 */
	function isWaitingMerge(path: string): boolean {
		return waitingMergeTargetPaths.value.has(path.replace(/\\/g, "/"));
	}

	/**
	 * 获取指定目标文件的合并进度百分比（0-99），未找到返回 null。
	 * Get the merge progress percentage (0-99) for a target file, returns null if not found.
	 *
	 * @param targetPath - 目标合并文件路径 / Target merged file path
	 */
	function getMergeProgress(targetPath: string): number | null {
		const norm = targetPath.replace(/\\/g, "/");
		for (const [sessionDir, tp] of mergingDirs.value) {
			if (tp.replace(/\\/g, "/") === norm) {
				const p = mergeProgress.value[sessionDir];
				if (!p || p.total_bytes === 0) return 0;
				// 最大显示 99%，100% 由文件出现在列表中来表示
				// Cap at 99%; 100% is indicated by the file appearing in the list
				return Math.min(
					99,
					Math.floor((p.out_bytes / p.total_bytes) * 10000) / 100,
				);
			}
		}
		return null;
	}

	/**
	 * 将会话目录标记为正在合并，并从等待队列中移除。
	 * Mark a session directory as actively merging and remove it from the waiting queue.
	 *
	 * @param sessionDir - 录制会话目录路径 / Recording session directory path
	 * @param mergedPath - 合并目标文件路径 / Merged target file path
	 */
	function addMerging(sessionDir: string, mergedPath: string) {
		mergingDirs.value = new Map(mergingDirs.value).set(sessionDir, mergedPath);
		mergeProgress.value[sessionDir] = { out_bytes: 0, total_bytes: 0 };
		const next = new Map(waitingMergeDirs.value);
		next.delete(sessionDir);
		waitingMergeDirs.value = next;
	}

	/**
	 * 将会话目录加入等待合并队列。
	 * Add a session directory to the waiting-to-merge queue.
	 *
	 * @param sessionDir - 录制会话目录路径 / Recording session directory path
	 * @param mergedPath - 合并目标文件路径 / Merged target file path
	 */
	function addWaitingMerge(sessionDir: string, mergedPath: string) {
		waitingMergeDirs.value = new Map(waitingMergeDirs.value).set(
			sessionDir,
			mergedPath,
		);
	}

	/**
	 * 清除指定会话目录的合并状态（合并完成或失败后调用）。
	 * Clear the merge state for a specific session directory (called after merge completes or fails).
	 *
	 * @param sessionDir - 录制会话目录路径 / Recording session directory path
	 */
	function clearMergingForSessionDir(sessionDir: string) {
		const nextMerging = new Map(mergingDirs.value);
		nextMerging.delete(sessionDir);
		mergingDirs.value = nextMerging;
		delete mergeProgress.value[sessionDir];
		const nextWaiting = new Map(waitingMergeDirs.value);
		nextWaiting.delete(sessionDir);
		waitingMergeDirs.value = nextWaiting;
	}

	/**
	 * 清除指定主播的所有合并状态（主播被移除时调用）。
	 * Clear all merge states for a specific streamer (called when a streamer is removed).
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	function clearMergingForUsername(username: string) {
		const next = new Map(mergingDirs.value);
		// 通过路径中倒数第二段判断是否属于该主播
		// Identify streamer by the second-to-last path segment
		for (const [dir] of next) {
			if (dir.split(/[\\/]/).slice(-2, -1)[0] === username) next.delete(dir);
		}
		mergingDirs.value = next;
		for (const dir of Object.keys(mergeProgress.value)) {
			if (!next.has(dir)) delete mergeProgress.value[dir];
		}
		const nextWaiting = new Map(waitingMergeDirs.value);
		for (const [dir] of nextWaiting) {
			if (dir.split(/[\\/]/).slice(-2, -1)[0] === username)
				nextWaiting.delete(dir);
		}
		waitingMergeDirs.value = nextWaiting;
	}

	/**
	 * 从后端恢复合并状态（页面刷新或重连后调用）。
	 * Restore merge state from the backend (called after page refresh or reconnect).
	 */
	async function initFromBackend() {
		try {
			const merging = await call<
				{
					session_dir: string;
					merged_path: string;
					merge_format: string;
					username: string;
					status?: string;
				}[]
			>("get_merging_dirs");
			const nextMerging = new Map(mergingDirs.value);
			const nextWaiting = new Map(waitingMergeDirs.value);
			for (const m of merging) {
				if (m.status === "waiting") {
					nextWaiting.set(m.session_dir, m.merged_path);
				} else {
					nextMerging.set(m.session_dir, m.merged_path);
					mergeProgress.value[m.session_dir] = { out_bytes: 0, total_bytes: 0 };
				}
			}
			mergingDirs.value = nextMerging;
			waitingMergeDirs.value = nextWaiting;
		} catch {
			console.log("Failed to get merging dirs from backend");
		}
	}

	return {
		mergingDirs,
		mergeProgress,
		waitingMergeDirs,
		isMerging,
		isWaitingMerge,
		getMergeProgress,
		addMerging,
		addWaitingMerge,
		clearMergingForUsername,
		clearMergingForSessionDir,
		initFromBackend,
	};
}
