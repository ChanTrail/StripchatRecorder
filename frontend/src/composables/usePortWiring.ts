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

	/**
	 * 在任意端点（输出端口/起始端点，或输入端口/结束端点）按下鼠标，开始拖拽连线。
	 * 两种端点都可以作为拖拽起点，实际连线方向由释放时的目标端点类型决定
	 * （见 onPortMouseup）。
	 *
	 * Mouse down on either kind of port (output/start point, or input/end point),
	 * starting a connection drag. Both port kinds may initiate a drag; the resulting
	 * edge's direction is determined by the target port's kind on release (see onPortMouseup).
	 */
	function onPortMousedown(e: MouseEvent, portRef: PortRef) {
		e.stopPropagation();
		// 注意：不调用 preventDefault()，否则会阻断后续 mouseup 事件的分发
		// Note: do NOT call preventDefault() here — it breaks mouseup dispatch
		connectingFrom.value = portRef;
		pendingLine.value = screenToCanvas(e.clientX, e.clientY);
	}

	/**
	 * 在另一端点释放鼠标：仅当起点和终点是"一输出一输入"的组合、且类型兼容时创建连线
	 * （无论拖拽是从输出端口还是输入端口开始，都会规范化为 输出→输入 的边）。
	 * 同类端点组合（输出→输出、输入→输入）不会创建连线。
	 *
	 * Mouse up on another port: creates an edge only when the start and end points are
	 * one output and one input, and their types are compatible (regardless of which side
	 * the drag started from, the resulting edge is normalized to output→input).
	 * Same-kind combinations (output→output, input→input) do not create an edge.
	 */
	function onPortMouseup(e: MouseEvent, target: PortRef) {
		e.stopPropagation();
		const from = connectingFrom.value;
		if (!from) return;
		if (from.isOutput !== target.isOutput) {
			const outputPort = from.isOutput ? from : target;
			const inputPort = from.isOutput ? target : from;
			if (isPortCompatible(outputPort.type, inputPort.type)) {
				store.addEdge({
					fromNodeId: outputPort.nodeId,
					fromPort: outputPort.portIndex,
					toNodeId: inputPort.nodeId,
					toPort: inputPort.portIndex,
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
	 * 全局 mouseup 时，若仍在拖拽连线（即未落在任何端口上），清空连线状态。
	 *
	 * 仅当拖拽起点是输出端口时才返回非 null 供调用方弹出模块选择菜单——
	 * 从输出端口拖出、松手到空白处表示"我想把这个输出接到一个新的下游模块"，
	 * 菜单据此列出输入类型兼容的模块，选中后自动创建 该输出端口 → 新模块.input 的连线。
	 * 从输入端口拖出释放到空白处则不弹出菜单（直接放弹，不做任何操作），
	 * 因为"新建上游模块"这一意图已可以通过右键菜单添加节点后手动连线实现，
	 * 避免两个方向都弹菜单导致用户分不清连线方向。
	 *
	 * On global mouseup, if still wiring (i.e. the drop didn't land on any port), clears
	 * the wiring state.
	 *
	 * Returns non-null (for the caller to show a module-selection menu) only when the drag
	 * originated from an output port — dragging from an output port and releasing on empty
	 * space means "I want to wire this output into a new downstream module", so the menu
	 * lists modules with a compatible input type, and picking one auto-creates a
	 * thisOutputPort → newModule.input edge. Dragging from an input port and releasing on
	 * empty space does not open a menu (the drag is simply discarded), since "create a new
	 * upstream module" is already achievable via the right-click menu followed by manual
	 * wiring — opening a menu in both directions would make the wiring direction ambiguous
	 * to the user.
	 */
	function endWireDrag(): { fromPort: PortRef; dropPos: { x: number; y: number } } | null {
		if (!connectingFrom.value) return null;
		const fromPort = connectingFrom.value;
		const dropPos = pendingLine.value;
		connectingFrom.value = null;
		pendingLine.value = null;
		if (!fromPort.isOutput) return null;
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

	/**
	 * 获取正在拖拽连线的起点画布坐标（依赖 portVersion）。
	 * 起点可能是输出端口或输入端口，取决于用户实际从哪个端点开始拖拽。
	 *
	 * Get pending wire origin canvas position (depends on portVersion).
	 * The origin may be an output or input port, depending on which endpoint the
	 * user actually started dragging from.
	 */
	function pendingLineFrom() {
		void portVersion.value;
		const from = connectingFrom.value;
		return from
			? portPositions[portKey(from.nodeId, from.isOutput, from.portIndex)]
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
