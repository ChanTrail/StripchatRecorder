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
	import { onMounted, onUnmounted, computed, ref, watchEffect } from "vue";
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
	import { useI18n } from "vue-i18n";

	const { toast, confirm } = useNotify();
	const { t } = useI18n();
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
			title: t("recordings.delete.title"),
			message: t("recordings.delete.message", { name: f.name }),
			confirmText: t("recordings.delete.confirm"),
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
			toast(t("recordings.delete.done", { name: f.name }), "success");
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
			title: t("recordings.delete.batchTitle"),
			message: t("recordings.delete.batchMessage", { count }),
			confirmText: t("recordings.delete.confirm"),
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
		if (failed > 0) toast(t("recordings.delete.batchFailed", { count: failed }), "error");
		else toast(t("recordings.delete.batchDone", { count }), "success");
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

	/** 正在录制的文件数量 / Number of files currently recording */
	const recordingCount = computed(
		() => files.value.filter((f) => f.is_recording).length,
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
					toast(t("recordings.otherClientDeleted", { name }), "info");
				}
			}),
		);

		unlisteners.push(
			await on("recording-file-update", async (payload) => {
				const p = payload as { path: string; size_bytes: number; speed_bps?: number };
				const f = files.value.find((r) => r.path === p.path);
				if (f) {
					if (p.speed_bps != null) {
						recordingSpeed.value = { ...recordingSpeed.value, [p.path]: p.speed_bps };
					}
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
				const prev = ppProgress.value[p.path];
				ppProgress.value[p.path] = makePpProgress(
					p.done,
					p.total,
					p.modDone,
					p.modTotal,
					p.moduleName ?? "",
					p.pct,
					prev?.moduleName ?? "",
					prev?.modulePct ?? 0,
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

	/** 顶部 header 元素引用，用于动态计算表头 sticky 偏移 */
	const headerEl = ref<HTMLElement | null>(null);
	const headerHeight = ref(0);
	let headerRo: ResizeObserver | null = null;
	watchEffect(() => {
		headerRo?.disconnect();
		if (!headerEl.value) return;
		headerRo = new ResizeObserver((entries) => {
			headerHeight.value = entries[0].borderBoxSize[0].blockSize;
		});
		headerRo.observe(headerEl.value);
	});
	onUnmounted(() => headerRo?.disconnect());
</script>

<template>
	<div class="flex flex-col h-full overflow-y-auto">
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
						<Button
							v-if="previewScale !== 1"
							variant="secondary"
							size="sm"
							class="absolute bottom-5 left-1/2 -translate-x-1/2 z-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xs px-3 py-1.5 backdrop-blur-sm"
							@click="resetPreviewTransform"
						>
							{{ t("recordings.resetZoom", { pct: Math.round(previewScale * 100) }) }}
						</Button>
					</Transition>
				</div>
			</DialogContent>
		</Dialog>

		<header ref="headerEl" class="flex items-start justify-between gap-4 shrink-0 px-6 pt-6 pb-4 sticky top-0 z-10 bg-background">
			<div class="flex-1 min-w-0">
				<h1 class="text-xl font-bold mb-0.5">{{ t("recordings.title") }}</h1>
				<div
					class="flex items-center gap-3 text-sm text-muted-foreground flex-wrap"
				>
					<span>{{ t("recordings.subtitle.total", { count: files.length }) }}</span>
					<span v-if="recordingCount > 0" class="text-destructive"
						>{{ t("recordings.subtitle.recording", { count: recordingCount }) }}</span
					>
					<span v-if="selectedCount > 0" class="text-foreground"
						>{{ t("recordings.subtitle.selected", { count: selectedCount }) }}</span
					>
					<span v-if="totalRecordingSpeed > 0">
						{{ t("recordings.subtitle.totalSpeed") }}
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
					{{ t("recordings.batchPostprocess", { count: ppSelectableCount }) }}
				</Button>
				<Button
					v-if="selectedCount > 0"
					variant="destructive"
					size="sm"
					@click="deleteSelected"
				>
					{{ t("recordings.deleteSelected", { count: selectedCount }) }}
				</Button>
				<Button v-if="isTauri" variant="outline" @click="openDir"
					>{{ t("recordings.openDir") }}</Button
				>
			</div>
		</header>

		<div class="px-6 pb-6">
		<div
			v-if="loading && files.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			{{ t("recordings.loading") }}
		</div>
		<div
			v-else-if="files.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			{{ t("recordings.empty") }}
		</div>

		<Table v-else>
			<TableHeader class="sticky z-10 bg-background" :style="{ top: `${headerHeight}px` }">
				<TableRow>
					<TableHead class="w-8">
						<Checkbox
							:model-value="getAllChecked()"
							@update:model-value="setAllChecked"
						/>
					</TableHead>
					<TableHead class="w-px whitespace-nowrap">{{ t("recordings.table.filename") }}</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('size_bytes')"
					>
						{{ t("recordings.table.size") }}
						<component
							:is="sortIcon('size_bytes')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('started_at')"
					>
						{{ t("recordings.table.startTime") }}
						<component
							:is="sortIcon('started_at')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead>{{ t("recordings.table.recordDuration") }}</TableHead>
					<TableHead
						class="cursor-pointer select-none whitespace-nowrap"
						@click="toggleSort('video_duration_secs')"
					>
						{{ t("recordings.table.videoDuration") }}
						<component
							:is="sortIcon('video_duration_secs')"
							class="inline size-3.5 ml-0.5"
						/>
					</TableHead>
					<TableHead>{{ t("recordings.table.speed") }}</TableHead>
					<TableHead class="min-w-45">{{ t("recordings.table.postprocess") }}</TableHead>
					<TableHead>{{ t("recordings.table.actions") }}</TableHead>
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
								>{{ t("recordings.status.recording") }}</Badge
							>
							<span class="ml-2 text-xs text-muted-foreground font-normal">
								{{ t("recordings.group.fileCount", { count: group.files.length }) }} ·
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
											isWaitingMerge(f.path) ? t("recordings.status.waitingMerge") : t("recordings.status.merging")
										}}</Badge>
									</div>
								</TableCell>
								<td colspan="7" class="p-2 align-middle w-full">
									<div class="flex items-center gap-3 h-9 w-full">
										<Loader2
											class="size-4 animate-spin shrink-0 text-muted-foreground"
										/>
										<span class="text-xs text-muted-foreground shrink-0">{{
											isWaitingMerge(f.path) ? t("recordings.status.waitingMergeVideo") : t("recordings.status.mergingVideo")
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
										>{{ t("recordings.status.recording") }}</Badge
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
														? t("recordings.status.overallProgressWithLabel", { label: ppProgress[f.path].moduleExecLabel })
														: t("recordings.status.overallProgress")
												}}</span>
												<span class="tabular-nums shrink-0">{{
													ppProgress[f.path].overallLabel
												}}</span>
											</div>
											<Progress
												:model-value="ppProgress[f.path].overallPct"
												:animated="false"
												class="h-1.5"
											/>
											<div
												class="flex items-center justify-between text-xs text-muted-foreground"
											>
												<span class="truncate max-w-50">{{
													ppProgress[f.path].moduleName === "processing"
														? t("usePostprocess.processing")
														: ppProgress[f.path].moduleName
												}}</span>
												<span class="tabular-nums shrink-0">{{
													ppProgress[f.path].moduleLabel === "waiting"
														? t("usePostprocess.waitingProgress")
														: ppProgress[f.path].moduleLabel
												}}</span>
											</div>
											<Progress
												:model-value="ppProgress[f.path].modulePct"
												:animated="false"
												class="h-1.5"
											/>
										</div>
										<div
											v-else-if="ppStatus[f.path] === 'waiting'"
											class="flex items-center gap-1.5 text-xs text-muted-foreground"
										>
											<Loader2 class="size-3 animate-spin shrink-0" />
											<span>{{ t("recordings.status.waiting") }}</span>
										</div>
										<div
											v-else-if="
												ppStatus[f.path] === 'done' && ppProgress[f.path]
											"
											class="flex flex-col gap-1.5"
										>
											<div class="text-lg text-green-500">{{ t("recordings.status.done") }}</div>
										</div>
										<div
											v-else-if="ppStatus[f.path] === 'error'"
											class="text-lg text-destructive"
										>
											{{ t("recordings.status.failed") }}
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
											:title="f.is_recording ? t('recordings.actions.playDisabled') : ''"
											@click="openFile(f.path)"
											>{{ t("recordings.actions.play") }}</Button
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
											:title="f.is_recording ? t('recordings.status.recording') : ''"
											@click="runPostprocess(f.path)"
										>
											<Loader2
												v-if="ppStatus[f.path] === 'running'"
												class="size-3.5 animate-spin"
											/>
											<span v-else>{{ t("recordings.actions.postprocess") }}</span>
										</Button>
										<Button
											size="sm"
											variant="destructive"
											:disabled="f.is_recording"
											:title="f.is_recording ? t('recordings.actions.deleteDisabled') : ''"
											@click="deleteFile(f)"
											>{{ t("recordings.actions.delete") }}</Button
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

