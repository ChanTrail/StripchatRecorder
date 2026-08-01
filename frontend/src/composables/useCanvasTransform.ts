/**
 * 节点图画布变换 Composable / Node Graph Canvas Transform Composable
 *
 * 管理节点图编辑器画布的平移（右键拖拽）、缩放（滚轮，以光标为锚点）、
 * 屏幕坐标 ↔ 画布坐标转换，以及自适应居中（fit view）和初始自动布局。
 * 不涉及节点选择、拖拽或端口连线（见 useNodeDragging / usePortWiring）。
 *
 * Manages the node graph editor canvas's panning (right-click drag), zooming
 * (mouse wheel, anchored at cursor), screen ↔ canvas coordinate conversion,
 * and fit-to-view / initial auto-layout. Does not handle node selection,
 * dragging, or port wiring (see useNodeDragging / usePortWiring).
 */

import { reactive, ref } from "vue";
import type { Ref } from "vue";
import { usePostprocessStore } from "@/stores/postprocess";

export function useCanvasTransform(canvasRef: Ref<HTMLElement | null>) {
	const store = usePostprocessStore();

	/** 画布平移/缩放状态 / Canvas pan/zoom state */
	const transform = reactive({ x: 0, y: 0, scale: 1 });

	/**
	 * 将屏幕坐标转换为画布坐标（考虑当前平移和缩放）。
	 * Convert screen coordinates to canvas coordinates (accounting for current pan/zoom).
	 */
	function screenToCanvas(sx: number, sy: number) {
		const rect = canvasRef.value?.getBoundingClientRect();
		if (!rect) return { x: 0, y: 0 };
		return {
			x: (sx - rect.left - transform.x) / transform.scale,
			y: (sy - rect.top - transform.y) / transform.scale,
		};
	}

	// ─── 右键平移 / Right-button panning ────────────────────────────────────
	const isPanning = ref(false);
	let panStart = { x: 0, y: 0 };
	/** 右键按下时的屏幕坐标，用于判断是否为"无移动的右键单击" / Screen pos at right-button-down, used to detect a no-move click */
	let rightMouseDownPos = { x: 0, y: 0 };

	/** 开始右键平移 / Start right-button panning */
	function startPan(e: MouseEvent) {
		isPanning.value = true;
		panStart = { x: e.clientX - transform.x, y: e.clientY - transform.y };
		rightMouseDownPos = { x: e.clientX, y: e.clientY };
	}

	/** 更新平移偏移（鼠标移动时调用）/ Update pan offset (called on mouse move) */
	function updatePan(e: MouseEvent) {
		transform.x = e.clientX - panStart.x;
		transform.y = e.clientY - panStart.y;
	}

	/**
	 * 结束平移。若鼠标几乎没有移动（<5px）且未落在节点上，视为"右键单击"，
	 * 返回 true 及画布坐标供调用方弹出上下文菜单；否则返回 false。
	 *
	 * End panning. If the mouse barely moved (<5px) and didn't land on a node,
	 * treat it as a "right-click" and return true plus the canvas position for
	 * the caller to open a context menu; otherwise return false.
	 */
	function endPan(e: MouseEvent): { wasClick: boolean; canvasPos: { x: number; y: number } } {
		isPanning.value = false;
		const dx = Math.abs(e.clientX - rightMouseDownPos.x);
		const dy = Math.abs(e.clientY - rightMouseDownPos.y);
		const wasClick =
			dx < 5 && dy < 5 && !(e.target as HTMLElement).closest(".pipeline-node");
		return { wasClick, canvasPos: screenToCanvas(e.clientX, e.clientY) };
	}

	/** 滚轮缩放，以光标位置为锚点 / Mouse wheel zoom, anchored at cursor position */
	function onCanvasWheel(e: WheelEvent) {
		e.preventDefault();
		const rawFactor = e.deltaY < 0 ? 1.1 : 0.9;
		const rect = canvasRef.value!.getBoundingClientRect();
		const cx = e.clientX - rect.left;
		const cy = e.clientY - rect.top;
		// 先 clamp 目标 scale，再用实际生效的 factor 更新平移，避免边界时偏移漂移
		// Clamp target scale first, then use the effective factor for translation to avoid drift at boundaries
		const newScale = Math.min(3, Math.max(0.2, transform.scale * rawFactor));
		const effectiveFactor = newScale / transform.scale;
		transform.x = cx - (cx - transform.x) * effectiveFactor;
		transform.y = cy - (cy - transform.y) * effectiveFactor;
		transform.scale = newScale;
	}

	/** 首次加载时为无位置的节点分配初始网格布局 / Assign an initial grid layout to nodes without a position on first load */
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
	 *
	 * @param inputNodePos - 虚拟输入节点的当前画布位置（由 useNodeDragging 管理）
	 *                       Virtual input node's current canvas position (managed by useNodeDragging)
	 */
	function fitView(inputNodePos: { x: number; y: number }, padding = 60) {
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

	return {
		transform,
		screenToCanvas,
		isPanning,
		startPan,
		updatePan,
		endPan,
		onCanvasWheel,
		autoLayoutNodes,
		fitView,
	};
}
