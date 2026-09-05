<!--
    社区模块页面 / Community Modules View

    浏览并安装社区贡献的后处理模块。
    Browse and install community-contributed post-processing modules.

    布局：左侧为模块卡片网格，点击卡片后从右侧滑入详情面板。
    Layout: left side is the module card grid; clicking a card slides in a detail panel from the right.
-->
<script setup lang="ts">
import { onMounted, onUnmounted, computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useCommunityStore } from "@/stores/community";
import type { CommunityModule } from "@/stores/community";
import { useSettingsStore } from "@/stores/settings";
import { useNotify } from "@/composables/useNotify";
import { marked } from "marked";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import {
	RefreshCw, ExternalLink, Download, Trash2, ArrowUpCircle, PackageOpen,
	Search, Tag, X, BookOpen, History, Loader2, Check,
} from "@lucide/vue";

const { t } = useI18n();
const store = useCommunityStore();
const settingsStore = useSettingsStore();
const { toast } = useNotify();

onMounted(() => {
	store.fetchModules();
	document.addEventListener("click", onDocClick);
});

onUnmounted(() => {
	document.removeEventListener("click", onDocClick);
});

// ─── 筛选与搜索 / Filter & Search ─────────────────────────────────────────────

type FilterStatus = "all" | "installed" | "updatable";

const filterStatus = ref<FilterStatus>("all");
const filterTags   = ref<Set<string>>(new Set());
const searchQuery  = ref("");

const allTags = computed(() => {
	const set = new Set<string>();
	for (const m of store.modules) {
		for (const tag of m.tags) set.add(tag);
	}
	return [...set].sort();
});

const filteredModules = computed(() => {
	const q = searchQuery.value.trim().toLowerCase();
	return store.modules.filter((m) => {
		if (filterStatus.value === "installed" && !m.installedVersion) return false;
		if (filterStatus.value === "updatable" && !m.updateAvailable) return false;
		if (filterTags.value.size > 0 && ![...filterTags.value].every((tag) => m.tags.includes(tag))) return false;
		if (q) {
			const hit = m.name.toLowerCase().includes(q) || m.author.toLowerCase().includes(q);
			if (!hit) return false;
		}
		return true;
	});
});

// ─── 工具函数 / Utilities ─────────────────────────────────────────────────────

function isPending(moduleId: string) {
	return store.pendingIds.has(moduleId);
}

function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function getProgressLabel(moduleId: string, isUpdate = false): string | null {
	const prog = store.downloadProgress.get(moduleId);
	if (!prog) return null;
	// downloaded=0 且 pct=-1：还没收到第一个 chunk，正在建立连接
	// downloaded=0 and pct=-1: no chunk yet, still connecting
	if (prog.downloaded === 0 && prog.pct < 0) return t("community.connecting");
	// 有百分比：显示 "下载中 50%"
	if (prog.pct >= 0) return `${isUpdate ? t("community.updating") : t("community.installing")} ${Math.round(prog.pct)}%`;
	// 无 Content-Length：只显示已下载字节
	if (prog.downloaded > 0) return `${isUpdate ? t("community.updating") : t("community.installing")} ${formatBytes(prog.downloaded)}`;
	return null;
}

async function handleInstall(mod: CommunityModule) {
	try {
		await store.installModule(mod);
		toast(t("community.installSuccess", { name: mod.name }), "success");
	} catch (e) {
		toast(t("community.installError", { error: e instanceof Error ? e.message : String(e) }), "error");
	}
}

async function handleUninstall(mod: CommunityModule) {
	try {
		await store.uninstallModule(mod.id);
		toast(t("community.uninstallSuccess", { name: mod.name }), "success");
	} catch (e) {
		toast(t("community.uninstallError", { error: e instanceof Error ? e.message : String(e) }), "error");
	}
}

function openUrl(url: string) {
	window.open(url, "_blank", "noopener,noreferrer");
}

const installedCount = computed(() => store.modules.filter((m) => m.installedVersion).length);

function toggleTag(tag: string) {
	const next = new Set(filterTags.value);
	if (next.has(tag)) next.delete(tag);
	else next.add(tag);
	filterTags.value = next;
}

const tagDropdownRef = ref<HTMLDetailsElement | null>(null);

function onDocClick(e: MouseEvent) {
	if (tagDropdownRef.value && !tagDropdownRef.value.contains(e.target as Node)) {
		tagDropdownRef.value.open = false;
	}
}

// ─── 右侧详情面板 / Right detail panel ────────────────────────────────────────

/** 当前选中的模块 id（null = 面板关闭）/ Currently selected module id (null = panel closed) */
const selectedId = ref<string | null>(null);

const selectedMod = computed<CommunityModule | undefined>(() =>
	selectedId.value ? store.modules.find((m) => m.id === selectedId.value) : undefined,
);

function openDetail(mod: CommunityModule) {
	selectedId.value = mod.id;
}

function closeDetail() {
	selectedId.value = null;
}

/** 所有平台标识符 / All platform identifiers */
const allPlatforms = [
	"windows-x86_64",
	"linux-x86_64",
	"linux-aarch64",
	"darwin-x86_64",
	"darwin-aarch64",
];

// ─── README 加载 / README loading ─────────────────────────────────────────────

const readmeHtml    = ref<string | null>(null);
const readmeLoading = ref(false);
const readmeFailed  = ref(false);

function repoReadmeUrls(repo: string): [string, string] {
	const r = repo.replace(/\/$/, "");
	const match = r.match(/^https:\/\/github\.com\/(.+)$/);
	if (match) {
		const path = match[1];
		return [
			`https://raw.githubusercontent.com/${path}/master/README.md`,
			`https://raw.githubusercontent.com/${path}/main/README.md`,
		];
	}
	return [`${r}/raw/master/README.md`, `${r}/raw/main/README.md`];
}

function repoReleasesUrl(repo: string): string {
	return `${repo.replace(/\/$/, "")}/releases`;
}

function applyMirror(url: string, mirror: string | null | undefined): string {
	if (!mirror) return url;
	return `${mirror.replace(/\/$/, "")}/${url}`;
}

async function loadReadme(repo: string) {
	readmeHtml.value   = null;
	readmeFailed.value = false;
	readmeLoading.value = true;
	try {
		const mirror = settingsStore.settings.community_mirror_url;
		const [masterUrl, mainUrl] = repoReadmeUrls(repo);
		const urls = [applyMirror(masterUrl, mirror), applyMirror(mainUrl, mirror)];
		let text: string | null = null;
		for (const url of urls) {
			const res = await fetch(url);
			if (res.ok) { text = await res.text(); break; }
		}
		if (text === null) {
			readmeFailed.value = true;
		} else {
			readmeHtml.value = await Promise.resolve(marked(text));
		}
	} catch {
		readmeFailed.value = true;
	} finally {
		readmeLoading.value = false;
	}
}

// 选中模块变化时加载 README
// Load README when selected module changes
watch(selectedMod, (m) => {
	if (m?.repo) loadReadme(m.repo);
	else { readmeHtml.value = null; readmeFailed.value = false; }
});
</script>

<template>
	<div class="flex flex-col h-full overflow-hidden">

		<!-- ── 左侧：列表区 / Left: list area ──────────────────────────────── -->
		<div class="flex flex-col flex-1 min-w-0 overflow-hidden">
			<!-- 顶部工具栏 / Top toolbar -->
			<header class="flex flex-col gap-3 shrink-0 pb-4 bg-background sticky top-0 z-20 px-6 pt-6 border-b">
				<div class="flex items-center justify-between gap-4">
					<h1 class="text-xl font-bold">{{ t("community.title") }}</h1>
					<div class="flex items-center gap-2 shrink-0">
						<span v-if="installedCount > 0" class="text-sm text-muted-foreground">
							{{ installedCount }} {{ t("community.installed") }}
						</span>
						<Button
							variant="outline"
							size="sm"
							:disabled="store.loading"
							@click="store.fetchModules()"
						>
							<RefreshCw class="size-3.5 mr-1.5" :class="{ 'animate-spin': store.loading }" />
							{{ store.loading ? t("community.refreshing") : t("community.refresh") }}
						</Button>
					</div>
				</div>

				<!-- 搜索 + 筛选行 / Search + filter row -->
				<div class="flex items-center gap-2">
					<div class="relative w-1/2 shrink-0">
						<Search class="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground pointer-events-none" />
						<Input v-model="searchQuery" :placeholder="t('community.search')" class="pl-8 h-7 text-xs" />
					</div>

					<div class="inline-flex h-7 rounded-md border overflow-hidden shrink-0">
						<button
							v-for="(s, i) in (['all', 'installed', 'updatable'] as const)"
							:key="s"
							class="px-3 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
							:class="[
								filterStatus === s
									? 'bg-primary text-primary-foreground'
									: 'bg-background text-muted-foreground hover:bg-muted hover:text-foreground',
								i > 0 ? 'border-l' : '',
							]"
							@click="filterStatus = s"
						>
							{{ t(`community.filter${s.charAt(0).toUpperCase() + s.slice(1)}`) }}
						</button>
					</div>

					<details v-if="allTags.length > 0" ref="tagDropdownRef" class="relative shrink-0">
						<summary
							class="flex items-center gap-1.5 h-7 px-2.5 text-xs rounded-md border bg-background cursor-pointer select-none list-none hover:bg-muted transition-colors"
							:class="filterTags.size > 0 ? 'border-primary text-primary' : 'text-muted-foreground'"
						>
							<Tag class="size-3.5" />
							{{ t("community.filterByTag") }}
							<Badge v-if="filterTags.size > 0" class="text-[10px] px-1.5 py-0 h-4 ml-0.5 bg-primary/20 text-primary border-primary/30">
								{{ filterTags.size }}
							</Badge>
						</summary>
						<div class="absolute top-8 left-0 z-30 min-w-32 rounded-md border bg-popover shadow-md p-1.5 flex flex-col gap-0.5">
							<button
								v-if="filterTags.size > 0"
								class="text-xs text-left px-2 py-1 text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors"
								@click="filterTags = new Set()"
							>
								{{ t("community.filterAll") }}
							</button>
							<label
								v-for="tag in allTags"
								:key="tag"
								class="flex items-center gap-2 px-2 py-1 text-xs rounded cursor-pointer hover:bg-muted transition-colors"
							>
								<Checkbox :model-value="filterTags.has(tag)" class="size-3.5" @update:model-value="toggleTag(tag)" />
								{{ tag }}
							</label>
						</div>
					</details>
				</div>
			</header>

			<!-- 内容区 / Content area -->
			<div class="flex-1 overflow-y-auto px-6 py-6">

				<!-- 加载中骨架屏 -->
				<div v-if="store.loading && store.modules.length === 0" class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
					<div v-for="i in 6" :key="i" class="rounded-lg border bg-card p-5 flex flex-col gap-3 animate-pulse">
						<div class="h-4 bg-muted rounded w-2/3" />
						<div class="h-3 bg-muted rounded w-full" />
						<div class="h-3 bg-muted rounded w-4/5" />
					</div>
				</div>

				<!-- 错误状态 -->
				<div v-else-if="store.error && store.modules.length === 0" class="flex flex-col items-center justify-center gap-4 py-24 text-center">
					<PackageOpen class="size-12 text-muted-foreground/40" />
					<p class="text-sm text-destructive">{{ store.error }}</p>
					<p class="text-sm text-muted-foreground">{{ t("community.fetchError") }}</p>
					<Button variant="outline" size="sm" @click="store.fetchModules()">{{ t("community.retry") }}</Button>
				</div>

				<!-- 空状态 -->
				<div v-else-if="!store.loading && store.modules.length === 0" class="flex flex-col items-center justify-center gap-3 py-24 text-center">
					<PackageOpen class="size-12 text-muted-foreground/40" />
					<p class="text-sm text-muted-foreground">{{ t("community.empty") }}</p>
				</div>

				<!-- 筛选后无结果 -->
				<div v-else-if="filteredModules.length === 0" class="flex flex-col items-center justify-center gap-3 py-24 text-center">
					<PackageOpen class="size-12 text-muted-foreground/40" />
					<p class="text-sm text-muted-foreground">{{ t("community.noResults") }}</p>
				</div>

				<!-- 模块卡片网格 / Module card grid -->
				<div v-else class="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
					<div
						v-for="mod in filteredModules"
						:key="mod.id"
						class="rounded-lg border bg-card p-5 flex flex-col gap-3 cursor-pointer transition-all
							hover:border-primary/40 hover:shadow-md hover:bg-accent/20"
						:class="{
							'opacity-60': !mod.platformSupported,
							'border-primary ring-1 ring-primary/30': selectedId === mod.id,
						}"
						@click="openDetail(mod)"
					>
						<!-- 卡片头部：名称 + 状态徽章 -->
						<div class="flex items-start justify-between gap-2 min-w-0">
							<h3 class="font-semibold text-sm leading-tight truncate">{{ mod.name }}</h3>
							<div class="flex flex-wrap gap-1 shrink-0 justify-end">
								<Badge v-if="mod.installedVersion" class="text-[10px] px-1.5 py-0 h-4 bg-emerald-500/20 text-emerald-500 border-emerald-500/30">
									{{ t("community.installed") }}
								</Badge>
								<Badge v-if="mod.updateAvailable" class="text-[10px] px-1.5 py-0 h-4 bg-amber-500/20 text-amber-500 border-amber-500/30">
									{{ t("community.updateAvailable") }}
								</Badge>
								<Badge v-if="!mod.platformSupported" variant="outline" class="text-[10px] px-1.5 py-0 h-4 text-muted-foreground">
									{{ t("community.notSupported") }}
								</Badge>
							</div>
						</div>

						<!-- 描述 -->
						<p class="text-sm text-muted-foreground leading-relaxed line-clamp-2 flex-1">
							{{ mod.description }}
						</p>

						<!-- 元信息：作者 / 版本 / 许可 / Meta: author / version / license -->
						<div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
							<span v-if="mod.author">
								<span class="font-medium text-foreground/70">{{ t("community.author") }}</span>
								&nbsp;<span class="text-foreground">{{ mod.author }}</span>
							</span>
							<span>
								<span class="font-medium text-foreground/70">{{ t("community.latestVersion") }}</span>
								&nbsp;<span class="font-mono font-semibold text-amber-500">{{ mod.latestVersion }}</span>
							</span>
							<span v-if="mod.installedVersion">
								<span class="font-medium text-foreground/70">{{ t("community.installedVersion") }}</span>
								&nbsp;<span class="font-mono text-emerald-500">{{ mod.installedVersion }}</span>
							</span>
							<span v-if="mod.license">
								<span class="font-medium">{{ t("community.license") }}</span>
								&nbsp;<span class=" text-primary">{{ mod.license }}</span>
							</span>
						</div>

						<!-- 标签 -->
						<div v-if="mod.tags.length > 0" class="flex flex-wrap gap-1">
							<Badge v-for="tag in mod.tags" :key="tag" variant="secondary" class="text-[10px] px-1.5 py-0 h-4">
								{{ tag }}
							</Badge>
						</div>

						<!-- 操作按钮行 -->
						<div class="flex items-center gap-2 pt-1 border-t border-border/50">
							<Button
								v-if="!mod.installedVersion || mod.updateAvailable"
								size="sm"
								class="h-7 text-xs px-3 flex-1"
								:disabled="!mod.platformSupported || isPending(mod.id)"
								@click.stop="handleInstall(mod)"
							>
								<ArrowUpCircle v-if="mod.updateAvailable" class="size-3.5 mr-1" />
								<Download v-else class="size-3.5 mr-1" />
								<template v-if="isPending(mod.id)">
									{{ getProgressLabel(mod.id, mod.updateAvailable) ?? (mod.updateAvailable ? t("community.updating") : t("community.installing")) }}
								</template>
								<template v-else-if="mod.updateAvailable">{{ t("community.update") }}</template>
								<template v-else>{{ t("community.install") }}</template>
							</Button>

							<Button
								v-if="mod.installedVersion"
								variant="outline"
								size="sm"
								class="h-7 text-xs px-3"
								:class="{ 'flex-1': !mod.updateAvailable }"
								:disabled="isPending(mod.id)"
								@click.stop="handleUninstall(mod)"
							>
								<Trash2 class="size-3.5 mr-1" />
								{{ t("community.uninstall") }}
							</Button>

							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-xs px-2 text-muted-foreground hover:text-foreground ml-auto"
								:title="t('community.viewRepo')"
								@click.stop="openUrl(mod.repo)"
							>
								<ExternalLink class="size-3.5" />
							</Button>
						</div>
					</div>
				</div>
			</div>
		</div>

		<!-- ── 详情面板（overlay，从右侧滑入）/ Detail panel (overlay, slides in from right) ── -->

		<!-- 遮罩层 / Backdrop -->
		<Transition
			enter-active-class="transition-opacity duration-200"
			enter-from-class="opacity-0"
			enter-to-class="opacity-100"
			leave-active-class="transition-opacity duration-200"
			leave-from-class="opacity-100"
			leave-to-class="opacity-0"
		>
			<div
				v-if="selectedMod"
				class="fixed inset-0 z-30 bg-black/30"
				@click="closeDetail"
			/>
		</Transition>

		<!-- 面板主体 / Panel body -->
		<Transition
			enter-active-class="transition-transform duration-250 ease-out"
			enter-from-class="translate-x-full"
			enter-to-class="translate-x-0"
			leave-active-class="transition-transform duration-200 ease-in"
			leave-from-class="translate-x-0"
			leave-to-class="translate-x-full"
		>
			<div
				v-if="selectedMod"
				class="fixed right-0 top-0 z-40 w-[460px] max-w-[90vw] h-full border-l bg-background shadow-xl flex flex-col overflow-hidden"
			>
				<!-- 面板顶栏 / Panel top bar -->
				<div class="flex items-start justify-between gap-3 px-5 pt-5 pb-4 border-b shrink-0">
					<div class="flex flex-col gap-1 min-w-0">
						<div class="flex items-center gap-2 flex-wrap">
							<h2 class="font-semibold text-base leading-tight truncate">{{ selectedMod.name }}</h2>
							<Badge v-if="selectedMod.installedVersion" class="text-[10px] px-1.5 py-0 h-4 bg-emerald-500/20 text-emerald-500 border-emerald-500/30 shrink-0">
								{{ t("community.installed") }}
							</Badge>
							<Badge v-if="selectedMod.updateAvailable" class="text-[10px] px-1.5 py-0 h-4 bg-amber-500/20 text-amber-500 border-amber-500/30 shrink-0">
								{{ t("community.updateAvailable") }}
							</Badge>
							<Badge v-if="!selectedMod.platformSupported" variant="outline" class="text-[10px] px-1.5 py-0 h-4 text-muted-foreground shrink-0">
								{{ t("community.notSupported") }}
							</Badge>
						</div>
						<p class="text-xs text-muted-foreground leading-relaxed line-clamp-2">{{ selectedMod.description }}</p>
					</div>
					<Button variant="ghost" size="sm" class="h-7 w-7 p-0 shrink-0 text-muted-foreground hover:text-foreground" @click="closeDetail">
						<X class="size-4" />
					</Button>
				</div>

				<!-- 面板内容区（可滚动）/ Panel content (scrollable) -->
				<div class="flex-1 overflow-y-auto">
					<div class="px-5 py-4 flex flex-col gap-5">

						<!-- 关键信息表格 / Key info table -->
						<table class="w-full text-xs border-collapse">
							<tbody>
								<tr v-if="selectedMod.author" class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap w-[6.5rem] align-top">{{ t("community.detail.author") }}</td>
									<td class="py-2 font-medium text-foreground">{{ selectedMod.author }}</td>
								</tr>
								<tr class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.latestVersion") }}</td>
									<td class="py-2 font-mono font-semibold text-amber-500">{{ selectedMod.latestVersion }}</td>
								</tr>
								<tr v-if="selectedMod.installedVersion" class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.installedVersion") }}</td>
									<td class="py-2 font-mono font-semibold text-emerald-500">{{ selectedMod.installedVersion }}</td>
								</tr>
								<tr v-if="selectedMod.license" class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.license") }}</td>
									<td class="py-2 text-foreground">{{ selectedMod.license }}</td>
								</tr>
								<tr v-if="selectedMod.tags.length > 0" class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.tags") }}</td>
									<td class="py-2">
										<div class="flex flex-wrap gap-1">
											<Badge v-for="tag in selectedMod.tags" :key="tag" variant="secondary" class="text-[10px] px-1.5 py-0 h-4">{{ tag }}</Badge>
										</div>
									</td>
								</tr>
								<tr class="border-b border-border/50">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.platforms") }}</td>
									<td class="py-2">
										<div class="flex flex-col gap-1">
											<div
												v-for="platform in allPlatforms"
												:key="platform"
												class="flex items-center justify-between gap-3"
											>
												<span :class="platform in selectedMod.downloads ? 'text-foreground' : 'text-muted-foreground/40'">
													{{ platform }}
												</span>
												<Check v-if="platform in selectedMod.downloads" class="size-3.5 text-emerald-500 shrink-0" />
												<span v-else class="size-3.5 shrink-0" />
											</div>
										</div>
									</td>
								</tr>
								<tr v-if="selectedMod.repo">
									<td class="py-2 pr-3 text-muted-foreground whitespace-nowrap align-top">{{ t("community.detail.repo") }}</td>
									<td class="py-2">
										<button class="text-primary hover:underline flex items-center gap-1 min-w-0 w-full" @click="openUrl(selectedMod.repo)">
											<span class="truncate">{{ selectedMod.repo }}</span>
											<ExternalLink class="size-3 shrink-0" />
										</button>
									</td>
								</tr>
							</tbody>
						</table>

						<!-- 操作按钮 / Action buttons -->
						<div class="flex flex-wrap items-center gap-2">
							<Button
								v-if="!selectedMod.installedVersion || selectedMod.updateAvailable"
								size="sm"
								class="gap-1.5 h-8"
								:disabled="!selectedMod.platformSupported || isPending(selectedMod.id)"
								@click="handleInstall(selectedMod)"
							>
								<ArrowUpCircle v-if="selectedMod.updateAvailable" class="size-3.5" />
								<Download v-else class="size-3.5" />
								<template v-if="isPending(selectedMod.id)">
									{{ getProgressLabel(selectedMod.id, selectedMod.updateAvailable) ?? (selectedMod.updateAvailable ? t("community.updating") : t("community.installing")) }}
								</template>
								<template v-else-if="selectedMod.updateAvailable">{{ t("community.update") }}</template>
								<template v-else>{{ t("community.install") }}</template>
							</Button>

							<Button
								v-if="selectedMod.installedVersion"
								variant="outline"
								size="sm"
								class="gap-1.5 h-8"
								:disabled="isPending(selectedMod.id)"
								@click="handleUninstall(selectedMod)"
							>
								<Trash2 class="size-3.5" />
								{{ t("community.uninstall") }}
							</Button>

							<Button
								v-if="selectedMod.repo"
								variant="outline"
								size="sm"
								class="gap-1.5 h-8"
								@click="openUrl(repoReleasesUrl(selectedMod.repo))"
							>
								<History class="size-3.5" />
								{{ t("community.detail.changelog") }}
								<ExternalLink class="size-3 text-muted-foreground" />
							</Button>

							<p v-if="!selectedMod.platformSupported" class="text-xs text-muted-foreground w-full">
								{{ t("community.detail.platformNotSupported") }}
							</p>
						</div>

						<!-- README / README section -->
						<div v-if="selectedMod.repo" class="flex flex-col gap-2">
							<div class="flex items-center gap-1.5 text-xs font-semibold text-foreground">
								<BookOpen class="size-3.5 text-muted-foreground" />
								{{ t("community.detail.readme") }}
							</div>

							<div v-if="readmeLoading" class="flex items-center gap-2 py-6 justify-center text-xs text-muted-foreground">
								<Loader2 class="size-3.5 animate-spin" />
								{{ t("community.detail.readmeLoading") }}
							</div>

							<div v-else-if="readmeFailed" class="py-6 text-center text-xs text-muted-foreground">
								{{ t("community.detail.readmeFailed") }}
							</div>

							<div
								v-else-if="readmeHtml"
								class="rounded-lg border bg-card/50 px-4 py-3 text-xs leading-relaxed overflow-x-hidden
									[&_h1]:text-base [&_h1]:font-bold [&_h1]:mt-4 [&_h1]:mb-2
									[&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-1.5
									[&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2.5 [&_h3]:mb-1
									[&_p]:my-1.5 [&_p]:text-muted-foreground
									[&_a]:text-primary [&_a]:underline-offset-2 hover:[&_a]:underline
									[&_ul]:list-disc [&_ul]:pl-4 [&_ul]:my-1.5 [&_ul]:text-muted-foreground
									[&_ol]:list-decimal [&_ol]:pl-4 [&_ol]:my-1.5 [&_ol]:text-muted-foreground
									[&_li]:my-0.5
									[&_code]:bg-muted [&_code]:px-1 [&_code]:py-0.5 [&_code]:rounded [&_code]:text-[11px] [&_code]:font-mono
									[&_pre]:bg-muted [&_pre]:border [&_pre]:rounded [&_pre]:p-3 [&_pre]:my-2 [&_pre]:overflow-x-auto [&_pre]:text-[11px]
									[&_pre_code]:bg-transparent [&_pre_code]:p-0
									[&_blockquote]:border-l-2 [&_blockquote]:border-border [&_blockquote]:pl-3 [&_blockquote]:text-muted-foreground [&_blockquote]:my-1.5
									[&_hr]:border-border [&_hr]:my-3
									[&_img]:rounded [&_img]:max-w-full
									[&_table]:w-full [&_table]:border-collapse [&_table]:my-2 [&_table]:text-[11px]
									[&_th]:border [&_th]:border-border [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_th]:bg-muted [&_th]:font-semibold
									[&_td]:border [&_td]:border-border [&_td]:px-2 [&_td]:py-1
									[&>*:first-child]:mt-0"
								v-html="readmeHtml"
							/>
						</div>

					</div>
				</div>
			</div>
		</Transition>

	</div>
</template>
