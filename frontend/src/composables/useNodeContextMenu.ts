/**
 * 节点图右键菜单与连线释放菜单 Composable
 * Node Graph Context Menu and Wire-drop Menu Composable
 *
 * 管理两个模块选择弹出菜单：
 * - 右键画布弹出的"添加节点"菜单（按已使用模块过滤）
 * - 拖拽连线释放到空白画布时弹出的"连接到…"菜单（按端口类型兼容性过滤）
 *
 * Manages two module-selection popup menus:
 * - The "add node" menu shown on right-click (filtered by already-used modules)
 * - The "connect to…" menu shown when a wire is dropped on empty canvas
 *   (filtered by port type compatibility)
 */

import { computed, reactive } from "vue";
import { usePostprocessStore, isPortCompatible, type PortType } from "@/stores/postprocess";
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
	 * 右键菜单中显示的模块列表：可选且未使用的模块，排除内置节点（不可手动放置）。
	 * Modules shown in context menu: available, not yet in the pipeline, excluding built-in nodes.
	 */
	const contextMenuModules = computed(() => {
		const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
		return store.modules.filter((m) => !used.has(m.id) && !m.id.startsWith("__builtin__recording_input"));
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
	 * 连线释放时兼容的模块列表：
	 * - 未已使用，且输入端口 0 类型与当前输出端口兼容
	 * - 排除 recording_input（永远不需要手动放置）
	 * Modules compatible with the current output port type when dropping a wire.
	 * recording_input is excluded (never placed manually).
	 */
	const wireMenuModules = computed(() => {
		if (!wireMenu.fromPort) return [];
		const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
		return store.modules.filter((m) => {
			if (used.has(m.id)) return false;
			if (m.id === "__builtin__recording_input") return false;
			// 取模块的第一个输入类型做兼容性检查 / Check compatibility with first input type
			const firstInput = (m.inputTypes?.[0] ?? "any_file") as PortType;
			return isPortCompatible(wireMenu.fromPort!.type, firstInput);
		});
	});

	/**
	 * 从连线菜单中选择模块：在释放位置添加节点并自动连线。
	 * Select a module from the wire menu: add node at drop position and auto-wire.
	 */
	function addModuleFromWire(moduleId: string, snapPos: (pos: { x: number; y: number }) => { x: number; y: number }) {
		if (!wireMenu.fromPort) return;
		const pos = snapPos({ x: wireMenu.canvasPos.x + 20, y: wireMenu.canvasPos.y });
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
