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
	 * 右键菜单中显示的模块列表：可选模块，排除已使用且不可复用的模块，
	 * 以及 recording_input（永远不需要手动放置）。可复用模块（reusable）
	 * 即使已放置过，仍会保留在列表中，支持多次添加。
	 *
	 * Modules shown in context menu: available modules, excluding already-used
	 * non-reusable modules and recording_input (never placed manually). Reusable
	 * modules stay in the list even after being placed, so they can be added multiple times.
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
	 * 连线释放时兼容的模块列表。
	 *
	 * `wireMenu.fromPort` 始终是拖拽起点为输出端口（起始端点/start point）的情况——
	 * 从输入端口拖出释放到空白处不会打开此菜单（见 usePortWiring.endWireDrag）。
	 * 因此这里列出的是"可以接收 fromPort 这个输出端口"的候选模块：
	 * - 未使用，或已使用但可复用（reusable）
	 * - 排除 recording_input（已是固定虚拟节点，不通过模块列表添加）
	 * - 模块的第一个输入类型与 fromPort 的类型兼容
	 *
	 * Modules compatible with the source output port when a wire is dropped on empty canvas.
	 *
	 * `wireMenu.fromPort` is always the case where the drag originated from an output port
	 * (the start point) — dragging from an input port and dropping on empty space never opens
	 * this menu (see usePortWiring.endWireDrag). So the candidates listed here are modules
	 * that can receive the `fromPort` output:
	 * - Unused, or used-but-reusable
	 * - recording_input is excluded (it's a fixed virtual node, not added via the module list)
	 * - The module's first input type is compatible with fromPort's type
	 */
	const wireMenuModules = computed(() => {
		if (!wireMenu.fromPort) return [];
		const used = new Set(store.pipeline.nodes.map((n) => n.moduleId));
		return store.modules.filter((m) => {
			if (used.has(m.id) && !m.reusable) return false;
			if (m.id === "__builtin__recording_input") return false;
			// 取模块的第一个输入类型做兼容性检查（作为下游，来源是 fromPort 的输出类型）
			// Check compatibility with the first input type (as downstream, source is fromPort's output type)
			const firstInput = (m.inputTypes?.[0] ?? "any_file") as PortType;
			return isPortCompatible(wireMenu.fromPort!.type, firstInput);
		});
	});

	/**
	 * 从连线菜单中选择模块：在释放位置添加节点，并将拖拽起点的输出端口（fromPort）
	 * 连接到新节点的输入端口 0——新模块作为下游，接收用户原本想连接的输出。
	 *
	 * Select a module from the wire menu: add a node at the drop position, and wire the
	 * drag-origin output port (fromPort) to the new node's input port 0 — the new module
	 * becomes the downstream target receiving the output the user was trying to connect.
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
