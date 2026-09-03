<!--
    关于页面 / About Page

    运行时从 GitHub API 获取：
    - 最新 release（版本对比 + 更新检查）
    - 贡献者列表（头像 + 链接）
    - 许可证名称

    Runtime data fetched from GitHub API:
    - Latest release (version comparison + update check)
    - Contributors list (avatar + link)
    - License name
-->
<script setup lang="ts">
	import { ref, onMounted, computed } from "vue";
	import { useI18n } from "vue-i18n";
	import { Bug, ExternalLink, Scale, Users, Link, RefreshCw } from "@lucide/vue";
	import { Button } from "@/components/ui/button";

	const { t } = useI18n();

	const appVersion = __APP_VERSION__;
	const appName = "StripchatRecorder";
	const owner = "ChanTrail";
	const repo = "StripchatRecorder";
	const repoUrl = `https://github.com/${owner}/${repo}`;
	const bugsUrl = `${repoUrl}/issues/new`;
	const contributorsUrl = `${repoUrl}/graphs/contributors`;
	const releasesUrl = `${repoUrl}/releases`;
	const currentYear = new Date().getFullYear();

	// ── GitHub API 数据类型 / GitHub API types ────────────────────────────────
	interface GhRelease {
		tag_name: string;
		name: string;
		html_url: string;
		published_at: string;
		body: string;
	}
	interface GhContributor {
		login: string;
		avatar_url: string;
		html_url: string;
		contributions: number;
	}
	interface GhLicense {
		license: { spdx_id: string; name: string };
	}

	// ── 状态 / State ──────────────────────────────────────────────────────────
	const latestRelease = ref<GhRelease | null>(null);
	const releaseLoading = ref(false);
	const releaseError = ref(false);

	const contributors = ref<GhContributor[]>([]);
	const contributorsLoading = ref(true);
	const contributorsError = ref(false);

	const licenseName = ref<string | null>(null);

	// ── 计算：是否有新版本 / Computed: whether a new version is available ─────
	const hasUpdate = computed(() => {
		if (!latestRelease.value) return false;
		const remote = latestRelease.value.tag_name.replace(/^v/, "");
		return remote !== appVersion;
	});

	// ── API 请求 / API calls ──────────────────────────────────────────────────
	const API = `https://api.github.com/repos/${owner}/${repo}`;

	async function fetchRelease() {
		releaseLoading.value = true;
		releaseError.value = false;
		try {
			const res = await fetch(`${API}/releases/latest`, {
				headers: { Accept: "application/vnd.github+json" },
			});
			if (!res.ok) throw new Error(`${res.status}`);
			latestRelease.value = await res.json() as GhRelease;
		} catch {
			releaseError.value = true;
		} finally {
			releaseLoading.value = false;
		}
	}

	async function fetchContributors() {
		contributorsLoading.value = true;
		contributorsError.value = false;
		try {
			const res = await fetch(`${API}/contributors?per_page=20&anon=false`, {
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
			const res = await fetch(`${API}/license`, {
				headers: { Accept: "application/vnd.github+json" },
			});
			if (!res.ok) return;
			const data = await res.json() as GhLicense;
			licenseName.value = data.license?.spdx_id ?? data.license?.name ?? null;
		} catch {
			// 静默失败，使用默认值 / fail silently, fall back to default
		}
	}

	onMounted(() => {
		fetchRelease();
		fetchContributors();
		fetchLicense();
	});
</script>

<template>
	<div class="flex flex-col">
		<!-- sticky 标题区 / Sticky header -->
		<header class="bg-background sticky top-0 z-20 px-6 border-b shrink-0 pt-6 pb-3">
			<h1 class="text-xl font-bold mb-0.5">{{ t("about.title") }}</h1>
			<p class="text-sm text-muted-foreground h-5"></p>
		</header>

		<!-- 主体 / Main body -->
		<div class="flex flex-col items-center px-6 py-12 gap-10 max-w-lg mx-auto w-full">

			<!-- 上部：图标 + 名称 + 版本 / Top: icon + name + version -->
			<div class="flex flex-col items-center gap-4">
				<img src="/icon.png" :alt="appName" class="w-24 h-24 rounded-2xl shadow-md" />
				<div class="text-center">
					<h2 class="text-2xl font-bold">{{ appName }}</h2>
					<p class="text-sm text-muted-foreground mt-1">
						{{ t("about.version") }} {{ appVersion }}
					</p>
					<p class="text-xs text-muted-foreground mt-0.5">
						{{ t("about.copyright") }} © {{ currentYear }} ChanTrail
					</p>
				</div>
			</div>

			<!-- 链接列表 / Links -->
			<div class="w-full flex flex-col gap-2">
				<p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1">
					{{ t("about.links") }}
				</p>

				<!-- GitHub 仓库 / Repository -->
				<a :href="repoUrl" target="_blank" rel="noopener noreferrer"
					class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors">
					<div class="flex items-center gap-2.5">
						<Link class="size-4 shrink-0 text-muted-foreground" />
						<span>{{ t("about.github") }}</span>
					</div>
					<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
				</a>

				<!-- 许可 / License -->
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

				<!-- 贡献者 / Contributors -->
				<div class="rounded-lg border px-4 py-3 flex flex-col gap-3">
					<a :href="contributorsUrl" target="_blank" rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 text-sm hover:text-foreground transition-colors">
						<div class="flex items-center gap-2.5">
							<Users class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.contributors") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>
					<!-- 头像列表 / Avatar list -->
					<div v-if="contributorsLoading" class="flex gap-2 flex-wrap">
						<div v-for="i in 5" :key="i"
							class="w-8 h-8 rounded-full bg-muted animate-pulse" />
					</div>
					<div v-else-if="contributorsError" class="text-xs text-muted-foreground">
						{{ t("about.updateFailed") }}
					</div>
					<div v-else class="flex gap-1.5 flex-wrap">
						<a v-for="c in contributors" :key="c.login"
							:href="c.html_url" target="_blank" rel="noopener noreferrer"
							:title="`${c.login} (${c.contributions})`"
							class="block rounded-full ring-2 ring-transparent hover:ring-ring transition-all"
						>
							<img :src="c.avatar_url" :alt="c.login"
								class="w-8 h-8 rounded-full" loading="lazy" />
						</a>
					</div>
				</div>

				<!-- 反馈 Bug / Report bug -->
				<a :href="bugsUrl" target="_blank" rel="noopener noreferrer"
					class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors">
					<div class="flex items-center gap-2.5">
						<Bug class="size-4 shrink-0 text-muted-foreground" />
						<span>{{ t("about.reportBug") }}</span>
					</div>
					<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
				</a>
			</div>

			<!-- 更新检查 / Update check -->
			<div class="w-full flex flex-col gap-2">
				<p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1">
					{{ t("about.changelog") }}
				</p>
				<div class="rounded-lg border px-4 py-4 flex flex-col gap-3">
					<!-- 加载中 / Loading -->
					<div v-if="releaseLoading" class="flex items-center gap-2 text-sm text-muted-foreground">
						<RefreshCw class="size-4 animate-spin shrink-0" />
						<span>{{ t("about.updateChecking") }}</span>
					</div>

					<!-- 请求失败 / Error -->
					<div v-else-if="releaseError" class="flex items-center justify-between gap-3">
						<span class="text-sm text-muted-foreground">{{ t("about.updateFailed") }}</span>
						<Button variant="outline" size="sm" @click="fetchRelease">
							<RefreshCw class="size-3.5 mr-1.5" />
							{{ t("about.updateCheck") }}
						</Button>
					</div>

					<!-- 有新版本 / Update available -->
					<template v-else-if="latestRelease && hasUpdate">
						<div class="flex items-center justify-between gap-3">
							<div class="flex flex-col gap-0.5">
								<span class="text-sm font-medium">
									{{ t("about.updateAvailable", { version: latestRelease.tag_name }) }}
								</span>
								<span class="text-xs text-muted-foreground">
									{{ new Date(latestRelease.published_at).toLocaleDateString() }}
								</span>
							</div>
							<a :href="latestRelease.html_url" target="_blank" rel="noopener noreferrer">
								<Button variant="default" size="sm">
									{{ t("about.download") }}
									<ExternalLink class="size-3.5 ml-1.5" />
								</Button>
							</a>
						</div>
						<!-- release notes 摘要 / Release notes excerpt -->
						<p v-if="latestRelease.body" class="text-xs text-muted-foreground line-clamp-3 whitespace-pre-line border-t pt-2">
							{{ latestRelease.body }}
						</p>
					</template>

					<!-- 已是最新 / Up to date -->
					<template v-else-if="latestRelease">
						<div class="flex items-center justify-between gap-3">
							<span class="text-sm text-muted-foreground">{{ t("about.updateNone") }}</span>
							<Button variant="outline" size="sm" @click="fetchRelease">
								<RefreshCw class="size-3.5 mr-1.5" />
								{{ t("about.updateCheck") }}
							</Button>
						</div>
					</template>

					<!-- 查看所有版本 / View all releases -->
					<a v-if="latestRelease && !releaseLoading"
						:href="releasesUrl" target="_blank" rel="noopener noreferrer"
						class="text-xs flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors self-start">
						{{ t("about.viewOnGitHub") }}
						<ExternalLink class="size-3 shrink-0" />
					</a>
				</div>
			</div>

		</div>
	</div>
</template>
