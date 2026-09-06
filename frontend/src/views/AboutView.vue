<!--
	关于页面 / About Page

	更新信息通过后端 /api/update/info 获取（后端再查 GitHub API）。
	贡献者列表仍直接调 GitHub API（公开无需鉴权）。
	Docker 环境下不显示更新按钮，提示更新 Docker 镜像。
	非 Docker 环境下触发后端下载 zip、解压替换 exe、自动重启；进度通过 SSE 推送。
-->
<script setup lang="ts">
	import { ref, onMounted, onUnmounted, computed } from "vue";
	import { useI18n } from "vue-i18n";
	import {
		Bug, ExternalLink, Scale, Users, Link, RefreshCw,
		ChevronDown, ChevronUp, Container, Download,
		CheckCircle2, AlertCircle, Loader2,
	} from "@lucide/vue";
	import { Button } from "@/components/ui/button";
	import { call, on } from "@/lib/api";

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

	// ── 后端返回的更新信息类型 ────────────────────────────────────────────────
	interface ReleaseInfo {
		latest_version: string;
		release_url: string;
		release_notes: string;
		published_at: string;
		download_url: string | null;
		download_size: number | null;
	}
	interface UpdateInfo {
		current_version: string;
		platform: string;
		is_docker: boolean;
		release: ReleaseInfo | null;
		asset_names: string[];
	}

	// ── 后端推送的进度事件类型（与 Rust UpdateProgress serde tag 对应）────────
	type UpdateProgressState =
		| { state: "idle" }
		| { state: "downloading"; downloaded: number; total: number; pct: number | null }
		| { state: "installing" }
		| { state: "done" }
		| { state: "error"; message: string };

	// ── 状态 ──────────────────────────────────────────────────────────────────
	const updateInfo = ref<UpdateInfo | null>(null);
	const updateLoading = ref(false);
	const updateError = ref(false);
	const changelogExpanded = ref(false);
	const updateProgress = ref<UpdateProgressState>({ state: "idle" });

	const contributors = ref<GhContributor[]>([]);
	const contributorsLoading = ref(true);
	const contributorsError = ref(false);

	const licenseName = ref<string | null>(null);

	// ── 开发模式测试开关 / Dev-mode test toggle ──────────────────────────────
	/** 点击版本号 5 次激活，强制显示更新 UI 以便测试 */
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

	// ── 计算属性 ──────────────────────────────────────────────────────────────

	/**
	 * 语义化版本比较：latest > current 时返回 true。
	 * 格式 major.minor.patch，逐段比较；解析失败时退回字符串不等值比较。
	 *
	 * Semantic version comparison: returns true when latest > current.
	 * Compares major.minor.patch segments; falls back to string inequality on parse failure.
	 */
	function semverGt(latest: string, current: string): boolean {
		const parse = (v: string) => v.split(".").map((n) => parseInt(n, 10));
		const [la, lb, lc] = parse(latest);
		const [ca, cb, cc] = parse(current);
		if ([la, lb, lc, ca, cb, cc].some(isNaN)) return latest !== current;
		if (la !== ca) return la > ca;
		if (lb !== cb) return lb > cb;
		return lc > cc;
	}

	const hasUpdate = computed(() => {
		if (forceShowUpdate.value) return true;
		if (!updateInfo.value?.release) return false;
		return semverGt(
			updateInfo.value.release.latest_version,
			updateInfo.value.current_version,
		);
	});

	const currentVersion = computed(() =>
		updateInfo.value?.current_version ?? __APP_VERSION__
	);

	// ── API ───────────────────────────────────────────────────────────────────
	const GH_API = `https://api.github.com/repos/${owner}/${repo}`;

	async function fetchUpdateInfo() {
		updateLoading.value = true;
		updateError.value = false;
		changelogExpanded.value = false;
		try {
			updateInfo.value = await call<UpdateInfo>("get_update_info");
			// 同步拉取当前进度（SSE 重连后恢复）
			const status = await call<UpdateProgressState>("get_update_status");
			updateProgress.value = status;
		} catch {
			updateError.value = true;
		} finally {
			updateLoading.value = false;
		}
	}

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

	/** 触发后端下载+安装，进度通过 SSE update-progress 回传 */
	async function startUpdate() {
		const url = updateInfo.value?.release?.download_url;
		if (!url) return;
		// 初始状态 pct=0，进度条从 0 开始
		updateProgress.value = { state: "downloading", downloaded: 0, total: 0, pct: 0 };
		try {
			await call("start_update_download", { download_url: url });
		} catch (e) {
			updateProgress.value = { state: "error", message: String(e) };
		}
	}

	// ── SSE 订阅 ──────────────────────────────────────────────────────────────
	let unlistenProgress: (() => void) | null = null;

	onMounted(async () => {
		fetchUpdateInfo();
		fetchContributors();
		fetchLicense();
		unlistenProgress = await on("update-progress", (payload) => {
			updateProgress.value = payload as UpdateProgressState;
		});
	});

	onUnmounted(() => {
		unlistenProgress?.();
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
						{{ t("about.version") }} {{ currentVersion }}
					</p>
					<p class="text-xs text-muted-foreground mt-0.5">
						{{ t("about.copyright") }} © {{ currentYear }} ChanTrail
					</p>
					<!-- 测试模式激活时显示 / Shown when test mode is active -->
					<button v-if="forceShowUpdate"
						class="mt-2 text-xs px-2 py-0.5 rounded border border-dashed border-green-500 text-green-600 dark:text-green-400"
						@click="forceShowUpdate = false">
						✓ force update · click to disable
					</button>
				</div>
			</div>

			<div class="grid grid-cols-1 md:grid-cols-2 gap-8 items-start w-full">

			<!-- ── 左列：链接 ──────────────────────────────────────────────────── -->
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

			<!-- ── 右列：更新 ──────────────────────────────────────────────────── -->
			<div class="flex flex-col gap-2">
				<p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1">
					{{ t("about.updates") }}
				</p>

				<div class="rounded-lg border overflow-hidden">

					<!-- 加载中 -->
					<div v-if="updateLoading"
						class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground">
						<RefreshCw class="size-4 animate-spin shrink-0" />
						<span>{{ t("about.updateChecking") }}</span>
					</div>

					<!-- 获取失败 -->
					<div v-else-if="updateError"
						class="flex items-center justify-between gap-3 px-4 py-3">
						<span class="text-sm text-muted-foreground">{{ t("about.updateFailed") }}</span>
						<Button variant="outline" size="sm" @click="fetchUpdateInfo">
							<RefreshCw class="size-3.5 mr-1.5" />
							{{ t("about.updateRetry") }}
						</Button>
					</div>

					<!-- 已加载 -->
					<template v-else-if="updateInfo">

						<!-- 头部行 -->
						<div class="flex items-center justify-between gap-3 px-4 py-3">
							<template v-if="hasUpdate && updateInfo.release">
								<div class="flex flex-col gap-0.5">
									<span class="text-sm font-medium text-foreground">
										{{ t("about.updateAvailable", { version: updateInfo.release.latest_version }) }}
									</span>
									<span class="text-xs text-muted-foreground">
										{{ new Date(updateInfo.release.published_at).toLocaleDateString() }}
									</span>
								</div>
							</template>
							<template v-else>
								<span class="text-sm text-muted-foreground">{{ t("about.updateNone") }}</span>
							</template>

							<div class="flex items-center gap-2 shrink-0">
								<!-- 展开日志 -->
								<Button v-if="hasUpdate && updateInfo.release"
									variant="ghost" size="sm" class="text-muted-foreground"
									@click="changelogExpanded = !changelogExpanded">
									<component :is="changelogExpanded ? ChevronUp : ChevronDown" class="size-3.5 mr-1" />
									{{ t("about.changelog") }}
								</Button>

								<!-- 刷新（已最新） -->
								<Button v-if="!hasUpdate" variant="outline" size="sm" @click="fetchUpdateInfo">
									<RefreshCw class="size-3.5 mr-1.5" />
									{{ t("about.updateCheck") }}
								</Button>

								<!-- Docker 提示 -->
								<template v-if="updateInfo.is_docker">
									<span v-if="hasUpdate"
										class="flex items-center gap-1.5 text-xs text-muted-foreground bg-muted rounded-md px-2.5 py-1.5">
										<Container class="size-3.5 shrink-0" />
										{{ t("about.dockerUpdateHint") }}
									</span>
								</template>

								<!-- 非 Docker 更新操作区 -->
								<template v-else-if="hasUpdate && updateInfo.release">
									<!-- 空闲 -->
									<template v-if="updateProgress.state === 'idle'">
										<Button v-if="updateInfo.release.download_url"
											size="sm"
											class="bg-green-600 hover:bg-green-700 text-white"
											@click="startUpdate">
											<Download class="size-3.5 mr-1.5" />
											{{ t("about.updateStart") }}
										</Button>
										<a v-else :href="updateInfo.release.release_url"
											target="_blank" rel="noopener noreferrer">
											<Button variant="outline" size="sm">
												<ExternalLink class="size-3.5 mr-1.5" />
												{{ t("about.viewOnGitHub") }}
											</Button>
										</a>
									</template>

									<!-- 下载中 -->
									<span v-else-if="updateProgress.state === 'downloading'"
										class="text-xs text-muted-foreground">
										{{ t("about.downloadProgress", { pct: updateProgress.pct ?? 0 }) }}
									</span>

									<!-- 安装中 -->
									<span v-else-if="updateProgress.state === 'installing'"
										class="text-xs text-muted-foreground flex items-center gap-1">
										<Loader2 class="size-3.5 animate-spin" />
										{{ t("about.installing") }}
									</span>

									<!-- 出错：重试 -->
									<Button v-else-if="updateProgress.state === 'error'"
										variant="outline" size="sm" @click="startUpdate">
										<RefreshCw class="size-3.5 mr-1.5" />
										{{ t("about.updateRetry") }}
									</Button>
								</template>
							</div>
						</div>

						<!-- 下载进度条 -->
						<div v-if="updateProgress.state === 'downloading' && !updateInfo.is_docker"
							class="px-4 pb-3">
							<div class="w-full h-1.5 bg-muted rounded-full overflow-hidden">
								<div class="h-full bg-green-600 rounded-full transition-all duration-300"
									:class="updateProgress.pct === null ? 'animate-pulse' : ''"
									:style="{ width: `${updateProgress.pct ?? 0}%` }" />
							</div>
							<p class="mt-1 text-xs text-muted-foreground">
								{{ t("about.downloadProgress", { pct: updateProgress.pct ?? 0 }) }}
							</p>
						</div>

						<!-- 安装中提示 -->
						<div v-if="updateProgress.state === 'installing'"
							class="border-t px-4 py-3 flex items-center gap-2 text-sm text-muted-foreground">
							<Loader2 class="size-4 shrink-0 animate-spin" />
							{{ t("about.installingHint") }}
						</div>

						<!-- 完成/重启 -->
						<div v-if="updateProgress.state === 'done'"
							class="border-t px-4 py-3 flex flex-col gap-1.5">
							<div class="flex items-center gap-2 text-sm text-green-600 dark:text-green-400 font-medium">
								<CheckCircle2 class="size-4 shrink-0" />
								{{ t("about.updateDone") }}
							</div>
							<p class="text-xs text-muted-foreground">{{ t("about.updateDoneHint") }}</p>
						</div>

						<!-- 错误详情 -->
						<div v-if="updateProgress.state === 'error'"
							class="border-t px-4 py-3 flex items-start gap-2 text-sm text-destructive">
							<AlertCircle class="size-4 shrink-0 mt-0.5" />
							<span>{{ t("about.updateError", { message: updateProgress.message }) }}</span>
						</div>

						<!-- 无 asset 提示 -->
						<div v-if="hasUpdate && updateInfo.release && !updateInfo.release.download_url && !updateInfo.is_docker"
							class="border-t px-4 py-3">
							<p class="text-xs text-muted-foreground">{{ t("about.downloadNoAsset") }}</p>
							<!-- force 模式下显示 asset 列表，方便排查命名问题 -->
							<div v-if="forceShowUpdate && updateInfo.asset_names.length > 0"
								class="mt-2 space-y-0.5">
								<p class="text-xs text-muted-foreground/60">platform: {{ updateInfo.platform }}</p>
								<p class="text-xs text-muted-foreground/60">assets:</p>
								<p v-for="name in updateInfo.asset_names" :key="name"
									class="text-xs font-mono text-muted-foreground/60 pl-2">{{ name }}</p>
							</div>
						</div>

						<!-- 展开的更新日志 -->
						<div v-if="changelogExpanded && updateInfo.release?.release_notes"
							class="border-t px-4 py-3">
							<p class="text-xs text-muted-foreground whitespace-pre-line leading-relaxed">
								{{ updateInfo.release.release_notes }}
							</p>
							<a :href="updateInfo.release.release_url"
								target="_blank" rel="noopener noreferrer"
								class="mt-2 text-xs flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors">
								{{ t("about.viewOnGitHub") }}
								<ExternalLink class="size-3 shrink-0" />
							</a>
						</div>
					</template>
				</div>
			</div>

			</div> <!-- end grid -->

		</div>
	</div>
</template>
