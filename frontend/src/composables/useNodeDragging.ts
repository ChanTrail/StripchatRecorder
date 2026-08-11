/**
 * 节点图节点选择与拖拽 Composable / Node Graph Node Selection and Dragging Composable
 *
 * 管理节点图编辑器中的节点选中状态（单选/多选/框选）、节点拖拽（含虚拟录制输入节点）、
 * 对齐到网格，以及虚拟输入节点位置与流水线配置的双向同步。
 * 不涉及画布本身的平移缩放（见 useCanvasTransform）或端口连线（见 usePortWiring）。
 *
 * Manages node selection state (single/multi/marquee), node dragging (including the
 * virtual recording input node), snap-to-grid, and two-way sync of the input node's
 * position with the pipeline config. Does not handle canvas pan/zoom (see
 * useCanvasTransform) or port wiring (see usePortWiring).
 */

import { reactive, ref, watch } from "vue";
import type { Ref } from "vue";
import { usePostprocessStore, nodeEffectiveId } from "@/stores/postprocess";

/** 虚拟录制输入节点的固定 ID / Fixed ID for the virtual recording input node */
export const INPUT_NODE_ID = "__recording_input__";

/** 网格尺寸（与背景点阵间距一致）/ Grid size (matches background dot spacing) */
const GRID_SIZE = 24;

export function useNodeDragging(
	canvasRef: Ref<HTMLElement | null>,
	screenToCanvas: (sx: number, sy: number) => { x: number; y: number },
	/** 画布的响应式 transform 对象（用于读取当前 scale）/ Canvas's reactive transform object (for reading current scale) */
	transform: { scale: number },
) {
	const store = usePostprocessStore();

	/** 当前选中的节点 ID 集合（含 INPUT_NODE_ID）/ Currently selected node IDs (may include INPUT_NODE_ID) */
	const selectedNodeIds = reactive<Set<string>>(new Set());

	/**
	 * 虚拟输入节点的画布位置，从 pipeline.inputNodePosition 加载，变化时写回 pipeline 触发自动保存。
	 * Virtual input node canvas position — loaded from pipeline.inputNodePosition,
	 * written back to pipeline on change to trigger auto-save.
	 */
	const inputNodePos = reactive<{ x: number; y: number }>({ x: 40, y: 80 });

	// 当 pipeline.inputNodePosition 变化时（SSE 多客户端同步等）同步到 inputNodePos
	// Sync inputNodePos when pipeline.inputNodePosition changes (e.g. multi-client SSE sync)
	watch(
		() => store.pipeline.inputNodePosition,
		(pos) => {
			if (pos) {
				inputNodePos.x = pos.x;
				inputNodePos.y = pos.y;
			}
		},
	);

	// 当 inputNodePos 变化时写回 pipeline.inputNodePosition，触发防抖自动保存
	// Write back to pipeline.inputNodePosition on change to trigger debounced auto-save
	watch(inputNodePos, (v) => {
		store.pipeline.inputNodePosition = { x: v.x, y: v.y };
	});

	/** 是否启用对齐到网格 / Whether snap-to-grid is enabled */
	const snapToGrid = ref(localStorage.getItem("pp_snap_to_grid") !== "false");
	watch(snapToGrid, (v) => localStorage.setItem("pp_snap_to_grid", String(v)));

	/** 将坐标吸附到网格 / Snap a coordinate to the grid */
	function snap(v: number): number {
		return Math.round(v / GRID_SIZE) * GRID_SIZE;
	}

	/** 将节点位置吸附到网格（若启用）/ Snap a node position to grid if enabled */
	function snapPos(pos: { x: number; y: number }): { x: number; y: number } {
		if (!snapToGrid.value) return pos;
		return { x: snap(pos.x), y: snap(pos.y) };
	}

	function selectNode(nodeId: string) {
		selectedNodeIds.clear();
		selectedNodeIds.add(nodeId);
	}

	function clearSelection() {
		selectedNodeIds.clear();
	}

	// ─── 框选 / Marquee selection ───────────────────────────────────────────
	const isMarquee = ref(false);
	const marquee = reactive({ x: 0, y: 0, w: 0, h: 0, visible: false });
	let marqueeStart = { x: 0, y: 0 }; // 屏幕坐标 / screen coords

	/** 开始框选（左键在空白画布按下时调用）/ Start marquee selection (called on left-button-down on empty canvas) */
	function startMarquee(e: MouseEvent) {
		isMarquee.value = true;
		marqueeStart = { x: e.clientX, y: e.clientY };
		const rect = canvasRef.value?.getBoundingClientRect();
		const ox = rect ? e.clientX - rect.left : e.clientX;
		const oy = rect ? e.clientY - rect.top : e.clientY;
		Object.assign(marquee, { x: ox, y: oy, w: 0, h: 0, visible: false });
	}

	/** 更新框选矩形（鼠标移动时调用）/ Update marquee rectangle (called on mouse move) */
	function updateMarquee(e: MouseEvent) {
		const rect = canvasRef.value?.getBoundingClientRect();
		const ox = rect ? rect.left : 0;
		const oy = rect ? rect.top : 0;
		const dx = e.clientX - marqueeStart.x;
		const dy = e.clientY - marqueeStart.y;
		if (Math.abs(dx) > 4 || Math.abs(dy) > 4) {
			marquee.visible = true;
		}
		marquee.x = Math.min(e.clientX, marqueeStart.x) - ox;
		marquee.y = Math.min(e.clientY, marqueeStart.y) - oy;
		marquee.w = Math.abs(dx);
		marquee.h = Math.abs(dy);
	}

	/**
	 * 结束框选：若矩形有效则按相交检测选中节点，否则清空选择。
	 * End marquee selection: if the rectangle is valid, select intersecting nodes;
	 * otherwise clear the selection.
	 */
	function endMarquee() {
		isMarquee.value = false;
		if (!marquee.visible) {
			selectedNodeIds.clear();
			marquee.visible = false;
			return;
		}

		// 把框选矩形转换到画布坐标，用矩形相交判断节点是否被覆盖
		// Convert marquee to canvas coords; use rectangle intersection for hit detection
		const canvasRect = canvasRef.value?.getBoundingClientRect();
		const ox = canvasRect ? canvasRect.left : 0;
		const oy = canvasRect ? canvasRect.top : 0;
		const sx1 = marquee.x + ox;
		const sy1 = marquee.y + oy;
		const sx2 = sx1 + marquee.w;
		const sy2 = sy1 + marquee.h;
		const c1 = screenToCanvas(sx1, sy1);
		const c2 = screenToCanvas(sx2, sy2);
		const selMinX = Math.min(c1.x, c2.x);
		const selMaxX = Math.max(c1.x, c2.x);
		const selMinY = Math.min(c1.y, c2.y);
		const selMaxY = Math.max(c1.y, c2.y);

		// 从 DOM 读取节点宽高做精确矩形相交（无法读取时用估算值）
		// Read node dimensions from DOM for accurate intersection (fallback to estimates)
		const nodeEls = canvasRef.value?.querySelectorAll(".pipeline-node");

		selectedNodeIds.clear();

		// 虚拟输入节点（nodeEls[0]）
		// Virtual input node (nodeEls[0])
		const inputEl = nodeEls?.[0] as HTMLElement | undefined;
		const inputW = (inputEl?.offsetWidth ?? 176) / transform.scale;
		const inputH = (inputEl?.offsetHeight ?? 80) / transform.scale;
		const ix = inputNodePos.x;
		const iy = inputNodePos.y;
		if (ix < selMaxX && ix + inputW > selMinX && iy < selMaxY && iy + inputH > selMinY) {
			selectedNodeIds.add(INPUT_NODE_ID);
		}

		// 普通节点（nodeEls[1] 起）
		// Regular nodes (nodeEls[1]+)
		store.pipeline.nodes.forEach((node, i) => {
			const nx = node.position?.x ?? 0;
			const ny = node.position?.y ?? 0;
			const el = nodeEls?.[i + 1] as HTMLElement | undefined;
			const nw = (el?.offsetWidth ?? 224) / transform.scale;
			const nh = (el?.offsetHeight ?? 120) / transform.scale;
			if (nx < selMaxX && nx + nw > selMinX && ny < selMaxY && ny + nh > selMinY) {
				selectedNodeIds.add(nodeEffectiveId(node));
			}
		});

		marquee.visible = false;
	}

	// ─── 节点拖拽 / Node dragging ────────────────────────────────────────────
	let draggingNodeId: string | null = null;
	let draggingInput = false;
	let dragOffset = { x: 0, y: 0 };
	let lastDragCanvasPos = { x: 0, y: 0 };

	/** 开始拖拽普通节点 / Start dragging a regular node */
	function startNodeDrag(e: MouseEvent, nodeId: string) {
		draggingNodeId = nodeId;
		// 若拖拽的节点已在选中集合内，保持多选状态；否则重新单选该节点
		// If the dragged node is already in the selection, keep multi-select; otherwise re-select just this node
		if (!selectedNodeIds.has(nodeId)) {
			selectedNodeIds.clear();
			selectedNodeIds.add(nodeId);
		}
		const canvasPos = screenToCanvas(e.clientX, e.clientY);
		dragOffset = {
			x: canvasPos.x - (store.pipeline.nodes.find((n) => nodeEffectiveId(n) === nodeId)?.position?.x ?? 0),
			y: canvasPos.y - (store.pipeline.nodes.find((n) => nodeEffectiveId(n) === nodeId)?.position?.y ?? 0),
		};
		lastDragCanvasPos = canvasPos;
	}

	/** 开始拖拽虚拟输入节点 / Start dragging the virtual input node */
	function startInputDrag(e: MouseEvent) {
		draggingInput = true;
		// 若输入节点已在选中集合内，保持多选状态
		// Keep multi-select if input node is already selected
		if (!selectedNodeIds.has(INPUT_NODE_ID)) {
			selectedNodeIds.clear();
			selectedNodeIds.add(INPUT_NODE_ID);
		}
		const canvasPos = screenToCanvas(e.clientX, e.clientY);
		dragOffset = { x: canvasPos.x - inputNodePos.x, y: canvasPos.y - inputNodePos.y };
		lastDragCanvasPos = canvasPos;
	}

	/** 是否正在拖拽任意节点（普通节点或输入节点）/ Whether any node (regular or input) is being dragged */
	function isDragging() {
		return draggingNodeId !== null || draggingInput;
	}

	/** 拖拽过程中更新节点位置（鼠标移动时调用）/ Update node position during drag (called on mouse move) */
	function updateNodeDrag(e: MouseEvent) {
		const pos = screenToCanvas(e.clientX, e.clientY);
		const dx = pos.x - lastDragCanvasPos.x;
		const dy = pos.y - lastDragCanvasPos.y;
		lastDragCanvasPos = pos;

		if (selectedNodeIds.size > 1) {
			// 多选：所有选中节点整体移动 delta
			// Multi-select: move all selected nodes by delta
			for (const id of selectedNodeIds) {
				if (id === INPUT_NODE_ID) {
					inputNodePos.x += dx;
					inputNodePos.y += dy;
				} else {
					const node = store.pipeline.nodes.find((n) => nodeEffectiveId(n) === id);
					if (node?.position) {
						store.updateNodePosition(id, {
							x: node.position.x + dx,
							y: node.position.y + dy,
						});
					}
				}
			}
		} else if (draggingNodeId) {
			// 单节点绝对定位（保持原有精度）
			// Single node: absolute positioning for accuracy
			store.updateNodePosition(draggingNodeId, {
				x: pos.x - dragOffset.x,
				y: pos.y - dragOffset.y,
			});
		} else if (draggingInput) {
			inputNodePos.x = pos.x - dragOffset.x;
			inputNodePos.y = pos.y - dragOffset.y;
		}
	}

	/**
	 * 结束拖拽：对所有移动过的选中节点执行网格吸附。
	 * End dragging: snap all moved/selected nodes to grid.
	 */
	function endNodeDrag() {
		const snapIds = new Set(selectedNodeIds);
		draggingNodeId = null;
		draggingInput = false;

		for (const id of snapIds) {
			if (id === INPUT_NODE_ID) {
				const snapped = snapPos({ x: inputNodePos.x, y: inputNodePos.y });
				inputNodePos.x = snapped.x;
				inputNodePos.y = snapped.y;
			} else {
				const node = store.pipeline.nodes.find((n) => nodeEffectiveId(n) === id);
				if (node?.position) {
					const snapped = snapPos(node.position);
					store.updateNodePosition(id, snapped);
				}
			}
		}
	}

	/** Delete/Backspace 键删除当前选中节点 / Delete/Backspace key removes the selected node(s) */
	function deleteSelectedNodes() {
		if (selectedNodeIds.size === 0) return;
		for (const id of [...selectedNodeIds]) {
			if (id !== INPUT_NODE_ID) store.removeNode(id);
		}
		selectedNodeIds.clear();
	}

	return {
		selectedNodeIds,
		selectNode,
		clearSelection,
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
		isDragging,
		updateNodeDrag,
		endNodeDrag,
		deleteSelectedNodes,
	};
}
