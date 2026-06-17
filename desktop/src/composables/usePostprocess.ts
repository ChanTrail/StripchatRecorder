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
	const overallPctByNode =
		overallTotal > 0 ? clampPct2((overallDone * 100) / overallTotal) : 0;
	// 取节点计算值和后端上报值中的较大值，避免进度倒退
	// Take the larger of node-calculated and backend-reported values to prevent progress regression
	const overallPct =
		overallTotal > 0
			? Math.max(overallPctByNode, clampPct2(overallPctFallback))
			: clampPct2(overallPctFallback);

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
 * 后处理任务状态与操作。
 * Post-processing task state and operations.
 */
export function usePostprocess() {
	const ppStore = usePostprocessStore();
	const ppStatusStore = usePpStatusStore();
	const { toast } = useNotify();
	const { t } = useI18n();

	/** i18n 标签，传入 makePpProgress / i18n labels passed to makePpProgress */
	const ppLabels = (): PpProgressLabels => ({
		processing: t("usePostprocess.processing"),
		waiting: t("usePostprocess.waitingProgress"),
	});

	/** 各文件路径的后处理状态（来自全局 store）/ Post-processing status per file path (from global store) */
	const { ppStatus, ppProgress, moduleOutputs } = storeToRefs(ppStatusStore);

	/**
	 * 根据当前流水线配置推断模块输出路径（无需请求后端）。
	 * Infer module output paths from the current pipeline config (without requesting backend).
	 *
	 * @param videoPath - 视频文件路径 / Video file path
	 * @returns 模块 ID -> 输出路径 的映射 / Map of module ID -> output path
	 */
	function inferModuleOutputs(videoPath: string): Record<string, string> {
		const outputs: Record<string, string> = {};
		const pipeline = ppStore.pipeline;
		if (!pipeline?.nodes) return outputs;
		// 兼容 Windows 和 Unix 路径分隔符 / Handle both Windows and Unix path separators
		const sep = videoPath.includes("\\") ? "\\" : "/";
		const parts = videoPath.split(sep);
		const filename = parts[parts.length - 1];
		const dir = parts.slice(0, -1).join(sep);
		const stem = filename.includes(".")
			? filename.slice(0, filename.lastIndexOf("."))
			: filename;
		for (const node of pipeline.nodes) {
			if (!node.enabled) continue;
			// contact_sheet 模块：输出与视频同名的图片文件
			// contact_sheet module: outputs an image file with the same name as the video
			if (node.moduleId === "contact_sheet") {
				const format = (node.params?.format as string) ?? "webp";
				outputs["contact_sheet"] = `${dir}${sep}${stem}.${format}`;
			}
		}
		return outputs;
	}

	/**
	 * 从后端获取指定文件的模块输出路径。
	 * Fetch module output paths for a specific file from the backend.
	 *
	 * @param path - 视频文件路径 / Video file path
	 */
	async function fetchModuleOutputs(path: string) {
		try {
			const result = await call<Record<string, string>>("get_module_outputs", {
				path,
			});
			if (result && Object.keys(result).length > 0) {
				moduleOutputs.value = { ...moduleOutputs.value, [path]: result };
			}
		} catch {
			toast(t("usePostprocess.fetchOutputFailed"), "error");
		}
	}

	/**
	 * 触发对指定文件执行后处理流水线。
	 * Trigger post-processing pipeline execution for a specific file.
	 *
	 * @param path - 视频文件路径 / Video file path
	 */
	async function runPostprocess(path: string) {
		ppStatus.value[path] = "running";
		ppProgress.value[path] = makePpProgress(0, 0, 0, 0, "", 0, "", 0, ppLabels());
		try {
			await call("run_postprocess_cmd", { path });
		} catch (e) {
			ppStatus.value[path] = "error";
			delete ppProgress.value[path];
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
	 * Handle post-processing done event, update state and trigger file list reload.
	 *
	 * @param payload - 后端推送的完成事件数据 / Done event data from backend
	 * @param onLoad - 文件列表刷新回调 / File list reload callback
	 * @param isFileDeleted - 文件是否已被用户删除（为 true 时跳过 toast 提示）/ Whether the file was deleted by the user (skip toast if true)
	 */
	function handlePostprocessDone(
		payload: {
			path: string;
			results: { moduleId: string; success: boolean; message: string }[];
		},
		onLoad: () => Promise<void>,
		isFileDeleted?: () => boolean,
	) {
		const allOk = payload.results.every((r) => r.success);
		ppStatus.value[payload.path] = allOk ? "done" : "error";

		// 若文件已被用户删除，跳过所有 toast 提示，仅刷新列表
		// If the file was deleted by the user, skip all toasts and just reload
		const deleted = isFileDeleted?.() ?? false;

		if (allOk) {
			// 所有模块成功：更新进度为 100% 并收集输出路径
			// All modules succeeded: set progress to 100% and collect output paths
			ppProgress.value[payload.path] = {
				...makePpProgress(
					payload.results.length,
					payload.results.length,
					0,
					0,
					"",
					100,
					"",
					0,
					ppLabels(),
				),
				moduleResults: payload.results,
			};
			if (!deleted) {
				const names = payload.results.map((r) => r.moduleId).join(" → ");
				toast(t("usePostprocess.done", { modules: names }), "success");
			}
			// NodeResult.output 字段在序列化时被 #[serde(skip)] 跳过，前端无法直接获取。
			// 使用 inferModuleOutputs 根据流水线配置推断输出路径（如 contact_sheet 图片路径）。
			// NodeResult.output is skipped during serialization (#[serde(skip)]), so the frontend
			// cannot access it directly. Use inferModuleOutputs to derive output paths from the
			// pipeline config (e.g., the contact_sheet image path).
			const inferred = inferModuleOutputs(payload.path);
			if (Object.keys(inferred).length > 0) {
				moduleOutputs.value = {
					...moduleOutputs.value,
					[payload.path]: inferred,
				};
			} else {
				fetchModuleOutputs(payload.path);
			}
		} else {
			ppProgress.value[payload.path] = {
				...makePpProgress(0, payload.results.length, 0, 0, "", 0, "", 0, ppLabels()),
				moduleResults: payload.results,
			};
			if (!deleted) {
				const failed = payload.results.find((r) => !r.success);
				toast(
					t("usePostprocess.failed", {
						moduleId: failed?.moduleId,
						message: failed?.message,
					}),
					"error",
				);
			}
		}
		return onLoad();
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
		inferModuleOutputs,
		fetchModuleOutputs,
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile,
	};
}
