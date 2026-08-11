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
import { useI18n } from "vue-i18n";
import { useModuleLocaleStore } from "@/stores/moduleLocale";

/**
 * 生成一个随机 ID，优先使用 crypto.randomUUID()，
 * 在非安全上下文（如通过 IP 访问的 HTTP 页面）下降级为 Math.random() 实现。
 *
 * Generate a random ID, preferring crypto.randomUUID().
 * Falls back to a Math.random()-based implementation in non-secure contexts
 * (e.g. HTTP pages accessed via IP address).
 */
function generateId(): string {
	if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
		return crypto.randomUUID();
	}
	return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
		const r = (Math.random() * 16) | 0;
		return (c === "x" ? r : (r & 0x3) | 0x8).toString(16);
	});
}

/** 端口类型，与后端 PortType 枚举对应 / Port type, mirrors backend PortType enum */
export type PortType = "ts_session_dir" | "video_file" | "image_file" | "media_bundle" | "any_file" | "any_dir";

/**
 * 判断上游输出类型是否兼容下游输入类型（与后端 is_compatible_with 逻辑一致）。
 * Check if an upstream output type is compatible with a downstream input type
 * (mirrors backend is_compatible_with logic).
 */
export function isPortCompatible(from: PortType, to: PortType): boolean {
	if (from === to) return true;
	if (to === "any_file" && (from === "video_file" || from === "image_file" || from === "media_bundle")) return true;
	if (to === "any_dir" && from === "ts_session_dir") return true;
	return false;
}

/** 端口类型的显示颜色 / Display color per port type */
export const PORT_TYPE_COLORS: Record<PortType, string> = {
	ts_session_dir: "#f59e0b", // amber
	video_file: "#3b82f6",     // blue
	image_file: "#10b981",     // emerald
	media_bundle: "#a855f7",   // purple
	any_file: "#8b5cf6",       // violet
	any_dir: "#f97316",        // orange
};

/** 模块参数定义 / Module parameter definition */
export interface ParamDef {
	/** 参数键名 / Parameter key */
	key: string;
	/** 参数显示标签 / Parameter display label */
	label: string;
	/** 参数类型；"dir" 渲染为带"浏览"按钮的目录选择输入框 / Parameter type; "dir" renders as a directory input with a "browse" button */
	type: "string" | "number" | "boolean" | "select" | "dir";
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

/** 模块 i18n 翻译（单个语言）/ Module i18n translation for a single locale */
export interface ModuleI18nLocale {
	name?: string;
	description?: string;
	params?: Record<string, { label?: string }>;
}

/** 后处理模块信息 / Post-processing module information */
export interface ModuleInfo {
	id: string;
	name: string;
	/**
	 * 模块版本号（从模块自身 Cargo.toml 的 version 字段读取，而非手写在名称里）。
	 * 旧版模块未提供此字段时为空字符串。
	 *
	 * Module version (read from the module's own Cargo.toml `version` field, rather
	 * than hardcoded into the name). Empty string if an older module omits this field.
	 */
	version: string;
	description: string;
	params: ParamDef[];
	inputTypes?: PortType[];
	outputTypes?: PortType[];
	official?: boolean;
	/** 是否为可复用内置节点（可在流水线中放置多次，需要 nodeId 区分实例）*/
	reusable?: boolean;
	i18n?: Record<string, ModuleI18nLocale>;
}

/** 单个输入端口的连接来源 / Wiring source for a single input port */
export interface NodeInputRef {
	/** 上游节点 ID（"0" 表示录制输入节点）/ Upstream node ID ("0" = recording input node) */
	nodeId: string;
	/** 上游节点的输出端口索引 / Upstream output port index */
	port: number;
}

/** 流水线节点（模块实例）/ Pipeline node (module instance) */
export interface PipelineNode {
	/** 模块 ID，同时也是普通节点的唯一标识 / Module ID, also serves as unique identifier for regular nodes */
	moduleId: string;
	/** 节点实例 ID（仅可复用内置节点的多个实例需要）/ Node instance ID (only for multiple instances of reusable built-in nodes) */
	nodeId?: string;
	/** 节点参数值 / Node parameter values */
	params: Record<string, string | number | boolean>;
	/** 是否启用此节点 / Whether this node is enabled */
	enabled: boolean;
	/** 节点在画布中的位置 / Node position on canvas */
	position?: { x: number; y: number };
	/**
	 * 输入端口的连接来源：端口索引（字符串化）→ 连接信息。
	 * nodeId="0" 表示录制输入节点。
	 */
	inputs?: Record<string, NodeInputRef>;
}

/** 辅助函数：返回节点的有效唯一标识 / Helper: returns the effective unique identifier of a node */
export function nodeEffectiveId(node: PipelineNode): string {
	return node.nodeId ?? node.moduleId;
}

/**
 * DAG 有向边，连接上游节点的输出端口到下游节点的输入端口。
 * 仅用作运行时视图（见 resolvedEdges），不持久化——连线信息唯一存储在
 * PipelineNode.inputs 中，避免两处数据不同步。
 *
 * DAG directed edge, connecting an upstream output port to a downstream input port.
 * Runtime view only (see resolvedEdges); not persisted — wiring is stored exclusively
 * in PipelineNode.inputs to avoid two sources of truth going out of sync.
 */
export interface PipelineEdge {
	fromNodeId: string;
	fromPort: number;
	toNodeId: string;
	toPort: number;
}

/** 流水线配置 / Pipeline configuration */
export interface PipelineConfig {
	/** 配置格式版本号（用于向后兼容和更新检测）/ Configuration format version (for backward compatibility and update detection) */
	version?: string;
	nodes: PipelineNode[];
	/** 虚拟录制输入节点在画布中的位置 / Virtual recording input node position on canvas */
	inputNodePosition?: { x: number; y: number };
}

/**
 * 统计当前流水线配置中"启用且非内置"的节点总数。
 * 这是后处理总进度的权威分母——不应从 meta 的 pp_execution 记录数推算，
 * 因为流水线刚开始执行、或部分重新触发（跳过未变更节点）时，
 * pp_execution 的条目数并不等于流水线的真实总节点数。
 *
 * Count the "enabled and non-builtin" nodes in the current pipeline config.
 * This is the authoritative denominator for overall post-processing progress —
 * it should not be derived from the number of pp_execution entries in meta,
 * since that count doesn't reflect the pipeline's true total when the pipeline
 * has just started running or is partially re-triggered (unchanged nodes skipped).
 */
export function countPipelineTotal(pipeline: PipelineConfig | null | undefined): number {
	if (!pipeline?.nodes) return 0;
	return pipeline.nodes.filter(
		(n) => n.enabled && !n.moduleId.includes("__builtin__"),
	).length;
}

/**
 * 从 pipeline.nodes[].inputs 派生完整的有向边列表（唯一的连线数据来源）。
 * nodeId="0" 映射为虚拟输入节点 INPUT_NODE_ID。
 *
 * Derive the directed edge list from pipeline.nodes[].inputs (the sole source of wiring
 * data). nodeId="0" is mapped to the virtual input node INPUT_NODE_ID.
 */
export function resolvedEdges(pipeline: PipelineConfig): PipelineEdge[] {
	const edges: PipelineEdge[] = [];
	for (const node of pipeline.nodes) {
		if (!node.inputs) continue;
		for (const [portStr, ref_] of Object.entries(node.inputs)) {
			edges.push({
				fromNodeId: ref_.nodeId === "0" ? "__recording_input__" : ref_.nodeId,
				fromPort: ref_.port,
				toNodeId: nodeEffectiveId(node),
				toPort: Number(portStr),
			});
		}
	}
	return edges;
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

	const { locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();

	/**
	 * 根据当前语言对模块的 name/description/params[].label 应用 i18n 翻译。
	 * 优先使用服务器端 locale JSON（moduleLocaleStore），回退到模块 --describe 中的 i18n 字段。
	 *
	 * Apply i18n translations to module name/description/params[].label based on current locale.
	 * Prefers server-side locale JSON (moduleLocaleStore), falls back to --describe i18n field.
	 */
	function applyModuleI18n(raw: ModuleInfo[]): ModuleInfo[] {
		const lang = locale.value;
		return raw.map((mod) => {
			// 优先使用服务器端 locale JSON / Prefer server-side locale JSON
			const serverTr = moduleLocaleStore.getModuleLocale(mod.id);
			// 回退到 --describe 中的 i18n 字段 / Fall back to --describe i18n field
			const describeTr = mod.i18n?.[lang] as
				| { name?: string; description?: string; params?: Record<string, { label?: string }> }
				| undefined;

			// 合并：服务器端优先，--describe 作为补充
			// Merge: server-side takes priority, --describe fills the gaps
			const name =
				serverTr?.name ?? describeTr?.name ?? mod.name;
			const description =
				serverTr?.description ?? describeTr?.description ?? mod.description;
			const params = mod.params.map((p) => ({
				...p,
				label:
					serverTr?.params?.[p.key]?.label ??
					describeTr?.params?.[p.key]?.label ??
					p.label,
			}));

			if (!serverTr && !describeTr) return mod;
			return { ...mod, name, description, params };
		});
	}

	/** 原始模块列表（未应用 i18n，用于语言切换时重新翻译）/ Raw module list (before i18n, for re-translating on locale change) */
	const _rawModules = ref<ModuleInfo[]>([]);

	/**
	 * 从后端获取可用模块列表。
	 * Fetch the available module list from the backend.
	 */
	async function fetchModules() {
		const raw = await call<ModuleInfo[]>("list_modules");
		_rawModules.value = raw;
		modules.value = applyModuleI18n(raw);
	}

	// 语言切换时重新应用模块翻译 / Re-apply module translations on locale change
	watch([locale, () => moduleLocaleStore.locales], () => {
		if (_rawModules.value.length > 0) {
			modules.value = applyModuleI18n(_rawModules.value);
		}
	});

	/**
	 * 从后端获取当前流水线配置。
	 * Fetch the current pipeline configuration from the backend.
	 */
	async function fetchPipeline() {
		loading.value = true;
		try {
			const raw = await call<PipelineConfig>("get_pipeline");
			// 兼容旧格式（无 version 字段）/ Compat with old format (no version field)
			pipeline.value = {
				version: raw.version ?? "1",
				nodes: raw.nodes ?? [],
				inputNodePosition: raw.inputNodePosition,
			};
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
			// 确保包含版本号 / Ensure version is included
			const configToSave: PipelineConfig = {
				...pipeline.value,
				version: pipeline.value.version ?? "1",
			};
			await call("save_pipeline", { pipeline: configToSave });
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
	 * @param position - 节点初始位置 / Initial node position
	 */
	function addNode(moduleId: string, position?: { x: number; y: number }) {
		const mod = modules.value.find((m) => m.id === moduleId);
		if (!mod) return;
		const defaults: Record<string, string | number | boolean> = {};
		for (const p of mod.params) {
			defaults[p.key] = coerceDefault(p.type, p.default);
		}
		// 普通节点不需要 nodeId（moduleId 即唯一标识）；
		// 可复用内置节点需要 nodeId 以区分多个实例。
		// Regular nodes don't need nodeId (moduleId is unique);
		// reusable built-in nodes need nodeId to distinguish multiple instances.
		const node: PipelineNode = {
			moduleId,
			params: defaults,
			enabled: true,
			position: position ?? { x: 200 + pipeline.value.nodes.length * 40, y: 200 },
		};
		if (mod.reusable) {
			node.nodeId = generateId();
		}
		pipeline.value.nodes.push(node);
	}

	/**
	 * 更新节点在画布中的位置。
	 * Update a node's position on the canvas.
	 */
	function updateNodePosition(nodeId: string, pos: { x: number; y: number }) {
		const node = pipeline.value.nodes.find((n) => nodeEffectiveId(n) === nodeId);
		if (node) node.position = pos;
	}

	/**
	 * 添加一条连线：写入目标节点 inputs[toPort]（同一输入端口只能有一个上游，自动覆盖旧连接）。
	 * 连线信息唯一存储在 node.inputs，不再维护单独的 edges 数组。
	 *
	 * Add a wire: writes to the target node's inputs[toPort] (each input port has at most
	 * one upstream; overwrites any existing connection). Wiring lives solely in node.inputs.
	 */
	function addEdge(edge: PipelineEdge) {
		const idx = pipeline.value.nodes.findIndex((n) => nodeEffectiveId(n) === edge.toNodeId);
		if (idx !== -1) {
			const node = pipeline.value.nodes[idx];
			const upstreamId = edge.fromNodeId === "__recording_input__" ? "0" : edge.fromNodeId;
			pipeline.value.nodes[idx] = {
				...node,
				inputs: {
					...(node.inputs ?? {}),
					[String(edge.toPort)]: { nodeId: upstreamId, port: edge.fromPort },
				},
			};
		}
	}

	/**
	 * 移除指定节点相关的所有连线：清空其自身 inputs，并从其他节点的 inputs 中删除
	 * 指向该节点的条目。
	 * Remove all wires touching a node: clears its own inputs, and removes entries
	 * pointing to it from other nodes' inputs.
	 */
	function removeEdgesForNode(nodeId: string) {
		const targetIdx = pipeline.value.nodes.findIndex((n) => nodeEffectiveId(n) === nodeId);
		if (targetIdx !== -1) {
			pipeline.value.nodes[targetIdx] = { ...pipeline.value.nodes[targetIdx], inputs: {} };
		}

		for (let i = 0; i < pipeline.value.nodes.length; i++) {
			const node = pipeline.value.nodes[i];
			if (!node.inputs) continue;
			const newInputs = Object.fromEntries(
				Object.entries(node.inputs).filter(([, ref_]) => ref_.nodeId !== nodeId),
			);
			if (Object.keys(newInputs).length !== Object.keys(node.inputs).length) {
				pipeline.value.nodes[i] = { ...node, inputs: newInputs };
			}
		}
	}

	/** 移除一条具体连线（从目标节点的 inputs 中删除对应端口条目）/ Remove a specific wire (deletes the port entry from the target node's inputs) */
	function removeEdge(fromNodeId: string, fromPort: number, toNodeId: string, toPort: number) {
		const idx = pipeline.value.nodes.findIndex((n) => nodeEffectiveId(n) === toNodeId);
		if (idx === -1) return;
		const node = pipeline.value.nodes[idx];
		const existing = node.inputs?.[String(toPort)];
		// 仅当该端口确实连接自 fromNodeId/fromPort 时才移除，避免误删已被覆盖的新连线
		// Only remove if the port is indeed wired from fromNodeId/fromPort, to avoid
		// deleting a newer connection that has since overwritten this port
		const expectedFromId = fromNodeId === "__recording_input__" ? "0" : fromNodeId;
		if (existing && existing.nodeId === expectedFromId && existing.port === fromPort) {
			const { [String(toPort)]: _removed, ...rest } = node.inputs!;
			pipeline.value.nodes[idx] = { ...node, inputs: rest };
		}
	}

	function removeNode(nodeId: string) {
		pipeline.value.nodes = pipeline.value.nodes.filter(
			(n) => nodeEffectiveId(n) !== nodeId,
		);
		removeEdgesForNode(nodeId);
	}

	function moveNode(nodeId: string, direction: "up" | "down") {
		const idx = pipeline.value.nodes.findIndex((n) => nodeEffectiveId(n) === nodeId);
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
			if (_isSavingLocally) return;
			_loaded = false;
			const raw = payload as PipelineConfig;
			pipeline.value = {
				version: raw.version ?? "1",
				nodes: raw.nodes ?? [],
				inputNodePosition: raw.inputNodePosition,
			};
			setTimeout(() => { _loaded = true; }, 0);
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
		updateNodePosition,
		addEdge,
		removeEdge,
		removeEdgesForNode,
		initModuleWatcher,
	};
});
