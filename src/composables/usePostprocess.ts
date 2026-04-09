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

import { ref } from "vue";
import { call } from "@/lib/api";
import { usePostprocessStore } from "@/stores/postprocess";
import { useNotify } from "./useNotify";

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
	const isSameModule = moduleName.trim() === prevModuleName.trim() && moduleName.trim() !== "";
	const modulePct = isSameModule ? Math.max(rawModulePct, prevModulePct) : rawModulePct;

	// 计算当前执行的模块序号（1-based）
	// Calculate the current executing module index (1-based)
	let moduleExecLabel = "";
	if (overallTotal > 0) {
		const moduleIndex = hasModuleProgress
			? Math.min(overallTotal, overallDone + 1)
			: Math.min(overallTotal, Math.max(1, overallDone));
		moduleExecLabel = `${moduleIndex}/${overallTotal}`;
	}

	const normalizedModuleName = moduleName.trim() || "处理中";

	return {
		overallDone,
		overallTotal,
		overallPct,
		overallLabel: formatPct2(overallPct),
		moduleDone,
		moduleTotal,
		modulePct,
		moduleLabel: hasModuleProgress ? formatPct2(modulePct) : "等待进度…",
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
	const { toast } = useNotify();

	/** 各文件路径的后处理状态 / Post-processing status per file path */
	const ppStatus = ref<Record<string, PpStatus>>({});
	/** 各文件路径的后处理进度 / Post-processing progress per file path */
	const ppProgress = ref<Record<string, PpProgress>>({});
	/** 各文件路径的模块输出路径（如 contact_sheet 图片路径）/ Module output paths per file path */
	const moduleOutputs = ref<Record<string, Record<string, string>>>({});

	/** 是否在 Tauri 桌面环境中运行 / Whether running in Tauri desktop environment */
	const isTauri =
		typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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
			if (Object.keys(result).length > 0) {
				moduleOutputs.value = { ...moduleOutputs.value, [path]: result };
			}
		} catch {
			toast("获取模块输出失败", "error");
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
		ppProgress.value[path] = makePpProgress(0, 0, 0, 0, "", 0);
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
	 * Restore all post-processing task states from the backend (called after page refresh or SSE reconnect).
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
				if (t.status === "error") {
					ppStatus.value[t.path] = "error";
					continue;
				}
				if (t.status === "done") {
					ppStatus.value[t.path] = "done";
					ppProgress.value[t.path] = makePpProgress(
						t.done,
						t.total,
						t.modDone,
						t.modTotal,
						t.moduleName,
						t.pct,
					);
					// 推断并缓存模块输出路径 / Infer and cache module output paths
					const inferred = inferModuleOutputs(t.path);
					if (Object.keys(inferred).length > 0) {
						moduleOutputs.value = {
							...moduleOutputs.value,
							[t.path]: inferred,
						};
					}
					continue;
				}
				// 仅恢复来自内存的运行中/等待中任务（持久化任务已在 done/error 中处理）
				// Only restore in-memory running/waiting tasks (persisted tasks handled above)
				if (!t.fromMemory) continue;
				if (t.status === "waiting") {
					ppStatus.value[t.path] = "waiting";
				} else if (t.status === "running") {
					ppStatus.value[t.path] = t.status as PpStatus;
					ppProgress.value[t.path] = makePpProgress(
						t.done,
						t.total,
						t.modDone,
						t.modTotal,
						t.moduleName,
						t.pct,
					);
				}
			}
		} catch {
			toast("获取后处理任务失败", "error");
		}
	}

	/**
	 * 处理后处理完成事件，更新状态并触发文件列表刷新。
	 * Handle post-processing done event, update state and trigger file list reload.
	 *
	 * @param payload - 后端推送的完成事件数据 / Done event data from backend
	 * @param onLoad - 文件列表刷新回调 / File list reload callback
	 */
	function handlePostprocessDone(
		payload: {
			path: string;
			results: { moduleId: string; success: boolean; message: string }[];
		},
		onLoad: () => Promise<void>,
	) {
		const allOk = payload.results.every((r) => r.success);
		ppStatus.value[payload.path] = allOk ? "done" : "error";
		if (allOk) {
			// 所有模块成功：更新进度为 100% 并收集输出路径
			// All modules succeeded: set progress to 100% and collect output paths
			ppProgress.value[payload.path] = makePpProgress(
				payload.results.length,
				payload.results.length,
				0,
				0,
				"",
				100,
			);
			const names = payload.results.map((r) => r.moduleId).join(" → ");
			toast(`后处理完成：${names}`, "success");
			// 从模块返回的 OUTPUT: 前缀消息中提取输出路径
			// Extract output paths from module messages prefixed with "OUTPUT:"
			const outputs: Record<string, string> = {};
			for (const r of payload.results) {
				if (r.success && r.message.startsWith("OUTPUT:")) {
					outputs[r.moduleId] = r.message.slice("OUTPUT:".length).trim();
				}
			}
			// 合并推断路径和实际输出路径，实际路径优先
			// Merge inferred and actual output paths, actual paths take precedence
			const inferred = inferModuleOutputs(payload.path);
			const merged = { ...inferred, ...outputs };
			if (Object.keys(merged).length > 0) {
				moduleOutputs.value = {
					...moduleOutputs.value,
					[payload.path]: merged,
				};
			} else {
				fetchModuleOutputs(payload.path);
			}
		} else {
			delete ppProgress.value[payload.path];
			const failed = payload.results.find((r) => !r.success);
			toast(`后处理失败 [${failed?.moduleId}]：${failed?.message}`, "error");
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
		delete ppStatus.value[path];
		delete ppProgress.value[path];
		delete moduleOutputs.value[path];
	}

	return {
		ppStatus,
		ppProgress,
		moduleOutputs,
		isTauri,
		inferModuleOutputs,
		fetchModuleOutputs,
		runPostprocess,
		restoreFromBackend,
		handlePostprocessDone,
		removeFile,
	};
}
