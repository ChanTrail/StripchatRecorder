<!--
    后处理流水线节点图编辑器 / Post-processing Pipeline Node Graph Editor

    UE/ComfyUI 风格的节点图编辑器，支持：
    - 节点拖拽放置
    - 端口连线（基于类型兼容性）
    - 画布平移和缩放
    - 右键画布添加节点（按上下文过滤）
    - 节点参数内联编辑
    - ts_merge 作为官方首节点（带徽章提示）

    UE/ComfyUI-style node graph editor supporting:
    - Node drag placement
    - Port wiring (with type compatibility)
    - Canvas pan and zoom
    - Right-click canvas to add nodes (context-filtered)
    - Inline node parameter editing
    - ts_merge as the official first node (with badge hint)
-->
<script setup lang="ts">
import { onMounted, computed, ref, reactive, onUnmounted, watch, nextTick } from "vue";
import {
	usePostprocessStore,
	type PipelineNode,
	type PipelineEdge,
	type PortType,
	isPortCompatible,
	PORT_TYPE_COLORS,
} from "@/stores/postprocess";
import { useNotify } from "@/composables/useNotify";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import {
	NumberField,
	NumberFieldContent,
	NumberFieldDecrement,
	NumberFieldIncrement,
	NumberFieldInput,
} from "@/components/ui/number-field";
import { Info, Maximize2 } from "@lucide/vue";
import { useI18n } from "vue-i18n";

// ─── 常量 / Constants ─────────────────────────────────────────────────────────

/** 虚拟录制输入节点的固定 ID / Fixed ID for the virtual recording input node */
const INPUT_NODE_ID = "__recording_input__";

const store = usePostprocessStore();
const { toast } = useNotify();
const { t } = useI18n();

onMounted(async () => {
	await Promise.all([store.fetchModules(), store.fetchPipeline()]);
	store.initModuleWatcher(() => toast(t("postprocess.updatedByOther"), "info"));
	autoLayoutNodes();
	ensureInputToTsMergeEdge();
	// DOM 渲染完成后自适应居中所有节点 / Fit all nodes to canvas after DOM renders
	await nextTick();
	fitView();
});

/**
 * 若流水线中存在 ts_merge 节点，且尚无从虚拟输入节点到它的连线，
 * 则自动补上该连线（虚拟输入端口 0 → ts_merge 输入端口 0）。
 *
 * If the pipeline contains a ts_merge node and no edge yet exists from the
 * virtual input node to it, automatically add that edge
 * (virtual input port 0 → ts_merge input port 0).
 */
function ensureInputToTsMergeEdge() {
	const tsMergeNode = store.pipeline.nodes.find((n) => n.moduleId === "ts_merge");
	if (!tsMergeNode) return;
	const alreadyConnected = store.pipeline.edges.some(
		(e) => e.fromNodeId === INPUT_NODE_ID && e.toNodeId === tsMergeNode.nodeId,
	);
	if (!alreadyConnected) {
		store.addEdge({
			fromNodeId: INPUT_NODE_ID,
			fromPort: 0,
			toNodeId: tsMergeNode.nodeId,
			toPort: 0,
		});
	}
}

// ─── 画布变换 / Canvas Transform ─────────────────────────────────────────────

const canvasRef = ref<HTMLElement | null>(null);
const transform = reactive({ x: 0, y: 0, scale: 1 });

function screenToCanvas(sx: number, sy: number) {
	const rect = canvasRef.value?.getBoundingClientRect();
	if (!rect) return { x: 0, y: 0 };
	return {
		x: (sx - rect.left - transform.x) / transform.scale,
		y: (sy - rect.top - transform.y) / transform.scale,
	};
}

let isPanning = false;
let panStart = { x: 0, y: 0 };

function onCanvasMousedown(e: MouseEvent) {
	if (e.button !== 0) return;
	if ((e.target as HTMLElement).closest(".pipeline-node")) return;
	closeContextMenu();
	closeWireMenu();
	isPanning = true;
	panStart = { x: e.clientX - transform.x, y: e.clientY - transform.y };
}

function onCanvasMousemove(e: MouseEvent) {
	if (!isPanning) return;
	transform.x = e.clientX - panStart.x;
	transform.y = e.clientY - panStart.y;
}

function onCanvasMouseup() {
	isPanning = false;
}

function onCanvasWheel(e: WheelEvent) {
	e.preventDefault();
	const factor = e.deltaY < 0 ? 1.1 : 0.9;
	const rect = canvasRef.value!.getBoundingClientRect();
	const cx = e.clientX - rect.left;
	const cy = e.clientY - rect.top;
	transform.x = cx - (cx - transform.x) * factor;
	transform.y = cy - (cy - transform.y) * factor;
	transform.scale = Math.min(3, Math.max(0.2, transform.scale * factor));
}

function autoLayoutNodes() {
	const nodes = store.pipeline.nodes;
	const spacing = { x: 280, y: 60 };
	nodes.forEach((n, i) => {
		if (!n.position) {
			n.position = { x: 60 + i * spacing.x, y: spacing.y };
		}
	});
}

/**
 * 自适应居中：计算所有节点（含虚拟输入节点）的包围盒，
 * 调整 transform 使所有内容居中显示在画布内，带 padding。
 * 优先从 DOM 读取真实宽高，无法读取时用估算值。
 *
 * Fit view: compute bounding box of all nodes (including virtual input node),
 * adjust transform so all content is centered in the canvas with padding.
 * Reads actual DOM sizes when available, falls back to estimates.
 */
function fitView(padding = 60) {
	const canvasRect = canvasRef.value?.getBoundingClientRect();
	if (!canvasRect || canvasRect.width === 0 || canvasRect.height === 0) return;

	// 收集所有节点的画布坐标和尺寸 / Collect canvas coords and sizes of all nodes
	interface NodeBox { x: number; y: number; w: number; h: number }
	const boxes: NodeBox[] = [];

	// 虚拟输入节点 / Virtual input node
	const inputEl = canvasRef.value?.querySelector(".pipeline-node") as HTMLElement | null;
	const inputW = inputEl?.offsetWidth ?? 176;
	const inputH = inputEl?.offsetHeight ?? 80;
	boxes.push({ x: inputNodePos.x, y: inputNodePos.y, w: inputW, h: inputH });

	// 普通节点：从 DOM 读取或估算 / Regular nodes: read from DOM or estimate
	const nodeEls = canvasRef.value?.querySelectorAll(".pipeline-node");
	store.pipeline.nodes.forEach((node, i) => {
		const x = node.position?.x ?? 0;
		const y = node.position?.y ?? 0;
		// nodeEls[0] 是虚拟输入节点，普通节点从 index 1 开始
		// nodeEls[0] is the virtual input node; regular nodes start at index 1
		const el = nodeEls ? (nodeEls[i + 1] as HTMLElement | undefined) : undefined;
		const w = el?.offsetWidth ?? 224;
		const h = el?.offsetHeight ?? 120;
		boxes.push({ x, y, w, h });
	});

	if (boxes.length === 0) return;

	// 计算包围盒 / Compute bounding box
	let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
	for (const b of boxes) {
		minX = Math.min(minX, b.x);
		minY = Math.min(minY, b.y);
		maxX = Math.max(maxX, b.x + b.w);
		maxY = Math.max(maxY, b.y + b.h);
	}

	const contentW = maxX - minX;
	const contentH = maxY - minY;
	if (contentW <= 0 || contentH <= 0) return;

	const availW = canvasRect.width - padding * 2;
	const availH = canvasRect.height - padding * 2;

	// 计算适合的缩放比例（不超过 1，即不放大）/ Calculate scale to fit (cap at 1, no upscaling)
	const scaleX = availW / contentW;
	const scaleY = availH / contentH;
	const newScale = Math.min(1, scaleX, scaleY);

	// 居中偏移 / Centering offset
	const newX = (canvasRect.width - contentW * newScale) / 2 - minX * newScale;
	const newY = (canvasRect.height - contentH * newScale) / 2 - minY * newScale;

	transform.scale = newScale;
	transform.x = newX;
	transform.y = newY;
}

// ─── 节点拖拽 / Node Dragging ─────────────────────────────────────────────────

let draggingNodeId: string | null = null;
let draggingInput = false;
let dragOffset = { x: 0, y: 0 };

function onNodeMousedown(e: MouseEvent, nodeId: string) {
	if (e.button !== 0) return;
	e.stopPropagation();
	// 如果正在拖连线，不启动节点拖拽 / Don't start node drag while wiring
	if (connectingFrom.value) return;
	draggingNodeId = nodeId;
	const node = store.pipeline.nodes.find((n) => n.nodeId === nodeId)!;
	const canvasPos = screenToCanvas(e.clientX, e.clientY);
	dragOffset = {
		x: canvasPos.x - (node.position?.x ?? 0),
		y: canvasPos.y - (node.position?.y ?? 0),
	};
	selectedNodeId.value = nodeId;
}

function onInputNodeMousedown(e: MouseEvent) {
	if (e.button !== 0) return;
	e.stopPropagation();
	if (connectingFrom.value) return;
	draggingInput = true;
	const canvasPos = screenToCanvas(e.clientX, e.clientY);
	dragOffset = { x: canvasPos.x - inputNodePos.x, y: canvasPos.y - inputNodePos.y };
}

function onGlobalMousemove(e: MouseEvent) {
	if (draggingNodeId) {
		const pos = screenToCanvas(e.clientX, e.clientY);
		store.updateNodePosition(draggingNodeId, {
			x: pos.x - dragOffset.x,
			y: pos.y - dragOffset.y,
		});
	}
	if (draggingInput) {
		const pos = screenToCanvas(e.clientX, e.clientY);
		inputNodePos.x = pos.x - dragOffset.x;
		inputNodePos.y = pos.y - dragOffset.y;
	}
	if (connectingFrom.value) {
		pendingLine.value = screenToCanvas(e.clientX, e.clientY);
	}
	onCanvasMousemove(e);
}

function onGlobalMouseup(e: MouseEvent) {
	draggingNodeId = null;
	draggingInput = false;
	onCanvasMouseup();
	// 连线释放：若未落在输入端口上，弹出模块选择菜单
	// Wire drop: if not on an input port, show module selection menu
	if (connectingFrom.value) {
		const fromPort = connectingFrom.value;
		const dropPos = pendingLine.value;
		connectingFrom.value = null;
		pendingLine.value = null;
		// 有释放位置就弹菜单（不再检查移动距离，因为端口位置可能未缓存）
		// Show menu whenever there's a drop position (no distance check — port pos may not be cached yet)
		if (dropPos) {
			const rect = canvasRef.value?.getBoundingClientRect();
			wireMenu.fromPort = fromPort;
			wireMenu.canvasPos = dropPos;
			wireMenu.x = e.clientX - (rect?.left ?? 0);
			wireMenu.y = e.clientY - (rect?.top ?? 0);
			wireMenu.visible = true;
		}
	}
}

// ─── 端口连线 / Port Wiring ───────────────────────────────────────────────────

interface PortRef {
	nodeId: string;
	portIndex: number;
	isOutput: boolean;
	type: PortType;
}

const connectingFrom = ref<PortRef | null>(null);
const pendingLine = ref<{ x: number; y: number } | null>(null);

/** 所有端口的画布坐标位置缓存（普通对象，不触发响应式）/ Canvas-coordinate position cache (plain object, non-reactive) */
const portPositions: Record<string, { x: number; y: number }> = {};
/** 端口位置版本号，写入后递增，让 SVG 重算连线 / Incremented on port position write to trigger SVG edge recompute */
const portVersion = ref(0);

function portKey(nodeId: string, isOutput: boolean, portIndex: number) {
	return `${nodeId}:${isOutput ? "o" : "i"}:${portIndex}`;
}

/** 将端口 DOM 元素的中心转换为画布坐标并缓存 / Convert port element center to canvas coords and cache */
function registerPortEl(el: HTMLElement | null, nodeId: string, isOutput: boolean, portIndex: number) {
	if (!el) return;
	// 同步读取 getBoundingClientRect（此时 DOM 已挂载，不需要 nextTick）
	// Read getBoundingClientRect synchronously (DOM is mounted, nextTick not needed)
	const rect = el.getBoundingClientRect();
	const canvasRect = canvasRef.value?.getBoundingClientRect();
	if (!canvasRect) return;
	const key = portKey(nodeId, isOutput, portIndex);
	const nx = (rect.left + rect.width / 2 - canvasRect.left - transform.x) / transform.scale;
	const ny = (rect.top + rect.height / 2 - canvasRect.top - transform.y) / transform.scale;
	const prev = portPositions[key];
	// 坐标变化超过 1px 才更新，触发一次 SVG 重算 / Only update on >1px change, triggers one SVG recompute
	if (!prev || Math.abs(prev.x - nx) > 1 || Math.abs(prev.y - ny) > 1) {
		portPositions[key] = { x: nx, y: ny };
		portVersion.value++;
	}
}

function onPortMousedown(e: MouseEvent, portRef: PortRef) {
	e.stopPropagation();
	// 注意：不调用 preventDefault()，否则会阻断后续 mouseup 事件的分发
	// Note: do NOT call preventDefault() here — it breaks mouseup dispatch
	if (portRef.isOutput) {
		connectingFrom.value = portRef;
		pendingLine.value = screenToCanvas(e.clientX, e.clientY);
	}
}

function onPortMouseup(e: MouseEvent, target: PortRef) {
	e.stopPropagation();
	if (!connectingFrom.value) return;
	// 只处理：从输出端口连到输入端口
	// Only handle: output port → input port
	if (connectingFrom.value.isOutput && !target.isOutput) {
		if (isPortCompatible(connectingFrom.value.type, target.type)) {
			store.addEdge({
				fromNodeId: connectingFrom.value.nodeId,
				fromPort: connectingFrom.value.portIndex,
				toNodeId: target.nodeId,
				toPort: target.portIndex,
			});
		}
	}
	connectingFrom.value = null;
	pendingLine.value = null;
}

/** 计算连线的 SVG 路径（贝塞尔曲线）/ Compute bezier path for an edge */
function edgePath(from: { x: number; y: number }, to: { x: number; y: number }): string {
	const dx = Math.max(80, Math.abs(to.x - from.x) * 0.5);
	return `M ${from.x} ${from.y} C ${from.x + dx} ${from.y}, ${to.x - dx} ${to.y}, ${to.x} ${to.y}`;
}

/** 获取边的 from/to 位置（依赖 portVersion 触发重算）/ Get edge from/to positions (depends on portVersion for reactivity) */
function edgePositions(edge: PipelineEdge) {
	void portVersion.value; // 依赖 portVersion，确保端口位置更新时 SVG 重算 / depend on portVersion so SVG recomputes on port position change
	const from = portPositions[portKey(edge.fromNodeId, true, edge.fromPort)];
	const to = portPositions[portKey(edge.toNodeId, false, edge.toPort)];
	return { from, to };
}

/** 获取正在拖拽连线的起点画布坐标（依赖 portVersion）/ Get pending wire origin canvas position */
function pendingLineFrom() {
	void portVersion.value;
	return connectingFrom.value
		? portPositions[portKey(connectingFrom.value.nodeId, true, connectingFrom.value.portIndex)]
		: null;
}

// ─── 连线释放菜单 / Wire-drop Module Menu ────────────────────────────────────

/** 从连线拖拽释放后弹出的模块选择菜单状态 / Module menu shown when a wire is dropped on empty canvas */
const wireMenu = reactive<{
	visible: boolean;
	x: number;
	y: number;
	canvasPos: { x: number; y: number };
	fromPort: PortRef | null;
}>({ visible: false, x: 0, y: 0, canvasPos: { x: 0, y: 0 }, fromPort: null });

function closeWireMenu() {
	wireMenu.visible = false;
	wireMenu.fromPort = null;
}

/**
 * 连线释放时兼容的模块列表：
 * - 未已使用，且输入端口 0 类型与当前输出端口兼容
 * Modules compatible with the current output port type when dropping a wire.
 */
const wireMenuModules = computed(() => {
	if (!wireMenu.fromPort) return [];
	const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
	return store.modules.filter((m) => {
		if (used.has(m.id)) return false;
		// 取模块的第一个输入类型做兼容性检查 / Check compatibility with first input type
		const firstInput = (m.inputTypes?.[0] ?? "any_file") as PortType;
		return isPortCompatible(wireMenu.fromPort!.type, firstInput);
	});
});

/**
 * 从连线菜单中选择模块：在释放位置添加节点并自动连线。
 * Select a module from the wire menu: add node at drop position and auto-wire.
 */
function addModuleFromWire(moduleId: string) {
	if (!wireMenu.fromPort) return;
	const pos = { ...wireMenu.canvasPos };
	// 偏移一点，避免和源节点重叠 / Offset slightly to avoid overlap with source node
	pos.x += 20;
	store.addNode(moduleId, pos);
	// 找到刚添加的节点（最后一个）/ Find the just-added node (last one)
	const newNode = store.pipeline.nodes[store.pipeline.nodes.length - 1];
	if (newNode) {
		store.addEdge({
			fromNodeId: wireMenu.fromPort.nodeId,
			fromPort: wireMenu.fromPort.portIndex,
			toNodeId: newNode.nodeId,
			toPort: 0,
		});
	}
	closeWireMenu();
}

const selectedNodeId = ref<string | null>(null);

function selectNode(nodeId: string) {
	selectedNodeId.value = nodeId;
	closeContextMenu();
}

// ─── 右键上下文菜单 / Context Menu ───────────────────────────────────────────

const contextMenu = reactive<{
	visible: boolean;
	x: number;
	y: number;
	canvasPos: { x: number; y: number };
}>({ visible: false, x: 0, y: 0, canvasPos: { x: 0, y: 0 } });

function openContextMenu(e: MouseEvent) {
	e.preventDefault();
	if ((e.target as HTMLElement).closest(".pipeline-node")) return;
	const rect = canvasRef.value?.getBoundingClientRect();
	contextMenu.canvasPos = screenToCanvas(e.clientX, e.clientY);
	contextMenu.x = e.clientX - (rect?.left ?? 0);
	contextMenu.y = e.clientY - (rect?.top ?? 0);
	contextMenu.visible = true;
	closeWireMenu();
	selectedNodeId.value = null;
}

function closeContextMenu() {
	contextMenu.visible = false;
}

function onCanvasClick() {
	// 点击画布空白处：取消选中、关闭右键菜单
	// 若 wireMenu 是刚刚由本次 mouseup 弹出的，不关闭它（click 紧跟 mouseup）
	// Click on canvas: deselect, close context menu.
	// If wireMenu was just opened by this mouseup, don't close it (click follows mouseup immediately)
	selectedNodeId.value = null;
	closeContextMenu();
	// wireMenu 仅在连线操作结束后弹出，不在普通 click 时关闭
	// wireMenu is only shown after wiring ends; don't close it on plain canvas click
	// (it closes itself when user picks a module or clicks its own cancel button)
}

function deselectAll() {
	selectedNodeId.value = null;
	closeContextMenu();
	// 注意：不在这里关闭 wireMenu，因为 click 紧跟 mouseup，会导致菜单一弹即关
	// Note: do NOT close wireMenu here — click follows mouseup, which would instantly close the menu
}

function addModuleAtCursor(moduleId: string) {
	store.addNode(moduleId, { ...contextMenu.canvasPos });
	closeContextMenu();
}

/**
 * 右键菜单中显示的模块列表：可选且未使用的模块。
 * Modules shown in context menu: available and not yet in the pipeline.
 */
const contextMenuModules = computed(() => {
	const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
	return store.modules.filter((m) => !used.has(m.id));
});

onMounted(() => {
	window.addEventListener("mousemove", onGlobalMousemove);
	window.addEventListener("mouseup", onGlobalMouseup);
	window.addEventListener("keydown", onGlobalKeydown);
});
onUnmounted(() => {
	window.removeEventListener("mousemove", onGlobalMousemove);
	window.removeEventListener("mouseup", onGlobalMouseup);
	window.removeEventListener("keydown", onGlobalKeydown);
});

/** Delete/Backspace 键删除当前选中节点 / Delete/Backspace key removes the selected node */
function onGlobalKeydown(e: KeyboardEvent) {
	if (e.key !== "Delete" && e.key !== "Backspace") return;
	const tag = (e.target as HTMLElement)?.tagName?.toLowerCase();
	if (tag === "input" || tag === "textarea" || (e.target as HTMLElement)?.isContentEditable) return;
	if (selectedNodeId.value) {
		store.removeNode(selectedNodeId.value);
		selectedNodeId.value = null;
	}
}

/** 虚拟输入节点的画布位置（持久化到 localStorage）/ Virtual input node canvas position (persisted to localStorage) */
const INPUT_NODE_POS_KEY = "pp_input_node_pos";
const savedInputPos = (() => {
	try { return JSON.parse(localStorage.getItem(INPUT_NODE_POS_KEY) ?? "null"); } catch { return null; }
})();
const inputNodePos = reactive<{ x: number; y: number }>(savedInputPos ?? { x: 40, y: 80 });

watch(inputNodePos, (v) => {
	localStorage.setItem(INPUT_NODE_POS_KEY, JSON.stringify({ x: v.x, y: v.y }));
});

function getModuleInfo(moduleId: string) {
	return store.modules.find((m) => m.id === moduleId);
}

function getNodeInputTypes(moduleId: string): PortType[] {
	const info = getModuleInfo(moduleId);
	return info?.inputTypes ?? ["any_file"];
}

function getNodeOutputTypes(moduleId: string): PortType[] {
	const info = getModuleInfo(moduleId);
	return info?.outputTypes ?? ["any_file"];
}
</script>

<template>
	<div class="flex flex-col h-full gap-0">
		<!-- 顶部工具栏 / Top toolbar -->
		<header class="flex items-start justify-between gap-4 shrink-0 pb-4 bg-background sticky top-0 z-20 px-6 pt-6 border-b">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("postprocess.title") }}</h1>
				<p class="text-sm text-muted-foreground">{{ t("postprocess.description") }}</p>
			</div>
			<div class="flex items-center gap-2 shrink-0">
				<button
					class="flex items-center gap-1.5 px-2.5 py-1 text-xs text-muted-foreground border rounded-md hover:bg-accent hover:text-foreground transition-colors"
					:title="t('postprocess.fitView')"
					@click="fitView()"
				>
					<Maximize2 class="size-3.5" />
					{{ t("postprocess.fitView") }}
				</button>
				<span class="text-sm text-muted-foreground">
					{{ store.saving ? t("postprocess.saving") : t("postprocess.saved") }}
				</span>
			</div>
		</header>

		<!-- 画布区域 / Canvas area -->
		<div
			ref="canvasRef"
			class="relative flex-1 overflow-hidden bg-[#0d0d0d] select-none cursor-default"
			style="background-image: radial-gradient(circle, #2a2a2a 1px, transparent 1px); background-size: 24px 24px;"
			@mousedown="onCanvasMousedown"
			@wheel.prevent="onCanvasWheel"
			@contextmenu.prevent="openContextMenu"
			@click="onCanvasClick"
		>
			<!-- 变换层 / Transform layer -->
			<div
				class="absolute top-0 left-0 origin-top-left"
				:style="{
					transform: `translate(${transform.x}px, ${transform.y}px) scale(${transform.scale})`,
				}"
			>
				<!-- SVG 连线层 / SVG wiring layer -->
				<svg
					class="absolute inset-0 overflow-visible pointer-events-none"
					style="left:0;top:0;width:1px;height:1px;"
				>
					<!-- 已有的边 / Existing edges -->
					<g v-for="edge in store.pipeline.edges" :key="`${edge.fromNodeId}-${edge.fromPort}-${edge.toNodeId}-${edge.toPort}`">
						<template v-if="edgePositions(edge).from && edgePositions(edge).to">
							<path
								:d="edgePath(edgePositions(edge).from!, edgePositions(edge).to!)"
								fill="none"
								stroke="#ffffff30"
								stroke-width="2"
								class="pointer-events-auto cursor-pointer hover:stroke-destructive transition-colors"
								@click.stop="store.removeEdge(edge.fromNodeId, edge.fromPort, edge.toNodeId, edge.toPort)"
							/>
						</template>
					</g>
					<!-- 正在拖拽的连线预览 / Pending connection preview -->
					<template v-if="connectingFrom && pendingLine">
						<path
							:d="edgePath(
								pendingLineFrom() ?? pendingLine,
								pendingLine,
							)"
							fill="none"
							:stroke="PORT_TYPE_COLORS[connectingFrom.type]"
							stroke-width="2"
							stroke-dasharray="6 3"
						/>
					</template>
				</svg>

				<!-- 节点 / Nodes -->
				<!-- 虚拟录制输入节点 / Virtual recording input node -->
				<div
					class="pipeline-node absolute"
					:style="{ left: `${inputNodePos.x}px`, top: `${inputNodePos.y}px`, zIndex: 1 }"
					@mousedown.stop
				>
					<div class="rounded-xl border border-amber-500/40 bg-card shadow-xl min-w-44">
						<div class="px-3 py-2 cursor-move" @mousedown.stop="onInputNodeMousedown($event)">
							<div class="flex items-center gap-1.5">
								<span class="text-xs font-semibold text-amber-400">{{ t("postprocess.input.label") }}</span>
								<Badge class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30">source</Badge>
							</div>
							<p class="text-[10px] text-muted-foreground mt-0.5">{{ t("postprocess.input.description") }}</p>
						</div>
						<div class="flex justify-end px-3 pb-2">
							<div class="flex items-center gap-1.5">
								<span class="text-[9px] text-muted-foreground">ts session dir</span>
								<div
									:ref="(el) => registerPortEl(el as HTMLElement | null, INPUT_NODE_ID, true, 0)"
									class="w-3 h-3 rounded-full border-2 cursor-crosshair -mr-4.5 shrink-0 transition-transform hover:scale-125"
									:style="{ borderColor: PORT_TYPE_COLORS['ts_session_dir'], backgroundColor: PORT_TYPE_COLORS['ts_session_dir'] + '40' }"
									title="ts_session_dir"
									@mousedown.stop="onPortMousedown($event, { nodeId: INPUT_NODE_ID, portIndex: 0, isOutput: true, type: 'ts_session_dir' })"
								/>
							</div>
						</div>
					</div>
				</div>

				<!-- 常规节点 / Regular nodes -->
				<div
					v-for="node in store.pipeline.nodes"
					:key="node.nodeId"
					class="pipeline-node absolute"
					:style="{
						left: `${node.position?.x ?? 0}px`,
						top: `${node.position?.y ?? 0}px`,
						zIndex: selectedNodeId === node.nodeId ? 10 : 1,
					}"
					@mousedown.stop="onNodeMousedown($event, node.nodeId)"
					@click.stop="selectNode(node.nodeId)"
				>
					<div
						class="rounded-xl border bg-card shadow-xl min-w-64 max-w-96 transition-colors"
						:class="[
							!node.enabled && 'opacity-50',
							selectedNodeId === node.nodeId ? 'border-primary' : 'border-white/10',
						]"
					>
						<!-- 节点头部 / Node header -->
						<div class="flex items-center gap-2 px-3 py-2 border-b border-white/5 cursor-move">
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-1.5 flex-wrap">
									<span class="text-xs font-semibold leading-none truncate">
										{{ getModuleInfo(node.moduleId)?.name ?? node.moduleId }}
									</span>
									<Badge
										v-if="getModuleInfo(node.moduleId)?.official"
										class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30"
									>official</Badge>
									<Badge
										v-if="!store.modules.some(m => m.id === node.moduleId)"
										variant="destructive"
										class="text-[9px] px-1 py-0 h-4"
									>{{ t("postprocess.node.missing") }}</Badge>
									<Badge
										v-else-if="!node.enabled"
										variant="secondary"
										class="text-[9px] px-1 py-0 h-4"
									>{{ t("postprocess.node.skipped") }}</Badge>
								</div>
								<p class="text-[10px] text-muted-foreground leading-none mt-0.5 truncate">
									{{ getModuleInfo(node.moduleId)?.description }}
								</p>
							</div>
							<div class="flex items-center gap-1 shrink-0">
								<Switch
									:id="`enable-${node.nodeId}`"
									:model-value="node.enabled"
									class="scale-75"
									@update:model-value="node.enabled = !!$event"
									@click.stop
								/>
							</div>
						</div>

						<!-- 端口区域 / Ports area -->
						<div class="flex gap-2 px-3 py-2">
							<!-- 输入端口 / Input ports -->
							<div class="flex flex-col gap-2 items-start shrink-0">
								<div
									v-for="(type, i) in getNodeInputTypes(node.moduleId)"
									:key="`in-${i}`"
									class="flex items-center gap-1.5"
								>
									<div
										:ref="(el) => registerPortEl(el as HTMLElement | null, node.nodeId, false, i)"
										class="w-3 h-3 rounded-full border-2 cursor-crosshair -ml-4.5 shrink-0 transition-transform hover:scale-125"
										:style="{ borderColor: PORT_TYPE_COLORS[type], backgroundColor: PORT_TYPE_COLORS[type] + '40' }"
										:title="type"
										@mousedown.stop
										@mouseup.stop="onPortMouseup($event, { nodeId: node.nodeId, portIndex: i, isOutput: false, type })"
									/>
									<span class="text-[9px] text-muted-foreground">{{ type.replace(/_/g, ' ') }}</span>
								</div>
							</div>

							<!-- 参数区域 / Parameters area -->
							<div class="flex-1 min-w-0">
								<div
									v-if="getModuleInfo(node.moduleId)?.params.length"
									class="flex flex-col gap-2"
								>
									<div
										v-for="param in getModuleInfo(node.moduleId)!.params"
										:key="`${node.nodeId}__${param.key}`"
										class="flex flex-col gap-0.5"
									>
										<Label class="text-[10px] text-muted-foreground">{{ param.label }}</Label>
										<Switch
											v-if="param.type === 'boolean'"
											:model-value="node.params[param.key] === true || node.params[param.key] === 'true'"
											class="scale-75 origin-left"
											@update:model-value="node.params[param.key] = $event"
											@click.stop
										/>
										<Select
											v-else-if="param.type === 'select'"
											:model-value="String(node.params[param.key] ?? param.default)"
											@update:model-value="node.params[param.key] = String($event ?? param.default)"
										>
											<SelectTrigger size="sm" class="h-6 text-xs w-full" @click.stop>
												<SelectValue />
											</SelectTrigger>
											<SelectContent>
												<SelectItem v-for="opt in param.options" :key="opt" :value="opt" class="text-xs">
													{{ opt }}
												</SelectItem>
											</SelectContent>
										</Select>
										<NumberField
											v-else-if="param.type === 'number'"
											:model-value="Number(node.params[param.key] ?? param.default)"
											@update:model-value="node.params[param.key] = $event ?? 0"
										>
											<NumberFieldContent>
												<NumberFieldDecrement class="h-6" />
												<NumberFieldInput class="h-6 text-xs" @click.stop />
												<NumberFieldIncrement class="h-6" />
											</NumberFieldContent>
										</NumberField>
										<Input
											v-else
											:model-value="String(node.params[param.key] ?? param.default)"
											class="h-6 text-xs"
											@update:model-value="node.params[param.key] = String($event)"
											@click.stop
										/>
									</div>
								</div>
							</div>

							<!-- 输出端口 / Output ports -->
							<div class="flex flex-col gap-2 items-end shrink-0">
								<div
									v-for="(type, i) in getNodeOutputTypes(node.moduleId)"
									:key="`out-${i}`"
									class="flex items-center gap-1.5"
								>
									<span class="text-[9px] text-muted-foreground">{{ type.replace(/_/g, ' ') }}</span>
									<div
										:ref="(el) => registerPortEl(el as HTMLElement | null, node.nodeId, true, i)"
										class="w-3 h-3 rounded-full border-2 cursor-crosshair -mr-4.5 shrink-0 transition-transform hover:scale-125"
										:style="{ borderColor: PORT_TYPE_COLORS[type], backgroundColor: PORT_TYPE_COLORS[type] + '40' }"
										:title="type"
										@mousedown.stop="onPortMousedown($event, { nodeId: node.nodeId, portIndex: i, isOutput: true, type })"
									/>
								</div>
							</div>
						</div>

						<!-- official 提示 / Official hint -->
						<div
							v-if="getModuleInfo(node.moduleId)?.official"
							class="px-3 pb-2 flex items-center gap-1 text-[10px] text-amber-400/70"
						>
							<Info class="size-2.5 shrink-0" />
							<span>{{ t("postprocess.node.officialHint") }}</span>
						</div>
					</div>
				</div>
			</div>

			<!-- 空白画布提示（仅在无用户节点时显示，不含输入节点）/ Canvas hint when no user nodes exist -->
			<!-- 右键上下文菜单 / Context menu -->
			<Transition name="fade">
				<div
					v-if="contextMenu.visible"
					class="absolute z-50 min-w-44 rounded-lg border bg-popover shadow-xl py-1 text-sm"
					:style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
					@mousedown.stop
					@click.stop
				>
					<div class="px-3 py-1 text-xs text-muted-foreground font-medium">
						{{ t("postprocess.picker.title") }}
					</div>
					<div
						v-if="contextMenuModules.length === 0"
						class="px-3 py-2 text-xs text-muted-foreground"
					>
						{{ t("postprocess.picker.allAdded") }}
					</div>
					<button
						v-for="mod in contextMenuModules"
						:key="mod.id"
						class="w-full flex items-start gap-2 px-3 py-1.5 hover:bg-accent transition-colors text-left"
						@click="addModuleAtCursor(mod.id)"
					>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-1.5">
								<span class="text-sm font-medium truncate">{{ mod.name }}</span>
								<Badge
									v-if="mod.official"
									class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
								>official</Badge>
							</div>
							<p class="text-xs text-muted-foreground truncate">{{ mod.description }}</p>
						</div>
					</button>
					<div class="border-t my-1" />
					<button
						class="w-full px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent transition-colors"
						@click="closeContextMenu"
					>取消</button>
				</div>
			</Transition>

			<!-- 连线释放模块选择菜单 / Wire-drop module selection menu -->
			<Transition name="fade">
				<div
					v-if="wireMenu.visible"
					class="absolute z-50 min-w-44 rounded-lg border bg-popover shadow-xl py-1 text-sm"
					:style="{ left: `${wireMenu.x}px`, top: `${wireMenu.y}px` }"
					@mousedown.stop
					@click.stop
				>
					<div class="px-3 py-1 text-xs text-muted-foreground font-medium flex items-center gap-1.5">
						<span>连接到…</span>
						<span
							v-if="wireMenu.fromPort"
							class="px-1.5 py-0.5 rounded text-[10px] font-mono"
							:style="{ background: PORT_TYPE_COLORS[wireMenu.fromPort.type] + '30', color: PORT_TYPE_COLORS[wireMenu.fromPort.type] }"
						>{{ wireMenu.fromPort.type.replace(/_/g, ' ') }}</span>
					</div>
					<div
						v-if="wireMenuModules.length === 0"
						class="px-3 py-2 text-xs text-muted-foreground"
					>
						没有兼容的模块
					</div>
					<button
						v-for="mod in wireMenuModules"
						:key="mod.id"
						class="w-full flex items-start gap-2 px-3 py-1.5 hover:bg-accent transition-colors text-left"
						@click="addModuleFromWire(mod.id)"
					>
						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-1.5">
								<span class="text-sm font-medium truncate">{{ mod.name }}</span>
								<Badge
									v-if="mod.official"
									class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
								>official</Badge>
							</div>
							<p class="text-xs text-muted-foreground truncate">{{ mod.description }}</p>
						</div>
					</button>
					<div class="border-t my-1" />
					<button
						class="w-full px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent transition-colors"
						@click="closeWireMenu"
					>取消</button>
				</div>
			</Transition>
		</div>

		<!-- 底部信息栏 / Bottom info bar -->
		<div class="px-6 py-2.5 border-t text-sm text-muted-foreground flex items-center gap-2 shrink-0 bg-background min-h-10">
			<template v-if="selectedNodeId">
				<!-- 选中节点时显示完整描述 / Show full description when a node is selected -->
				<template v-if="selectedNodeId === INPUT_NODE_ID">
					<span class="font-medium text-amber-400 shrink-0">{{ t("postprocess.input.label") }}</span>
					<span class="text-muted-foreground">{{ t("postprocess.input.description") }}</span>
				</template>
				<template v-else>
					<span
						v-if="getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === selectedNodeId)?.moduleId ?? '')"
						class="font-medium text-foreground shrink-0"
					>
						{{ getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === selectedNodeId)!.moduleId)?.name }}
					</span>
					<span class="truncate">
						{{ getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === selectedNodeId)?.moduleId ?? '')?.description }}
					</span>
					<Badge
						v-if="getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === selectedNodeId)?.moduleId ?? '')?.official"
						class="text-[10px] px-1.5 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
					>official</Badge>
				</template>
			</template>
			<template v-else>
				<!-- 无选中时显示操作指南 / Show guide when nothing is selected -->
				<span class="text-xs">{{ t("postprocess.guide") }}</span>
			</template>
		</div>
	</div>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
	transition: opacity 0.1s, transform 0.1s;
}
.fade-enter-from,
.fade-leave-to {
	opacity: 0;
	transform: scale(0.97);
}
</style>
