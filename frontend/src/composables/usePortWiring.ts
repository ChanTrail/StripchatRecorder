/**
 * 节点图端口连线 Composable / Node Graph Port Wiring Composable
 *
 * 管理节点图编辑器中输出端口到输入端口的连线交互：端口位置缓存与注册、
 * 拖拽连线中的预览线、以及基于类型兼容性的连线创建。连线渲染为 SVG 贝塞尔曲线。
 * 不涉及画布平移缩放（见 useCanvasTransform）或节点拖拽（见 useNodeDragging）。
 *
 * Manages output-to-input port wiring interactions: port position caching and
 * registration, the in-progress connection preview line, and type-compatible edge
 * creation. Edges are rendered as SVG bezier curves. Does not handle canvas pan/zoom
 * (see useCanvasTransform) or node dragging (see useNodeDragging).
 */

import { ref } from "vue";
import type { Ref } from "vue";
import {
	usePostprocessStore,
	isPortCompatible,
	type PipelineEdge,
	type PortType,
} from "@/stores/postprocess";

/** 端口引用：定位一个节点上的某个输入或输出端口 / Port reference: locates an input or output port on a node */
export interface PortRef {
	nodeId: string;
	portIndex: number;
	isOutput: boolean;
	type: PortType;
}

export function usePortWiring(
	canvasRef: Ref<HTMLElement | null>,
	transform: { x: number; y: number; scale: number },
	screenToCanvas: (sx: number, sy: number) => { x: number; y: number },
) {
	const store = usePostprocessStore();

	/** 正在从哪个输出端口拖拽连线（null = 未在连线）/ Output port currently being dragged from (null = not wiring) */
	const connectingFrom = ref<PortRef | null>(null);
	/** 拖拽连线预览终点的画布坐标 / Canvas position of the pending connection preview endpoint */
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

	/** 在输出端口按下鼠标，开始拖拽连线 / Mouse down on an output port, start dragging a connection */
	function onPortMousedown(e: MouseEvent, portRef: PortRef) {
		e.stopPropagation();
		// 注意：不调用 preventDefault()，否则会阻断后续 mouseup 事件的分发
		// Note: do NOT call preventDefault() here — it breaks mouseup dispatch
		if (portRef.isOutput) {
			connectingFrom.value = portRef;
			pendingLine.value = screenToCanvas(e.clientX, e.clientY);
		}
	}

	/** 在输入端口释放鼠标，若类型兼容则创建连线 / Mouse up on an input port; creates the edge if types are compatible */
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

	/** 拖拽连线时更新预览线终点（全局 mousemove 时调用）/ Update preview line endpoint while dragging (called on global mousemove) */
	function updatePendingLine(e: MouseEvent) {
		if (connectingFrom.value) {
			pendingLine.value = screenToCanvas(e.clientX, e.clientY);
		}
	}

	/**
	 * 全局 mouseup 时，若仍在拖拽连线（即未落在任何输入端口上），
	 * 清空连线状态并返回起点端口和释放位置，供调用方弹出模块选择菜单。
	 * 若当前未在连线，返回 null。
	 *
	 * On global mouseup, if still wiring (i.e. the drop didn't land on an input port),
	 * clears the wiring state and returns the origin port plus drop position, so the
	 * caller can show a module-selection menu. Returns null if not currently wiring.
	 */
	function endWireDrag(): { fromPort: PortRef; dropPos: { x: number; y: number } } | null {
		if (!connectingFrom.value) return null;
		const fromPort = connectingFrom.value;
		const dropPos = pendingLine.value;
		connectingFrom.value = null;
		pendingLine.value = null;
		return dropPos ? { fromPort, dropPos } : null;
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

	return {
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
	};
}
