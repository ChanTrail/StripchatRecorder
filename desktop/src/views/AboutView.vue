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
import { ref, computed, onMounted, onUnmounted, nextTick } from "vue";
import { useI18n } from "vue-i18n";
import { marked } from "marked";
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";
import { on } from "@/lib/api";
import { useMobileLayout } from "@/composables/useMobileLayout";
import {
	Bug,
	ExternalLink,
	Scale,
	Users,
	Link,
	RefreshCw,
	ChevronDown,
	ChevronUp,
	Download,
	CheckCircle2,
	AlertCircle,
	Loader2,
	Send,
	Wrench,
} from "@lucide/vue";
import { Button } from "@/components/ui/button";

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

const hasUpdate = computed(
	() =>
		updateState.value.phase === "available" ||
		updateState.value.phase === "downloading" ||
		updateState.value.phase === "installing" ||
		updateState.value.phase === "done",
);

/** 展开的更新日志是否实际可见（驱动加宽布局；updateState 是判别联合需先收窄）/ Whether the expanded changelog is actually shown (drives the widened layout; narrows the UpdateState union) */
const changelogVisible = computed(
	() =>
		changelogExpanded.value &&
		updateState.value.phase === "available" &&
		!!updateState.value.body,
);

/** logo 元素引用，用于展开/收起时的 FLIP 过渡 / logo element ref for the expand/collapse FLIP transition */
const logoEl = ref<HTMLElement | null>(null);
const headerEl = ref<HTMLElement | null>(null);

/**
 * 视口宽度是否达到 Tailwind `md` 断点（≥768px）。
 * 桌面端 logo 用 `md:fixed` 锚定视口，需要 <Teleport> 脱离祖先 transform
 * （page/layout 过渡、FLIP）对 fixed 定位包含块的劫持——transform 祖先
 * 存在时 fixed 的包含块变成该祖先而非视口，进入动画期间 logo 错位、
 * 过渡结束后瞬间跳回正确位置。移动端 `md:fixed` 本就不生效，logo 保持
 * 流内布局不传送。
 * Whether the viewport is at least Tailwind's `md` breakpoint (≥768px).
 * On desktop the logo is `md:fixed`-anchored to the viewport and needs a
 * <Teleport> to escape ancestor transforms (page/layout transitions,
 * FLIP) hijacking the fixed containing block — while a transformed
 * ancestor exists, the fixed containing block becomes that ancestor
 * instead of the viewport, so the logo sits misplaced during the enter
 * animation and snaps back the moment the transition ends. On mobile
 * `md:fixed` doesn't apply, so the logo stays in flow unteleported.
 */
const desktopMq = window.matchMedia("(min-width: 768px)");
const isDesktopViewport = ref(desktopMq.matches);
function onViewportChange(e: MediaQueryListEvent) {
	isDesktopViewport.value = e.matches;
}

/**
 * 滚动区视口左偏移（=侧栏宽）——`--sbx` 供 logo 的 fixed inset 把视口
 * 坐标换算回滚动区坐标。初值必须首帧即对：移动端 UA 无侧栏（0）；桌面端
 * 与 App.vue 同源读 localStorage["sidebar_collapsed"]，展开 w-44=11rem、
 * 折叠 w-14=3.5rem。旧初值 "0px" 会把 logo 首帧画在偏左 ~130px 处，等
 * onMounted 实测 header.left 修正——修正帧前后的绘制差就表现为"logo
 * 从左侧滑入位置"。ResizeObserver 继续跟踪后续变化（侧栏折叠动画、
 * 窗口缩放、字体晚加载）。
 * Scroller's viewport-left offset (= sidebar width) — `--sbx` lets the
 * logo's fixed insets map viewport coords back to scroller coords. The
 * initial value must be correct on the very first frame: mobile UAs have
 * no sidebar (0); desktop reads localStorage["sidebar_collapsed"] from the
 * same key as App.vue — expanded w-44=11rem, collapsed w-14=3.5rem. The old
 * "0px" start painted the logo ~130px left of final until onMounted measured
 * header.left — the frame delta showed as the logo sliding in from the left.
 * The ResizeObserver keeps tracking later changes (sidebar collapse
 * animation, window resizes, late-loading fonts).
 */
const { isMobile } = useMobileLayout();
const SIDEBAR_COLLAPSED_KEY = "sidebar_collapsed"; // 与 App.vue SIDEBAR_KEY 一致 / same key as App.vue's SIDEBAR_KEY
const scrollerX = ref(
	isMobile.value
		? "0px"
		: localStorage.getItem(SIDEBAR_COLLAPSED_KEY) === "1"
			? "3.5rem"
			: "11rem",
);

/** 跟踪 header/logo 尺寸与位置变化的观察器 / Observer tracking header/logo size & position changes */
let layoutRO: ResizeObserver | null = null;

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

// ── 开发模式测试开关（点版本号 5 次激活）/ Dev-mode test toggle (tap version 5×) ──
/** 点击版本号 5 次激活，强制以 allowDowngrades 模式检查更新，便于测试更新 UI */
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

// ── 版本号 / App version ──────────────────────────────────────────────────
const appVersion = ref("—");

/** 拆分版本号的预发布后缀（如 -beta），用于徽章展示 / Splits the prerelease suffix (e.g. -beta) off the version for badge display */
const versionParts = computed(() => {
	const [base, ...rest] = appVersion.value.split("-");
	return { base, suffix: rest.join("-") };
});

/** 将 changelog body Markdown 转为 HTML，供 v-html 渲染 */
const changelogHtml = computed(() => {
	if (updateState.value.phase !== "available") return null;
	const body = updateState.value.body;
	if (!body) return null;
	return marked(body) as string;
});

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
		const result = await invoke<{
			available: boolean;
			version?: string;
			date?: number | null;
			body?: string | null;
			current_version?: string;
		}>("check_for_updates_cmd", {
			// 强制模式下跳过版本比较，远端有 release 即视为可更新 / In force mode the backend skips version comparison — any remote release counts
			force: forceShowUpdate.value,
		});
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
	desktopMq.addEventListener("change", onViewportChange);
	// 量滚动容器视口左偏移（侧栏宽）——供 logo 的 fixed 定位把 inset 换算回滚动区坐标；ResizeObserver 持续跟踪：侧栏折叠/展开的 200ms 宽度过渡与窗口尺寸变化都会移动 header.left，一次测量不够（--sbx 初值已按侧栏静态布局算对，此处兜底跟踪）/ measure the scroller's viewport-left offset (sidebar width) — lets the logo's fixed positioning map its insets back into scroller coordinates; a ResizeObserver keeps tracking it: the sidebar collapse/expand 200ms width transition and window resizes all move header.left, so one measurement isn't enough (--sbx already starts correct from the sidebar's static layout; this just tracks drift)
	const measureLayout = () => {
		scrollerX.value = `${headerEl.value?.getBoundingClientRect().left ?? 0}px`;
	};
	measureLayout();
	layoutRO = new ResizeObserver(measureLayout);
	if (headerEl.value) layoutRO.observe(headerEl.value);
	fetchContributors();
	fetchLicense();
	getVersion()
		.then((v) => {
			appVersion.value = v;
		})
		.catch(() => {});
	on("desktop-update-progress", (payload) => {
		const p = payload as {
			phase: string;
			pct?: number | null;
			downloaded?: number;
			total?: number;
		};
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
	}).then((fn) => {
		unlistenUpdateProgress = fn;
	});
});

onUnmounted(() => {
	desktopMq.removeEventListener("change", onViewportChange);
	layoutRO?.disconnect();
	unlistenUpdateProgress?.();
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
			:class="changelogVisible ? 'mb-auto md:max-w-[calc(50%_+_28rem)]' : 'my-auto max-w-4xl'"
		>
			<!-- 左列 logo 浮动块：md:fixed 相对视口锚定——展开滚动长日志时固定在卡片左侧留白内居中（不随滚动）；--sbx=侧栏宽把视口 inset 换算回滚动区坐标（left=24px 内边距，right=(滚动区宽)/2+卡片左缘差）。桌面端 <Teleport> 到 <body>：fixed 需要视口作包含块，留在过渡子树里会被 page/layout 过渡与 FLIP 的祖先 transform 劫持成相对该祖先定位，进入动画期间错位、结束后瞬间跳回正确位置；移动端 :disabled 关闭传送留在流内自然居顶。grid 的兄弟节点而非子项 / Left-column floating logo: md:fixed anchors to the viewport — it stays centered in the card's left margin while scrolling the expanded changelog; --sbx (sidebar width) remaps the viewport insets into scroller coordinates (left = 24px padding, right = scroller-width/2 + card-left offset). Teleported to <body> on desktop: fixed needs the viewport as its containing block — staying inside the transitioned subtree lets page/layout transition & FLIP ancestor transforms hijack it into ancestor-relative positioning, misplaced during the enter animation then snapping into place; :disabled keeps it in-flow on mobile. A sibling of the grid, not a child -->
			<Teleport to="body" :disabled="!isDesktopViewport">
				<div
					ref="logoEl"
					class="animate-in fade-in duration-150 flex flex-col items-center gap-4 md:fixed md:left-[calc(var(--sbx)_+_1.5rem)] md:right-[calc((100%_-_var(--sbx))_/_2_+_11rem)] md:top-1/2 md:-translate-y-1/2 md:w-fit md:mx-auto"
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
						<!-- 测试模式激活时显示 / Shown when force-update test mode is active -->
						<button
							v-if="forceShowUpdate"
							class="mt-2 text-xs px-2 py-0.5 rounded border border-dashed border-green-500 text-green-600 dark:text-green-400"
							@click="forceShowUpdate = false"
						>
							✓ force update · click to disable
						</button>
					</div>
				</div>
			</Teleport>

			<!-- 布局：左列 14rem 定宽轨占位（logo md:fixed 浮动出流、位置两态一致；轨道用编译期常量——旧实现把 mount 后实测的 logo 宽写回 --lw，轨道首帧后变化导致右列卡片水平移动）+ 右列卡片轨 minmax(0,52rem)——两态同一轨道列表不瞬切，卡片宽完全随容器 max-width 连续收放，配合左缘钉死实现纯向右/向下生长。grid 元素经 FLIP 平滑上下位移 / Layout: a constant 14rem track holds the left column (the logo floats out of flow via md:fixed at the same spot in both states; the track is a compile-time constant — the old code wrote the measured logo width back to --lw after mount, shifting the card column horizontally in a later frame) + a minmax(0,52rem) card track — one track list in both states, no instant switch; the card width scales continuously with the container's max-width, and with the pinned left edge the growth is purely rightward/downward. The grid itself is FLIP-animated for vertical moves -->
			<div
				ref="gridEl"
				class="grid grid-cols-1 gap-x-8 gap-y-4 items-start w-full md:grid-cols-[14rem_minmax(0,52rem)]"
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
						<!-- 检查中 / Checking -->
						<div
							v-if="updateState.phase === 'checking'"
							class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground"
						>
							<RefreshCw class="size-4 animate-spin shrink-0" />
							<span>{{ t("about.updateChecking") }}</span>
						</div>

						<!-- 出错 / Error -->
						<div
							v-else-if="updateState.phase === 'error'"
							class="flex items-center justify-between gap-3 px-4 py-3"
						>
							<span class="text-sm text-muted-foreground">{{
								t("about.updateFailed")
							}}</span>
							<Button variant="outline" size="sm" @click="checkForUpdate">
								<RefreshCw class="size-3.5 mr-1.5" />
								{{ t("about.updateRetry") }}
							</Button>
						</div>

						<!-- 已是最新 / Up to date -->
						<div
							v-else-if="updateState.phase === 'none'"
							class="flex items-center justify-between gap-3 px-4 py-3"
						>
							<span class="text-sm text-muted-foreground">{{
								t("about.updateNone")
							}}</span>
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
										{{
											t("about.updateAvailable", {
												version: updateState.version,
											})
										}}
									</span>
									<span
										v-if="updateState.date"
										class="text-xs text-muted-foreground"
									>
										{{
											new Date(
												(updateState.date as number) * 1000,
											).toLocaleDateString()
										}}
									</span>
								</div>
								<div class="flex items-center gap-2 shrink-0">
									<!-- 展开更新日志 / Expand changelog -->
									<Button
										v-if="updateState.body"
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
									<!-- 开始更新 / Start update -->
									<Button
										size="sm"
										class="bg-green-600 hover:bg-green-700 text-white"
										@click="startUpdate"
									>
										<Download class="size-3.5 mr-1.5" />
										{{ t("about.updateStart") }}
									</Button>
								</div>
							</div>
							<!-- 更新日志：grid-rows 0fr→1fr 高度过渡——卡片只向下平滑长高，非瞬间出现 / Changelog: grid-rows 0fr→1fr height transition — the card grows downward smoothly instead of popping in -->
							<div
								class="grid transition-[grid-template-rows] duration-700 ease-in-out"
								:class="changelogVisible ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'"
							>
								<div class="overflow-hidden min-h-0">
									<div v-if="updateState.body" class="border-t px-4 py-3">
										<div
											class="text-xs leading-relaxed overflow-x-hidden [&_h1]:text-sm [&_h1]:font-bold [&_h1]:mt-3 [&_h1]:mb-1.5 [&_h2]:text-sm [&_h2]:font-semibold [&_h2]:mt-3 [&_h2]:mb-1.5 [&_h3]:text-xs [&_h3]:font-semibold [&_h3]:mt-2 [&_h3]:mb-1 [&_p]:my-1.5 [&_p]:text-muted-foreground [&_ul]:my-1.5 [&_ul]:pl-4 [&_ul]:list-disc [&_ol]:my-1.5 [&_ol]:pl-4 [&_ol]:list-decimal [&_li]:my-0.5 [&_li]:text-muted-foreground [&_code]:bg-muted [&_code]:px-1 [&_code]:rounded [&_code]:text-xs [&_code]:font-mono [&_pre]:bg-muted [&_pre]:p-2 [&_pre]:rounded [&_pre]:overflow-x-auto [&_pre]:my-1.5 [&_a]:text-primary [&_a]:underline [&_a]:underline-offset-2 [&_hr]:border-border [&_hr]:my-2 [&_table]:my-1.5 [&_table]:block [&_table]:overflow-x-auto [&_table]:border-spacing-0 [&_th]:px-2 [&_th]:py-1 [&_th]:text-left [&_th]:font-semibold [&_th]:text-foreground [&_th]:border-b [&_th]:border-border [&_th]:whitespace-nowrap [&_td]:px-2 [&_td]:py-1 [&_td]:align-top [&_td]:text-muted-foreground [&_td_a]:whitespace-nowrap [&_td_code]:whitespace-nowrap [&_img]:max-w-full [&>*:first-child]:mt-0"
											v-html="changelogHtml"
										/>
										<a
											:href="`${repoUrl}/releases`"
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

						<!-- 下载中 / Downloading -->
						<template v-else-if="updateState.phase === 'downloading'">
							<div class="px-4 py-3 flex items-center justify-between gap-3">
								<span class="text-sm text-muted-foreground">
									{{ t("about.downloadProgress", { pct: updateState.pct ?? 0 }) }}
								</span>
								<span
									v-if="updateState.pct !== null"
									class="text-xs text-muted-foreground tabular-nums shrink-0"
								>
									{{ updateState.pct }}%
								</span>
							</div>
							<div class="px-4 pb-3">
								<div class="w-full h-1.5 bg-muted rounded-full overflow-hidden">
									<div
										class="h-full bg-green-600 rounded-full transition-all duration-300"
										:class="updateState.pct === null ? 'animate-pulse' : ''"
										:style="{ width: `${updateState.pct ?? 0}%` }"
									/>
								</div>
							</div>
						</template>

						<!-- 安装中 / Installing -->
						<div
							v-else-if="updateState.phase === 'installing'"
							class="flex items-center gap-2 px-4 py-3 text-sm text-muted-foreground"
						>
							<Loader2 class="size-4 shrink-0 animate-spin" />
							{{ t("about.installing") }}
						</div>

						<!-- 完成 / Done -->
						<div
							v-else-if="updateState.phase === 'done'"
							class="flex items-center gap-2 px-4 py-3 text-sm text-green-600 dark:text-green-400"
						>
							<CheckCircle2 class="size-4 shrink-0" />
							{{ t("about.updateDone") }}
						</div>

						<!-- 空闲（初始状态）/ Idle (initial state) -->
						<div v-else class="flex items-center justify-between gap-3 px-4 py-3">
							<span class="text-sm text-muted-foreground">{{
								t("about.updateNone")
							}}</span>
							<Button variant="outline" size="sm" @click="checkForUpdate">
								<RefreshCw class="size-3.5 mr-1.5" />
								{{ t("about.updateCheck") }}
							</Button>
						</div>

						<!-- 错误详情 / Error detail -->
						<div
							v-if="updateState.phase === 'error'"
							class="border-t px-4 py-3 flex items-start gap-2 text-sm text-destructive"
						>
							<AlertCircle class="size-4 shrink-0 mt-0.5" />
							<span>{{
								t("about.updateError", { message: updateState.message })
							}}</span>
						</div>
					</div>
				</div>
			</div>
		</div>
	</div>
</template>
