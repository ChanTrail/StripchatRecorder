<!--
    录制文件管理页面 / Recording File Management View

    展示所有录制文件，按主播分组，支持：
    - 实时录制时长计时和录制速度显示
    - 磁盘空间监控
    - 文件合并进度跟踪
    - 后处理流水线触发和进度显示
    - Contact Sheet 预览图查看（带缩放/平移）
    - 单文件和批量删除
    - 多列排序和分组折叠

    Displays all recording files grouped by streamer, supporting:
    - Real-time recording duration timer and recording speed display
    - Disk space monitoring
    - File merge progress tracking
    - Post-processing pipeline triggering and progress display
    - Contact Sheet preview image viewing (with zoom/pan)
    - Single and batch file deletion
    - Multi-column sorting and group collapsing
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, computed, ref } from "vue";
	import { call, on } from "@/lib/api";
	import { useNotify } from "../composables/useNotify";
	import { usePostprocessStore } from "@/stores/postprocess";
	import { useMerging } from "@/composables/useMerging";
	import { useRecordings, usernameFromFile } from "@/composables/useRecordings";
	import { usePostprocess, makePpProgress } from "@/composables/usePostprocess";
	import { useImagePreview } from "@/composables/useImagePreview";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Loader2, Image } from "lucide-vue-next";
	import { Progress } from "@/components/ui/progress";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow,
	} from "@/components/ui/table";
	import { formatSize, formatDuration } from "@/utils/format";

	const { toast, confirm } = useNotify();
	const ppStore = usePostprocessStore();
	/** 事件取消订阅函数列表 / Event unsubscribe function list */
	const unlisteners: (() => void)[] = [];
	/** 本地已发起删除的文件路径集合（用于过滤 recording-deleted 事件通知）/ Locally deleted paths (to filter recording-deleted notifications) */
	const localDeletedPaths = new Set<string>();

	/** 磁盘空间信息 / Disk space information */
	interface DiskSpace {
		total_bytes: number;
		available_bytes: number;
		used_bytes: number;
	}
	const diskSpace = ref<DiskSpace | null>(null);

	/**
	 * 从后端刷新磁盘空间信息。
	 * Refresh disk space information from the backend.
	 */
	async function refreshDiskSpace() {
		try {
			diskSpace.value = await call<DiskSpace>("get_disk_space");
		} catch {}
	}

	/** 各文件的实时录制速度（字节/秒）/ Real-time recording speed per file (bytes/second) */
	const recordingSpeed = ref<Record<string, number>>({});
	/** 上次文件大小快照 / Last file size snapshot */
	const lastSizeSnapshot = new Map<string, { size: number; time: number }>();
	/** 待处理的文件大小更新 / Pending file size updates */
	const pendingSizeUpdate = new Map<string, { size: number; time: number }>();

	/**
	 * 每 2 秒计算一次各录制文件的录制速度。
	 * Calculate recording speed for each file every 2 seconds.
	 */
	function tickRecordingSpeed() {
		const now = Date.now();
		const updated: Record<string, number> = { ...recordingSpeed.value };
		for (const [path, pending] of pendingSizeUpdate.entries()) {
			const prev = lastSizeSnapshot.get(path);
			if (prev && now > prev.time) {
				const dt = (pending.time - prev.time) / 1000;
				const ds = pending.size - prev.size;
				updated[path] = dt > 0 && ds > 0 ? ds / dt : 0;
			}
			lastSizeSnapshot.set(path, pending);
		}
		pendingSizeUpdate.clear();
		recordingSpeed.value = updated;
	}

	const merging = useMerging();
	const {
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
	} = merging;

	const rec = useRecordings(mergingDirs, isMerging, waitingMergeDirs);
	const {
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
		startTick,
		stopTick,
		scheduleDirRefresh,
		cleanup: recCleanup,
		toggleSort,
		sortIcon,
		toggleGroup,
		getFileChecked,
		setFileChecked,
		getGroupChecked,
		setGroupChecked,
		getAllChecked,
		setAllChecked,
	} = rec;

	const pp = usePostprocess();
	const {
		ppStatus,
		ppProgress,
		moduleOutputs,
		isTauri,
		fetchModuleOutputs,
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile: ppRemoveFile,
	} = pp;

	const preview = useImagePreview();
	const {
		previewOpen,
		previewUrl,
		previewTitle,
		previewScale,
		previewTranslate,
		previewViewportRef,
		previewImageRef,
		isDragging,
		resetPreviewTransform,
		onPreviewImageLoad,
		onPreviewWheel,
		onPreviewMousedown,
		onDocMousemove,
		onDocMouseup,
		openPreview,
	} = preview;

	/**
	 * 用系统默认程序打开录制文件。
	 * Open a recording file with the system default application.
	 */
	async function openFile(path: string) {
		await call("open_recording", { path });
	}

	/**
	 * 打开录制输出目录。
	 * Open the recording output directory.
	 */
	async function openDir() {
		await call("open_output_dir");
	}

	/**
	 * 打开模块输出文件（Tauri 用系统程序，Web 用预览弹窗）。
	 * Open module output file (system app in Tauri, preview dialog in Web).
	 */
	async function openModuleOutput(filePath: string, moduleId: string) {
		const outputPath = moduleOutputs.value[filePath]?.[moduleId];
		if (!outputPath) return;
		if (isTauri) {
			await call("open_recording", { path: outputPath });
		} else {
			openPreview(
				`/api/files?path=${encodeURIComponent(outputPath)}`,
				outputPath.split(/[\\/]/).pop() ?? "预览图",
			);
		}
	}

	/**
	 * 删除单个录制文件（需要用户确认）。
	 * Delete a single recording file (requires user confirmation).
	 */
	async function deleteFile(f: {
		name: string;
		path: string;
		is_recording: boolean;
	}) {
		const ok = await confirm({
			title: "删除录制文件",
			message: `确定要删除 ${f.name} 吗？此操作不可撤销。`,
			confirmText: "删除",
			danger: true,
		});
		if (!ok) return;
		try {
			if (ppStatus.value[f.path] === "running") {
				await call("cancel_postprocess", { path: f.path }).catch(() => {});
			}
			localDeletedPaths.add(f.path);
			await call("delete_recording", { path: f.path });
			files.value = files.value.filter((r) => r.path !== f.path);
			delete elapsed.value[f.path];
			ppRemoveFile(f.path);
			selected.value.delete(f.path);
			toast(`已删除 ${f.name}`, "success");
		} catch (e) {
			localDeletedPaths.delete(f.path);
			toast(String(e), "error");
		}
	}

	/**
	 * 批量删除已选中的文件（需要用户确认）。
	 * Batch delete selected files (requires user confirmation).
	 */
	async function deleteSelected() {
		const paths = [...selected.value];
		const count = paths.length;
		const ok = await confirm({
			title: "批量删除",
			message: `确定要删除选中的 ${count} 个文件吗？此操作不可撤销。`,
			confirmText: "删除",
			danger: true,
		});
		if (!ok) return;
		await Promise.all(
			paths
				.filter((p) => ppStatus.value[p] === "running")
				.map((p) => call("cancel_postprocess", { path: p }).catch(() => {})),
		);
		let failed = 0;
		for (const path of paths) {
			try {
				localDeletedPaths.add(path);
				await call("delete_recording", { path });
				files.value = files.value.filter((r) => r.path !== path);
				delete elapsed.value[path];
				ppRemoveFile(path);
				selected.value.delete(path);
			} catch {
				localDeletedPaths.delete(path);
				failed++;
			}
		}
		if (failed > 0) toast(`删除完成，${failed} 个文件失败`, "error");
		else toast(`已删除 ${count} 个文件`, "success");
	}

	/**
	 * 对所有已选中且符合条件的文件批量触发后处理。
	 * 按录制开始时间排序，确保处理顺序一致。
	 *
	 * Trigger post-processing for all selected eligible files in batch.
	 * Sorted by recording start time to ensure consistent processing order.
	 */
	async function postProcessSelected() {
		const paths = [...selected.value].filter(
			(p) =>
				ppStatus.value[p] !== "running" &&
				ppStatus.value[p] !== "waiting" &&
				!files.value.find((f) => f.path === p)?.is_recording &&
				!isMerging(p),
		);
		if (paths.length === 0) return;
		selected.value.clear();
		paths.sort((a, b) => {
			const fa = files.value.find((f) => f.path === a);
			const fb = files.value.find((f) => f.path === b);
			return (
				new Date(fa?.started_at ?? 0).getTime() -
				new Date(fb?.started_at ?? 0).getTime()
			);
		});
		for (const path of paths) {
			await call("run_postprocess_cmd", { path }).catch((e) => {
				toast(String(e), "error");
			});
		}
	}

	/** 已选中文件中可触发后处理的数量 / Number of selected files eligible for post-processing */
	const ppSelectableCount = computed(
		() =>
			[...selected.value].filter(
				(p) =>
					ppStatus.value[p] !== "running" &&
					ppStatus.value[p] !== "waiting" &&
					!files.value.find((f) => f.path === p)?.is_recording &&
					!isMerging(p),
			).length,
	);

	/** 所有正在录制文件的总录制速度（字节/秒）/ Total recording speed (bytes/second) */
	const totalRecordingSpeed = computed(() =>
		Object.values(recordingSpeed.value).reduce((sum, s) => sum + s, 0),
	);

	/** 磁盘使用率百分比 / Disk usage percentage */
	const diskUsedPct = computed(() => {
		if (!diskSpace.value || diskSpace.value.total_bytes === 0) return 0;
		return Math.min(
			100,
			(diskSpace.value.used_bytes / diskSpace.value.total_bytes) * 100,
		);
	});
	onMounted(async () => {
		document.addEventListener("mousemove", onDocMousemove);
		document.addEventListener("mouseup", onDocMouseup);

		await initFromBackend();
		await load();
		startTick();
		await refreshDiskSpace();
		const diskTimer = setInterval(refreshDiskSpace, 30_000);
		unlisteners.push(() => clearInterval(diskTimer));
		const speedTimer = setInterval(tickRecordingSpeed, 2_000);
		unlisteners.push(() => clearInterval(speedTimer));
		if (!ppStore.pipeline?.nodes?.length) await ppStore.fetchPipeline();
		await restoreFromBackend();

		for (const f of files.value) {
			if (!f.is_recording && !moduleOutputs.value[f.path])
				fetchModuleOutputs(f.path);
		}

		unlisteners.push(
			await on("recordings-dir-changed", () => scheduleDirRefresh()),
		);

		unlisteners.push(
			await on("recording-deleted", (payload) => {
				const p = payload as { path: string };
				const isLocal = localDeletedPaths.has(p.path);
				localDeletedPaths.delete(p.path);
				files.value = files.value.filter((r) => r.path !== p.path);
				delete elapsed.value[p.path];
				ppRemoveFile(p.path);
				selected.value.delete(p.path);
				if (!files.value.some((f) => f.is_recording)) stopTick();
				if (!isLocal) {
					const name = p.path.split(/[\\/]/).pop() ?? p.path;
					toast(`其他客户端删除了录制文件：${name}`, "info");
				}
			}),
		);

		unlisteners.push(
			await on("recording-file-update", async (payload) => {
				const p = payload as { path: string; size_bytes: number };
				const f = files.value.find((r) => r.path === p.path);
				if (f) {
					pendingSizeUpdate.set(p.path, {
						size: p.size_bytes,
						time: Date.now(),
					});
					f.size_bytes = p.size_bytes;
				} else {
					await load();
					startTick();
				}
			}),
		);

		unlisteners.push(
			await on("recording-started", async () => {
				await load();
				startTick();
			}),
		);

		unlisteners.push(
			await on("recording-merge-waiting", async (payload) => {
				const p = payload as {
					username: string;
					session_dir: string;
					merge_format: string;
				};
				const sep = p.session_dir.includes("\\") ? "\\" : "/";
				const parts = p.session_dir.split(sep);
				const stem = parts[parts.length - 1];
				const parent = parts.slice(0, -1).join(sep);
				addWaitingMerge(
					p.session_dir,
					`${parent}${sep}${stem}.${p.merge_format}`,
				);
				await load();
				if (!files.value.some((f) => f.is_recording)) stopTick();
			}),
		);

		unlisteners.push(
			await on("recording-merging", async (payload) => {
				const p = payload as {
					username: string;
					session_dir: string;
					merge_format: string;
				};
				const sep = p.session_dir.includes("\\") ? "\\" : "/";
				const parts = p.session_dir.split(sep);
				const stem = parts[parts.length - 1];
				const parent = parts.slice(0, -1).join(sep);
				addMerging(p.session_dir, `${parent}${sep}${stem}.${p.merge_format}`);
				await load();
				if (!files.value.some((f) => f.is_recording)) stopTick();
			}),
		);

		unlisteners.push(
			await on("merge-progress", (payload) => {
				const p = payload as {
					session_dir: string;
					out_bytes: number;
					total_bytes: number;
				};
				mergeProgress.value[p.session_dir] = {
					out_bytes: p.out_bytes,
					total_bytes: p.total_bytes,
				};
			}),
		);

		unlisteners.push(
			await on("recording-stopped", async (payload) => {
				const p = payload as {
					username: string;
					session_dir?: string;
					record_duration_secs: number | null;
					video_duration_secs: number | null;
				};
				const activeFile = files.value.find(
					(f) => f.is_recording && usernameFromFile(f) === p.username,
				);
				if (activeFile) {
					frozenDuration.value[activeFile.path] =
						p.record_duration_secs ?? elapsed.value[activeFile.path] ?? 0;
					delete recordingSpeed.value[activeFile.path];
					lastSizeSnapshot.delete(activeFile.path);
					pendingSizeUpdate.delete(activeFile.path);
				}
				if (p.session_dir) {
					clearMergingForSessionDir(p.session_dir);
				} else {
					clearMergingForUsername(p.username);
				}
				await load();
				if (activeFile) {
					const sep = activeFile.path.includes("\\") ? "\\" : "/";
					const stem = activeFile.path.split(sep).pop() ?? "";
					const mergedFile = files.value.find(
						(f) =>
							f.name.replace(/\.[^.]+$/, "") === stem &&
							usernameFromFile(f) === p.username,
					);
					if (mergedFile && mergedFile.path !== activeFile.path) {
						if (frozenDuration.value[activeFile.path] != null) {
							frozenDuration.value[mergedFile.path] =
								frozenDuration.value[activeFile.path];
							delete frozenDuration.value[activeFile.path];
						}
						if (p.video_duration_secs != null) {
							frozenVideoDuration.value[mergedFile.path] =
								p.video_duration_secs;
						}
					}
				}
				await restoreFromBackend();
			}),
		);

		unlisteners.push(
			await on("postprocess-waiting", (payload) => {
				const p = payload as { path: string };
				ppStatus.value[p.path] = "waiting";
			}),
		);

		unlisteners.push(
			await on("postprocess-started", (payload) => {
				const p = payload as { path: string };
				ppStatus.value[p.path] = "running";
				ppProgress.value[p.path] = makePpProgress(0, 0, 0, 0, "", 0);
			}),
		);

		unlisteners.push(
			await on("postprocess-progress", (payload) => {
				const p = payload as {
					path: string;
					done: number;
					total: number;
					pct: number;
					modDone: number;
					modTotal: number;
					moduleName: string;
				};
				ppProgress.value[p.path] = makePpProgress(
					p.done,
					p.total,
					p.modDone,
					p.modTotal,
					p.moduleName ?? "",
					p.pct,
				);
			}),
		);

		unlisteners.push(
			await on("postprocess-done", async (payload) => {
				handlePostprocessDone(
					payload as {
						path: string;
						results: { moduleId: string; success: boolean; message: string }[];
					},
					() => load(),
				);
				await restoreFromBackend();
			}),
		);
	});

	onUnmounted(() => {
		document.removeEventListener("mousemove", onDocMousemove);
		document.removeEventListener("mouseup", onDocMouseup);
		recCleanup();
		unlisteners.forEach((fn) => fn());
	});
</script>

<template>
	<div class="flex flex-col gap-5">
		<Dialog :open="previewOpen" @update:open="previewOpen = $event">
			<DialogContent
				class="p-0 overflow-hidden flex flex-col"
				style="width: 90vw; max-width: 90vw; max-height: 90vh; height: 90vh"
			>
				<DialogHeader class="px-4 pt-4 pb-2 shrink-0">
					<DialogTitle class="text-sm font-mono truncate">{{
						previewTitle
					}}</DialogTitle>
				</DialogHeader>
				<div
					ref="previewViewportRef"
					class="relative flex-1 overflow-hidden flex items-center justify-center bg-black/5 px-4 pb-4 min-h-0"
					:style="{
						cursor: isDragging
							? 'grabbing'
							: previewScale > 1
								? 'grab'
								: 'default',
					}"
					@wheel.prevent="onPreviewWheel"
					@mousedown="onPreviewMousedown"
				>
					<img
						ref="previewImageRef"
						:src="previewUrl"
						:alt="previewTitle"
						class="rounded select-none pointer-events-none"
						@load="onPreviewImageLoad"
						:style="{
							maxWidth: '100%',
							maxHeight: '100%',
							transform: `translate(${previewTranslate.x}px, ${previewTranslate.y}px) scale(${previewScale})`,
							transformOrigin: 'center center',
							transition: isDragging ? 'none' : 'transform 0.1s',
						}"
					/>
					<Transition name="fade">
						<button
							v-if="previewScale !== 1"
							class="absolute bottom-5 left-1/2 -translate-x-1/2 z-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xs px-3 py-1.5 backdrop-blur-sm transition-colors"
							@click="resetPreviewTransform"
						>
							{{ Math.round(previewScale * 100) }}% · 重置
						</button>
					</Transition>
				</div>
			</DialogContent>
		</Dialog>

		<header class="flex items-start justify-between gap-4">
			<div class="flex-1 min-w-0">
				<h1 class="text-xl font-bold mb-0.5">录制文件</h1>
				<div
					class="flex items-center gap-3 text-sm text-muted-foreground flex-wrap"
				>
					<span>共 {{ files.length }} 个文件</span>
					<span v-if="selectedCount > 0" class="text-foreground"
						>已选 {{ selectedCount }} 个</span
					>
					<span v-if="totalRecordingSpeed > 0">
						总录制速度
						<span class="text-foreground tabular-nums"
							>{{ formatSize(totalRecordingSpeed) }}/s</span
						>
					</span>
				</div>
				<div v-if="diskSpace" class="mt-2 flex items-center gap-2 max-w-xs">
					<Progress
						:model-value="diskUsedPct"
						class="h-1.5 flex-1"
						:class="
							diskSpace.available_bytes < 5 * 1024 ** 3
								? '[&>div]:bg-destructive'
								: ''
						"
					/>
					<span
						class="text-xs text-muted-foreground whitespace-nowrap tabular-nums"
						:class="
							diskSpace.available_bytes < 5 * 1024 ** 3
								? 'text-destructive'
								: ''
						"
					>
						{{ formatSize(diskSpace.used_bytes) }} /
						{{ formatSize(diskSpace.total_bytes) }}
					</span>
				</div>
			</div>
			<div class="flex gap-2 shrink-0">
				<Button
					v-if="selectedCount > 0"
					variant="outline"
					size="sm"
					:disabled="ppSelectableCount === 0"
					@click="postProcessSelected"
				>
					批量后处理 ({{ ppSelectableCount }})
				</Button>
				<Button
					v-if="selectedCount > 0"
					variant="destructive"
					size="sm"
					@click="deleteSelected"
				>
					删除选中 ({{ selectedCount }})
				</Button>
				<Button v-if="isTauri" variant="outline" @click="openDir"
					>打开目录</Button
				>
			</div>
		</header>

		<div
			v-if="loading && files.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			加载中...
		</div>
		<div
			v-else-if="files.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			暂无录制文件
		</div>

		<Table v-else>
			<TableHeader>
				<TableRow>
					<TableHead class="w-8">
						<Checkbox
							:model-value="getAllChecked()"
							@update:model-value="setAllChecked"
						/>
					</TableHead>
					<TableHead class="w-px whitespace-nowrap">文件名</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('size_bytes')"
					>
						大小
						<component
							:is="sortIcon('size_bytes')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('started_at')"
					>
						起始时间
						<component
							:is="sortIcon('started_at')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead>录制时长</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('video_duration_secs')"
					>
						视频时长
						<component
							:is="sortIcon('video_duration_secs')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead>录制速度</TableHead>
					<TableHead class="min-w-45">后处理</TableHead>
					<TableHead>操作</TableHead>
				</TableRow>
			</TableHeader>
			<TableBody>
				<template v-for="group in groups" :key="group.username">
					<TableRow
						class="bg-muted/40 hover:bg-muted/60 cursor-pointer"
						@click="toggleGroup(group.username)"
					>
						<TableCell class="w-8" @click.stop>
							<Checkbox
								:model-value="getGroupChecked(group)"
								@update:model-value="setGroupChecked(group)"
							/>
						</TableCell>
						<TableCell colspan="7" class="font-semibold">
							<span class="mr-2 text-muted-foreground text-xs">{{
								collapsedGroups.has(group.username) ? "▶" : "▼"
							}}</span>
							{{ group.username }}
							<Badge
								v-if="group.hasRecording"
								variant="destructive"
								class="ml-2 text-[10px]"
								>录制中</Badge
							>
							<span class="ml-2 text-xs text-muted-foreground font-normal">
								{{ group.files.length }} 个文件 ·
								{{ formatSize(group.totalSize) }}
							</span>
						</TableCell>
						<TableCell />
					</TableRow>

					<template v-if="!collapsedGroups.has(group.username)">
						<TableRow v-for="f in group.files" :key="f.path" class="relative">
							<template v-if="isMerging(f.path)">
								<TableCell class="w-8">
									<Checkbox :model-value="false" :disabled="true" />
								</TableCell>
								<TableCell class="font-medium w-px whitespace-nowrap pl-7">
									<div class="flex items-center gap-1.5">
										<span>{{ f.name }}</span>
										<Badge variant="outline" class="text-[10px] shrink-0">{{
											isWaitingMerge(f.path) ? "等待合并" : "合并中"
										}}</Badge>
									</div>
								</TableCell>
								<td colspan="7" class="p-2 align-middle w-full">
									<div class="flex items-center gap-3 h-9 w-full">
										<Loader2
											class="size-4 animate-spin shrink-0 text-muted-foreground"
										/>
										<span class="text-xs text-muted-foreground shrink-0">{{
											isWaitingMerge(f.path) ? "等待合并视频…" : "正在合并视频…"
										}}</span>
										<template v-if="!isWaitingMerge(f.path)">
											<div
												class="flex-1 bg-muted rounded-full h-1.5 overflow-hidden"
											>
												<div
													class="h-full bg-primary rounded-full transition-all duration-500"
													:style="{
														width: `${getMergeProgress(f.path) ?? 0}%`,
													}"
												/>
											</div>
											<span
												class="tabular-nums text-xs text-muted-foreground w-14 shrink-0"
												>{{ (getMergeProgress(f.path) ?? 0).toFixed(2) }}%</span
											>
										</template>
									</div>
								</td>
							</template>

							<template v-else>
								<TableCell class="w-8">
									<Checkbox
										:model-value="getFileChecked(f.path)"
										:disabled="f.is_recording"
										@update:model-value="setFileChecked(f.path)"
									/>
								</TableCell>
								<TableCell class="font-medium w-px whitespace-nowrap pl-7">
									{{ f.name }}
									<Badge
										v-if="f.is_recording"
										variant="destructive"
										class="ml-1.5 text-[10px]"
										>录制中</Badge
									>
								</TableCell>
								<TableCell class="tabular-nums">{{
									formatSize(f.size_bytes)
								}}</TableCell>
								<TableCell class="tabular-nums text-muted-foreground">{{
									new Date(f.started_at).toLocaleString()
								}}</TableCell>
								<TableCell class="tabular-nums">
									<span v-if="f.is_recording" class="text-destructive">{{
										formatDuration(elapsed[f.path] ?? 0)
									}}</span>
									<span
										v-else-if="frozenDuration[f.path] != null"
										class="text-muted-foreground"
										>{{ formatDuration(frozenDuration[f.path]) }}</span
									>
									<span v-else class="text-muted-foreground">—</span>
								</TableCell>
								<TableCell class="tabular-nums">
									<span v-if="f.video_duration_secs != null">{{
										formatDuration(f.video_duration_secs)
									}}</span>
									<span v-else-if="frozenVideoDuration[f.path] != null">{{
										formatDuration(frozenVideoDuration[f.path])
									}}</span>
									<span v-else class="text-muted-foreground">—</span>
								</TableCell>
								<TableCell class="tabular-nums">
									<span
										v-if="f.is_recording && recordingSpeed[f.path] != null"
										class="text-xs"
									>
										{{ formatSize(recordingSpeed[f.path]) }}/s
									</span>
									<span v-else class="text-muted-foreground">—</span>
								</TableCell>
								<TableCell class="min-w-45">
									<div v-if="!f.is_recording">
										<div
											v-if="
												ppStatus[f.path] === 'running' && ppProgress[f.path]
											"
											class="flex flex-col gap-1.5"
										>
											<div
												class="flex items-center justify-between text-xs text-muted-foreground"
											>
												<span>{{
													ppProgress[f.path].moduleExecLabel
														? `${ppProgress[f.path].moduleExecLabel} 总进度`
														: "总进度"
												}}</span>
												<span class="tabular-nums shrink-0">{{
													ppProgress[f.path].overallLabel
												}}</span>
											</div>
											<Progress
												:model-value="ppProgress[f.path].overallPct"
												class="h-1.5"
											/>
											<div
												class="flex items-center justify-between text-xs text-muted-foreground"
											>
												<span class="truncate max-w-45">{{
													ppProgress[f.path].moduleName
												}}</span>
												<span class="tabular-nums shrink-0">{{
													ppProgress[f.path].moduleLabel
												}}</span>
											</div>
											<Progress
												:model-value="ppProgress[f.path].modulePct"
												class="h-1.5"
											/>
										</div>
										<div
											v-else-if="ppStatus[f.path] === 'waiting'"
											class="flex items-center gap-1.5 text-xs text-muted-foreground"
										>
											<Loader2 class="size-3 animate-spin shrink-0" />
											<span>等待中…</span>
										</div>
										<div
											v-else-if="
												ppStatus[f.path] === 'done' && ppProgress[f.path]
											"
											class="flex flex-col gap-1.5"
										>
											<div class="text-lg text-green-500">已完成</div>
										</div>
										<div
											v-else-if="ppStatus[f.path] === 'error'"
											class="text-lg text-destructive"
										>
											失败
										</div>
										<span v-else class="text-xs text-muted-foreground">—</span>
									</div>
									<span v-else class="text-xs text-muted-foreground">—</span>
								</TableCell>
								<TableCell>
									<div class="flex gap-1.5">
										<Button
											size="sm"
											variant="outline"
											:disabled="f.is_recording"
											:title="f.is_recording ? '录制中，无法播放' : ''"
											@click="openFile(f.path)"
											>播放</Button
										>
										<Button
											v-if="moduleOutputs[f.path]?.['contact_sheet']"
											size="sm"
											variant="outline"
											title="查看 Contact Sheet 预览图"
											@click="openModuleOutput(f.path, 'contact_sheet')"
										>
											<Image class="size-3.5" />
										</Button>
										<Button
											size="sm"
											variant="outline"
											:disabled="
												f.is_recording ||
												ppStatus[f.path] === 'running' ||
												ppStatus[f.path] === 'waiting'
											"
											:title="f.is_recording ? '录制中' : ''"
											@click="runPostprocess(f.path)"
										>
											<Loader2
												v-if="ppStatus[f.path] === 'running'"
												class="size-3.5 animate-spin"
											/>
											<span v-else>后处理</span>
										</Button>
										<Button
											size="sm"
											variant="destructive"
											:disabled="f.is_recording"
											:title="f.is_recording ? '文件正在录制中' : ''"
											@click="deleteFile(f)"
											>删除</Button
										>
									</div>
								</TableCell>
							</template>
						</TableRow>
					</template>
				</template>
			</TableBody>
		</Table>
	</div>
</template>

<style scoped>
	.fade-enter-active,
	.fade-leave-active {
		transition: opacity 0.15s;
	}
	.fade-enter-from,
	.fade-leave-to {
		opacity: 0;
	}
</style>

