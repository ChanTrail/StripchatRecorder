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
 *
 * 注意：此 composable 现在是全局 useMergingStore 的薄包装，
 * 以保持与现有调用方的接口兼容性。
 *
 * Note: This composable is now a thin wrapper around the global useMergingStore
 * to maintain interface compatibility with existing callers.
 */

import { useMergingStore } from "@/stores/merging";

/**
 * 视频合并状态与操作。
 * Video merge state and operations.
 */
export function useMerging() {
	const store = useMergingStore();
	return {
		mergingDirs: store.mergingDirs,
		mergeProgress: store.mergeProgress,
		waitingMergeDirs: store.waitingMergeDirs,
		isMerging: store.isMerging,
		isWaitingMerge: store.isWaitingMerge,
		getMergeProgress: store.getMergeProgress,
		addMerging: store.addMerging,
		addWaitingMerge: store.addWaitingMerge,
		clearMergingForUsername: store.clearMergingForUsername,
		clearMergingForSessionDir: store.clearMergingForSessionDir,
		initFromBackend: store.initFromBackend,
	};
}
