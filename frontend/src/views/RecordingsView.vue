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
	import { usePostprocessStore, countPipelineTotal } from "@/stores/postprocess";
	import { useRecordings } from "@/composables/useRecordings";
	import { usePostprocess, ppProgressFromMeta } from "@/composables/usePostprocess";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Tooltip } from "@/components/ui/tooltip";
	import { Progress } from "@/components/ui/progress";
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow,
	} from "@/components/ui/table";
	import { ChevronRight, ChevronDown } from "@lucide/vue";
	import RecordingRow from "@/components/RecordingRow.vue";
	import ImagePreviewDialog from "@/components/ImagePreviewDialog.vue";
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
	 * 直接采用后端已验证的 `f.module_outputs`（result.code === "ok" 且路径当前
	 * 确实存在于磁盘上，见 `extract_verified_module_outputs`），不再自行从
	 * pp_execution 推断——前端没有文件系统访问权限，无法验证路径是否真实存在。
	 *
	 * Sync module output paths from the current files.value list into moduleOutputs.
	 * Directly uses the backend-verified `f.module_outputs` (result.code === "ok" and
	 * the path currently exists on disk, see `extract_verified_module_outputs`), no
	 * longer inferring from pp_execution itself — the frontend has no filesystem access
	 * and cannot verify a path actually exists.
	 */
	function syncModuleOutputsFromFiles() {
		for (const f of files.value) {
			if (f.is_recording) continue;
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				moduleOutputs.value[f.path] = { ...moduleOutputs.value[f.path], ...f.module_outputs };
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
					f.pp_execution, f.pp_progress, countPipelineTotal(ppStore.pipeline),
					{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				);
			}
			// 直接使用后端已验证的模块输出路径（result.code === "ok" 且文件当前确实存在）
			// Directly use the backend-verified module output paths (result.code === "ok"
			// and the file currently exists)
			if (f.module_outputs && Object.keys(f.module_outputs).length > 0) {
				moduleOutputs.value[f.path] = f.module_outputs;
			}
		}
	}

	/** 图片预览对话框组件引用 / Image preview dialog component ref */
	const previewDialogRef = ref<InstanceType<typeof ImagePreviewDialog> | null>(null);

	/**
	 * 用系统默认程序打开录制文件。
	 * Open a recording file with the system default application.
	 */
	async function openFile(path: string) {
		await call("open_recording", { path });
	}

	/**
	 * 打开模块输出文件（使用预览弹窗）。
	 * Web 版：直接用后端的静态文件路由（`GET /api/files?video_path=...&module_id=...`，
	 * 见 backend/src/server_mod/routes/recording.rs 的 serve_output_file）拼出图片 URL
	 * 交给 <img> 加载，不需要先整个读进内存转 base64。
	 *
	 * 传的是 `(video_path, module_id)` 而不是具体文件路径——后端会通过 meta 解析出
	 * 该模块真正的输出路径（与 moduleOutputs 的来源同一套校验逻辑），不管该路径是否
	 * 落在默认的 output_dir 内（ts_merge 等模块支持自定义输出目录，产物可能在别处）。
	 *
	 * Open module output file (preview dialog).
	 * Web version: builds the image URL directly from the backend's static file route
	 * (`GET /api/files?video_path=...&module_id=...`, see serve_output_file in
	 * backend/src/server_mod/routes/recording.rs) and lets <img> load it — no need to
	 * read the whole file into memory and base64-encode it first.
	 *
	 * Passes `(video_path, module_id)` rather than a concrete file path — the backend
	 * resolves that module's actual output path via meta (the same verification logic
	 * that populates moduleOutputs), regardless of whether that path falls within the
	 * default output_dir (modules like ts_merge support a custom output directory, so
	 * the artifact may live elsewhere).
	 */
	function openModuleOutput(filePath: string, moduleId: string) {
		const outputPath = moduleOutputs.value[filePath]?.[moduleId];
		if (!outputPath) return;
		const url = `/api/files?video_path=${encodeURIComponent(filePath)}&module_id=${encodeURIComponent(moduleId)}`;
		previewDialogRef.value?.openPreview(url, outputPath.split(/[\\/]/).pop() ?? "预览图");
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
					module_outputs?: Record<string, string>;
				};
				if (!p.meta) return;
				
				// 检测路径迁移：如果新路径不在 ppStatus 中，但有旧路径（文件夹路径）存在，
				// 则需要将状态从旧路径迁移到新路径（ts_merge 后从文件夹路径切换到视频文件路径）
				// Detect path migration: if new path is not in ppStatus but an old path (folder path) exists,
				// migrate state from old path to new path (after ts_merge switches from folder to video file path)
				if (!ppStatus.value[p.path]) {
					// 查找可能的旧路径：找到 ppStatus 中状态为 "running" 且不在当前文件列表中的路径
					// Find possible old path: look for paths in ppStatus with status "running" that aren't in current file list
					const currentPaths = new Set(files.value.map((f) => f.path));
					const oldPath = Object.keys(ppStatus.value).find(
						(path) => ppStatus.value[path] === "running" && !currentPaths.has(path)
					);
					
					if (oldPath) {
						// 迁移状态：旧路径 → 新路径
						// Migrate state: old path → new path
						ppStatus.value[p.path] = ppStatus.value[oldPath];
						if (ppProgress.value[oldPath]) {
							ppProgress.value[p.path] = ppProgress.value[oldPath];
						}
						if (moduleOutputs.value[oldPath]) {
							moduleOutputs.value[p.path] = moduleOutputs.value[oldPath];
						}
						// 清理旧路径
						// Clean up old path
						delete ppStatus.value[oldPath];
						delete ppProgress.value[oldPath];
						delete moduleOutputs.value[oldPath];
					}
				}
				
				ppProgress.value[p.path] = ppProgressFromMeta(
					p.meta.pp_execution, p.meta.pp_progress, countPipelineTotal(ppStore.pipeline),
					{ processing: t("usePostprocess.processing"), waiting: t("usePostprocess.waitingProgress") },
				);
				// 直接使用后端已验证的模块输出路径（result.code === "ok" 且文件当前确实
				// 存在于磁盘上，见 extract_verified_module_outputs），实时反映新完成节点
				// 的预览图（如 contact_sheet），不展示失败节点残留的旧输出
				// Directly use the backend-verified module output paths (result.code === "ok"
				// and the file currently exists on disk, see extract_verified_module_outputs),
				// reflecting newly completed nodes' previews (e.g. contact_sheet) in real
				// time, without showing stale outputs left over from a failed node
				if (p.module_outputs && Object.keys(p.module_outputs).length > 0) {
					moduleOutputs.value = {
						...moduleOutputs.value,
						[p.path]: { ...moduleOutputs.value[p.path], ...p.module_outputs },
					};
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
		<ImagePreviewDialog ref="previewDialogRef" />

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
								<component
									:is="collapsedGroups.has(group.username) ? ChevronRight : ChevronDown"
									class="inline-block size-3.5 mr-1.5 text-muted-foreground align-middle"
								/>
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
