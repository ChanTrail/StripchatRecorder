/**
 * 节点图画布变换 Composable / Node Graph Canvas Transform Composable
 *
 * 管理节点图编辑器画布的平移（右键拖拽）、缩放（滚轮，以光标为锚点）、
 * 屏幕坐标 ↔ 画布坐标转换，以及自适应居中（fit view）和初始自动布局。
 *
 * Manages the node graph editor canvas's panning (right-click drag), zooming
 * (mouse wheel, anchored at cursor), screen ↔ canvas coordinate conversion,
 * and fit-to-view / initial auto-layout.
 */

import { reactive, ref, onMounted, onUnmounted } from "vue";
import type { Ref } from "vue";
import { usePostprocessStore } from "@/stores/postprocess";

export function useCanvasTransform(canvasRef: Ref<HTMLElement | null>, onBeforeWheel?: () => void) {
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
	let rightMouseDownPos = { x: 0, y: 0 };

	function startPan(e: MouseEvent) {
		isPanning.value = true;
		panStart = { x: e.clientX - transform.x, y: e.clientY - transform.y };
		rightMouseDownPos = { x: e.clientX, y: e.clientY };
	}

	function updatePan(e: MouseEvent) {
		transform.x = e.clientX - panStart.x;
		transform.y = e.clientY - panStart.y;
	}

	/**
	 * 结束平移。若鼠标几乎没有移动（<5px）且未落在节点上，视为"右键单击"，
	 * 返回 true 及画布坐标供调用方弹出上下文菜单；否则返回 false。
	 */
	function endPan(e: MouseEvent): { wasClick: boolean; canvasPos: { x: number; y: number } } {
		isPanning.value = false;
		const dx = Math.abs(e.clientX - rightMouseDownPos.x);
		const dy = Math.abs(e.clientY - rightMouseDownPos.y);
		const wasClick =
			dx < 5 && dy < 5 && !(e.target as HTMLElement).closest(".pipeline-node");
		return { wasClick, canvasPos: screenToCanvas(e.clientX, e.clientY) };
	}

	/**
	 * 滚轮缩放，以光标位置为锚点。
	 * 通过 addEventListener 以 { passive: false } 注册，确保 preventDefault() 能生效。
	 */
	function onCanvasWheel(e: WheelEvent) {
		e.preventDefault();
		onBeforeWheel?.();
		const rawFactor = e.deltaY < 0 ? 1.1 : 0.9;
		const rect = canvasRef.value!.getBoundingClientRect();
		const cx = e.clientX - rect.left;
		const cy = e.clientY - rect.top;
		const newScale = Math.min(3, Math.max(0.2, transform.scale * rawFactor));
		const effectiveFactor = newScale / transform.scale;
		transform.x = cx - (cx - transform.x) * effectiveFactor;
		transform.y = cy - (cy - transform.y) * effectiveFactor;
		transform.scale = newScale;
	}

	onMounted(() => {
		canvasRef.value?.addEventListener("wheel", onCanvasWheel, { passive: false });
	});
	onUnmounted(() => {
		canvasRef.value?.removeEventListener("wheel", onCanvasWheel);
	});

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
	 * 自适应居中：计算所有节点（含虚拟输入节点）的包围盒并居中显示。
	 * Fit view: compute bounding box of all nodes and center them in the canvas.
	 */
	function fitView(inputNodePos: { x: number; y: number }, padding = 60) {
		const canvasRect = canvasRef.value?.getBoundingClientRect();
		if (!canvasRect || canvasRect.width === 0 || canvasRect.height === 0) return;

		interface NodeBox { x: number; y: number; w: number; h: number }
		const boxes: NodeBox[] = [];

		const inputEl = canvasRef.value?.querySelector(".pipeline-node") as HTMLElement | null;
		const inputW = inputEl?.offsetWidth ?? 176;
		const inputH = inputEl?.offsetHeight ?? 80;
		boxes.push({ x: inputNodePos.x, y: inputNodePos.y, w: inputW, h: inputH });

		const nodeEls = canvasRef.value?.querySelectorAll(".pipeline-node");
		store.pipeline.nodes.forEach((node, i) => {
			const x = node.position?.x ?? 0;
			const y = node.position?.y ?? 0;
			const el = nodeEls ? (nodeEls[i + 1] as HTMLElement | undefined) : undefined;
			const w = el?.offsetWidth ?? 224;
			const h = el?.offsetHeight ?? 120;
			boxes.push({ x, y, w, h });
		});

		if (boxes.length === 0) return;

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
		const scaleX = availW / contentW;
		const scaleY = availH / contentH;
		const newScale = Math.min(1, scaleX, scaleY);

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
		autoLayoutNodes,
		fitView,
	};
}
