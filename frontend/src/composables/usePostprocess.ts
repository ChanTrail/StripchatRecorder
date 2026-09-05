/**
 * 后处理任务管理 Composable / Post-processing Task Management Composable
 *
 * 管理录制文件的后处理流水线执行状态和进度，包括：
 * - 任务状态跟踪（空闲/等待/运行/完成/错误）
 * - 整体进度和当前模块进度计算
 * - 模块输出路径推断（如 contact_sheet 预览图路径）
 * - 页面刷新后从后端恢复任务状态
 *
 * Manages post-processing pipeline execution state and progress for recording files, including:
 * - Task status tracking (idle/waiting/running/done/error)
 * - Overall and per-module progress calculation
 * - Module output path inference (e.g., contact_sheet preview image path)
 * - Restoring task state from backend after page refresh
 */

import { call } from "@/lib/api";
import { usePostprocessStore } from "@/stores/postprocess";
import { usePpStatusStore } from "@/stores/ppStatus";
import { storeToRefs } from "pinia";
import { useNotify } from "./useNotify";
import { useI18n } from "vue-i18n";
import type { PpExecutionEntry, PpNodeProgress } from "@/types/recordings";

/** 后处理任务状态 / Post-processing task status */
export type PpStatus = "idle" | "waiting" | "running" | "done" | "error";

/** 后处理进度信息 / Post-processing progress information */
export interface PpProgress {
	/** 已完成的模块数 / Number of completed modules */
	overallDone: number;
	/** 总模块数 / Total number of modules */
	overallTotal: number;
	/** 整体进度百分比 / Overall progress percentage */
	overallPct: number;
	/** 整体进度标签文字 / Overall progress label text */
	overallLabel: string;
	/** 当前模块已完成进度值 / Current module done progress value */
	moduleDone: number;
	/** 当前模块总进度值 / Current module total progress value */
	moduleTotal: number;
	/** 当前模块进度百分比 / Current module progress percentage */
	modulePct: number;
	/** 当前模块进度标签文字 / Current module progress label text */
	moduleLabel: string;
	/** 当前模块名称 / Current module name */
	moduleName: string;
	/** 模块执行序号标签（如 "2/3"）/ Module execution index label (e.g. "2/3") */
	moduleExecLabel: string;
	/** 当前模块完整显示文字 / Full display text for current module */
	currentModuleText: string;
	/**
	 * 各模块执行结果（完成后填充，来自 postprocess-done 事件或 meta pp_results）。
	 * Per-module execution results (filled after completion, from postprocess-done event or meta pp_results).
	 */
	moduleResults?: { moduleId: string; success: boolean; message: string }[];
}

/**
 * 将百分比值限制在 [0, 100] 并保留两位小数。
 * Clamp a percentage value to [0, 100] with two decimal places.
 */
function clampPct2(value: number): number {
	if (!Number.isFinite(value)) return 0;
	return Math.min(100, Math.max(0, Math.round(value * 100) / 100));
}

/**
 * 将百分比值格式化为带两位小数的字符串（如 "42.50%"）。
 * Format a percentage value as a string with two decimal places (e.g. "42.50%").
 */
function formatPct2(value: number): string {
	return `${clampPct2(value).toFixed(2)}%`;
}

/** 传入 makePpProgress 的 i18n 标签 / i18n labels passed to makePpProgress */
export interface PpProgressLabels {
	/** 无模块名时的占位文字 / Placeholder when module name is empty */
	processing: string;
	/** 无进度数据时的标签文字 / Label when no progress data is available */
	waiting: string;
}

const DEFAULT_LABELS: PpProgressLabels = {
	processing: "processing",
	waiting: "waiting",
};

/**
 * 根据整体进度和模块进度构建 PpProgress 对象。
 * Build a PpProgress object from overall and module progress values.
 *
 * @param overallDone - 已完成模块数 / Number of completed modules
 * @param overallTotal - 总模块数 / Total number of modules
 * @param moduleDone - 当前模块已完成进度 / Current module done progress
 * @param moduleTotal - 当前模块总进度 / Current module total progress
 * @param moduleName - 当前模块名称 / Current module name
 * @param overallPctFallback - 整体进度的备用百分比（来自后端上报）/ Fallback overall percentage (from backend)
 * @param prevModuleName - 上一次的模块名称（用于防止进度倒退）/ Previous module name (for regression prevention)
 * @param prevModulePct - 上一次的模块进度（用于防止进度倒退）/ Previous module progress (for regression prevention)
 * @param labels - i18n 标签 / i18n labels
 */
export function makePpProgress(
	overallDone: number,
	overallTotal: number,
	moduleDone: number,
	moduleTotal: number,
	moduleName: string,
	overallPctFallback = 0,
	prevModuleName = "",
	prevModulePct = 0,
	labels: PpProgressLabels = DEFAULT_LABELS,
): PpProgress {
	const hasModuleProgress = moduleTotal > 0;
	const rawModulePct = hasModuleProgress
		? clampPct2((moduleDone * 100) / moduleTotal)
		: 0;
	// 同一模块内防止进度倒退；模块切换时允许从 0 重新开始
	// Prevent regression within the same module; allow reset to 0 on module switch
	const isSameModule =
		moduleName.trim() === prevModuleName.trim() && moduleName.trim() !== "";
	const modulePct = isSameModule
		? Math.max(rawModulePct, prevModulePct)
		: rawModulePct;

	// 计算总进度：已完成节点 + 当前模块的进度（作为分数）
	// Calculate overall progress: completed nodes + current module progress (as a fraction)
	const overallPctByNode =
		overallTotal > 0
			? clampPct2(((overallDone + modulePct / 100) * 100) / overallTotal)
			: 0;
	// 取节点计算值和后端上报值中的较大值，避免进度倒退
	// Take the larger of node-calculated and backend-reported values to prevent progress regression
	const overallPct =
		overallTotal > 0
			? Math.max(overallPctByNode, clampPct2(overallPctFallback))
			: clampPct2(overallPctFallback);

	// 计算当前执行的模块序号（1-based）
	// Calculate the current executing module index (1-based)
	let moduleExecLabel = "";
	if (overallTotal > 0) {
		const moduleIndex = hasModuleProgress
			? Math.min(overallTotal, overallDone + 1)
			: Math.min(overallTotal, Math.max(1, overallDone));
		moduleExecLabel = `${moduleIndex}/${overallTotal}`;
	}

	const normalizedModuleName = moduleName.trim() || labels.processing;

	return {
		overallDone,
		overallTotal,
		overallPct,
		overallLabel: formatPct2(overallPct),
		moduleDone,
		moduleTotal,
		modulePct,
		moduleLabel: hasModuleProgress ? formatPct2(modulePct) : labels.waiting,
		moduleName: normalizedModuleName,
		moduleExecLabel,
		currentModuleText: moduleExecLabel
			? `${moduleExecLabel} ${normalizedModuleName}`
			: normalizedModuleName,
	};
}

/**
 * 从 meta 的 pp_execution 和 pp_progress 字段直接计算 PpProgress。
 *
 * 规则：
 * - result != null 的条目 → 已完成（用于 overallDone / moduleResults）
 * - result == null 的条目 → 正在执行（当前节点）
 * - pp_progress → 当前节点的模块内进度
 * - overallTotal 来自 pipelineTotal 参数（当前流水线配置中启用且非内置的节点数），
 *   而非 pp_execution 的条目数——后者在流水线刚开始执行、或部分重新触发时不完整，
 *   不能作为总量的权威来源。
 *
 * Build PpProgress directly from meta's pp_execution and pp_progress fields.
 *
 * Rules:
 * - Entry with result != null → completed (contributes to overallDone / moduleResults)
 * - Entry with result == null → currently executing (current node)
 * - pp_progress → intra-module progress of the current node
 * - overallTotal comes from the pipelineTotal parameter (enabled, non-builtin node count
 *   in the current pipeline config), not the number of pp_execution entries — the latter
 *   is incomplete when the pipeline has just started or is partially re-triggered, so it
 *   cannot serve as the authoritative total.
 *
 * @param pipelineTotal - 当前流水线的总节点数（见 countPipelineTotal）/ Current pipeline's total node count (see countPipelineTotal)
 */
export function ppProgressFromMeta(
	ppExecution: PpExecutionEntry[] | null | undefined,
	ppProgress: PpNodeProgress | null | undefined,
	pipelineTotal: number,
	labels: PpProgressLabels = DEFAULT_LABELS,
): PpProgress {
	const entries = ppExecution ?? [];

	// 过滤掉内置节点（module_id 包含 __builtin__），并按有效 ID（node_id ?? module_id）
	// 去重，只保留每个节点最新的一条记录——重新触发后处理时，pp_execution 中可能同时
	// 存在同一节点的旧记录（被跳过，来自上次成功）和新记录（本次重新执行），需要以
	// 后者覆盖前者，避免同一节点被计数两次。
	//
	// Filter out builtin nodes (module_id contains __builtin__), and dedupe by effective ID
	// (node_id ?? module_id), keeping only the latest record per node — when post-processing
	// is re-triggered, pp_execution may contain both a stale record (skipped, from the
	// previous successful run) and a fresh one (re-executed this time) for the same node;
	// the latter must take precedence so the node isn't counted twice.
	const dedup = new Map<string, PpExecutionEntry>();
	for (const e of entries) {
		if (e.module_id.includes("__builtin__")) continue;
		dedup.set(e.node_id ?? e.module_id, e);
	}
	const userNodes = Array.from(dedup.values());

	// 统计已完成的节点数；总数使用当前流水线配置的节点数（权威来源）
	// Count completed nodes; total uses the current pipeline config's node count (authoritative)
	const overallDone = userNodes.filter((e) => e.result != null).length;
	const overallTotal = pipelineTotal;

	// 查找第一个正在执行的节点
	// Find the first running node
	const runningNode = userNodes.find((e) => e.result == null);

	const moduleResults = userNodes
		.filter((e) => e.result != null)
		.map((e) => ({
			moduleId: e.module_id,
			success:
				e.result?.code === "ok" ||
				e.result?.code === "done" ||
				e.result?.code === "skipped",
			message: e.result?.message ?? "",
		}));

	const moduleName = runningNode?.module_id ?? ppProgress?.module_id ?? "";
	const modDone = ppProgress?.mod_done ?? 0;
	// mod_total 固定为 10000（PROGRESS_SCALE）——但仅当确实存在正在执行的节点时才
	// 传给 makePpProgress，否则传 0。
	//
	// 若没有正在执行的节点（runningNode 为 undefined，即所有条目都已有 result），
	// 却仍无条件传一个非零的 modTotal，会导致 makePpProgress 内部
	// `hasModuleProgress = moduleTotal > 0` 恒为 true，进而虚构出一条
	// "当前模块 0% / 处理中" 的进度行——即使流水线已经全部跑完。这正是"总进度
	// 5/5 100%，但模块行卡在'处理中 0.00%'"现象的直接原因：整体进度和模块进度
	// 是两个独立字段，整体进度不为 100% 不代表模块进度就该显示"运行中"。
	//
	// mod_total is fixed at 10000 (PROGRESS_SCALE) — but only passed to
	// makePpProgress when a node is actually running; otherwise 0 is passed.
	//
	// If no node is currently running (runningNode is undefined, i.e. every entry
	// already has a result) but a non-zero modTotal is still passed unconditionally,
	// makePpProgress's internal `hasModuleProgress = moduleTotal > 0` is always true,
	// fabricating a "current module 0% / processing" row even though the pipeline has
	// fully finished. This is the direct cause of "overall progress 5/5 100%, but the
	// module row stuck at 'processing 0.00%'" — overall progress and module progress
	// are independent fields; overall not being 100% doesn't mean module progress
	// should show "running".
	const modTotal = runningNode ? 10000 : 0;

	return {
		...makePpProgress(
			overallDone,
			overallTotal,
			modDone,
			modTotal,
			moduleName,
			overallTotal > 0 ? clampPct2((overallDone * 100) / overallTotal) : 0,
			"",
			0,
			labels,
		),
		moduleResults,
	};
}

/**
 * 后处理任务状态与操作。
 * Post-processing task state and operations.
 */
export function usePostprocess() {
	const ppStore = usePostprocessStore();
	const ppStatusStore = usePpStatusStore();
	const { toast, notify } = useNotify();
	const { t } = useI18n();

	/** i18n 标签，传入 makePpProgress / i18n labels passed to makePpProgress */
	const ppLabels = (): PpProgressLabels => ({
		processing: t("usePostprocess.processing"),
		waiting: t("usePostprocess.waitingProgress"),
	});

	/** 各文件路径的后处理状态（来自全局 store）/ Post-processing status per file path (from global store) */
	const { ppStatus, ppProgress, moduleOutputs } = storeToRefs(ppStatusStore);

	/**
	 * 触发对指定文件执行后处理流水线。
	 * Trigger post-processing pipeline execution for a specific file.
	 *
	 * @param path - 视频文件路径 / Video file path
	 */
	async function runPostprocess(path: string) {
		try {
			await call("run_postprocess_cmd", { path });
			// 状态由 SSE 事件驱动：postprocess-waiting → postprocess-started → …
			// Status is driven by SSE events: postprocess-waiting → postprocess-started → …
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 从后端恢复所有后处理任务状态（页面刷新或 SSE 重连后调用）。
	 * 仅恢复运行中/等待中的瞬态任务；done/error 状态由 list_recordings 返回的 meta 字段负责。
	 *
	 * Restore all post-processing task states from the backend (called after page refresh or SSE reconnect).
	 * Only restores running/waiting transient tasks; done/error status is handled by meta fields from list_recordings.
	 */
	async function restoreFromBackend() {
		try {
			const tasks = await call<
				{
					path: string;
					pct: number;
					modDone: number;
					modTotal: number;
					moduleName: string;
					done: number;
					total: number;
					status: string;
					fromMemory: boolean;
				}[]
			>("get_postprocess_tasks");
			for (const t of tasks) {
				// 仅恢复来自内存的运行中/等待中任务
				// Only restore in-memory running/waiting tasks
				if (!t.fromMemory) continue;
				if (t.status === "waiting") {
					// 若已有更新的状态（running），不降级覆盖
					// Don't downgrade if a newer status (running) is already set
					if (ppStatus.value[t.path] !== "running") {
						ppStatus.value[t.path] = "waiting";
					}
				} else if (t.status === "running") {
					ppStatus.value[t.path] = t.status as PpStatus;
					ppProgress.value[t.path] = makePpProgress(
						t.done,
						t.total,
						t.modDone,
						t.modTotal,
						t.moduleName,
						t.pct,
						"",
						0,
						ppLabels(),
					);
				}
			}
		} catch {
			toast(t("usePostprocess.fetchTasksFailed"), "error");
		}
	}

	/**
	 * 处理后处理完成事件，更新状态并触发文件列表刷新。
	 *
	 * toast 提示只展示具体的视频文件名，不逐一列出每个模块的执行情况——用户
	 * 关心的是"哪个视频处理完了"，模块级别的成功/失败明细已经能在后处理列的
	 * 进度/状态展示中查看，重复堆在 toast 里反而降低可读性（尤其流水线模块
	 * 较多时，一条 toast 文字会很长）。失败时的详细模块信息保留在 message 里，
	 * 但同样以文件名作为提示的主体。
	 *
	 * Handle post-processing done event, update state and trigger file list reload.
	 *
	 * The toast only shows the specific video's filename, not a per-module rundown —
	 * users care about "which video finished", and per-module success/failure detail
	 * is already visible in the post-processing column's progress/status display;
	 * repeating it in the toast only hurts readability (especially with pipelines that
	 * have many modules, where the toast text would get very long). Failure detail is
	 * still included in the message, but the filename remains the toast's main subject.
	 */
	async function handlePostprocessDone(
		payload: { path: string; success: boolean; message?: string },
		onLoad: () => Promise<void>,
		isFileDeleted?: () => boolean,
	) {
		const allOk = payload.success;
		ppStatus.value[payload.path] = allOk ? "done" : "error";

		const deleted = isFileDeleted?.() ?? false;
		const fileName = payload.path.split(/[\\/]/).pop() ?? payload.path;

		// 执行 onLoad() 刷新文件列表（meta 已是最新，进度由 postprocess-meta-update 维护）
		// Execute onLoad() to refresh file list (meta is up-to-date; progress maintained by postprocess-meta-update)
		await onLoad();

		if (allOk) {
			if (!deleted) {
				notify(t("usePostprocess.doneForFile", { name: fileName }), "success");
			}
			// moduleOutputs 已通过上面的 onLoad()（内部调用 syncModuleOutputsFromFiles）
			// 从刷新后的 meta.pp_execution 中提取真实、经校验的输出路径，无需在此
			// 额外推断或请求——避免展示未经确认成功（result.code !== "ok"）或凭
			// 命名规则猜测（可能与实际参数不符）的路径。
			//
			// moduleOutputs was already populated by the onLoad() call above (which
			// internally calls syncModuleOutputsFromFiles) from the refreshed
			// meta.pp_execution — real, validated output paths. No need to infer or
			// fetch separately here, avoiding paths that weren't confirmed successful
			// (result.code !== "ok") or guessed from naming conventions (which may not
			// match the actual params).
		} else {
			if (!deleted) {
				const failedModules = ppProgress.value[payload.path]?.moduleResults?.filter((r) => !r.success);
				const detail = failedModules?.length
					? failedModules.map((r) => `${r.moduleId}: ${r.message}`).join("; ")
					: payload.message;
				notify(
					detail
						? t("usePostprocess.failedWithDetail", { name: fileName, detail })
						: t("usePostprocess.failedGeneric", { name: fileName }),
					"error",
				);
			}
		}
	}

	/**
	 * 清除指定文件的所有后处理状态（文件被删除时调用）。
	 * Clear all post-processing state for a specific file (called when file is deleted).
	 *
	 * @param path - 视频文件路径 / Video file path
	 */
	function removeFile(path: string) {
		ppStatusStore.removeFile(path);
	}

	return {
		ppStatus,
		ppProgress,
		moduleOutputs,
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile,
	};
}
