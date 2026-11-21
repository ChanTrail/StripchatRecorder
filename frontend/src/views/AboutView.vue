<!--
	关于页面 / About Page

	更新信息通过后端 /api/update/info 获取（后端再查 GitHub API）。
	贡献者列表仍直接调 GitHub API（公开无需鉴权）。
	Docker 环境下不显示更新按钮，提示更新 Docker 镜像。
	非 Docker 环境下触发后端下载 zip、解压替换 exe、自动重启；进度通过 SSE 推送。
-->
<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed, nextTick } from "vue";
import { useI18n } from "vue-i18n";
import { marked } from "marked";
import {
	Bug,
	ExternalLink,
	Scale,
	Users,
	Link,
	RefreshCw,
	ChevronDown,
	ChevronUp,
	Container,
	Download,
	CheckCircle2,
	AlertCircle,
	Loader2,
	Send,
	Wrench,
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
/** Telegram 频道链接 / Telegram channel link */
const tgUrl = "https://t.me/+HzvDUiJ9ewRlMDk1";
/** FFmpeg 构建项目地址 / FFmpeg builds project URL */
const ffmpegBuildsUrl = "https://github.com/ChanTrail/FFmpeg-Builds-Full";
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
	is_beta: boolean;
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
	tapTimer = setTimeout(() => {
		tapCount = 0;
	}, 1500);
	if (tapCount >= 5) {
		forceShowUpdate.value = !forceShowUpdate.value;
		tapCount = 0;
	}
}

// ── 计算属性 ──────────────────────────────────────────────────────────────

/**
 * 语义化版本比较：latest > current 时返回 true。
 * 剥除 -beta 等预发布后缀后只比较 major.minor.patch 数字部分，从左到右逐段比较。
 */
function semverGt(latest: string, current: string): boolean {
	const parse = (v: string) =>
		v
			.split("-")[0]
			.split(".")
			.map((n) => parseInt(n, 10));
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
	return semverGt(updateInfo.value.release.latest_version, updateInfo.value.current_version);
});

const currentVersion = computed(() => updateInfo.value?.current_version ?? __APP_VERSION__);

/** 拆分版本号的预发布后缀（如 -beta），用于徽章展示 / Splits the prerelease suffix (e.g. -beta) off the version for badge display */
const versionParts = computed(() => {
	const [base, ...rest] = currentVersion.value.split("-");
	return { base, suffix: rest.join("-") };
});

/** 展开的更新日志是否实际可见（驱动加宽布局）/ Whether the expanded changelog is actually shown (drives the widened layout) */
const changelogVisible = computed(
	() => changelogExpanded.value && !!updateInfo.value?.release?.release_notes,
);

/** logo 元素引用，用于展开/收起时的 FLIP 过渡 / logo element ref for the expand/collapse FLIP transition */
const logoEl = ref<HTMLElement | null>(null);
const headerEl = ref<HTMLElement | null>(null);

/**
 * logo 列宽（mount 时量一次）——展开后 logo 脱离流时用 `var(--lw)` 定宽轨
 * 保住更新卡左缘不动 / logo column width measured once on mount — while the logo
 * is out of flow the `var(--lw)` fixed track keeps the card's left edge pinned
 */
const logoColW = ref("14rem");
const scrollerX = ref("0px");

/** 主布局 grid 元素引用，用于展开/收起时 FLIP 平滑上下位移 / main layout grid ref for the FLIP-smoothed vertical move on expand/collapse */
const gridEl = ref<HTMLElement | null>(null);

/**
 * 对元素做 FLIP 位移动画：从旧矩形位置平滑滑向当前位置。
 * FLIP-animates an element from its old rect to the current spot.
 */
function flipTo(el: HTMLElement | null, first: DOMRect | undefined) {
	if (!el || !first) return;
	const last = el.getBoundingClientRect();
	const dx = first.left - last.left;
	const dy = first.top - last.top;
	if (!dx && !dy) return;
	// FLIP：先应用反向位移，再动画归零 / FLIP: apply the inverse offset, then animate back to identity
	el.style.transition = "none";
	el.style.transform = `translate(${dx}px, ${dy}px)`;
	requestAnimationFrame(() => {
		el.style.transition = "transform 0.6s ease";
		el.style.transform = "";
		el.addEventListener(
			"transitionend",
			() => {
				el.style.transition = "";
			},
			{ once: true },
		);
	});
}

/**
 * 切换更新日志展开态：分别记录 logo 与布局 grid 的旧位置，
 * 类切换后对两者 FLIP——logo 两态同位基本不动，grid 把 my-auto↔mb-auto
 * 的垂直瞬移平滑成滑动 / Toggles the changelog: records both the logo's and the
 * layout grid's old rects, then FLIPs each — the logo barely moves (same spot
 * in both states) while the grid's my-auto↔mb-auto vertical snap becomes a
 * smooth slide.
 */
async function toggleChangelog() {
	// 侧栏折叠可能已改变滚动区左偏移，FLIP 测量前刷新 / the sidebar may have been toggled — refresh the scroller offset before measuring rects
	scrollerX.value = `${headerEl.value?.getBoundingClientRect().left ?? 0}px`;
	const firstLogo = logoEl.value?.getBoundingClientRect();
	const firstGrid = gridEl.value?.getBoundingClientRect();
	changelogExpanded.value = !changelogExpanded.value;
	await nextTick();
	flipTo(logoEl.value, firstLogo);
	flipTo(gridEl.value, firstGrid);
}

/** 将 release_notes Markdown 转为 HTML，供 v-html 渲染 */
const changelogHtml = computed(() => {
	const notes = updateInfo.value?.release?.release_notes;
	if (!notes) return null;
	return marked(notes) as string;
});

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
		contributors.value = (await res.json()) as GhContributor[];
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
		const data = (await res.json()) as { license: { spdx_id: string; name: string } };
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
	// 量 logo 实际列宽（定宽轨占位）与滚动容器视口左偏移（侧栏宽）——后者供 logo 的 fixed 定位把 inset 换算回滚动区坐标 / measure the logo's real width (for the fixed track) and the scroller's viewport-left offset (sidebar width) — the latter lets the logo's fixed positioning map its insets back into scroller coordinates
	if (logoEl.value) logoColW.value = `${logoEl.value.offsetWidth}px`;
	scrollerX.value = `${headerEl.value?.getBoundingClientRect().left ?? 0}px`;
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
	<div class="flex flex-col min-h-full">
		<header
			ref="headerEl"
			class="bg-background sticky top-0 z-20 px-6 border-b shrink-0 pt-6 pb-3"
		>
			<h1 class="text-xl font-bold mb-0.5">{{ t("about.title") }}</h1>
			<p class="text-sm text-muted-foreground h-5"></p>
		</header>

		<!-- 容器左缘两态恒定钉在 calc(50%-28rem)（恰等于居中态左缘）——ml/mr-0 无条件生效，收起时不会因 mx-auto 按收缩中宽度居中而左跳；max-width 只驱动右缘。收起时 my-auto 纵向居中；展开时 mb-auto 顶对齐 / The container's left edge is pinned at calc(50%-28rem) in both states (exactly where centering puts it) — ml/mr-0 apply unconditionally so collapsing can't jump left via mx-auto centering a still-shrinking width; max-width drives only the right edge. Collapsed: my-auto centered; expanded: mb-auto top-aligned -->
		<div
			class="flex flex-col px-6 py-12 gap-10 mx-auto w-full md:ml-[max(0rem,calc(50%_-_28rem))] md:mr-0 transition-[max-width] duration-700 ease-in-out"
			:class="
				changelogVisible ? 'mb-auto md:max-w-[calc(50%_+_28rem)]' : 'my-auto max-w-4xl'
			"
		>
			<!-- 左列 logo 浮动块：md:fixed 相对视口锚定——展开滚动长日志时固定在卡片左侧留白内居中（不随滚动）；--sbx=侧栏宽把视口 inset 换算回滚动区坐标（left=24px 内边距，right=(滚动区宽)/2+卡片左缘差）。grid 的兄弟节点而非子项（FLIP transform 会改变后代的定位包含块）；移动端回流式块自然居顶 / Left-column floating logo: md:fixed anchors to the viewport — it stays centered in the card's left margin while scrolling the expanded changelog; --sbx (sidebar width) remaps the viewport insets into scroller coordinates (left = 24px padding, right = scroller-width/2 + card-left offset). A sibling of the grid, not a child (the FLIP transform would change a descendant's containing block); on mobile it stays an in-flow block at top -->
			<div
				ref="logoEl"
				class="flex flex-col items-center gap-4 md:fixed md:left-[calc(var(--sbx)_+_1.5rem)] md:right-[calc((100%_-_var(--sbx))_/_2_+_11rem)] md:top-1/2 md:-translate-y-1/2 md:w-fit md:mx-auto"
				:style="{ '--sbx': scrollerX }"
			>
				<img
					src="/icon.png"
					:alt="appName"
					class="w-24 h-24 rounded-2xl shadow-lg select-none transition-transform duration-300 hover:scale-105"
				/>
				<div class="text-center">
					<h2 class="text-2xl font-bold">{{ appName }}</h2>
					<p
						class="text-sm text-muted-foreground mt-1 cursor-default select-none flex items-center justify-center gap-1.5"
						@click="onVersionTap"
					>
						{{ t("about.version") }} {{ versionParts.base }}
						<span
							v-if="versionParts.suffix"
							class="px-1.5 py-px rounded-md text-[10px] font-semibold uppercase tracking-wide bg-amber-500/15 text-amber-600 dark:text-amber-400 border border-amber-500/30"
							>{{ versionParts.suffix }}</span
						>
					</p>
					<p class="text-xs text-muted-foreground mt-0.5">
						{{ t("about.copyright") }} © {{ currentYear }} ChanTrail
					</p>
					<!-- 测试模式激活时显示 / Shown when test mode is active -->
					<button
						v-if="forceShowUpdate"
						class="mt-2 text-xs px-2 py-0.5 rounded border border-dashed border-green-500 text-green-600 dark:text-green-400"
						@click="forceShowUpdate = false"
					>
						✓ force update · click to disable
					</button>
				</div>
			</div>

			<!-- 布局：左列 var(--lw) 定宽轨占位（logo 浮动出流、位置两态一致）+ 右列卡片轨 minmax(0,52rem)——两态同一轨道列表不瞬切，卡片宽完全随容器 max-width 连续收放，配合左缘钉死实现纯向右/向下生长。grid 元素经 FLIP 平滑上下位移 / Layout: a var(--lw) fixed track holds the left column (logo floats out of flow at the same spot in both states) + a minmax(0,52rem) card track — one track list in both states, no instant switch; the card width scales continuously with the container's max-width, and with the pinned left edge the growth is purely rightward/downward. The grid itself is FLIP-animated for vertical moves -->
			<div
				ref="gridEl"
				class="grid grid-cols-1 gap-x-8 gap-y-4 items-start w-full md:grid-cols-[var(--lw)_minmax(0,52rem)]"
				:style="{ '--lw': logoColW }"
			>
				<!-- ── 右下：链接（左锚定：展开时左缘不动、宽度随 1fr 轨道向右涨到 52rem；收起时轨道本身 40rem 为空操作）── -->
				<div
					class="flex flex-col gap-2 order-3 md:col-start-2 md:row-start-2 md:w-full md:max-w-[52rem] md:justify-self-start"
				>
					<p
						class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1"
					>
						{{ t("about.links") }}
					</p>

					<a
						:href="repoUrl"
						target="_blank"
						rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors"
					>
						<div class="flex items-center gap-2.5">
							<Link class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.github") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>

					<a
						:href="ffmpegBuildsUrl"
						target="_blank"
						rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors"
					>
						<div class="flex items-center gap-2.5">
							<Wrench class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.ffmpegBuilds") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>

					<a
						:href="`${repoUrl}/blob/main/LICENSE`"
						target="_blank"
						rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors"
					>
						<div class="flex items-center gap-2.5">
							<Scale class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.license") }}</span>
						</div>
						<span class="text-xs text-muted-foreground">
							{{ licenseName ?? t("about.licenseValue") }}
						</span>
					</a>

					<div class="rounded-lg border px-4 py-3 flex flex-col gap-3">
						<a
							:href="contributorsUrl"
							target="_blank"
							rel="noopener noreferrer"
							class="flex items-center justify-between gap-3 text-sm hover:text-foreground transition-colors"
						>
							<div class="flex items-center gap-2.5">
								<Users class="size-4 shrink-0 text-muted-foreground" />
								<span>{{ t("about.contributors") }}</span>
							</div>
							<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
						</a>
						<div v-if="contributorsLoading" class="flex gap-2 flex-wrap">
							<div
								v-for="i in 5"
								:key="i"
								class="w-8 h-8 rounded-full bg-muted animate-pulse"
							/>
						</div>
						<div v-else-if="contributorsError" class="text-xs text-muted-foreground">
							{{ t("about.contributorsFailed") }}
						</div>
						<div v-else class="flex gap-1.5 flex-wrap">
							<a
								v-for="c in contributors"
								:key="c.login"
								:href="c.html_url"
								target="_blank"
								rel="noopener noreferrer"
								:title="`${c.login} (${c.contributions})`"
								class="block rounded-full ring-2 ring-transparent hover:ring-ring transition-all"
							>
								<img
									:src="c.avatar_url"
									:alt="c.login"
									class="w-8 h-8 rounded-full"
									loading="lazy"
								/>
							</a>
						</div>
					</div>

					<a
						:href="bugsUrl"
						target="_blank"
						rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors"
					>
						<div class="flex items-center gap-2.5">
							<Bug class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.reportBug") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>

					<a
						:href="tgUrl"
						target="_blank"
						rel="noopener noreferrer"
						class="flex items-center justify-between gap-3 rounded-lg border px-4 py-3 text-sm hover:bg-muted/50 transition-colors"
					>
						<div class="flex items-center gap-2.5">
							<Send class="size-4 shrink-0 text-muted-foreground" />
							<span>{{ t("about.telegram") }}</span>
						</div>
						<ExternalLink class="size-3.5 text-muted-foreground shrink-0" />
					</a>
				</div>

				<!-- ── 右上：更新（左锚定：展开时左缘不动、宽度随 1fr 轨道向右涨到 52rem）── -->
				<div
					class="flex flex-col gap-2 order-2 md:col-start-2 md:row-start-1 md:w-full md:max-w-[52rem] md:justify-self-start"
				>
					<p
						class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1"
					>
						{{ t("about.updates") }}
					</p>

					<div class="rounded-lg border overflow-hidden">
						<!-- 加载中 -->
						<div
							v-if="updateLoading"
							class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground"
						>
							<RefreshCw class="size-4 animate-spin shrink-0" />
							<span>{{ t("about.updateChecking") }}</span>
						</div>

						<!-- 获取失败 -->
						<div
							v-else-if="updateError"
							class="flex items-center justify-between gap-3 px-4 py-3"
						>
							<span class="text-sm text-muted-foreground">{{
								t("about.updateFailed")
							}}</span>
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
											{{
												t("about.updateAvailable", {
													version: updateInfo.release.latest_version,
												})
											}}
										</span>
										<span class="text-xs text-muted-foreground">
											{{
												new Date(
													updateInfo.release.published_at,
												).toLocaleDateString()
											}}
										</span>
									</div>
								</template>
								<template v-else>
									<!-- 强制模式下 release 缺失意味着拉取失败而非无更新 / In force mode a missing release means the fetch failed, not "up to date" -->
									<span class="text-sm text-muted-foreground">{{
										forceShowUpdate
											? t("about.updateFailed")
											: t("about.updateNone")
									}}</span>
								</template>

								<div class="flex items-center gap-2 shrink-0">
									<!-- 展开日志 -->
									<Button
										v-if="hasUpdate && updateInfo.release"
										variant="ghost"
										size="sm"
										class="text-muted-foreground"
										@click="toggleChangelog"
									>
										<component
											:is="changelogExpanded ? ChevronUp : ChevronDown"
											class="size-3.5 mr-1"
										/>
										{{ t("about.changelog") }}
									</Button>

									<!-- 刷新（已最新） -->
									<Button
										v-if="!hasUpdate"
										variant="outline"
										size="sm"
										@click="fetchUpdateInfo"
									>
										<RefreshCw class="size-3.5 mr-1.5" />
										{{ t("about.updateCheck") }}
									</Button>

									<!-- Docker 提示 -->
									<template v-if="updateInfo.is_docker">
										<span
											v-if="hasUpdate"
											class="flex items-center gap-1.5 text-xs text-muted-foreground bg-muted rounded-md px-2.5 py-1.5"
										>
											<Container class="size-3.5 shrink-0" />
											{{ t("about.dockerUpdateHint") }}
										</span>
									</template>

									<!-- 非 Docker 更新操作区 -->
									<template v-else-if="hasUpdate && updateInfo.release">
										<!-- 空闲 -->
										<template v-if="updateProgress.state === 'idle'">
											<Button
												v-if="updateInfo.release.download_url"
												size="sm"
												class="bg-green-600 hover:bg-green-700 text-white"
												@click="startUpdate"
											>
												<Download class="size-3.5 mr-1.5" />
												{{ t("about.updateStart") }}
											</Button>
											<a
												v-else
												:href="updateInfo.release.release_url"
												target="_blank"
												rel="noopener noreferrer"
											>
												<Button variant="outline" size="sm">
													<ExternalLink class="size-3.5 mr-1.5" />
													{{ t("about.viewOnGitHub") }}
												</Button>
											</a>
										</template>

										<!-- 下载中 -->
										<span
											v-else-if="updateProgress.state === 'downloading'"
											class="text-xs text-muted-foreground"
										>
											{{
												t("about.downloadProgress", {
													pct: updateProgress.pct ?? 0,
												})
											}}
										</span>

										<!-- 安装中 -->
										<span
											v-else-if="updateProgress.state === 'installing'"
											class="text-xs text-muted-foreground flex items-center gap-1"
										>
											<Loader2 class="size-3.5 animate-spin" />
											{{ t("about.installing") }}
										</span>

										<!-- 出错：重试 -->
										<Button
											v-else-if="updateProgress.state === 'error'"
											variant="outline"
											size="sm"
											@click="startUpdate"
										>
											<RefreshCw class="size-3.5 mr-1.5" />
											{{ t("about.updateRetry") }}
										</Button>
									</template>
								</div>
							</div>

							<!-- 下载进度条 -->
							<div
								v-if="
									updateProgress.state === 'downloading' && !updateInfo.is_docker
								"
								class="px-4 pb-3"
							>
								<div class="w-full h-1.5 bg-muted rounded-full overflow-hidden">
									<div
										class="h-full bg-green-600 rounded-full transition-all duration-300"
										:class="updateProgress.pct === null ? 'animate-pulse' : ''"
										:style="{ width: `${updateProgress.pct ?? 0}%` }"
									/>
								</div>
								<p class="mt-1 text-xs text-muted-foreground">
									{{
										t("about.downloadProgress", {
											pct: updateProgress.pct ?? 0,
										})
									}}
								</p>
							</div>

							<!-- 安装中提示 -->
							<div
								v-if="updateProgress.state === 'installing'"
								class="border-t px-4 py-3 flex items-center gap-2 text-sm text-muted-foreground"
							>
								<Loader2 class="size-4 shrink-0 animate-spin" />
								{{ t("about.installingHint") }}
							</div>

							<!-- 完成/重启 -->
							<div
								v-if="updateProgress.state === 'done'"
								class="border-t px-4 py-3 flex flex-col gap-1.5"
							>
								<div
									class="flex items-center gap-2 text-sm text-green-600 dark:text-green-400 font-medium"
								>
									<CheckCircle2 class="size-4 shrink-0" />
									{{ t("about.updateDone") }}
								</div>
								<p class="text-xs text-muted-foreground">
									{{ t("about.updateDoneHint") }}
								</p>
							</div>

							<!-- 错误详情 -->
							<div
								v-if="updateProgress.state === 'error'"
								class="border-t px-4 py-3 flex items-start gap-2 text-sm text-destructive"
							>
								<AlertCircle class="size-4 shrink-0 mt-0.5" />
								<span>{{
									t("about.updateError", { message: updateProgress.message })
								}}</span>
							</div>

							<!-- 无 asset 提示 -->
							<div
								v-if="
									hasUpdate &&
									updateInfo.release &&
									!updateInfo.release.download_url &&
									!updateInfo.is_docker
								"
								class="border-t px-4 py-3"
							>
								<p class="text-xs text-muted-foreground">
									{{ t("about.downloadNoAsset") }}
								</p>
								<!-- force 模式下显示 asset 列表，方便排查命名问题 -->
								<div
									v-if="forceShowUpdate && updateInfo.asset_names.length > 0"
									class="mt-2 space-y-0.5"
								>
									<p class="text-xs text-muted-foreground/60">
										platform: {{ updateInfo.platform }}
									</p>
									<p class="text-xs text-muted-foreground/60">assets:</p>
									<p
										v-for="name in updateInfo.asset_names"
										:key="name"
										class="text-xs font-mono text-muted-foreground/60 pl-2"
									>
										{{ name }}
									</p>
								</div>
							</div>

							<!-- 展开的更新日志：grid-rows 0fr→1fr 高度过渡——卡片只向下平滑长高，非瞬间出现 / Expanded changelog: grid-rows 0fr→1fr height transition — the card grows downward smoothly instead of popping in -->
							<div
								class="grid transition-[grid-template-rows] duration-700 ease-in-out"
								:class="changelogVisible ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'"
							>
								<div class="overflow-hidden min-h-0">
									<div
										v-if="updateInfo.release?.release_notes"
										class="border-t px-4 py-3"
									>
										<div
											class="text-xs leading-relaxed overflow-x-hidden [&_h1]:text-sm [&_h1]:font-bold [&_h1]:mt-3 [&_h1]:mb-1.5 [&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-1.5 [&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 [&_p]:my-1.5 [&_p]:text-muted-foreground [&_ul]:my-1.5 [&_ul]:pl-4 [&_ul]:list-disc [&_ol]:my-1.5 [&_ol]:pl-4 [&_ol]:list-decimal [&_li]:my-0.5 [&_li]:text-muted-foreground [&_code]:bg-muted [&_code]:px-1 [&_code]:rounded [&_code]:text-xs [&_code]:font-mono [&_pre]:bg-muted [&_pre]:p-2 [&_pre]:rounded [&_pre]:overflow-x-auto [&_pre]:my-1.5 [&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2 [&_hr]:border-border [&_hr]:my-2 [&_table]:my-1.5 [&_table]:block [&_table]:overflow-x-auto [&_table]:border-spacing-0 [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_th]:font-semibold [&_th]:text-foreground [&_th]:border-b [&_th]:border-border [&_th]:whitespace-nowrap [&_td]:px-2 [&_td]:py-1 [&_td]:align-top [&_td]:text-muted-foreground [&_td_a]:whitespace-nowrap [&_td_code]:whitespace-nowrap [&_img]:max-w-full [&>*:first-child]:mt-0"
											v-html="changelogHtml"
										/>
										<a
											:href="updateInfo.release.release_url"
											target="_blank"
											rel="noopener noreferrer"
											class="mt-2 text-xs flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors"
										>
											{{ t("about.viewOnGitHub") }}
											<ExternalLink class="size-3 shrink-0" />
										</a>
									</div>
								</div>
							</div>
						</template>
					</div>
				</div>
			</div>
			<!-- end grid -->
		</div>
	</div>
</template>
