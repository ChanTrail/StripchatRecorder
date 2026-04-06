/**
 * 后处理流水线状态管理 Store / Post-processing Pipeline State Management Store
 *
 * 管理后处理模块列表和流水线配置。流水线由有序的节点组成，每个节点对应一个处理模块。
 * 流水线变更后会自动防抖保存（600ms），并支持多客户端实时同步。
 *
 * Manages the post-processing module list and pipeline configuration.
 * The pipeline consists of ordered nodes, each corresponding to a processing module.
 * Pipeline changes are auto-saved with debounce (600ms) and support real-time multi-client sync.
 */

import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { call, on } from "@/lib/api";

/** 模块参数定义 / Module parameter definition */
export interface ParamDef {
	/** 参数键名 / Parameter key */
	key: string;
	/** 参数显示标签 / Parameter display label */
	label: string;
	/** 参数类型 / Parameter type */
	type: "string" | "number" | "boolean" | "select";
	/** 参数默认值 / Parameter default value */
	default: unknown;
	/** select 类型的可选项 / Options for select type */
	options?: string[];
}

/**
 * 将参数默认值强制转换为对应类型的 JS 值。
 * Coerce a parameter default value to the corresponding JS type.
 *
 * @param type - 参数类型 / Parameter type
 * @param value - 原始值 / Raw value
 */
function coerceDefault(
	type: ParamDef["type"],
	value: unknown,
): string | number | boolean {
	if (type === "boolean") return Boolean(value);
	if (type === "number") {
		const n = Number(value);
		return isNaN(n) ? 0 : n;
	}
	if (value === null || value === undefined) return "";
	return String(value);
}

/** 后处理模块信息 / Post-processing module information */
export interface ModuleInfo {
	/** 模块唯一 ID / Module unique ID */
	id: string;
	/** 模块显示名称 / Module display name */
	name: string;
	/** 模块功能描述 / Module description */
	description: string;
	/** 模块参数定义列表 / Module parameter definitions */
	params: ParamDef[];
}

/** 流水线节点（模块实例）/ Pipeline node (module instance) */
export interface PipelineNode {
	/** 节点唯一 ID（UUID）/ Node unique ID (UUID) */
	nodeId: string;
	/** 对应的模块 ID / Corresponding module ID */
	moduleId: string;
	/** 节点参数值 / Node parameter values */
	params: Record<string, string | number | boolean>;
	/** 是否启用此节点 / Whether this node is enabled */
	enabled: boolean;
}

/** 流水线配置 / Pipeline configuration */
export interface PipelineConfig {
	nodes: PipelineNode[];
}

export const usePostprocessStore = defineStore("postprocess", () => {
	/** 可用的后处理模块列表 / Available post-processing modules */
	const modules = ref<ModuleInfo[]>([]);
	/** 当前流水线配置 / Current pipeline configuration */
	const pipeline = ref<PipelineConfig>({ nodes: [] });
	/** 是否正在加载 / Whether loading */
	const loading = ref(false);
	/** 是否正在保存 / Whether saving */
	const saving = ref(false);
	/** 是否正在本地保存（用于过滤自身触发的 pipeline-updated 事件）/ Whether saving locally (to filter self-triggered pipeline-updated events) */
	let _isSavingLocally = false;
	/** 流水线是否已从后端加载完成（防止初始化前触发自动保存）/ Whether pipeline has been loaded from backend (prevents auto-save before init) */
	let _loaded = false;
	/** 防抖保存定时器 / Debounce save timer */
	let _saveTimer: ReturnType<typeof setTimeout> | null = null;

	/**
	 * 从后端获取可用模块列表。
	 * Fetch the available module list from the backend.
	 */
	async function fetchModules() {
		modules.value = await call<ModuleInfo[]>("list_modules");
	}

	/**
	 * 从后端获取当前流水线配置。
	 * Fetch the current pipeline configuration from the backend.
	 */
	async function fetchPipeline() {
		loading.value = true;
		try {
			pipeline.value = await call<PipelineConfig>("get_pipeline");
		} finally {
			loading.value = false;
			_loaded = true;
		}
	}

	/**
	 * 将当前流水线配置保存到后端。
	 * Save the current pipeline configuration to the backend.
	 */
	async function savePipeline() {
		saving.value = true;
		_isSavingLocally = true;
		try {
			await call("save_pipeline", { pipeline: pipeline.value });
		} finally {
			saving.value = false;
			setTimeout(() => {
				_isSavingLocally = false;
			}, 500);
		}
	}

	// 监听流水线变化，防抖 600ms 后自动保存
	// Watch pipeline changes and auto-save after 600ms debounce
	watch(
		pipeline,
		() => {
			if (!_loaded) return;
			if (_saveTimer) clearTimeout(_saveTimer);
			_saveTimer = setTimeout(() => savePipeline(), 600);
		},
		{ deep: true },
	);

	/**
	 * 向流水线末尾添加一个新节点，使用模块的默认参数值。
	 * Add a new node to the end of the pipeline with the module's default parameter values.
	 *
	 * @param moduleId - 要添加的模块 ID / Module ID to add
	 */
	function addNode(moduleId: string) {
		const mod = modules.value.find((m) => m.id === moduleId);
		if (!mod) return;
		// 使用模块定义的默认值初始化参数 / Initialize params with module-defined defaults
		const defaults: Record<string, string | number | boolean> = {};
		for (const p of mod.params) {
			defaults[p.key] = coerceDefault(p.type, p.default);
		}
		pipeline.value.nodes.push({
			nodeId: crypto.randomUUID(),
			moduleId,
			params: defaults,
			enabled: true,
		});
	}

	/**
	 * 从流水线中移除指定节点。
	 * Remove a specific node from the pipeline.
	 *
	 * @param nodeId - 要移除的节点 ID / Node ID to remove
	 */
	function removeNode(nodeId: string) {
		pipeline.value.nodes = pipeline.value.nodes.filter(
			(n) => n.nodeId !== nodeId,
		);
	}

	/**
	 * 在流水线中上移或下移指定节点。
	 * Move a specific node up or down in the pipeline.
	 *
	 * @param nodeId - 要移动的节点 ID / Node ID to move
	 * @param direction - 移动方向 / Move direction
	 */
	function moveNode(nodeId: string, direction: "up" | "down") {
		const idx = pipeline.value.nodes.findIndex((n) => n.nodeId === nodeId);
		if (idx < 0) return;
		const target = direction === "up" ? idx - 1 : idx + 1;
		if (target < 0 || target >= pipeline.value.nodes.length) return;
		const nodes = [...pipeline.value.nodes];
		[nodes[idx], nodes[target]] = [nodes[target], nodes[idx]];
		pipeline.value.nodes = nodes;
	}

	let _moduleWatcherReady = false;
	let _onPipelineUpdated: (() => void) | null = null;

	/**
	 * 初始化模块和流水线的实时更新监听器（只执行一次）。
	 * Initialize real-time update listeners for modules and pipeline (executed only once).
	 *
	 * @param onPipelineUpdated - 流水线被其他客户端更新时的回调 / Callback when pipeline is updated by another client
	 */
	async function initModuleWatcher(onPipelineUpdated?: () => void) {
		_onPipelineUpdated = onPipelineUpdated ?? null;
		if (_moduleWatcherReady) return;
		_moduleWatcherReady = true;
		await on("modules-changed", () => {
			void fetchModules();
		});
		await on("pipeline-updated", (payload) => {
			// 本地保存时忽略自身触发的事件 / Ignore self-triggered events during local save
			if (_isSavingLocally) return;
			// 暂时禁用自动保存，防止接收到的配置被立即重新保存
			// Temporarily disable auto-save to prevent received config from being immediately re-saved
			_loaded = false;
			pipeline.value = payload as PipelineConfig;
			setTimeout(() => {
				_loaded = true;
			}, 0);
			_onPipelineUpdated?.();
		});
	}

	return {
		modules,
		pipeline,
		loading,
		saving,
		fetchModules,
		fetchPipeline,
		savePipeline,
		addNode,
		removeNode,
		moveNode,
		initModuleWatcher,
	};
});
