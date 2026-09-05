/**
 * 社区模块 Store / Community Modules Store
 *
 * 前端直接拉取 GitHub 中央索引和各模块仓库的 registry.json，
 * 合并本地安装状态后展示。安装/卸载通过后端 API 完成（后端负责文件下载和写入）。
 *
 * The frontend directly fetches the GitHub central index and each module repo's
 * registry.json, merges with local installation status, then displays the result.
 * Install/uninstall is handled via the backend API (backend handles file I/O).
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { call, on } from "@/lib/api";
import { useSettingsStore } from "@/stores/settings";

// ─── 中央索引 URL / Central Index URL ─────────────────────────────────────────

const CENTRAL_INDEX_URL =
	"https://raw.githubusercontent.com/ChanTrail/StripchatRecorderCommunity/master/registry.json";

// ─── 类型定义 / Type Definitions ─────────────────────────────────────────────

/** 中央索引中单个条目 / Single entry in the central index */
interface IndexEntry {
	id: string;
	repo: string;
}

/** 模块维护者仓库中的 registry.json 结构 / Module repo's registry.json structure */
export interface RegistryModule {
	id: string;
	name: string;
	description: string;
	author: string;
	tags: string[];
	license: string;
	latestVersion: string;
	downloads: Record<string, string>;
	sha256: Record<string, string>;
	repo: string;
}

/** 带安装状态的社区模块条目 / Community module entry with installation status */
export interface CommunityModule extends RegistryModule {
	/** 当前平台是否有可下载的二进制文件 / Whether a binary is available for this platform */
	platformSupported: boolean;
	/** 本地已安装版本（未安装时为 null）/ Locally installed version (null if not installed) */
	installedVersion: string | null;
	/** 是否有可用更新 / Whether an update is available */
	updateAvailable: boolean;
}

// ─── 平台检测 / Platform Detection ───────────────────────────────────────────

/**
 * 检测当前浏览器/平台对应的平台标识符。
 * 与后端 `current_platform()` 和 release 脚本的 `detectPlatform()` 保持一致。
 *
 * Detect the current browser/platform identifier.
 * Consistent with the backend `current_platform()` and release script `detectPlatform()`.
 */
function detectPlatform(): string {
	const ua = navigator.userAgent.toLowerCase();
	// macOS
	if (ua.includes("mac os x") || ua.includes("macintosh")) {
		// Apple Silicon 浏览器会在 ua 中包含 arm 相关信息，或通过 navigator.platform 判断
		// Apple Silicon browsers include arm info in ua or via navigator.platform
		const isArm =
			ua.includes("arm") ||
			(navigator as unknown as { userAgentData?: { platform?: string } })
				.userAgentData?.platform
				?.toLowerCase()
				.includes("arm") ||
			false;
		return isArm ? "darwin-aarch64" : "darwin-x86_64";
	}
	// Windows
	if (ua.includes("win")) {
		return "windows-x86_64";
	}
	// Linux
	if (ua.includes("aarch64") || ua.includes("arm64")) {
		return "linux-aarch64";
	}
	return "linux-x86_64";
}

// ─── 版本比较 / Semver Comparison ────────────────────────────────────────────

function compareSemver(a: string, b: string): number {
	const parse = (s: string) => s.split(".").map((x) => parseInt(x, 10) || 0);
	const va = parse(a);
	const vb = parse(b);
	const len = Math.max(va.length, vb.length);
	for (let i = 0; i < len; i++) {
		const diff = (va[i] ?? 0) - (vb[i] ?? 0);
		if (diff !== 0) return diff;
	}
	return 0;
}

/**
 * 将镜像站 URL 应用到给定 URL 上（在原始 URL 前加前缀）。
 * Apply mirror URL to a given URL (prepend mirror to original URL).
 */
function applyMirror(url: string, mirror: string | null | undefined): string {
	if (!mirror) return url;
	return `${mirror.replace(/\/$/, "")}/${url}`;
}

// ─── raw URL 构造 / Raw URL Construction ─────────────────────────────────────

/**
 * 将仓库 URL 转换为 registry.json 的 raw 内容 URL（master 优先，main 备用）。
 * Convert a repo URL to raw registry.json URLs (master first, main fallback).
 */
function repoRegistryUrls(repo: string): [string, string] {
	const r = repo.replace(/\/$/, "");
	const match = r.match(/^https:\/\/github\.com\/(.+)$/);
	if (match) {
		const path = match[1];
		return [
			`https://raw.githubusercontent.com/${path}/master/registry.json`,
			`https://raw.githubusercontent.com/${path}/main/registry.json`,
		];
	}
	return [
		`${r}/raw/master/registry.json`,
		`${r}/raw/main/registry.json`,
	];
}

// ─── Store ────────────────────────────────────────────────────────────────────

export const useCommunityStore = defineStore("community", () => {
	const settingsStore = useSettingsStore();

	/** 模块列表 / Module list */
	const modules = ref<CommunityModule[]>([]);
	/** 是否正在加载 / Whether loading */
	const loading = ref(false);
	/** 加载错误信息（null 表示无错误）/ Load error message (null = no error) */
	const error = ref<string | null>(null);
	/** 正在安装/卸载的模块 ID 集合 / Set of module IDs currently being installed/uninstalled */
	const pendingIds = ref<Set<string>>(new Set());
	/**
	 * 各模块的下载进度快照（moduleId → { downloaded, total, pct }）。
	 * pct = -1 表示 Content-Length 未知，只能显示已下载字节数。
	 *
	 * Download progress per module (moduleId → { downloaded, total, pct }).
	 * pct = -1 means Content-Length is unknown; only downloaded bytes can be shown.
	 */
	const downloadProgress = ref<Map<string, { downloaded: number; total: number; pct: number }>>(new Map());

	// ─── SSE 订阅 / SSE Subscriptions ─────────────────────────────────────────

	// 下载进度 / Download progress
	on("community-module-download-progress", (payload) => {
		const p = payload as { moduleId: string; downloaded: number; total: number; pct: number };
		downloadProgress.value = new Map(downloadProgress.value).set(p.moduleId, {
			downloaded: p.downloaded,
			total: p.total,
			pct: p.pct,
		});
	});

	// 安装完成（成功或失败）/ Install done (success or failure)
	on("community-module-install-done", (payload) => {
		const p = payload as { moduleId: string; success: boolean; error?: string };
		// 清除进度和 pending 状态 / Clear progress and pending state
		const nextProgress = new Map(downloadProgress.value);
		nextProgress.delete(p.moduleId);
		downloadProgress.value = nextProgress;
		const nextPending = new Set(pendingIds.value);
		nextPending.delete(p.moduleId);
		pendingIds.value = nextPending;
		// 安装成功后刷新模块列表 / Refresh module list on success
		if (p.success) {
			fetchModules();
		}
	});

	// SSE 重连后拉取进行中的安装任务，恢复 pending 状态 / Restore pending state on SSE reconnect
	async function restoreInstallTasks() {
		try {
			const tasks = await call<Record<string, number>>("get_install_tasks");
			if (!tasks || Object.keys(tasks).length === 0) return;
			const nextPending = new Set(pendingIds.value);
			const nextProgress = new Map(downloadProgress.value);
			for (const [moduleId, downloaded] of Object.entries(tasks)) {
				nextPending.add(moduleId);
				nextProgress.set(moduleId, { downloaded, total: 0, pct: -1 });
			}
			pendingIds.value = nextPending;
			downloadProgress.value = nextProgress;
		} catch {
			// 静默忽略 / silently ignore
		}
	}

	// 订阅 SSE 重连事件 / Subscribe to SSE reconnect
	import("@/lib/api").then(({ onSseReconnect }) => {
		onSseReconnect(() => restoreInstallTasks());
	});
	// 初始化时也拉一次（处理页面刷新后后端仍在安装的情况）
	// Also call on init (handles page refresh while backend is still installing)
	restoreInstallTasks();

	/**
	 * 从 GitHub 拉取社区模块列表：
	 * 1. 拉取中央索引（`registry.json`）得到 repo 列表
	 * 2. 并发拉取每个 repo 的 `registry.json`（master 优先，main 备用）
	 * 3. 从后端获取本地已安装模块列表，标注安装状态
	 *
	 * Fetch the community module list from GitHub:
	 * 1. Fetch the central index to get repo URLs
	 * 2. Concurrently fetch each repo's registry.json (master first, main fallback)
	 * 3. Fetch locally installed modules from the backend to annotate install status
	 */
	async function fetchModules() {
		loading.value = true;
		error.value = null;
		try {
			const mirror = settingsStore.settings.community_mirror_url;

			// 1. 拉取中央索引 / Fetch central index
			const indexUrl = applyMirror(CENTRAL_INDEX_URL, mirror);
			const indexResp = await fetch(indexUrl);
			if (!indexResp.ok) {
				throw new Error(`中央索引请求失败：HTTP ${indexResp.status}`);
			}
			const index: IndexEntry[] = await indexResp.json();

			// 2. 并发拉取各 repo 的 registry.json，应用镜像站
			//    Concurrently fetch each repo's registry.json, applying mirror
			const results = await Promise.allSettled(
				index.map(async (entry) => {
					const [masterUrl, mainUrl] = repoRegistryUrls(entry.repo);
					const urls = [applyMirror(masterUrl, mirror), applyMirror(mainUrl, mirror)];
					// 先试 master，失败再试 main
					let resp = await fetch(urls[0]);
					if (!resp.ok) {
						resp = await fetch(urls[1]);
					}
					if (!resp.ok) {
						throw new Error(`无法拉取 ${entry.repo} 的 registry.json`);
					}
					const mod: RegistryModule = await resp.json();
					mod.repo = entry.repo;
					return { indexEntry: entry, mod };
				}),
			);

			// 3. 按中央索引顺序保留成功的条目 / Keep successful entries in central index order
			const rawModules: RegistryModule[] = [];
			for (const result of results) {
				if (result.status === "fulfilled") {
					rawModules.push(result.value.mod);
				}
				// rejected 的静默跳过（仓库已删除或不可访问）/ silently skip rejected (repo deleted or unreachable)
			}

			// 4. 从后端获取已安装模块列表（discover_modules 扫描磁盘）
			//    Get locally installed modules from backend (discover_modules scans disk)
			const installedList = await call<Array<{ id: string; version: string }>>(
				"list_modules",
			);
			const installed = new Map<string, string>(
				(installedList ?? [])
					.filter((m) => !m.id.startsWith("__builtin__"))
					.map((m) => [m.id, m.version]),
			);

			// 5. 合并安装状态 / Merge installation status
			const platform = detectPlatform();
			modules.value = rawModules.map((rm) => {
				const platformSupported = platform in (rm.downloads ?? {});
				const installedVersion = installed.get(rm.id) ?? null;
				const updateAvailable =
					installedVersion !== null &&
					compareSemver(installedVersion, rm.latestVersion) < 0;
				return {
					...rm,
					platformSupported,
					installedVersion,
					updateAvailable,
				};
			});
		} catch (e) {
			error.value = e instanceof Error ? e.message : String(e);
		} finally {
			loading.value = false;
		}
	}

	/**
	 * 安装指定模块：将完整模块数据传给后端，后端负责下载和文件写入。
	 * Install the specified module: pass the complete module data to the backend,
	 * which handles the download and file writing.
	 */
	async function installModule(mod: CommunityModule): Promise<void> {
		pendingIds.value = new Set([...pendingIds.value, mod.id]);
		downloadProgress.value = new Map(downloadProgress.value).set(mod.id, { downloaded: 0, total: 0, pct: -1 });
		try {
			await call("install_community_module", { module: mod });
			// 安装 API 返回后清理状态（SSE 事件可能已先清理，这里是兜底）
			// Clean up state after API returns (SSE event may have already cleaned up; this is a fallback)
		} finally {
			const nextProgress = new Map(downloadProgress.value);
			nextProgress.delete(mod.id);
			downloadProgress.value = nextProgress;
			const next = new Set(pendingIds.value);
			next.delete(mod.id);
			pendingIds.value = next;
		}
	}

	/**
	 * 卸载指定模块。
	 * Uninstall the specified module.
	 */
	async function uninstallModule(moduleId: string): Promise<void> {
		pendingIds.value = new Set([...pendingIds.value, moduleId]);
		try {
			await call("uninstall_community_module", { moduleId });
			// 卸载后重新拉取列表 / Re-fetch the list after uninstall
			await fetchModules();
		} finally {
			const next = new Set(pendingIds.value);
			next.delete(moduleId);
			pendingIds.value = next;
		}
	}

	return {
		modules,
		loading,
		error,
		pendingIds,
		downloadProgress,
		fetchModules,
		installModule,
		uninstallModule,
	};
});
