<!--
    后处理流水线节点图编辑器 / Post-processing Pipeline Node Graph Editor

    与 frontend 版对齐，完整支持：
    - 节点图 composables（useCanvasTransform / useNodeDragging / usePortWiring / useNodeContextMenu）
    - 框选（Marquee）、多选、Delete 键删除
    - 右键上下文菜单 + 连线释放菜单
    - 对齐到网格（snapToGrid）
    - 移动端提示（isMobile）
-->
<script setup lang="ts">
import { onMounted, computed, ref, onUnmounted, nextTick } from "vue";
import { useMobileLayout } from "@/composables/useMobileLayout";
import {
	usePostprocessStore,
	nodeEffectiveId,
	resolvedEdges,
	type PortType,
	PORT_TYPE_COLORS,
} from "@/stores/postprocess";
import { useNotify } from "@/composables/useNotify";
import { useCanvasTransform } from "@/composables/useCanvasTransform";
import { useNodeDragging, INPUT_NODE_ID } from "@/composables/useNodeDragging";
import { usePortWiring } from "@/composables/usePortWiring";
import { useNodeContextMenu } from "@/composables/useNodeContextMenu";
import RecordingInputNode from "@/components/RecordingInputNode.vue";
import PipelineNodeCard from "@/components/PipelineNodeCard.vue";
import ModulePickerMenu from "@/components/ModulePickerMenu.vue";
import { Badge } from "@/components/ui/badge";
import { Maximize2, Grid2x2, Clapperboard } from "@lucide/vue";
import { useI18n } from "vue-i18n";
import type { PortRef } from "@/composables/usePortWiring";

const store = usePostprocessStore();
const { toast } = useNotify();
const { t } = useI18n();
const { isMobile } = useMobileLayout();

// ─── 画布变换 / Canvas Transform ─────────────────────────────────────────────
const canvasRef = ref<HTMLElement | null>(null);
const {
	transform,
	screenToCanvas,
	isPanning,
	startPan,
	updatePan,
	endPan,
	autoLayoutNodes,
	fitView,
} = useCanvasTransform(canvasRef, () => {
	closeContextMenu();
	closeWireMenu();
});

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

// ─── 右键菜单与连线释放菜单 ────────────────────────────────────────────────
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
	if (store.pipeline.inputNodePosition) {
		inputNodePos.x = store.pipeline.inputNodePosition.x;
		inputNodePos.y = store.pipeline.inputNodePosition.y;
	}
	store.initModuleWatcher(() => {
		toast(t("postprocess.updatedByOther"), "info");
	});
	autoLayoutNodes();
	await nextTick();
	fitView(inputNodePos);
});

const edges = computed(() => resolvedEdges(store.pipeline));

const canvasCursor = computed(() => {
	if (isPanning.value) return "grabbing";
	if (isMarquee.value) return "crosshair";
	return "default";
});

function onCanvasMousedown(e: MouseEvent) {
	if ((e.target as HTMLElement).closest(".pipeline-node")) return;
	if (e.button === 2) {
		startPan(e);
		closeContextMenu();
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
	if (isPanning.value) { updatePan(e); return; }
	if (isMarquee.value) updateMarquee(e);
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
	if (isDraggingNode()) updateNodeDrag(e);
	updatePendingLine(e);
	onCanvasMousemoveInternal(e);
}

function onGlobalMouseup(e: MouseEvent) {
	const wasDraggingNode = isDraggingNode();
	onCanvasMouseupInternal(e);
	if (wasDraggingNode) endNodeDrag();

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
	return getModuleInfo(moduleId)?.inputTypes ?? ["any_file"];
}

function getNodeOutputTypes(moduleId: string): PortType[] {
	return getModuleInfo(moduleId)?.outputTypes ?? ["any_file"];
}

function onNodePortMousedown(e: MouseEvent, nodeId: string, portIndex: number, type: PortType, isOutput: boolean) {
	onPortMousedown(e, { nodeId, portIndex, isOutput, type });
}
function onNodePortMouseup(e: MouseEvent, nodeId: string, portIndex: number, type: PortType, isOutput: boolean) {
	onPortMouseup(e, { nodeId, portIndex, isOutput, type });
}
function onInputPortMousedown(e: MouseEvent) {
	onPortMousedown(e, { nodeId: INPUT_NODE_ID, portIndex: 0, isOutput: true, type: "ts_session_dir" });
}
function onInputPortMouseup(e: MouseEvent) {
	onPortMouseup(e, { nodeId: INPUT_NODE_ID, portIndex: 0, isOutput: true, type: "ts_session_dir" });
}

const inputPortConnected = computed(() =>
	store.pipeline.nodes.some((n) =>
		Object.values(n.inputs ?? {}).some((ref) => ref.nodeId === "0"),
	),
);
</script>

<template>
	<div class="flex flex-col h-full gap-0">
		<!-- 移动端提示 / Mobile notice -->
		<div v-if="isMobile" class="flex flex-col items-center justify-center h-full gap-4 px-8 text-center">
			<Clapperboard class="size-12 text-muted-foreground/40" />
			<h2 class="text-base font-semibold">{{ t("postprocess.title") }}</h2>
			<p class="text-sm text-muted-foreground leading-relaxed max-w-xs">{{ t("postprocess.mobileHint") }}</p>
		</div>

		<template v-else>
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
			@contextmenu.prevent="openContextMenu"
		>
			<!-- 框选矩形 / Marquee -->
			<div
				v-if="marquee.visible"
				class="absolute pointer-events-none z-50 border border-primary/60 bg-primary/10"
				:style="{
					left: `${marquee.x}px`, top: `${marquee.y}px`,
					width: `${marquee.w}px`, height: `${marquee.h}px`,
				}"
			/>

			<!-- 变换层 / Transform layer -->
			<div
				class="absolute top-0 left-0 origin-top-left"
				:style="{ transform: `translate(${transform.x}px, ${transform.y}px) scale(${transform.scale})` }"
			>
				<!-- SVG 连线层 / SVG wiring layer -->
				<svg class="absolute inset-0 overflow-visible pointer-events-none" style="left:0;top:0;width:1px;height:1px;">
					<g v-for="edge in edges" :key="`${edge.fromNodeId}-${edge.fromPort}-${edge.toNodeId}-${edge.toPort}`">
						<template v-if="edgePositions(edge).from && edgePositions(edge).to">
							<path
								:d="edgePath(edgePositions(edge).from!, edgePositions(edge).to!)"
								fill="none" stroke="#ffffff30" stroke-width="2"
								class="pointer-events-auto cursor-pointer hover:stroke-destructive transition-colors"
								@click.stop="store.removeEdge(edge.fromNodeId, edge.fromPort, edge.toNodeId, edge.toPort)"
							/>
						</template>
					</g>
					<template v-if="connectingFrom && pendingLine">
						<path
							:d="edgePath(pendingLineFrom() ?? pendingLine, pendingLine)"
							fill="none"
							:stroke="PORT_TYPE_COLORS[connectingFrom.type]"
							stroke-width="2" stroke-dasharray="6 3"
						/>
					</template>
				</svg>

				<!-- 虚拟录制输入节点 / Virtual recording input node -->
				<RecordingInputNode
					:x="inputNodePos.x" :y="inputNodePos.y"
					:register-port-el="registerPortEl"
					:connected="inputPortConnected"
					@header-mousedown="onInputNodeMousedown"
					@port-mousedown="onInputPortMousedown"
					@port-mouseup="onInputPortMouseup"
				/>

				<!-- 常规节点 / Regular nodes -->
				<PipelineNodeCard
					v-for="node in store.pipeline.nodes"
					:key="nodeEffectiveId(node)"
					:node="node"
					:module-info="getModuleInfo(node.moduleId)"
					:input-types="getNodeInputTypes(node.moduleId)"
					:output-types="getNodeOutputTypes(node.moduleId)"
					:selected="selectedNodeIds.has(nodeEffectiveId(node))"
					:register-port-el="registerPortEl"
					@select="selectNode(nodeEffectiveId(node))"
					@header-mousedown="onNodeMousedown($event, nodeEffectiveId(node))"
					@toggle-enabled="node.enabled = $event"
					@update-param="(key, value) => { node.params[key] = value; }"
					@port-mousedown="(e, portIndex, type, isOutput) => onNodePortMousedown(e, nodeEffectiveId(node), portIndex, type, isOutput)"
					@port-mouseup="(e, portIndex, type, isOutput) => onNodePortMouseup(e, nodeEffectiveId(node), portIndex, type, isOutput)"
				/>
			</div>

			<!-- 右键菜单 / Context menu -->
			<ModulePickerMenu
				:visible="contextMenu.visible" :x="contextMenu.x" :y="contextMenu.y"
				:modules="contextMenuModules"
				:empty-message="t('postprocess.picker.allAdded')"
				@select="addModuleAtCursor"
				@close="closeContextMenu"
			>
				{{ t("postprocess.picker.title") }}
			</ModulePickerMenu>

			<!-- 连线释放菜单 / Wire-drop menu -->
			<ModulePickerMenu
				:visible="wireMenu.visible" :x="wireMenu.x" :y="wireMenu.y"
				:modules="wireMenuModules"
				:empty-message="t('postprocess.picker.noCompatible')"
				@select="addModuleFromWire"
				@close="closeWireMenu"
			>
				<span>{{ t("postprocess.picker.connectTo") }}</span>
				<span
					v-if="wireMenu.fromPort"
					class="px-1.5 py-0.5 rounded text-[10px] font-mono"
					:style="{ background: PORT_TYPE_COLORS[(wireMenu.fromPort as PortRef).type] + '30', color: PORT_TYPE_COLORS[(wireMenu.fromPort as PortRef).type] }"
				>{{ (wireMenu.fromPort as PortRef).type.replace(/_/g, ' ') }}</span>
			</ModulePickerMenu>
		</div>

		<!-- 底部信息栏 / Bottom info bar -->
		<div class="px-6 py-2.5 border-t text-sm text-muted-foreground flex items-center gap-2 shrink-0 bg-background min-h-10">
			<template v-if="selectedNodeIds.size > 0">
				<template v-if="selectedNodeIds.size > 1">
					<span class="text-xs">{{ t("postprocess.multiSelected", { count: selectedNodeIds.size }) }}</span>
				</template>
				<template v-else-if="selectedNodeIds.has(INPUT_NODE_ID)">
					<span class="font-medium text-amber-400 shrink-0">{{ t("postprocess.input.label") }}</span>
					<span class="text-muted-foreground">{{ t("postprocess.input.description") }}</span>
				</template>
				<template v-else>
					<template v-for="nodeId in selectedNodeIds" :key="nodeId">
						<span class="font-medium text-foreground shrink-0">
							{{ getModuleInfo(store.pipeline.nodes.find(n => nodeEffectiveId(n) === nodeId)?.moduleId ?? '')?.name }}
						</span>
						<span class="truncate">
							{{ getModuleInfo(store.pipeline.nodes.find(n => nodeEffectiveId(n) === nodeId)?.moduleId ?? '')?.description }}
						</span>
						<Badge
							v-if="getModuleInfo(store.pipeline.nodes.find(n => nodeEffectiveId(n) === nodeId)?.moduleId ?? '')?.official"
							class="text-[10px] px-1.5 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
						>official</Badge>
					</template>
				</template>
			</template>
			<template v-else>
				<span class="text-xs">{{ t("postprocess.guide") }}</span>
			</template>
		</div>
		</template>
	</div>
</template>

<style scoped>
.fade-enter-active, .fade-leave-active { transition: opacity 0.1s, transform 0.1s; }
.fade-enter-from, .fade-leave-to { opacity: 0; transform: scale(0.97); }
</style>
