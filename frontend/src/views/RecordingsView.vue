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
	import { useRecordings } from "@/composables/useRecordings";
	import { usePostprocess, ppProgressFromMeta } from "@/composables/usePostprocess";
	import { useImagePreview } from "@/composables/useImagePreview";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Tooltip } from "@/components/ui/tooltip";
	import { Loader2, Image } from "@lucide/vue";
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
	/**
	 * 删除时正在后处理的文件路径集合。
	 * 与 localDeletedPaths 不同，此集合不在 recording-deleted 时清除，
	 * 而是等到 postprocess-done 事件处理完后才清除，确保能正确抑制后处理失败 toast。
	 *
	 * Paths that were being post-processed when deleted.
	 * Unlike localDeletedPaths, this set is NOT cleared on recording-deleted;
	 * it is cleared after postprocess-done is handled, so the failure toast is correctly suppressed.
	 */
	const ppCancelledByDelete = new Set<string>();

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

	/** 分片下载统计（video_path -> {downloaded, failed}）/ Segment download stats per video path */
	const segmentStats = ref<Record<string, { downloaded: number; failed: number }>>({});

	const rec = useRecordings();
	const {
		files,
		loading,
		elapsed,
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
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile: ppRemoveFile,
	} = pp;

	/**
	 * 从当前 files.value 列表中同步模块输出路径到 moduleOutputs。
	 * 仅补充缺失的条目，不覆盖已有的（如推断值或实时更新值）。
	 * 用于 load() 之后确保 contact_sheet 等预览图按钮能正确显示。
	 *
	 * Sync module output paths from the current files.value list into moduleOutputs.
	 * Only fills in missing entries; does not overwrite existing ones (e.g. inferred or live-updated values).
	 * Called after load() to ensure preview buttons (e.g. contact_sheet) are correctly shown.
	 */
	function syncModuleOutputsFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.pp_execution) {
				const outputs: Record<string, string> = {};
				for (const entry of f.pp_execution) {
					if (entry.outputs && entry.outputs.length > 0) {
						const nonVideo = entry.outputs.find(
							(p) => !/\.(mp4|mkv|ts|avi|mov)$/i.test(p),
						);
						if (nonVideo) outputs[entry.module_id] = nonVideo;
					}
				}
				if (Object.keys(outputs).length > 0) {
					moduleOutputs.value[f.path] = { ...moduleOutputs.value[f.path], ...outputs };
				}
			}
		}
	}

	/**
	 * 从当前 files.value 列表的 meta status/pp_execution/pp_progress 字段，
	 * 同步后处理状态(ppStatus)、进度(ppProgress)和模块输出路径(moduleOutputs)。
	 * meta 是持久化的真相来源，优先级高于内存推断值，直接覆盖写入。
	 * 用于 onMounted 初始加载和 sse-lagged 事件后的全量状态恢复。
	 *
	 * Sync post-processing status (ppStatus), progress (ppProgress), and module output
	 * paths (moduleOutputs) from the current files.value list's meta status/pp_execution/pp_progress
	 * fields. Meta is the persistent source of truth and takes priority over in-memory inferred
	 * values, overwriting them directly. Used for the initial onMounted load and full state
	 * restoration after an sse-lagged event.
	 */
	function syncPpStateFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.status === "finish") {
				ppStatus.value[f.path] = "done";
			} else if (f.status === "pp_error") {
				ppStatus.value[f.path] = "error";
			} else if (f.status === "pp_waiting") {
				if (ppStatus.value[f.path] !== "running") ppStatus.value[f.path] = "waiting";
			} else if (f.status === "pp_running") {
				ppStatus.value[f.path] = "running";
			}
			// 从 meta 的 pp_execution + pp_progress 恢复进度（含运行中节点）
			// Restore progress from meta pp_execution + pp_progress (covers running nodes too)
			if (f.pp_execution && f.pp_execution.length > 0) {
				ppProgress.value[f.path] = ppProgressFromMeta(
					f.pp_execution, f.pp_progress,
					{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				);
			}
			// 从 pp_execution 输出路径推断模块输出（contact_sheet 等）
			// Infer module outputs from pp_execution output paths (e.g. contact_sheet)
			if (f.pp_execution) {
				const outputs: Record<string, string> = {};
				for (const entry of f.pp_execution) {
					if (entry.outputs && entry.outputs.length > 0) {
						const nonVideo = entry.outputs.find(
							(p) => !/\.(mp4|mkv|ts|avi|mov)$/i.test(p),
						);
						if (nonVideo) outputs[entry.module_id] = nonVideo;
					}
				}
				if (Object.keys(outputs).length > 0) {
					moduleOutputs.value[f.path] = outputs;
				}
			}
		}
	}

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
		viewportSize,
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
	 * 打开模块输出文件（使用预览弹窗）。
	 * Tauri 版：通过 invoke 读取文件为 base64 data URL 后显示。
	 *
	 * Open module output file (preview dialog).
	 * Tauri version: reads file as base64 data URL via invoke.
	 */
	async function openModuleOutput(filePath: string, moduleId: string) {
		const outputPath = moduleOutputs.value[filePath]?.[moduleId];
		if (!outputPath) return;
		try {
			const result = await call<{ data: string }>("read_output_file", { path: outputPath });
			openPreview(result.data, outputPath.split(/[\\/]/).pop() ?? "预览图");
		} catch (e) {
			toast(String(e), "error");
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
				.map((p) => {
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
		if (failed > 0)
			toast(t("recordings.delete.batchFailed", { count: failed }), "error");
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
					!files.value.find((f) => f.path === p)?.is_recording,
			).length,
	);

	/** 流水线是否有已连接输入节点的启用节点（即输入节点有连线）
	 * / Whether the pipeline has any enabled node connected to the recording input node */
	const hasPipelineNodes = computed(
		() => ppStore.pipeline?.nodes?.some(
			(n) => n.enabled && Object.values(n.inputs ?? {}).some((ref) => ref.nodeId === "0"),
		) ?? false,
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

		await load();
		startTick();
		await refreshDiskSpace();
		const diskTimer = setInterval(refreshDiskSpace, 30_000);
		unlisteners.push(() => clearInterval(diskTimer));
		if (!ppStore.pipeline?.nodes?.length) await ppStore.fetchPipeline();

		// 监听其他客户端的流水线更新，实时刷新模块输出路径推断结果
		// Listen for pipeline updates from other clients and re-infer module output paths
		ppStore.initModuleWatcher(() => {
			syncModuleOutputsFromFiles();
		});

		// 先恢复运行中/等待中的后处理任务状态（来自内存，不依赖 meta）
		// First restore running/waiting post-processing task states (from memory, independent of meta)
		await restoreFromBackend();

		// 再从文件列表的 meta status 字段初始化 done/error 状态和模块输出路径。
		// meta 是持久化的真相来源，优先级高于推断值，直接覆盖写入。
		//
		// Then initialize status and module output paths from meta status fields in the file list.
		// Meta is the persistent source of truth and takes priority over inferred values.
		syncPpStateFromFiles();

		unlisteners.push(
			await on("recordings-dir-changed", () => scheduleDirRefresh(syncModuleOutputsFromFiles)),
		);

		unlisteners.push(
			await on("sse-lagged", async () => {
				// SSE 广播队列溢出，事件已丢失，重新从后端恢复完整状态
				// SSE broadcast queue overflowed, events lost; restore full state from backend
				await load();
				await restoreFromBackend();
				syncPpStateFromFiles();
			}),
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
				const p = payload as {
					path: string;
					size_bytes: number;
					speed_bps?: number;
					segments_downloaded?: number;
					segments_failed?: number;
				};
				// path is the video file path (from meta)
				const f = files.value.find((r) => r.path === p.path);
				if (f) {
					if (p.speed_bps != null && f.is_recording) {
						recordingSpeed.value = {
							...recordingSpeed.value,
							[p.path]: p.speed_bps,
						};
					} else if (!f.is_recording) {
						delete recordingSpeed.value[p.path];
					}
					f.size_bytes = p.size_bytes;
					// 更新分片统计 / Update segment stats
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
					await load();
					startTick();
					syncModuleOutputsFromFiles();
				}
			}),
		);

		unlisteners.push(
			await on("recording-started", async () => {
				await load();
				startTick();
				syncModuleOutputsFromFiles();
			}),
		);

		unlisteners.push(
			await on("recording-stopped", async (payload) => {
				const p = payload as { video_path?: string };
				await load();
				syncModuleOutputsFromFiles();
				// 录制结束时清理速度数据 / Clean up recording speed when recording stops
				if (p.video_path) {
					const nextSpeed = { ...recordingSpeed.value };
					delete nextSpeed[p.video_path];
					recordingSpeed.value = nextSpeed;
				}
				// 录制结束时清理分片统计 / Clean up segment stats when recording stops
				if (p.video_path) {
					const nextStats = { ...segmentStats.value };
					delete nextStats[p.video_path];
					segmentStats.value = nextStats;
				}
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
				// 进度由后续 postprocess-meta-update 事件初始化，此处无需设置
				// Progress will be initialized by the first postprocess-meta-update event
			}),
		);

		// postprocess-meta-update：每次节点开始/进度更新/节点完成时后端推送最新 meta 快照，
		// 前端直接从 pp_execution + pp_progress 重算进度，无需依赖独立进度事件字段。
		// postprocess-meta-update: backend pushes the latest meta snapshot on each node start/progress/done.
		// Frontend recalculates progress directly from pp_execution + pp_progress without relying on
		// individual progress event fields.
		unlisteners.push(
			await on("postprocess-meta-update", (payload) => {
				const p = payload as {
					path: string;
					meta: {
						pp_execution?: import("@/types/recordings").PpExecutionEntry[] | null;
						pp_progress?: import("@/types/recordings").PpNodeProgress | null;
					};
				};
				if (!p.meta) return;
				ppProgress.value[p.path] = ppProgressFromMeta(
					p.meta.pp_execution, p.meta.pp_progress,
					{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				);
				// 实时提取新完成节点的非视频输出（如 contact_sheet 图片）
				// Extract non-video outputs of newly completed nodes in real time (e.g. contact_sheet image)
				if (p.meta.pp_execution) {
					const outputs: Record<string, string> = { ...moduleOutputs.value[p.path] };
					for (const entry of p.meta.pp_execution) {
						if (entry.result != null && entry.outputs && entry.outputs.length > 0) {
							const nonVideo = entry.outputs.find(
								(op) => !/\.(mp4|mkv|ts|avi|mov)$/i.test(op),
							);
							if (nonVideo) outputs[entry.module_id] = nonVideo;
						}
					}
					if (Object.keys(outputs).length > 0) {
						moduleOutputs.value = { ...moduleOutputs.value, [p.path]: outputs };
					}
				}
			}),
		);

		unlisteners.push(
			await on("postprocess-done", async (payload) => {
				const p = payload as { path: string; success: boolean; message?: string };
				const wasCancelledByDelete = ppCancelledByDelete.has(p.path);
				ppCancelledByDelete.delete(p.path);
				handlePostprocessDone(
					p,
					async () => {
						await load();
						syncModuleOutputsFromFiles();
					},
					() => wasCancelledByDelete,
				);
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
	<div class="flex flex-col h-full gap-0">
		<Dialog :open="previewOpen" @update:open="previewOpen = $event">
			<DialogContent
				class="p-0 overflow-hidden flex flex-col w-fit"
				style="max-width: 90vw; max-height: 90vh"
			>
				<DialogHeader class="px-4 pt-4 pb-2 shrink-0">
					<DialogTitle class="text-sm font-mono truncate">{{
						previewTitle
					}}</DialogTitle>
				</DialogHeader>
				<div
					ref="previewViewportRef"
					class="relative overflow-hidden flex items-center justify-center bg-black/5 px-4 pb-4"
					:style="{
						width: viewportSize.width,
						height: viewportSize.height,
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
							{{
								t("recordings.resetZoom", {
									pct: Math.round(previewScale * 100),
								})
							}}
						</Button>
					</Transition>
				</div>
			</DialogContent>
		</Dialog>

		<header
			ref="headerEl"
			class="flex items-start justify-between gap-4 shrink-0 pb-4 bg-background sticky top-0 z-20 px-6 pt-6 border-b"
		>
			<div class="flex-1 min-w-0">
				<h1 class="text-xl font-bold mb-0.5">{{ t("recordings.title") }}</h1>
				<div
					class="flex items-center gap-3 text-sm text-muted-foreground flex-wrap"
				>
					<span>{{
						t("recordings.subtitle.total", { count: files.length })
					}}</span>
					<span v-if="recordingCount > 0" class="text-destructive">{{
						t("recordings.subtitle.recording", { count: recordingCount })
					}}</span>
					<span v-if="selectedCount > 0" class="text-foreground">{{
						t("recordings.subtitle.selected", { count: selectedCount })
					}}</span>
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
						variant="outline"
						size="sm"
						:disabled="ppSelectableCount === 0 || !hasPipelineNodes"
						@click="postProcessSelected"
					>
						{{ t("recordings.batchPostprocess", { count: ppSelectableCount }) }}
					</Button>
				</Tooltip>
				<Button
					v-if="selectedCount > 0"
					variant="destructive"
					size="sm"
					@click="deleteSelected"
				>
					{{ t("recordings.deleteSelected", { count: selectedCount }) }}
				</Button>
			</div>
		</header>

		<div class="px-6 flex-1 overflow-y-auto">
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
				<TableHeader
					class="sticky top-0 z-10 bg-background"
				>
					<TableRow>
						<TableHead class="w-8">
							<Checkbox
								:model-value="getAllChecked()"
								@update:model-value="setAllChecked"
							/>
						</TableHead>
						<TableHead class="w-px whitespace-nowrap">{{
							t("recordings.table.filename")
						}}</TableHead>
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
						<TableHead class="whitespace-nowrap">{{ t("recordings.table.resolution") }}</TableHead>
						<TableHead>{{ t("recordings.table.speed") }}</TableHead>
						<TableHead>{{ t("recordings.table.segments") }}</TableHead>
						<TableHead class="min-w-45">{{
							t("recordings.table.postprocess")
						}}</TableHead>
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
							<TableCell colspan="9" class="font-semibold">
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
									{{
										t("recordings.group.fileCount", {
											count: group.files.length,
										})
									}}
									·
									{{ formatSize(group.totalSize) }}
								</span>
							</TableCell>
							<TableCell />
						</TableRow>

						<template v-if="!collapsedGroups.has(group.username)">
							<TableRow v-for="f in group.files" :key="f.path" class="relative">
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
										<span v-else class="text-muted-foreground">—</span>
									</TableCell>
									<TableCell class="tabular-nums">
										<span v-if="f.video_duration_secs != null">{{
											formatDuration(f.video_duration_secs)
										}}</span>
										<span v-else class="text-muted-foreground">—</span>
									</TableCell>
									<TableCell class="tabular-nums font-mono text-xs">
										<span v-if="f.video_resolution">{{ f.video_resolution }}</span>
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
									<TableCell class="min-w-36">
										<template v-if="f.is_recording && segmentStats[f.path] != null">
											<div class="flex items-center gap-1 flex-wrap">
												<Badge variant="secondary" class="tabular-nums text-[11px] px-1.5 py-0">
													{{ segmentStats[f.path].downloaded }}
												</Badge>
												<Badge
													v-if="segmentStats[f.path].failed > 0"
													variant="secondary"
													class="tabular-nums text-[11px] px-1.5 py-0 bg-destructive/15 text-destructive border-0"
												>
													{{ segmentStats[f.path].failed }}
												</Badge>
												<Badge
													variant="outline"
													class="tabular-nums text-[11px] px-1.5 py-0"
													:class="segmentStats[f.path].failed === 0
														? 'border-green-500 text-green-500'
														: 'border-destructive text-destructive'"
												>
													{{
														segmentStats[f.path].downloaded + segmentStats[f.path].failed > 0
															? Math.round(segmentStats[f.path].downloaded / (segmentStats[f.path].downloaded + segmentStats[f.path].failed) * 100)
															: 100
													}}%
												</Badge>
											</div>
										</template>
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
															? t(
																	"recordings.status.overallProgressWithLabel",
																	{ label: ppProgress[f.path].moduleExecLabel },
																)
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
												v-else-if="ppStatus[f.path] === 'done' || ppStatus[f.path] === 'error'"
												class="flex flex-col gap-0.5"
											>
												<template v-if="ppProgress[f.path]?.moduleResults?.length">
													<div
														v-for="r in ppProgress[f.path].moduleResults"
														:key="r.moduleId"
														class="flex items-center gap-1.5 text-xs"
														:class="r.success ? 'text-green-500' : 'text-destructive'"
														:title="r.success ? r.moduleId : `${r.moduleId}: ${r.message}`"
													>
														<span class="shrink-0">{{ r.success ? "✓" : "✗" }}</span>
														<span class="truncate max-w-40">{{ r.moduleId }}</span>
													</div>
												</template>
											</div>
											<span v-else class="text-xs text-muted-foreground"
												>—</span
											>
										</div>
										<span v-else class="text-xs text-muted-foreground">—</span>
									</TableCell>
									<TableCell>
										<div class="flex gap-1.5">
											<Button
												size="sm"
												variant="outline"
												:disabled="f.is_recording"
												:title="
													f.is_recording
														? t('recordings.actions.playDisabled')
														: ''
												"
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
											<Tooltip
												:content="
													f.is_recording
														? t('recordings.status.recording')
														: !hasPipelineNodes
															? t('recordings.postprocessEmptyPipeline')
															: undefined
												"
											>
												<Button
													size="sm"
													variant="outline"
													:disabled="
														f.is_recording ||
														ppStatus[f.path] === 'running' ||
														ppStatus[f.path] === 'waiting' ||
														!hasPipelineNodes
													"
													@click="runPostprocess(f.path)"
												>
													<Loader2
														v-if="ppStatus[f.path] === 'running'"
														class="size-3.5 animate-spin"
													/>
													<span v-else>{{
														t("recordings.actions.postprocess")
													}}</span>
												</Button>
											</Tooltip>
											<Button
												size="sm"
												variant="destructive"
												:disabled="f.is_recording"
												:title="
													f.is_recording
														? t('recordings.actions.deleteDisabled')
														: ''
												"
												@click="deleteFile(f)"
												>{{ t("recordings.actions.delete") }}</Button
											>
										</div>
									</TableCell>
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
