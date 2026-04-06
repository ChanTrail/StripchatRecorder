/**
 * 录制文件管理 Composable / Recording File Management Composable
 *
 * 管理录制文件列表的加载、分组、排序、选择和计时功能。
 * 文件按主播用户名分组，支持多列排序，并为正在录制的文件提供实时计时器。
 *
 * Manages loading, grouping, sorting, selection, and timing of recording files.
 * Files are grouped by streamer username, support multi-column sorting,
 * and provide a real-time timer for actively recording files.
 */

import { ref, computed, type Ref } from "vue";
import { call } from "@/lib/api";
import type { RecordingFile } from "@/types/recordings";
import { ArrowUpDown, ArrowUp, ArrowDown } from "lucide-vue-next";

/** 支持的排序字段 / Supported sort keys */
export type SortKey = "started_at" | "size_bytes" | "video_duration_secs";
/** 排序方向 / Sort direction */
export type SortDir = "asc" | "desc";

/** 按主播分组的录制文件组 / Recording file group by streamer */
export interface Group {
	username: string;
	files: RecordingFile[];
	/** 组内所有文件的总大小（字节）/ Total size of all files in the group (bytes) */
	totalSize: number;
	/** 组内是否有正在录制的文件 / Whether any file in the group is currently recording */
	hasRecording: boolean;
	/** 组内是否有正在合并的文件 / Whether any file in the group is currently merging */
	hasMerging: boolean;
}

/**
 * 从录制文件名中提取主播用户名。
 * 文件名格式为 `{username}_{YYYYMMDD}_{HHmmss}.ext`。
 *
 * Extract the streamer username from a recording filename.
 * Filename format: `{username}_{YYYYMMDD}_{HHmmss}.ext`
 *
 * @param f - 录制文件对象 / Recording file object
 * @returns 主播用户名 / Streamer username
 */
export function usernameFromFile(f: RecordingFile): string {
	const stem = f.name.replace(/\.[^.]+$/, "");
	const parts = stem.split("_");
	// 去掉末尾的日期和时间两段，剩余部分为用户名
	// Remove the last two segments (date and time), the rest is the username
	return parts.slice(0, -2).join("_");
}

/**
 * 录制文件列表状态与操作。
 *
 * @param mergingDirs - 正在合并的会话目录映射 / Map of merging session directories
 * @param isMerging - 判断文件是否正在合并的函数 / Function to check if a file is merging
 * @param waitingMergeDirs - 等待合并的会话目录映射 / Map of waiting-to-merge session directories
 */
export function useRecordings(
	mergingDirs: Ref<Map<string, string>>,
	isMerging: (path: string) => boolean,
	waitingMergeDirs: Ref<Map<string, string>>,
) {
	/** 所有录制文件列表 / All recording files */
	const files = ref<RecordingFile[]>([]);
	/** 是否正在加载 / Whether loading */
	const loading = ref(false);
	/** 各文件的已录制时长（秒，实时递增）/ Elapsed recording duration per file (seconds, increments in real-time) */
	const elapsed = ref<Record<string, number>>({});
	/** 停止录制时冻结的录制时长快照 / Frozen recording duration snapshot when recording stops */
	const frozenDuration = ref<Record<string, number>>({});
	/** 停止录制时冻结的视频时长快照 / Frozen video duration snapshot when recording stops */
	const frozenVideoDuration = ref<Record<string, number>>({});
	/** 已选中的文件路径集合 / Set of selected file paths */
	const selected = ref<Set<string>>(new Set());
	/** 已折叠的分组用户名集合 / Set of collapsed group usernames */
	const collapsedGroups = ref<Set<string>>(new Set());
	/** 当前排序字段 / Current sort key */
	const sortKey = ref<SortKey>("started_at");
	/** 当前排序方向 / Current sort direction */
	const sortDir = ref<SortDir>("desc");

	/** 计时器句柄：每秒递增录制时长 / Timer handle: increments recording duration every second */
	let tickTimer: ReturnType<typeof setInterval> | null = null;
	/** 计时器句柄：防抖刷新文件列表 / Timer handle: debounced file list refresh */
	let dirRefreshTimer: ReturnType<typeof setTimeout> | null = null;

	/**
	 * 切换排序字段，若字段相同则切换排序方向。
	 * Toggle sort key; if the same key, toggle sort direction.
	 *
	 * @param key - 要排序的字段 / Field to sort by
	 */
	function toggleSort(key: SortKey) {
		if (sortKey.value === key) {
			sortDir.value = sortDir.value === "desc" ? "asc" : "desc";
		} else {
			sortKey.value = key;
			sortDir.value = "desc";
		}
	}

	/**
	 * 返回指定排序字段对应的图标组件。
	 * Return the icon component for the given sort key.
	 *
	 * @param key - 排序字段 / Sort key
	 */
	function sortIcon(key: SortKey) {
		if (sortKey.value !== key) return ArrowUpDown;
		return sortDir.value === "desc" ? ArrowDown : ArrowUp;
	}

	/**
	 * 按主播分组的文件列表（计算属性）。
	 * 正在合并的会话目录会被过滤掉，并以虚拟行的形式显示合并目标文件。
	 *
	 * Files grouped by streamer (computed property).
	 * Session directories being merged are filtered out and shown as virtual rows for the target file.
	 */
	const groups = computed<Group[]>(() => {
		const map = new Map<string, RecordingFile[]>();
		const normPath = (p: string) => p.replace(/\\/g, "/");

		// 收集所有合并相关的会话目录（需要从文件列表中过滤掉）
		// Collect all merge-related session dirs (to be filtered from file list)
		const mergingSessionDirs = new Set([
			...[...mergingDirs.value.keys()].map(normPath),
			...[...waitingMergeDirs.value.keys()].map(normPath),
		]);

		// 将非合并会话目录的文件按用户名分组
		// Group non-session-dir files by username
		for (const f of files.value) {
			if (mergingSessionDirs.has(normPath(f.path))) continue;
			const u = usernameFromFile(f);
			if (!map.has(u)) map.set(u, []);
			map.get(u)!.push(f);
		}

		/**
		 * 为合并目标文件添加虚拟行（若目标文件尚未出现在文件列表中）。
		 * Add a virtual row for the merge target file (if not yet in the file list).
		 */
		function addVirtualMergeRow(sessionDir: string, targetPath: string) {
			const username = sessionDir.split(/[\\/]/).slice(-2, -1)[0] ?? "";
			if (!username) return;
			const alreadyPresent = files.value.some((f) => f.path === targetPath);
			if (!alreadyPresent) {
				// 从会话目录名中解析开始时间 / Parse start time from session directory name
				const stem = sessionDir.split(/[\\/]/).pop() ?? "";
				const startedAt = (() => {
					const parts = stem.split("_");
					const date = parts[parts.length - 2];
					const time = parts[parts.length - 1];
					if (date?.length === 8 && time?.length === 6) {
						const s = `${date.slice(0, 4)}-${date.slice(4, 6)}-${date.slice(6, 8)}T${time.slice(0, 2)}:${time.slice(2, 4)}:${time.slice(4, 6)}`;
						return new Date(s).toISOString();
					}
					return new Date().toISOString();
				})();
				if (!map.has(username)) map.set(username, []);
				map.get(username)!.push({
					name: targetPath.split(/[\\/]/).pop() ?? stem,
					path: targetPath,
					size_bytes: 0,
					started_at: startedAt,
					is_recording: false,
					record_duration_secs: null,
					video_duration_secs: null,
				});
			}
		}

		// 为正在合并和等待合并的目录添加虚拟行
		// Add virtual rows for merging and waiting-to-merge directories
		for (const [sessionDir, targetPath] of mergingDirs.value) {
			addVirtualMergeRow(sessionDir, targetPath);
		}
		for (const [sessionDir, targetPath] of waitingMergeDirs.value) {
			addVirtualMergeRow(sessionDir, targetPath);
		}

		// 对每组文件按当前排序字段和方向排序
		// Sort each group's files by current sort key and direction
		const result: Group[] = [];
		for (const [username, list] of map) {
			const sorted = [...list].sort((a, b) => {
				let av: number, bv: number;
				if (sortKey.value === "started_at") {
					av = new Date(a.started_at).getTime();
					bv = new Date(b.started_at).getTime();
				} else if (sortKey.value === "size_bytes") {
					av = a.size_bytes;
					bv = b.size_bytes;
				} else {
					av = a.video_duration_secs ?? 0;
					bv = b.video_duration_secs ?? 0;
				}
				return sortDir.value === "desc" ? bv - av : av - bv;
			});
			result.push({
				username,
				files: sorted,
				totalSize: list.reduce((s, f) => s + f.size_bytes, 0),
				hasRecording: list.some((f) => f.is_recording),
				hasMerging: list.some((f) => isMerging(f.path)),
			});
		}
		// 分组按用户名字母顺序排列 / Sort groups alphabetically by username
		result.sort((a, b) => a.username.localeCompare(b.username));
		return result;
	});

	/** 所有可选中的文件（排除正在录制的）/ All selectable files (excluding actively recording ones) */
	const allSelectableFiles = computed(() =>
		files.value.filter((f) => !f.is_recording),
	);
	/** 已选中文件数量 / Number of selected files */
	const selectedCount = computed(() => selected.value.size);

	/**
	 * 获取单个文件的选中状态。
	 * Get selection state of a single file.
	 */
	function getFileChecked(path: string) {
		return selected.value.has(path);
	}

	/**
	 * 切换单个文件的选中状态。
	 * Toggle selection state of a single file.
	 */
	function setFileChecked(path: string) {
		if (selected.value.has(path)) selected.value.delete(path);
		else selected.value.add(path);
	}

	/**
	 * 获取分组的选中状态（全选/全不选/部分选中）。
	 * Get the selection state of a group (all/none/indeterminate).
	 */
	function getGroupChecked(group: Group): boolean | "indeterminate" {
		const selectable = group.files.filter((f) => !f.is_recording);
		if (selectable.length === 0) return false;
		const n = selectable.filter((f) => selected.value.has(f.path)).length;
		if (n === 0) return false;
		if (n === selectable.length) return true;
		return "indeterminate";
	}

	/**
	 * 切换分组的全选/全不选状态。
	 * Toggle all-selected/all-deselected state for a group.
	 */
	function setGroupChecked(group: Group) {
		const selectable = group.files.filter((f) => !f.is_recording);
		const allSel = selectable.every((f) => selected.value.has(f.path));
		if (allSel) selectable.forEach((f) => selected.value.delete(f.path));
		else selectable.forEach((f) => selected.value.add(f.path));
	}

	/**
	 * 获取全局全选状态（全选/全不选/部分选中）。
	 * Get the global all-select state (all/none/indeterminate).
	 */
	function getAllChecked(): boolean | "indeterminate" {
		const selectable = allSelectableFiles.value;
		if (selectable.length === 0) return false;
		const n = selectable.filter((f) => selected.value.has(f.path)).length;
		if (n === 0) return false;
		if (n === selectable.length) return true;
		return "indeterminate";
	}

	/**
	 * 切换全局全选/全不选状态。
	 * Toggle global all-selected/all-deselected state.
	 */
	function setAllChecked() {
		const selectable = allSelectableFiles.value;
		const allSel = selectable.every((f) => selected.value.has(f.path));
		if (allSel) selectable.forEach((f) => selected.value.delete(f.path));
		else selectable.forEach((f) => selected.value.add(f.path));
	}

	/**
	 * 切换分组的折叠/展开状态。
	 * Toggle the collapsed/expanded state of a group.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	function toggleGroup(username: string) {
		if (collapsedGroups.value.has(username))
			collapsedGroups.value.delete(username);
		else collapsedGroups.value.add(username);
	}

	/**
	 * 从后端加载录制文件列表，并重建计时器状态。
	 * Load recording file list from backend and rebuild timer state.
	 */
	async function load() {
		loading.value = true;
		try {
			files.value = await call<RecordingFile[]>("list_recordings");
			rebuildElapsed();
			// 清理已不存在文件的选中状态 / Clean up selections for files that no longer exist
			const paths = new Set(files.value.map((f) => f.path));
			for (const p of selected.value) {
				if (!paths.has(p)) selected.value.delete(p);
			}
		} finally {
			loading.value = false;
		}
	}

	/**
	 * 重建 elapsed 计时器状态，保留已有的较大值（防止倒退）。
	 * Rebuild elapsed timer state, keeping the larger existing value (prevent regression).
	 */
	function rebuildElapsed() {
		const next: Record<string, number> = {};
		for (const f of files.value) {
			if (f.is_recording) {
				const current = elapsed.value[f.path] ?? 0;
				next[f.path] = Math.max(current, f.record_duration_secs ?? 0);
			}
		}
		elapsed.value = next;
	}

	/**
	 * 启动每秒递增的计时器（用于正在录制的文件）。
	 * Start the per-second increment timer (for actively recording files).
	 */
	function startTick() {
		if (tickTimer) return;
		tickTimer = setInterval(() => {
			for (const path of Object.keys(elapsed.value)) elapsed.value[path]++;
		}, 1000);
	}

	/**
	 * 停止计时器。
	 * Stop the timer.
	 */
	function stopTick() {
		if (tickTimer) {
			clearInterval(tickTimer);
			tickTimer = null;
		}
	}

	/**
	 * 延迟 300ms 后刷新文件列表（防抖，避免目录变更事件频繁触发）。
	 * Debounced file list refresh after 300ms (prevents excessive reloads on directory change events).
	 */
	function scheduleDirRefresh() {
		if (dirRefreshTimer) clearTimeout(dirRefreshTimer);
		dirRefreshTimer = setTimeout(async () => {
			dirRefreshTimer = null;
			await load();
			if (files.value.some((f) => f.is_recording)) startTick();
			else stopTick();
		}, 300);
	}

	/**
	 * 清理所有计时器（组件卸载时调用）。
	 * Clean up all timers (called when component unmounts).
	 */
	function cleanup() {
		stopTick();
		if (dirRefreshTimer) {
			clearTimeout(dirRefreshTimer);
			dirRefreshTimer = null;
		}
	}

	return {
		files,
		loading,
		elapsed,
		frozenDuration,
		frozenVideoDuration,
		selected,
		selectedCount,
		collapsedGroups,
		groups,
		load,
		rebuildElapsed,
		startTick,
		stopTick,
		scheduleDirRefresh,
		cleanup,
		toggleSort,
		sortIcon,
		toggleGroup,
		getFileChecked,
		setFileChecked,
		getGroupChecked,
		setGroupChecked,
		getAllChecked,
		setAllChecked,
	};
}
