/**
 * 全局目录浏览器状态管理 / Global Directory Browser State Management
 *
 * 整个应用共享同一个目录浏览器对话框实例（挂载一次于 App.vue），
 * 任意组件调用 useDirectoryBrowser().open(...) 即可复用它来选择目录。
 *
 * The entire app shares a single directory browser dialog instance (mounted once
 * in App.vue). Any component can call useDirectoryBrowser().open(...) to reuse it
 * for picking a directory.
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
	/** 文件条目（仅文件模式下返回）/ File entries (returned only in file mode) */
	files?: DirEntry[];
	is_drives: boolean;
}

/** 全局共享状态（模块级单例）/ Globally shared state (module-level singleton) */
const state = reactive({
	visible: false,
	/** 选择模式："dir" 选目录，"file" 选文件 / Pick mode: "dir" picks a directory, "file" picks a file */
	mode: "dir" as "dir" | "file",
	currentPath: "",
	pathInput: "",
	parentPath: null as string | null,
	dirs: [] as DirEntry[],
	/** 当前目录下的文件条目（仅文件模式填充）/ File entries under the current dir (populated only in file mode) */
	files: [] as DirEntry[],
	/** 文件条目是否仅列可执行程序（选程序场景）/ Whether file entries are restricted to executables (program picking) */
	execOnly: false,
	/** 文件模式下当前选中的文件路径 / Currently selected file path (file mode) */
	selectedFile: "",
	/** 是否处于"此电脑/驱动器列表"视图 / Whether in the "This PC / drive list" view */
	isDrives: false,
	loading: false,
	error: "",
	showNewFolder: false,
	newFolderName: "",
	creatingFolder: false,
});

/** 当前打开会话的选择回调 / Selection callback for the currently open session */
let onSelectCallback: ((path: string) => void) | null = null;
/** 文件模式打开时待预选中的文件名（load 成功后消费一次）/ File name to
 * preselect after the file-mode open's first load (consumed once) */
let preselectName: string | null = null;

async function load(path: string) {
	state.loading = true;
	state.error = "";
	try {
		const result = await call<ListDirResult>("list_dir", {
			path,
			// tauri 命令参数走 camelCase 约定（snake_case 键匹配不上会变
			// undefined，Option 落默认 false——曾导致文件模式只回目录）/
			// tauri command args follow camelCase (a snake_case key misses the
			// binding and the Option falls back to false — which made file mode
			// return directories only)
			includeFiles: state.mode === "file",
			execOnly: state.execOnly,
		});
		state.currentPath = result.path;
		state.pathInput = result.path;
		state.parentPath = result.parent;
		state.dirs = result.dirs;
		state.files = result.files ?? [];
		// 进入新目录时清掉上次选中；若本次打开带预选文件名且该文件在列，恢复选中
		// Clear the previous selection on entering a new dir; restore it when a
		// preselected file name (from open()) exists in this listing
		state.selectedFile =
			(preselectName && state.files.find((f) => f.name === preselectName)?.path) || "";
		preselectName = null;
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
 * `opts.file` 为 true 时进入文件选择模式（列出文件条目，确认时返回选中的
 * 文件路径）；此时 initialPath 若是文件路径会自动取其父目录并把文件名
 * 预选高亮。
 *
 * Open the directory browser; the selected path is returned via the onSelect
 * callback. `opts.file` enables file-picking mode (file entries are listed and
 * confirm returns the selected file path); a file path passed as initialPath
 * is then split into its parent dir plus a preselected file name.
 */
function open(
	initialPath: string,
	onSelect: (path: string) => void,
	opts?: { file?: boolean; exec?: boolean },
) {
	onSelectCallback = onSelect;
	state.mode = opts?.file ? "file" : "dir";
	// 选程序场景只列可执行文件（exec 隐含 file）/ Program picking lists
	// executables only (exec implies file)
	if (opts?.exec) state.mode = "file";
	state.execOnly = !!opts?.exec;
	state.showNewFolder = false;
	state.newFolderName = "";
	state.selectedFile = "";
	// 避免上一轮文件模式的残留条目在 load 完成前闪现 / Clear leftover
	// file entries from a previous file-mode session until load fills them
	state.files = [];
	preselectName = null;
	let startDir = initialPath;
	if (state.mode === "file" && initialPath && !/[\\/]$/.test(initialPath)) {
		// 初始值看起来像文件路径：拆开为父目录 + 预选文件名 /
		// The initial value looks like a file path: split into parent dir +
		// preselected file name
		const idx = Math.max(initialPath.lastIndexOf("/"), initialPath.lastIndexOf("\\"));
		if (idx > 0) {
			startDir = initialPath.slice(0, idx);
			preselectName = initialPath.slice(idx + 1);
		}
	}
	state.visible = true;
	void load(startDir);
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

/** 文件模式：选中一个文件条目 / File mode: mark a file entry selected */
function selectFile(entry: DirEntry) {
	state.selectedFile = entry.path;
}

function confirmSelect() {
	if (state.isDrives) return;
	if (state.mode === "file") {
		// 文件模式确认时必须有选中文件 / File mode requires a selected file to confirm
		if (!state.selectedFile) return;
		onSelectCallback?.(state.selectedFile);
	} else {
		onSelectCallback?.(state.currentPath);
	}
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
		selectFile,
		confirmSelect,
		cancel,
	};
}
