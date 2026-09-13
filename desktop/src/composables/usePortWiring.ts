/**
 * 节点图端口连线 Composable / Node Graph Port Wiring Composable
 *
 * 管理节点图编辑器中输出端口到输入端口的连线交互：端口位置缓存与注册、
 * 拖拽连线中的预览线、以及基于类型兼容性的连线创建。
 *
 * Manages output-to-input port wiring interactions: port position caching and
 * registration, the in-progress connection preview line, and type-compatible edge creation.
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

	const connectingFrom = ref<PortRef | null>(null);
	const pendingLine = ref<{ x: number; y: number } | null>(null);

	const portPositions: Record<string, { x: number; y: number }> = {};
	const portVersion = ref(0);

	function portKey(nodeId: string, isOutput: boolean, portIndex: number) {
		return `${nodeId}:${isOutput ? "o" : "i"}:${portIndex}`;
	}

	function registerPortEl(el: HTMLElement | null, nodeId: string, isOutput: boolean, portIndex: number) {
		if (!el) return;
		const rect = el.getBoundingClientRect();
		const canvasRect = canvasRef.value?.getBoundingClientRect();
		if (!canvasRect) return;
		const key = portKey(nodeId, isOutput, portIndex);
		const nx = (rect.left + rect.width / 2 - canvasRect.left - transform.x) / transform.scale;
		const ny = (rect.top + rect.height / 2 - canvasRect.top - transform.y) / transform.scale;
		const prev = portPositions[key];
		if (!prev || Math.abs(prev.x - nx) > 1 || Math.abs(prev.y - ny) > 1) {
			portPositions[key] = { x: nx, y: ny };
			portVersion.value++;
		}
	}

	function onPortMousedown(e: MouseEvent, portRef: PortRef) {
		e.stopPropagation();
		connectingFrom.value = portRef;
		pendingLine.value = screenToCanvas(e.clientX, e.clientY);
	}

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

	function updatePendingLine(e: MouseEvent) {
		if (connectingFrom.value) {
			pendingLine.value = screenToCanvas(e.clientX, e.clientY);
		}
	}

	/**
	 * 全局 mouseup 时，若仍在拖拽连线，清空连线状态。
	 * 仅当拖拽起点是输出端口时才返回非 null 供调用方弹出模块选择菜单。
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

	function edgePath(from: { x: number; y: number }, to: { x: number; y: number }): string {
		const dx = Math.max(80, Math.abs(to.x - from.x) * 0.5);
		return `M ${from.x} ${from.y} C ${from.x + dx} ${from.y}, ${to.x - dx} ${to.y}, ${to.x} ${to.y}`;
	}

	function edgePositions(edge: PipelineEdge) {
		void portVersion.value;
		const from = portPositions[portKey(edge.fromNodeId, true, edge.fromPort)];
		const to = portPositions[portKey(edge.toNodeId, false, edge.toPort)];
		return { from, to };
	}

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
