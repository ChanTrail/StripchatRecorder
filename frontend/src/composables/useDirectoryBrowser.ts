/**
 * 全局目录浏览器状态管理 / Global Directory Browser State Management
 *
 * 整个应用共享同一个目录浏览器对话框实例（挂载一次于 App.vue），
 * 任意组件调用 useDirectoryBrowser().open(...) 即可复用它来选择目录，
 * 弹窗内容（当前路径、列表等）随每次调用传入的初始路径和回调而变化。
 * 避免了每个使用场景各自实例化一份 DirectoryBrowserDialog 组件。
 *
 * The entire app shares a single directory browser dialog instance (mounted once
 * in App.vue). Any component can call useDirectoryBrowser().open(...) to reuse it
 * for picking a directory; the dialog's content (current path, listing, etc.)
 * changes per call based on the initial path and callback passed in. This avoids
 * every call site instantiating its own copy of the DirectoryBrowserDialog component.
 */

import { reactive } from "vue";
import { call } from "@/lib/api";

export interface DirEntry {
	name: string;
	path: string;
}

interface ListDirResult {
	path: string;
	parent: string | null;
	dirs: DirEntry[];
	is_drives: boolean;
}

/** 全局共享状态（模块级单例，所有调用方共用同一份）/ Globally shared state (module-level singleton, shared by all callers) */
const state = reactive({
	visible: false,
	currentPath: "",
	pathInput: "",
	parentPath: null as string | null,
	dirs: [] as DirEntry[],
	/** 是否处于"此电脑/驱动器列表"视图（此时不可确认选择）/ Whether in the "This PC / drive list" view (selection disabled) */
	isDrives: false,
	loading: false,
	error: "",
	showNewFolder: false,
	newFolderName: "",
	creatingFolder: false,
});

/** 当前打开会话的选择回调 / Selection callback for the currently open session */
let onSelectCallback: ((path: string) => void) | null = null;

async function load(path: string) {
	state.loading = true;
	state.error = "";
	try {
		const result = await call<ListDirResult>("list_dir", { path });
		state.currentPath = result.path;
		state.pathInput = result.path;
		state.parentPath = result.parent;
		state.dirs = result.dirs;
		state.isDrives = false;
	} catch (e) {
		state.error = String(e);
	} finally {
		state.loading = false;
	}
}

async function loadDrives() {
	state.loading = true;
	state.error = "";
	try {
		const result = await call<ListDirResult>("list_drives");
		state.currentPath = "";
		state.pathInput = "";
		state.parentPath = null;
		state.dirs = result.dirs;
		state.isDrives = true;
	} catch (e) {
		state.error = String(e);
	} finally {
		state.loading = false;
	}
}

/**
 * 打开目录浏览器，选择结果通过 onSelect 回调返回。
 * Open the directory browser; the selected path is returned via the onSelect callback.
 *
 * @param initialPath - 初始路径（为空或无效时后端会回退到用户主目录）/ Initial path (backend falls back to home dir if empty/invalid)
 * @param onSelect - 用户确认选择时的回调 / Callback invoked when the user confirms the selection
 */
function open(initialPath: string, onSelect: (path: string) => void) {
	onSelectCallback = onSelect;
	state.showNewFolder = false;
	state.newFolderName = "";
	state.visible = true;
	void load(initialPath);
}

function enterDir(dir: DirEntry) {
	void load(dir.path);
}

function goUp() {
	if (state.parentPath != null) void load(state.parentPath);
}

function goToInputPath() {
	void load(state.pathInput);
}

function refresh() {
	if (state.isDrives) void loadDrives();
	else void load(state.currentPath);
}

/** 显示"此电脑"驱动器列表 / Show the "This PC" drive list */
function showDrives() {
	void loadDrives();
}

function toggleNewFolder() {
	state.showNewFolder = !state.showNewFolder;
	state.newFolderName = "";
}

async function createFolder() {
	const name = state.newFolderName.trim();
	if (!name || state.isDrives) return;
	state.creatingFolder = true;
	state.error = "";
	try {
		await call("create_dir", { parent: state.currentPath, name });
		state.showNewFolder = false;
		state.newFolderName = "";
		await load(state.currentPath);
	} catch (e) {
		state.error = String(e);
	} finally {
		state.creatingFolder = false;
	}
}

function confirmSelect() {
	if (state.isDrives) return;
	onSelectCallback?.(state.currentPath);
	state.visible = false;
}

function cancel() {
	state.visible = false;
}

/**
 * 获取全局目录浏览器的状态与操作方法。
 * Get the global directory browser's state and action methods.
 */
export function useDirectoryBrowser() {
	return {
		state,
		open,
		enterDir,
		goUp,
		goToInputPath,
		refresh,
		showDrives,
		toggleNewFolder,
		createFolder,
		confirmSelect,
		cancel,
	};
}
