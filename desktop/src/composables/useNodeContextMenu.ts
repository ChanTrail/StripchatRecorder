/**
 * 节点图右键菜单与连线释放菜单 Composable
 * Node Graph Context Menu and Wire-drop Menu Composable
 *
 * 管理两个模块选择弹出菜单：
 * - 右键画布弹出的"添加节点"菜单
 * - 拖拽连线释放到空白画布时弹出的"连接到…"菜单
 */

import { computed, reactive } from "vue";
import { usePostprocessStore, isPortCompatible, nodeEffectiveId, type PortType } from "@/stores/postprocess";
import type { PortRef } from "./usePortWiring";

export function useNodeContextMenu() {
	const store = usePostprocessStore();

	// ─── 右键上下文菜单 / Right-click context menu ──────────────────────────
	const contextMenu = reactive<{
		visible: boolean;
		x: number;
		y: number;
		canvasPos: { x: number; y: number };
	}>({ visible: false, x: 0, y: 0, canvasPos: { x: 0, y: 0 } });

	function openContextMenu(x: number, y: number, canvasPos: { x: number; y: number }) {
		contextMenu.x = x;
		contextMenu.y = y;
		contextMenu.canvasPos = canvasPos;
		contextMenu.visible = true;
	}

	function closeContextMenu() {
		contextMenu.visible = false;
	}

	/**
	 * 右键菜单中显示的模块列表：排除已使用且不可复用的模块，以及 recording_input。
	 * Modules shown in context menu: exclude already-used non-reusable modules and recording_input.
	 */
	const contextMenuModules = computed(() => {
		const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
		return store.modules.filter((m) =>
			(m.reusable || !used.has(m.id)) && !m.id.startsWith("__builtin__recording_input"),
		);
	});

	function addModuleAtCursor(moduleId: string, snapPos: (pos: { x: number; y: number }) => { x: number; y: number }) {
		store.addNode(moduleId, snapPos({ ...contextMenu.canvasPos }));
		closeContextMenu();
	}

	// ─── 连线释放菜单 / Wire-drop module menu ───────────────────────────────
	const wireMenu = reactive<{
		visible: boolean;
		x: number;
		y: number;
		canvasPos: { x: number; y: number };
		fromPort: PortRef | null;
	}>({ visible: false, x: 0, y: 0, canvasPos: { x: 0, y: 0 }, fromPort: null });

	function openWireMenu(x: number, y: number, canvasPos: { x: number; y: number }, fromPort: PortRef) {
		wireMenu.x = x;
		wireMenu.y = y;
		wireMenu.canvasPos = canvasPos;
		wireMenu.fromPort = fromPort;
		wireMenu.visible = true;
	}

	function closeWireMenu() {
		wireMenu.visible = false;
		wireMenu.fromPort = null;
	}

	/**
	 * 连线释放时兼容的模块列表（仅当起点为输出端口时展示）。
	 * Modules compatible with the source output port when a wire is dropped on empty canvas.
	 */
	const wireMenuModules = computed(() => {
		if (!wireMenu.fromPort) return [];
		const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
		return store.modules.filter((m) => {
			if (used.has(m.id) && !m.reusable) return false;
			if (m.id === "__builtin__recording_input") return false;
			const firstInput = (m.inputTypes?.[0] ?? "any_file") as PortType;
			return isPortCompatible(wireMenu.fromPort!.type, firstInput);
		});
	});

	function addModuleFromWire(moduleId: string, snapPos: (pos: { x: number; y: number }) => { x: number; y: number }) {
		if (!wireMenu.fromPort) return;
		const pos = snapPos({ x: wireMenu.canvasPos.x + 20, y: wireMenu.canvasPos.y });
		store.addNode(moduleId, pos);
		const newNode = store.pipeline.nodes[store.pipeline.nodes.length - 1];
		if (newNode) {
			store.addEdge({
				fromNodeId: wireMenu.fromPort.nodeId,
				fromPort: wireMenu.fromPort.portIndex,
				toNodeId: nodeEffectiveId(newNode),
				toPort: 0,
			});
		}
		closeWireMenu();
	}

	function closeAllMenus() {
		closeContextMenu();
		closeWireMenu();
	}

	return {
		contextMenu,
		openContextMenu,
		closeContextMenu,
		contextMenuModules,
		addModuleAtCursor,
		wireMenu,
		openWireMenu,
		closeWireMenu,
		wireMenuModules,
		addModuleFromWire,
		closeAllMenus,
	};
}
