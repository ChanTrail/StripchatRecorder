<!--
	关于页面 / About Page

	显示版本信息、项目链接、贡献者列表和应用内更新。
	Desktop 版用 @tauri-apps/plugin-updater 检查并应用更新；
	更新需签名密钥（tauri.conf.json plugins.updater.pubkey），
	详见 README 中"Desktop 更新密钥配置"章节。
	贡献者列表直接调 GitHub API。

	Displays version info, links, contributors, and in-app updates.
	Desktop uses @tauri-apps/plugin-updater to check for and apply updates;
	updates require a signing key (tauri.conf.json plugins.updater.pubkey),
	see README "Desktop Update Key Setup" section.
	Contributors are fetched directly from the GitHub API.
-->
<script setup lang="ts">
	import { ref, computed, onMounted, onUnmounted } from "vue";
	import { useI18n } from "vue-i18n";
	import { getVersion } from "@tauri-apps/api/app";
	import { relaunch } from "@tauri-apps/plugin-process";
	import { invoke } from "@tauri-apps/api/core";
	import { on } from "@/lib/api";
	import {
		Bug, ExternalLink, Scale, Users, Link, RefreshCw,
		ChevronDown, ChevronUp, Download, CheckCircle2, AlertCircle, Loader2,
	} from "@lucide/vue";
	import { Button } from "@/components/ui/button";

	const { t } = useI18n();

	const appName = "StripchatRecorder";
	const owner = "ChanTrail";
	const repo = "StripchatRecorder";
	const repoUrl = `https://github.com/${owner}/${repo}`;
	const bugsUrl = `${repoUrl}/issues/new`;
	const contributorsUrl = `${repoUrl}/graphs/contributors`;
	const currentYear = new Date().getFullYear();

	// ── GitHub API 贡献者类型 ─────────────────────────────────────────────────
	interface GhContributor {
		login: string;
		avatar_url: string;
		html_url: string;
		contributions: number;
	}

	// ── 更新状态 ──────────────────────────────────────────────────────────────
	type UpdateState =
		| { phase: "idle" }
		| { phase: "checking" }
		| { phase: "available"; version: string; date: number | null; body: string | null }
		| { phase: "none" }
		| { phase: "error"; message: string }
		| { phase: "downloading"; pct: number | null; downloaded: number; total: number }
		| { phase: "installing" }
		| { phase: "done" };

	const updateState = ref<UpdateState>({ phase: "idle" });
	const changelogExpanded = ref(false);

	const hasUpdate = computed(() =>
		updateState.value.phase === "available"
		|| updateState.value.phase === "downloading"
		|| updateState.value.phase === "installing"
		|| updateState.value.phase === "done"
	);

	// ── 开发模式测试开关（点版本号 5 次激活）/ Dev-mode test toggle (tap version 5×) ──
	/** 点击版本号 5 次激活，强制以 allowDowngrades 模式检查更新，便于测试更新 UI */
	const forceShowUpdate = ref(false);
	let tapCount = 0;
	let tapTimer: ReturnType<typeof setTimeout> | null = null;
	function onVersionTap() {
		tapCount++;
		if (tapTimer) clearTimeout(tapTimer);
		tapTimer = setTimeout(() => { tapCount = 0; }, 1500);
		if (tapCount >= 5) {
			forceShowUpdate.value = !forceShowUpdate.value;
			tapCount = 0;
		}
	}

	// ── 贡献者 / Contributors ─────────────────────────────────────────────────
	const contributors = ref<GhContributor[]>([]);
	const contributorsLoading = ref(true);
	const contributorsError = ref(false);
	const licenseName = ref<string | null>(null);

	const GH_API = `https://api.github.com/repos/${owner}/${repo}`;

	async function fetchContributors() {
		contributorsLoading.value = true;
		contributorsError.value = false;
		try {
			const res = await fetch(`${GH_API}/contributors?per_page=20&anon=false`, {
				headers: { Accept: "application/vnd.github+json" },
			});
			if (!res.ok) throw new Error(`${res.status}`);
			contributors.value = await res.json() as GhContributor[];
		} catch {
			contributorsError.value = true;
		} finally {
			contributorsLoading.value = false;
		}
	}

	async function fetchLicense() {
		try {
			const res = await fetch(`${GH_API}/license`, {
				headers: { Accept: "application/vnd.github+json" },
			});
			if (!res.ok) return;
			const data = await res.json() as { license: { spdx_id: string; name: string } };
			licenseName.value = data.license?.spdx_id ?? data.license?.name ?? null;
		} catch {
			// 静默失败
		}
	}

	// ── 版本号 / App version ──────────────────────────────────────────────────
	const appVersion = ref("—");

	// ── 更新操作 / Update actions ─────────────────────────────────────────────

	/**
	 * 检查更新：调用后端 check_for_updates_cmd，
	 * 根据设置中的 check_prerelease（beta 版强制开启）决定检查正式版还是 beta 版。
	 * 强制模式（点版本号 5 次激活）下允许降级检查，便于测试 UI。
	 *
	 * Check for updates via backend check_for_updates_cmd.
	 * Respects check_prerelease setting (forced on for beta builds).
	 */
	async function checkForUpdate() {
		updateState.value = { phase: "checking" };
		changelogExpanded.value = false;
		try {
			const result = await invoke<{ available: boolean; version?: string; date?: number | null; body?: string | null; current_version?: string }>(
				"check_for_updates_cmd",
			);
			if (result.available && result.version) {
				updateState.value = {
					phase: "available",
					version: result.version,
					date: result.date ?? null,
					body: result.body ?? null,
				};
			} else {
				updateState.value = { phase: "none" };
			}
		} catch (e) {
			updateState.value = { phase: "error", message: String(e) };
		}
	}

	/**
	 * 下载并安装更新：调用后端 apply_update_cmd，进度通过 SSE desktop-update-progress 推送。
	 * Download and install: calls apply_update_cmd; progress is pushed via desktop-update-progress SSE.
	 */
	async function startUpdate() {
		if (updateState.value.phase !== "available") return;
		updateState.value = { phase: "downloading", pct: 0, downloaded: 0, total: 0 };
		try {
			await invoke("apply_update_cmd");
			// apply_update_cmd 安装完成后 SSE 会推送 done；此处也保底设置
			updateState.value = { phase: "done" };
			await relaunch();
		} catch (e) {
			updateState.value = { phase: "error", message: String(e) };
		}
	}

	// ── SSE 订阅：desktop-update-progress ────────────────────────────────────
	let unlistenUpdateProgress: (() => void) | null = null;

	onMounted(() => {
		fetchContributors();
		fetchLicense();
		getVersion().then((v) => { appVersion.value = v; }).catch(() => {});
		on("desktop-update-progress", (payload) => {
			const p = payload as { phase: string; pct?: number | null; downloaded?: number; total?: number };
			if (p.phase === "downloading") {
				updateState.value = {
					phase: "downloading",
					pct: p.pct ?? null,
					downloaded: p.downloaded ?? 0,
					total: p.total ?? 0,
				};
			} else if (p.phase === "installing") {
				updateState.value = { phase: "installing" };
			} else if (p.phase === "done") {
				updateState.value = { phase: "done" };
			}
		}).then((fn) => { unlistenUpdateProgress = fn; });
	});

	onUnmounted(() => {
		unlistenUpdateProgress?.();
	});
</script>

<template>
	<div class="flex flex-col">
		<header class="bg-background sticky top-0 z-20 px-6 border-b shrink-0 pt-6 pb-3">
			<h1 class="text-xl font-bold mb-0.5">{{ t("about.title") }}</h1>
			<p class="text-sm text-muted-foreground h-5"></p>
		</header>

		<div class="flex flex-col px-6 py-12 gap-10 max-w-4xl mx-auto w-full">

			<!-- 图标 + 名称 + 版本 -->
			<div class="flex flex-col items-center gap-4">
				<img src="/icon.png" :alt="appName" class="w-24 h-24 rounded-2xl shadow-md" />
				<div class="text-center">
					<h2 class="text-2xl font-bold">{{ appName }}</h2>
					<p class="text-sm text-muted-foreground mt-1 cursor-default select-none"
						@click="onVersionTap">
						{{ t("about.version") }} {{ appVersion }}
					</p>
					<p class="text-xs text-muted-foreground mt-0.5">
						{{ t("about.copyright") }} © {{ currentYear }} ChanTrail
					</p>
					<!-- 测试模式激活时显示 / Shown when force-update test mode is active -->
					<button v-if="forceShowUpdate"
						class="mt-2 text-xs px-2 py-0.5 rounded border border-dashed border-green-500 text-green-600 dark:text-green-400"
						@click="forceShowUpdate = false">
						✓ force update · click to disable
					</button>
				</div>
			</div>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-8 items-start w-full">

			<!-- ── 左列：链接 ──────────────────────────────────────────────── -->
			<div class="flex flex-col gap-2">
				<p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1">
					{{ t("about.links") }}
				</p>

				<a :href="repoUrl" target="_blank" rel="noopener noreferrer"
					class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors">
					<div class="flex items-center gap-2.5">
						<Link class="size-4 shrink-0 text-muted-foreground" />
						<span>{{ t("about.github") }}</span>
					</div>
					<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
				</a>

				<a :href="`${repoUrl}/blob/main/LICENSE`" target="_blank" rel="noopener noreferrer"
					class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors">
					<div class="flex items-center gap-2.5">
						<Scale class="size-4 shrink-0 text-muted-foreground" />
						<span>{{ t("about.license") }}</span>
					</div>
					<span class="text-xs text-muted-foreground">
						{{ licenseName ?? t("about.licenseValue") }}
					</span>
				</a>

				<div class="rounded-lg border px-4 py-3 flex flex-col gap-3">
					<a :href="contributorsUrl" target="_blank" rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 text-sm hover:text-foreground transition-colors">
						<div class="flex items-center gap-2.5">
							<Users class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.contributors") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>
					<div v-if="contributorsLoading" class="flex gap-2 flex-wrap">
						<div v-for="i in 5" :key="i" class="w-8 h-8 rounded-full bg-muted animate-pulse" />
					</div>
					<div v-else-if="contributorsError" class="text-xs text-muted-foreground">
						{{ t("about.contributorsFailed") }}
					</div>
					<div v-else class="flex gap-1.5 flex-wrap">
						<a v-for="c in contributors" :key="c.login"
							:href="c.html_url" target="_blank" rel="noopener noreferrer"
							:title="`${c.login} (${c.contributions})`"
							class="block rounded-full ring-2 ring-transparent hover:ring-ring transition-all">
							<img :src="c.avatar_url" :alt="c.login" class="w-8 h-8 rounded-full" loading="lazy" />
						</a>
					</div>
				</div>

				<a :href="bugsUrl" target="_blank" rel="noopener noreferrer"
					class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors">
					<div class="flex items-center gap-2.5">
						<Bug class="size-4 shrink-0 text-muted-foreground" />
						<span>{{ t("about.reportBug") }}</span>
					</div>
					<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
				</a>
			</div>

			<!-- ── 右列：更新（tauri-plugin-updater 应用内更新）── -->
			<div class="flex flex-col gap-2">
				<p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1">
					{{ t("about.updates") }}
				</p>

				<div class="rounded-lg border overflow-hidden">

					<!-- 检查中 / Checking -->
					<div v-if="updateState.phase === 'checking'"
						class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground">
						<RefreshCw class="size-4 animate-spin shrink-0" />
						<span>{{ t("about.updateChecking") }}</span>
					</div>

					<!-- 出错 / Error -->
					<div v-else-if="updateState.phase === 'error'"
						class="flex items-center justify-between gap-3 px-4 py-3">
						<span class="text-sm text-muted-foreground">{{ t("about.updateFailed") }}</span>
						<Button variant="outline" size="sm" @click="checkForUpdate">
							<RefreshCw class="size-3.5 mr-1.5" />
							{{ t("about.updateRetry") }}
						</Button>
					</div>

					<!-- 已是最新 / Up to date -->
					<div v-else-if="updateState.phase === 'none'"
						class="flex items-center justify-between gap-3 px-4 py-3">
						<span class="text-sm text-muted-foreground">{{ t("about.updateNone") }}</span>
						<Button variant="outline" size="sm" @click="checkForUpdate">
							<RefreshCw class="size-3.5 mr-1.5" />
							{{ t("about.updateCheck") }}
						</Button>
					</div>

					<!-- 有更新 / Update available -->
					<template v-else-if="updateState.phase === 'available'">
						<div class="flex items-center justify-between gap-3 px-4 py-3">
							<div class="flex flex-col gap-0.5">
								<span class="text-sm font-medium text-foreground">
									{{ t("about.updateAvailable", { version: updateState.version }) }}
								</span>
								<span v-if="updateState.date" class="text-xs text-muted-foreground">
									{{ new Date((updateState.date as number) * 1000).toLocaleDateString() }}
								</span>
							</div>
							<div class="flex items-center gap-2 shrink-0">
								<!-- 展开更新日志 / Expand changelog -->
								<Button v-if="updateState.body"
									variant="ghost" size="sm" class="text-muted-foreground"
									@click="changelogExpanded = !changelogExpanded">
									<component :is="changelogExpanded ? ChevronUp : ChevronDown" class="size-3.5 mr-1" />
									{{ t("about.changelog") }}
								</Button>
								<!-- 开始更新 / Start update -->
								<Button
									size="sm"
									class="bg-green-600 hover:bg-green-700 text-white"
									@click="startUpdate">
									<Download class="size-3.5 mr-1.5" />
									{{ t("about.updateStart") }}
								</Button>
							</div>
						</div>
						<!-- 更新日志 / Changelog -->
						<div v-if="changelogExpanded && updateState.body"
							class="border-t px-4 py-3">
							<p class="text-xs text-muted-foreground whitespace-pre-line leading-relaxed">
								{{ updateState.body }}
							</p>
							<a :href="`${repoUrl}/releases`"
								target="_blank" rel="noopener noreferrer"
								class="mt-2 text-xs flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors">
								{{ t("about.viewOnGitHub") }}
								<ExternalLink class="size-3 shrink-0" />
							</a>
						</div>
					</template>

					<!-- 下载中 / Downloading -->
					<template v-else-if="updateState.phase === 'downloading'">
						<div class="px-4 py-3 flex items-center justify-between gap-3">
							<span class="text-sm text-muted-foreground">
								{{ t("about.downloadProgress", { pct: updateState.pct ?? 0 }) }}
							</span>
							<span v-if="updateState.pct !== null" class="text-xs text-muted-foreground tabular-nums shrink-0">
								{{ updateState.pct }}%
							</span>
						</div>
						<div class="px-4 pb-3">
							<div class="w-full h-1.5 bg-muted rounded-full overflow-hidden">
								<div class="h-full bg-green-600 rounded-full transition-all duration-300"
									:class="updateState.pct === null ? 'animate-pulse' : ''"
									:style="{ width: `${updateState.pct ?? 0}%` }" />
							</div>
						</div>
					</template>

					<!-- 安装中 / Installing -->
					<div v-else-if="updateState.phase === 'installing'"
						class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground">
						<Loader2 class="size-4 shrink-0 animate-spin" />
						{{ t("about.installing") }}
					</div>

					<!-- 完成 / Done -->
					<div v-else-if="updateState.phase === 'done'"
						class="flex items-center gap-2 px-4 py-3 text-sm text-green-600 dark:text-green-400">
						<CheckCircle2 class="size-4 shrink-0" />
						{{ t("about.updateDone") }}
					</div>

					<!-- 空闲（初始状态）/ Idle (initial state) -->
					<div v-else class="flex items-center justify-between gap-3 px-4 py-3">
						<span class="text-sm text-muted-foreground">{{ t("about.updateNone") }}</span>
						<Button variant="outline" size="sm" @click="checkForUpdate">
							<RefreshCw class="size-3.5 mr-1.5" />
							{{ t("about.updateCheck") }}
						</Button>
					</div>

					<!-- 错误详情 / Error detail -->
					<div v-if="updateState.phase === 'error'"
						class="border-t px-4 py-3 flex items-start gap-2 text-sm text-destructive">
						<AlertCircle class="size-4 shrink-0 mt-0.5" />
						<span>{{ t("about.updateError", { message: updateState.message }) }}</span>
					</div>
				</div>
			</div>

			</div>
		</div>
	</div>
</template>
