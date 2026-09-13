<!--
    录制文件管理页面 / Recording File Management View

    与 frontend 版对齐：
    - 拆分 RecordingRow / PostprocessProgressCell / SegmentStatsBadges 子组件
    - 磁盘空间升级为多分区结构（DiskSpaceEntry[]，含 label/path）
    - hasPipelineNodes 判断（流水线是否有连接到输入节点的启用节点）
    - isMobile 移动端布局
    - countPipelineTotal / ppProgressFromMeta 进度计算
    - postprocess-meta-update 事件处理（路径迁移兜底）

    Desktop 差异：
    - openModuleOutput 通过 open_output_file 命令经 meta 解析路径后调用系统 opener（同 open_recording），
      无需 base64 转换，也不使用 ImagePreviewDialog
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, computed, ref, watchEffect } from "vue";
	import { call, on } from "@/lib/api";
	import { useNotify } from "../composables/useNotify";
	import { usePostprocessStore, countPipelineTotal } from "@/stores/postprocess";
	import { useRecordings } from "@/composables/useRecordings";
	import { usePostprocess, ppProgressFromMeta } from "@/composables/usePostprocess";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Tooltip } from "@/components/ui/tooltip";
	import { Progress } from "@/components/ui/progress";
	import {
		Table, TableBody, TableHead, TableHeader, TableRow, TableCell,
	} from "@/components/ui/table";
	import { ChevronRight, ChevronDown, Image, Loader2, FolderOpen } from "@lucide/vue";
	import RecordingRow from "@/components/RecordingRow.vue";
	import SegmentStatsBadges from "@/components/SegmentStatsBadges.vue";
	import PostprocessProgressCell from "@/components/PostprocessProgressCell.vue";
	import { formatSize, formatDuration } from "@/utils/format";
	import { useI18n } from "vue-i18n";
	import { useMobileLayout } from "@/composables/useMobileLayout";

	const { toast, confirm } = useNotify();
	const { t } = useI18n();
	const ppStore = usePostprocessStore();
	const { isMobile } = useMobileLayout();
	const unlisteners: (() => void)[] = [];
	const localDeletedPaths = new Set<string>();
	const ppCancelledByDelete = new Set<string>();

	/** 磁盘空间条目 / Disk space entry */
	interface DiskSpaceEntry {
		label: string;
		path: string;
		total_bytes: number;
		available_bytes: number;
		used_bytes: number;
	}
	const diskSpaces = ref<DiskSpaceEntry[]>([]);

	async function refreshDiskSpace() {
		try {
			diskSpaces.value = await call<DiskSpaceEntry[]>("get_disk_space");
		} catch {}
	}

	const recordingSpeed = ref<Record<string, number>>({});
	const segmentStats = ref<Record<string, { downloaded: number; failed: number }>>({});

	const rec = useRecordings();
	const {
		files, loading, elapsed, selected, selectedCount,
		collapsedGroups, groups, load, startTick, stopTick,
		scheduleDirRefresh, cleanup: recCleanup, toggleSort, sortIcon,
		toggleGroup, getFileChecked, setFileChecked,
		getGroupChecked, setGroupChecked, getAllChecked, setAllChecked,
	} = rec;

	const pp = usePostprocess();
	const {
		ppStatus, ppProgress, moduleOutputs, runPostprocess,
		restoreFromBackend, handlePostprocessDone, removeFile: ppRemoveFile,
	} = pp;

	function syncModuleOutputsFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				moduleOutputs.value[f.path] = { ...moduleOutputs.value[f.path], ...f.module_outputs };
			}
		}
	}

	function syncPpStateFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.status === "finish")          ppStatus.value[f.path] = "done";
			else if (f.status === "pp_error")   ppStatus.value[f.path] = "error";
			else if (f.status === "pp_waiting") { if (ppStatus.value[f.path] !== "running") ppStatus.value[f.path] = "waiting"; }
			else if (f.status === "pp_running") ppStatus.value[f.path] = "running";

			if (f.pp_execution && f.pp_execution.length > 0) {
				ppProgress.value[f.path] = ppProgressFromMeta(
					f.pp_execution, f.pp_progress, countPipelineTotal(ppStore.pipeline),
					{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				);
			}
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				moduleOutputs.value[f.path] = f.module_outputs;
			}
		}
	}


	async function openFile(path: string) {
		await call("open_recording", { path });
	}

	/**
	 * 在文件管理器中打开合并后视频文件夹。
	 * ts_merge 配置了 output_dir → 打开该目录；否则打开 settings.output_dir。
	 *
	 * Open the merged video folder in the file manager.
	 * Uses ts_merge's output_dir if set, otherwise falls back to settings.output_dir.
	 */
	async function openMergedDir() {
		try {
			await call("open_merged_dir");
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 用系统默认程序打开模块输出文件（如 contact_sheet 预览图）。
	 * 后端通过 video_path + module_id 经 meta 解析出真实路径后调用 opener，
	 * 与 open_recording 保持一致，无需 base64 转换或内嵌预览弹窗。
	 *
	 * Open a module output file with the system default application.
	 * The backend resolves the real path via meta from video_path + module_id,
	 * consistent with open_recording — no base64 conversion or preview dialog needed.
	 */
	async function openModuleOutput(filePath: string, moduleId: string) {
		try {
			await call("open_output_file", { videoPath: filePath, moduleId });
		} catch (e) {
			toast(String(e), "error");
		}
	}

	async function deleteFile(f: { name: string; path: string; is_recording: boolean }) {
		const ok = await confirm({
			title: t("recordings.delete.title"),
			message: t("recordings.delete.message", { name: f.name }),
			confirmText: t("recordings.delete.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			if (ppStatus.value[f.path] === "running") {
				ppCancelledByDelete.add(f.path);
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
			paths.filter((p) => ppStatus.value[p] === "running").map((p) => {
				ppCancelledByDelete.add(p);
				return call("cancel_postprocess", { path: p }).catch(() => {});
			}),
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

	async function postProcessSelected() {
		if (!hasPipelineNodes.value) {
			toast(t("recordings.postprocessEmptyPipeline"), "error");
			return;
		}
		const paths = [...selected.value].filter(
			(p) =>
				ppStatus.value[p] !== "running" &&
				ppStatus.value[p] !== "waiting" &&
				!files.value.find((f) => f.path === p)?.is_recording,
		);
		if (paths.length === 0) return;
		selected.value.clear();
		paths.sort((a, b) => {
			const fa = files.value.find((f) => f.path === a);
			const fb = files.value.find((f) => f.path === b);
			return new Date(fa?.started_at ?? 0).getTime() - new Date(fb?.started_at ?? 0).getTime();
		});
		for (const path of paths) {
			await call("run_postprocess_cmd", { path }).catch((e) => { toast(String(e), "error"); });
		}
	}

	const ppSelectableCount = computed(
		() => [...selected.value].filter(
			(p) =>
				ppStatus.value[p] !== "running" &&
				ppStatus.value[p] !== "waiting" &&
				!files.value.find((f) => f.path === p)?.is_recording,
		).length,
	);

	const hasPipelineNodes = computed(
		() => ppStore.pipeline?.nodes?.some(
			(n) => n.enabled && Object.values(n.inputs ?? {}).some((ref) => ref.nodeId === "0"),
		) ?? false,
	);

	const totalRecordingSpeed = computed(() =>
		Object.values(recordingSpeed.value).reduce((sum, s) => sum + s, 0),
	);

	const recordingCount = computed(() => files.value.filter((f) => f.is_recording).length);

	function diskUsedPct(entry: DiskSpaceEntry) {
		if (entry.total_bytes === 0) return 0;
		return Math.min(100, (entry.used_bytes / entry.total_bytes) * 100);
	}

	function diskColorTier(entry: DiskSpaceEntry): "warn" | "danger" | "" {
		if (entry.total_bytes === 0) return "";
		const pct = (entry.used_bytes / entry.total_bytes) * 100;
		if (pct >= 80) return "danger";
		if (pct >= 50) return "warn";
		return "";
	}

	onMounted(async () => {
		await load();
		startTick();
		await refreshDiskSpace();
		const diskTimer = setInterval(refreshDiskSpace, 30_000);
		unlisteners.push(() => clearInterval(diskTimer));
		if (!ppStore.pipeline?.nodes?.length) await ppStore.fetchPipeline();

		ppStore.initModuleWatcher(() => syncModuleOutputsFromFiles());
		await restoreFromBackend();
		syncPpStateFromFiles();

		unlisteners.push(await on("recordings-dir-changed", () => scheduleDirRefresh(syncModuleOutputsFromFiles)));

		unlisteners.push(await on("sse-lagged", async () => {
			await load();
			await restoreFromBackend();
			syncPpStateFromFiles();
		}));

		unlisteners.push(await on("recording-deleted", (payload) => {
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
		}));

		unlisteners.push(await on("recording-file-update", async (payload) => {
			const p = payload as {
				path: string; size_bytes: number; speed_bps?: number;
				segments_downloaded?: number; segments_failed?: number;
			};
			const f = files.value.find((r) => r.path === p.path);
			if (f) {
				if (p.speed_bps != null && f.is_recording) {
					recordingSpeed.value = { ...recordingSpeed.value, [p.path]: p.speed_bps };
				} else if (!f.is_recording) {
					delete recordingSpeed.value[p.path];
				}
				f.size_bytes = p.size_bytes;
				if (f.is_recording && (p.segments_downloaded != null || p.segments_failed != null)) {
					segmentStats.value = {
						...segmentStats.value,
						[p.path]: {
							downloaded: p.segments_downloaded ?? segmentStats.value[p.path]?.downloaded ?? 0,
							failed: p.segments_failed ?? segmentStats.value[p.path]?.failed ?? 0,
						},
					};
				}
			} else {
				await load(); startTick(); syncModuleOutputsFromFiles();
			}
		}));

		unlisteners.push(await on("recording-started", async () => {
			await load(); startTick(); syncModuleOutputsFromFiles();
		}));

		unlisteners.push(await on("recording-stopped", async (payload) => {
			const p = payload as { video_path?: string };
			await load();
			syncModuleOutputsFromFiles();
			if (p.video_path) {
				const ns = { ...recordingSpeed.value }; delete ns[p.video_path]; recordingSpeed.value = ns;
				const ss = { ...segmentStats.value }; delete ss[p.video_path]; segmentStats.value = ss;
			}
		}));

		unlisteners.push(await on("postprocess-waiting", (payload) => {
			const p = payload as { path: string };
			ppStatus.value[p.path] = "waiting";
		}));

		unlisteners.push(await on("postprocess-started", (payload) => {
			const p = payload as { path: string };
			ppStatus.value[p.path] = "running";
		}));

		unlisteners.push(await on("postprocess-meta-update", (payload) => {
			const p = payload as {
				path: string;
				meta: {
					pp_execution?: import("@/types/recordings").PpExecutionEntry[] | null;
					pp_progress?: import("@/types/recordings").PpNodeProgress | null;
				};
				module_outputs?: Record<string, string>;
			};
			if (!p.meta) return;

			if (!ppStatus.value[p.path]) {
				const currentPaths = new Set(files.value.map((f) => f.path));
				const oldPath = Object.keys(ppStatus.value).find(
					(path) => ppStatus.value[path] === "running" && !currentPaths.has(path)
				);
				if (oldPath) {
					ppStatus.value[p.path] = ppStatus.value[oldPath];
					if (ppProgress.value[oldPath]) ppProgress.value[p.path] = ppProgress.value[oldPath];
					if (moduleOutputs.value[oldPath]) moduleOutputs.value[p.path] = moduleOutputs.value[oldPath];
					delete ppStatus.value[oldPath];
					delete ppProgress.value[oldPath];
					delete moduleOutputs.value[oldPath];
				}
			}

			ppProgress.value[p.path] = ppProgressFromMeta(
				p.meta.pp_execution, p.meta.pp_progress, countPipelineTotal(ppStore.pipeline),
				{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				ppStore.pipeline?.nodes
					? new Set(ppStore.pipeline.nodes
						.filter((n) => n.enabled && !n.moduleId.includes("__builtin__"))
						.map((n) => n.nodeId ?? n.moduleId))
					: undefined,
			);
			if (p.module_outputs && Object.keys(p.module_outputs).length > 0) {
				moduleOutputs.value = {
					...moduleOutputs.value,
					[p.path]: { ...moduleOutputs.value[p.path], ...p.module_outputs },
				};
			}
		}));

		unlisteners.push(await on("postprocess-done", async (payload) => {
			const p = payload as { path: string; success: boolean; message?: string };
			const wasCancelledByDelete = ppCancelledByDelete.has(p.path);
			ppCancelledByDelete.delete(p.path);
			handlePostprocessDone(
				p,
				async () => { await load(); syncPpStateFromFiles(); syncModuleOutputsFromFiles(); },
				() => wasCancelledByDelete,
			);
		}));
	});

	onUnmounted(() => {
		recCleanup();
		unlisteners.forEach((fn) => fn());
	});

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
	<div class="flex flex-col h-full gap-0">

		<header
			ref="headerEl"
			class="flex items-start justify-between gap-4 shrink-0 pb-4 bg-background sticky top-0 z-20 px-4 pt-5 border-b"
		>
			<div class="flex-1 min-w-0">
				<h1 class="text-xl font-bold mb-0.5">{{ t("recordings.title") }}</h1>
				<div class="flex items-center gap-3 text-sm text-muted-foreground flex-wrap">
					<span>{{ t("recordings.subtitle.total", { count: files.length }) }}</span>
					<span v-if="recordingCount > 0" class="text-destructive">
						{{ t("recordings.subtitle.recording", { count: recordingCount }) }}
					</span>
					<span v-if="selectedCount > 0" class="text-foreground">
						{{ t("recordings.subtitle.selected", { count: selectedCount }) }}
					</span>
					<span v-if="totalRecordingSpeed > 0">
						{{ t("recordings.subtitle.totalSpeed") }}
						<span class="text-foreground tabular-nums">{{ formatSize(totalRecordingSpeed) }}/s</span>
					</span>
				</div>

				<!-- 多磁盘分区 / Multiple disk partitions -->
				<div class="mt-2 flex gap-3" :class="isMobile ? 'flex-col' : 'flex-row'">
					<div
						v-for="entry in diskSpaces"
						:key="entry.label"
						class="flex flex-col gap-1"
						:class="isMobile ? '' : 'min-w-40 max-w-56 flex-1'"
					>
						<div class="flex items-center justify-between gap-2">
							<span class="text-xs text-muted-foreground whitespace-nowrap">
								{{ t(`recordings.diskLabel.${entry.label}`) }}
							</span>
							<span
								class="text-xs text-muted-foreground whitespace-nowrap tabular-nums"
								:class="{
									'text-destructive': diskColorTier(entry) === 'danger',
									'text-yellow-500': diskColorTier(entry) === 'warn',
								}"
							>
								{{ formatSize(entry.used_bytes) }} / {{ formatSize(entry.total_bytes) }}
							</span>
						</div>
						<Progress
							:model-value="diskUsedPct(entry)"
							class="h-1.5"
							:class="{
								'[&>div]:bg-destructive': diskColorTier(entry) === 'danger',
								'[&>div]:bg-yellow-500': diskColorTier(entry) === 'warn',
							}"
						/>
					</div>
				</div>
			</div>

			<div class="flex gap-2 shrink-0" :class="isMobile ? 'flex-col items-end' : ''">
				<Button variant="outline" size="sm" @click="openMergedDir">
					<FolderOpen class="size-3.5 mr-1.5" />
					{{ t("recordings.openDir") }}
				</Button>
				<Tooltip
					v-if="selectedCount > 0"
					:content="
						!hasPipelineNodes
							? t('recordings.postprocessEmptyPipeline')
							: ppSelectableCount === 0
								? t('recordings.postprocessNoneSelectable')
								: undefined
					"
				>
					<Button
						variant="outline" size="sm"
						:disabled="ppSelectableCount === 0 || !hasPipelineNodes"
						@click="postProcessSelected"
					>
						{{ t("recordings.batchPostprocess", { count: ppSelectableCount }) }}
					</Button>
				</Tooltip>
				<Button v-if="selectedCount > 0" variant="destructive" size="sm" @click="deleteSelected">
					{{ t("recordings.deleteSelected", { count: selectedCount }) }}
				</Button>
			</div>
		</header>

		<div class="px-4 flex-1 overflow-y-auto">
			<div v-if="loading && files.length === 0" class="text-center text-muted-foreground py-16">
				{{ t("recordings.loading") }}
			</div>
			<div v-else-if="files.length === 0" class="text-center text-muted-foreground py-16">
				{{ t("recordings.empty") }}
			</div>

			<!-- 桌面端表格 / Desktop table -->
			<Table v-else-if="!isMobile">
				<TableHeader
					class="sticky top-0 z-10 bg-background"
				>
					<TableRow>
						<TableHead class="w-8">
							<Checkbox :model-value="getAllChecked()" @update:model-value="setAllChecked" />
						</TableHead>
						<TableHead class="w-px whitespace-nowrap">{{ t("recordings.table.filename") }}</TableHead>
						<TableHead class="cursor-pointer select-none whitespace-nowrap" @click="toggleSort('size_bytes')">
							{{ t("recordings.table.size") }}
							<component :is="sortIcon('size_bytes')" class="inline size-3.5 ml-0.5" />
						</TableHead>
						<TableHead class="cursor-pointer select-none whitespace-nowrap" @click="toggleSort('started_at')">
							{{ t("recordings.table.startTime") }}
							<component :is="sortIcon('started_at')" class="inline size-3.5 ml-0.5" />
						</TableHead>
						<TableHead>{{ t("recordings.table.recordDuration") }}</TableHead>
						<TableHead class="cursor-pointer select-none whitespace-nowrap" @click="toggleSort('video_duration_secs')">
							{{ t("recordings.table.videoDuration") }}
							<component :is="sortIcon('video_duration_secs')" class="inline size-3.5 ml-0.5" />
						</TableHead>
						<TableHead class="whitespace-nowrap">{{ t("recordings.table.resolution") }}</TableHead>
						<TableHead>{{ t("recordings.table.speed") }}</TableHead>
						<TableHead>{{ t("recordings.table.segments") }}</TableHead>
						<TableHead class="min-w-45">{{ t("recordings.table.postprocess") }}</TableHead>
						<TableHead>{{ t("recordings.table.actions") }}</TableHead>
					</TableRow>
				</TableHeader>
				<TableBody>
					<template v-for="group in groups" :key="group.username">
						<TableRow class="bg-muted/40 hover:bg-muted/60 cursor-pointer" @click="toggleGroup(group.username)">
							<TableCell class="w-8" @click.stop>
								<Checkbox :model-value="getGroupChecked(group)" @update:model-value="setGroupChecked(group)" />
							</TableCell>
							<TableCell colspan="9" class="font-semibold">
								<component
									:is="collapsedGroups.has(group.username) ? ChevronRight : ChevronDown"
									class="inline-block size-3.5 mr-1.5 text-muted-foreground align-middle"
								/>
								{{ group.username }}
								<Badge v-if="group.hasRecording" variant="destructive" class="ml-2 text-[10px]">
									{{ t("recordings.status.recording") }}
								</Badge>
								<span class="ml-2 text-xs text-muted-foreground font-normal">
									{{ t("recordings.group.fileCount", { count: group.files.length }) }}
									· {{ formatSize(group.totalSize) }}
								</span>
							</TableCell>
							<TableCell />
						</TableRow>

						<template v-if="!collapsedGroups.has(group.username)">
							<RecordingRow
								v-for="f in group.files"
								:key="f.path"
								:file="f"
								:checked="getFileChecked(f.path)"
								:elapsed-secs="elapsed[f.path] ?? 0"
								:speed-bps="recordingSpeed[f.path] ?? null"
								:segment-stats="segmentStats[f.path] ?? null"
								:pp-status="ppStatus[f.path]"
								:pp-progress="ppProgress[f.path]"
								:has-contact-sheet="!!moduleOutputs[f.path]?.['contact_sheet']"
								:has-pipeline-nodes="hasPipelineNodes"
								@toggle-checked="setFileChecked(f.path)"
								@open="openFile(f.path)"
								@open-contact-sheet="openModuleOutput(f.path, 'contact_sheet')"
								@run-postprocess="runPostprocess(f.path)"
								@delete="deleteFile(f)"
							/>
						</template>
					</template>
				</TableBody>
			</Table>

			<!-- 移动端卡片列表 / Mobile card list -->
			<div v-else class="flex flex-col gap-3 py-3">
				<!-- 全选行 / Select-all row -->
				<div class="flex items-center gap-3 px-1 pb-1 border-b">
					<Checkbox :model-value="getAllChecked()" @update:model-value="setAllChecked" />
					<span class="text-sm text-muted-foreground">
						{{ selectedCount > 0
							? t("recordings.subtitle.selected", { count: selectedCount })
							: t("recordings.subtitle.total", { count: files.length }) }}
					</span>
				</div>

				<template v-for="group in groups" :key="group.username">
					<!-- 分组标题 / Group header -->
					<div class="flex items-center gap-2 px-1 py-1">
						<Checkbox
							:model-value="getGroupChecked(group)"
							@update:model-value="setGroupChecked(group)"
							@click.stop
						/>
						<button
							class="flex items-center gap-2 flex-1 text-left min-w-0"
							@click="toggleGroup(group.username)"
						>
							<component
								:is="collapsedGroups.has(group.username) ? ChevronRight : ChevronDown"
								class="size-4 text-muted-foreground shrink-0"
							/>
							<span class="font-semibold text-sm truncate">{{ group.username }}</span>
							<Badge v-if="group.hasRecording" variant="destructive" class="text-[10px] shrink-0">
								{{ t("recordings.status.recording") }}
							</Badge>
						</button>
						<span class="text-xs text-muted-foreground shrink-0">
							{{ t("recordings.group.fileCount", { count: group.files.length }) }}
							· {{ formatSize(group.totalSize) }}
						</span>
					</div>

					<template v-if="!collapsedGroups.has(group.username)">
						<div
							v-for="f in group.files"
							:key="f.path"
							class="rounded-lg border bg-card px-4 py-3 flex flex-col gap-2"
						>
							<!-- 文件名 + 录制角标 -->
							<div class="flex items-start gap-2">
								<Checkbox
									:model-value="getFileChecked(f.path)"
									:disabled="f.is_recording"
									class="mt-0.5 shrink-0"
									@update:model-value="setFileChecked(f.path)"
								/>
								<div class="flex-1 min-w-0">
									<span class="text-sm font-medium break-all leading-snug">{{ f.name }}</span>
									<Badge
										v-if="f.is_recording"
										variant="destructive"
										class="ml-1.5 text-[10px] align-middle"
									>{{ t("recordings.status.recording") }}</Badge>
								</div>
							</div>

							<!-- 元数据行 / Metadata row -->
							<div class="grid grid-cols-2 gap-x-4 gap-y-1 text-xs text-muted-foreground pl-6">
								<span>
									<span class="font-medium text-foreground tabular-nums">{{ formatSize(f.size_bytes) }}</span>
								</span>
								<span class="tabular-nums">{{ new Date(f.started_at).toLocaleString() }}</span>
								<span v-if="f.is_recording" class="text-destructive tabular-nums">
									{{ formatDuration(elapsed[f.path] ?? 0) }}
								</span>
								<span v-else-if="f.video_duration_secs != null" class="tabular-nums">
									{{ formatDuration(f.video_duration_secs) }}
								</span>
								<span v-if="f.video_resolution" class="font-mono tabular-nums">{{ f.video_resolution }}</span>
								<span v-if="f.is_recording && recordingSpeed[f.path] != null" class="tabular-nums">
									{{ formatSize(recordingSpeed[f.path]!) }}/s
								</span>
							</div>

							<!-- 分片统计 / Segment stats -->
							<div v-if="(segmentStats[f.path]?.downloaded ?? f.segments_downloaded) != null" class="pl-6">
								<SegmentStatsBadges
									:downloaded="segmentStats[f.path]?.downloaded ?? f.segments_downloaded ?? 0"
									:failed="segmentStats[f.path]?.failed ?? f.segments_failed ?? 0"
								/>
							</div>

							<!-- 后处理进度 / Post-process progress -->
							<div v-if="!f.is_recording && ppStatus[f.path]" class="pl-6 text-xs">
								<PostprocessProgressCell :status="ppStatus[f.path]" :progress="ppProgress[f.path]" />
							</div>

							<!-- 操作按钮 / Action buttons -->
							<div class="flex gap-2 pl-6 flex-wrap">
								<Button size="sm" variant="outline" :disabled="f.is_recording" @click="openFile(f.path)">
									{{ t("recordings.actions.play") }}
								</Button>
								<Button
									v-if="moduleOutputs[f.path]?.['contact_sheet']"
									size="sm" variant="outline"
									@click="openModuleOutput(f.path, 'contact_sheet')"
								>
									<Image class="size-3.5" />
								</Button>
								<Button
									size="sm" variant="outline"
									:disabled="f.is_recording || ppStatus[f.path] === 'running' || ppStatus[f.path] === 'waiting' || !hasPipelineNodes"
									@click="runPostprocess(f.path)"
								>
									<Loader2 v-if="ppStatus[f.path] === 'running'" class="size-3.5 animate-spin" />
									<span v-else>{{ t("recordings.actions.postprocess") }}</span>
								</Button>
								<Button
									size="sm" variant="destructive"
									:disabled="f.is_recording"
									@click="deleteFile(f)"
								>{{ t("recordings.actions.delete") }}</Button>
							</div>
						</div>
					</template>
				</template>
			</div>
		</div>
	</div>
</template>

<style scoped>
.fade-enter-active, .fade-leave-active { transition: opacity 0.15s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
</style>
