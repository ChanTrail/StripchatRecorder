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
import { onMounted, computed, ref, onUnmounted, nextTick } from "vue";
import {
	usePostprocessStore,
	type PortType,
	PORT_TYPE_COLORS,
} from "@/stores/postprocess";
import { useNotify } from "@/composables/useNotify";
import { useCanvasTransform } from "@/composables/useCanvasTransform";
import { useNodeDragging, INPUT_NODE_ID } from "@/composables/useNodeDragging";
import { usePortWiring } from "@/composables/usePortWiring";
import { useNodeContextMenu } from "@/composables/useNodeContextMenu";
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
import { Maximize2, Grid2x2 } from "@lucide/vue";
import { useI18n } from "vue-i18n";

const store = usePostprocessStore();
const { toast } = useNotify();
const { t } = useI18n();

// ─── 画布变换 / Canvas Transform ─────────────────────────────────────────────
const canvasRef = ref<HTMLElement | null>(null);
const {
	transform,
	screenToCanvas,
	isPanning,
	startPan,
	updatePan,
	endPan,
	onCanvasWheel,
	autoLayoutNodes,
	fitView,
} = useCanvasTransform(canvasRef);

// ─── 节点选择与拖拽 / Node Selection and Dragging ────────────────────────────
const {
	selectedNodeIds,
	selectNode: selectNodeInternal,
	inputNodePos,
	snapToGrid,
	snapPos,
	isMarquee,
	marquee,
	startMarquee,
	updateMarquee,
	endMarquee,
	startNodeDrag,
	startInputDrag,
	isDragging: isDraggingNode,
	updateNodeDrag,
	endNodeDrag,
	deleteSelectedNodes,
} = useNodeDragging(canvasRef, screenToCanvas, transform);

// ─── 端口连线 / Port Wiring ───────────────────────────────────────────────────
const {
	connectingFrom,
	pendingLine,
	registerPortEl,
	onPortMousedown,
	onPortMouseup,
	updatePendingLine,
	endWireDrag,
	edgePath,
	edgePositions,
	pendingLineFrom,
} = usePortWiring(canvasRef, transform, screenToCanvas);

// ─── 右键菜单与连线释放菜单 / Context Menu and Wire-drop Menu ───────────────
const {
	contextMenu,
	openContextMenu: openContextMenuInternal,
	closeContextMenu,
	contextMenuModules,
	addModuleAtCursor: addModuleAtCursorInternal,
	wireMenu,
	openWireMenu,
	closeWireMenu,
	wireMenuModules,
	addModuleFromWire: addModuleFromWireInternal,
} = useNodeContextMenu();

onMounted(async () => {
	await Promise.all([store.fetchModules(), store.fetchPipeline()]);
	// 从 pipeline.inputNodePosition 恢复输入节点位置
	// Restore input node position from pipeline.inputNodePosition
	if (store.pipeline.inputNodePosition) {
		inputNodePos.x = store.pipeline.inputNodePosition.x;
		inputNodePos.y = store.pipeline.inputNodePosition.y;
	}
	store.initModuleWatcher(() => {
		toast(t("postprocess.updatedByOther"), "info");
		// 其他客户端更新流水线后，重新从 node.inputs 还原 edges
		// Restore edges from node.inputs after pipeline is updated by another client
		nextTick(() => restoreEdgesFromInputs());
	});
	// 从 node.inputs 恢复前端 edges（含虚拟输入节点的连线）
	// Restore frontend edges from node.inputs (including virtual input node wiring)
	restoreEdgesFromInputs();
	autoLayoutNodes();
	// DOM 渲染完成后自适应居中所有节点 / Fit all nodes to canvas after DOM renders
	await nextTick();
	fitView(inputNodePos);
});

/**
 * 从每个节点的 inputs 字段还原前端 pipeline.edges，
 * 将 nodeId="0" 映射回虚拟输入节点 __recording_input__。
 * 只补充 edges 中尚不存在的边，不覆盖已有边。
 *
 * Restore frontend pipeline.edges from each node's inputs field,
 * mapping nodeId="0" back to the virtual input node __recording_input__.
 * Only adds edges that don't already exist; does not overwrite.
 */
function restoreEdgesFromInputs() {
	for (const node of store.pipeline.nodes) {
		if (!node.inputs) continue;
		for (const [portStr, ref_] of Object.entries(node.inputs)) {
			const toPort = Number(portStr);
			const fromNodeId = ref_.nodeId === "0" ? INPUT_NODE_ID : ref_.nodeId;
			const fromPort = ref_.port;
			// 检查该边是否已存在 / Check if edge already exists
			const exists = store.pipeline.edges.some(
				(e) => e.fromNodeId === fromNodeId && e.fromPort === fromPort
					&& e.toNodeId === node.nodeId && e.toPort === toPort,
			);
			if (!exists) {
				store.pipeline.edges.push({
					fromNodeId,
					fromPort,
					toNodeId: node.nodeId,
					toPort,
				});
			}
		}
	}
}

/** 当前画布光标样式 / Current canvas cursor style */
const canvasCursor = computed(() => {
	if (isPanning.value) return "grabbing";
	if (isMarquee.value) return "crosshair";
	return "default";
})

function onCanvasMousedown(e: MouseEvent) {
	if ((e.target as HTMLElement).closest(".pipeline-node")) return;

	if (e.button === 2) {
		startPan(e);
		closeWireMenu();
		return;
	}

	if (e.button === 0) {
		closeContextMenu();
		closeWireMenu();
		startMarquee(e);
	}
}

function onCanvasMousemoveInternal(e: MouseEvent) {
	if (isPanning.value) {
		updatePan(e);
		return;
	}
	if (isMarquee.value) {
		updateMarquee(e);
	}
}

function onCanvasMouseupInternal(e: MouseEvent) {
	if (isPanning.value) {
		const { wasClick, canvasPos } = endPan(e);
		if (wasClick) {
			const rect = canvasRef.value?.getBoundingClientRect();
			openContextMenuInternal(
				e.clientX - (rect?.left ?? 0),
				e.clientY - (rect?.top ?? 0),
				canvasPos,
			);
			selectedNodeIds.clear();
		}
		return;
	}
	if (isMarquee.value) {
		const hadMarquee = marquee.visible;
		endMarquee();
		if (!hadMarquee) closeContextMenu();
	}
}

function onNodeMousedown(e: MouseEvent, nodeId: string) {
	if (e.button !== 0) return;
	e.stopPropagation();
	if (connectingFrom.value) return;
	startNodeDrag(e, nodeId);
}

function onInputNodeMousedown(e: MouseEvent) {
	if (e.button !== 0) return;
	e.stopPropagation();
	if (connectingFrom.value) return;
	startInputDrag(e);
}

function onGlobalMousemove(e: MouseEvent) {
	if (isDraggingNode()) {
		updateNodeDrag(e);
	}
	updatePendingLine(e);
	onCanvasMousemoveInternal(e);
}

function onGlobalMouseup(e: MouseEvent) {
	const wasDraggingNode = isDraggingNode();
	onCanvasMouseupInternal(e);

	// 拖拽结束后对所有移动过的节点执行 snap
	// Snap all moved nodes to grid after drag ends
	if (wasDraggingNode) {
		endNodeDrag();
	}

	// 连线释放：若未落在输入端口上，弹出模块选择菜单
	// Wire drop: if not on an input port, show module selection menu
	const wireDrop = endWireDrag();
	if (wireDrop) {
		const rect = canvasRef.value?.getBoundingClientRect();
		openWireMenu(
			e.clientX - (rect?.left ?? 0),
			e.clientY - (rect?.top ?? 0),
			wireDrop.dropPos,
			wireDrop.fromPort,
		);
	}
}

function selectNode(nodeId: string) {
	selectNodeInternal(nodeId);
	closeContextMenu();
}

function openContextMenu(e: MouseEvent) {
	// 仅阻止浏览器默认右键菜单，实际弹出逻辑在 onCanvasMouseupInternal
	// Only prevent browser default context menu; actual popup logic is in onCanvasMouseupInternal
	e.preventDefault();
}

function addModuleAtCursor(moduleId: string) {
	addModuleAtCursorInternal(moduleId, snapPos);
}

function addModuleFromWire(moduleId: string) {
	addModuleFromWireInternal(moduleId, snapPos);
}

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

/** Delete/Backspace 键删除当前选中节点 / Delete/Backspace key removes the selected node(s) */
function onGlobalKeydown(e: KeyboardEvent) {
	if (e.key !== "Delete" && e.key !== "Backspace") return;
	const tag = (e.target as HTMLElement)?.tagName?.toLowerCase();
	if (tag === "input" || tag === "textarea" || (e.target as HTMLElement)?.isContentEditable) return;
	deleteSelectedNodes();
}

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
					class="flex items-center gap-1.5 px-2.5 py-1 text-xs border rounded-md transition-colors"
					:class="snapToGrid
						? 'bg-primary/15 border-primary/40 text-primary hover:bg-primary/25'
						: 'text-muted-foreground border-border hover:bg-accent hover:text-foreground'"
					:title="t('postprocess.snapToGrid')"
					@click="snapToGrid = !snapToGrid"
				>
					<Grid2x2 class="size-3.5" />
					{{ t("postprocess.snapToGrid") }}
				</button>
				<button
					class="flex items-center gap-1.5 px-2.5 py-1 text-xs text-muted-foreground border rounded-md hover:bg-accent hover:text-foreground transition-colors"
					:title="t('postprocess.fitView')"
					@click="fitView(inputNodePos)"
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
			class="relative flex-1 overflow-hidden bg-[#0d0d0d] select-none"
			:style="{
				cursor: canvasCursor,
				backgroundImage: 'radial-gradient(circle, #2a2a2a 1px, transparent 1px)',
				backgroundSize: '24px 24px',
			}"
			@mousedown="onCanvasMousedown"
			@wheel.prevent="onCanvasWheel"
			@contextmenu.prevent="openContextMenu"
		>
			<!-- 框选矩形覆盖层（z-50 确保在节点之上）/ Marquee selection overlay (z-50 on top of nodes) -->
			<div
				v-if="marquee.visible"
				class="absolute pointer-events-none z-50 border border-primary/60 bg-primary/10"
				:style="{
					left: `${marquee.x}px`,
					top: `${marquee.y}px`,
					width: `${marquee.w}px`,
					height: `${marquee.h}px`,
				}"
			/>
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
						zIndex: selectedNodeIds.has(node.nodeId) ? 10 : 1,
					}"
					@mousedown.stop
					@click.stop="selectNode(node.nodeId)"
				>
					<div
						class="rounded-xl border bg-card shadow-xl min-w-64 max-w-96 transition-colors"
						:class="[
							!node.enabled && 'opacity-50',
							selectedNodeIds.has(node.nodeId) ? 'border-primary' : 'border-white/10',
						]"
					>
						<!-- 节点头部 / Node header -->
						<div class="flex items-center gap-2 px-3 py-2 border-b border-white/5 cursor-move" @mousedown.stop="onNodeMousedown($event, node.nodeId)">
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

						<!-- official 提示已移至底部信息栏 / Official hint moved to bottom info bar -->
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
			<template v-if="selectedNodeIds.size > 0">
				<!-- 多选时显示数量 / Show count when multiple selected -->
				<template v-if="selectedNodeIds.size > 1">
					<span class="text-xs">{{ t("postprocess.multiSelected", { count: selectedNodeIds.size }) }}</span>
				</template>
				<!-- 单选时显示节点描述 / Show node description when single selected -->
				<template v-else-if="selectedNodeIds.has(INPUT_NODE_ID)">
					<span class="font-medium text-amber-400 shrink-0">{{ t("postprocess.input.label") }}</span>
					<span class="text-muted-foreground">{{ t("postprocess.input.description") }}</span>
				</template>
				<template v-else>
					<template v-for="nodeId in selectedNodeIds" :key="nodeId">
						<span
							v-if="getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === nodeId)?.moduleId ?? '')"
							class="font-medium text-foreground shrink-0"
						>
							{{ getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === nodeId)!.moduleId)?.name }}
						</span>
						<span class="truncate">
							{{ getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === nodeId)?.moduleId ?? '')?.description }}
						</span>
						<Badge
							v-if="getModuleInfo(store.pipeline.nodes.find(n => n.nodeId === nodeId)?.moduleId ?? '')?.official"
							class="text-[10px] px-1.5 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
						>official</Badge>
					</template>
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
