<script setup lang="ts">
/**
 * StripChat 官方分类浏览页 / StripChat official category browse page
 *
 * 数据源为后端 `GET /api/sc/models`（v1 `/api/front/models` 平铺分页接口，
 * 经应用代理/ECH 通道访问）。一级分类（girls/couples/men/trans）+ 二级
 * 标签（filterGroupTags，均为对线上接口实测有效的 slug）+ 排序，无限滚动
 * 加载（IntersectionObserver 哨兵）。
 *
 * Data comes from backend `GET /api/sc/models` (the v1 `/api/front/models`
 * flat paginated endpoint, fetched through the app's proxy/ECH channel).
 * Top-level categories (girls/couples/men/trans) + second-level tags
 * (filterGroupTags — every slug below was verified live) + sort, with
 * IntersectionObserver-driven infinite scroll.
 */
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Check, Eye, EyeOff, Loader2, Play, Plus, Users, Vibrate } from "@lucide/vue";
import { call } from "@/lib/api";
import { toast } from "@/composables/useNotify";
import { useStreamersStore } from "@/stores/streamers";
import { Button } from "@/components/ui/button";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";

const { t } = useI18n();
const streamersStore = useStreamersStore();

// ─── 类型 / Types ───────────────────────────────────────────────────────────

/** 后端精简后的模型卡片字段 / Slim model-card fields normalized by the backend */
interface ScModel {
	username: string;
	model_id: number;
	status: string;
	viewers: number;
	country: string;
	is_hd: boolean;
	is_new: boolean;
	is_vr: boolean;
	is_mobile: boolean;
	is_non_nude: boolean;
	has_toy: boolean;
	preview: string | null;
	avatar: string | null;
	gender_group: string;
}

interface ScModelsResp {
	models: ScModel[];
	total: number;
	filtered: number;
}

// ─── 常量 / Constants ───────────────────────────────────────────────────────

/** 每页数量（上游 limit 上限 ~80，超限返回空）/ Page size (upstream limit caps ~80; beyond that it returns empty) */
const PAGE_SIZE = 40;

/** 一级分类（SC 官方 primaryTag——实测「男生」为 men，guys 会 400）/
 * Top-level categories (official SC primaryTag — "men" is correct; "guys" 400s) */
const PRIMARY_TAGS = ["girls", "couples", "men", "trans"] as const;

/** 二级标签 slug：外观/特质组（全部对线上接口实测返回非空）/
 * Second-level tag slugs — traits group (all verified non-empty against the live API) */
const TAG_SLUGS = [
	"ageTeen", "ageYoung", "ageMilf", "ageMature",
	"ethnicityAsian", "ethnicityWhite", "ethnicityEbony", "ethnicityLatino",
	"ethnicityIndian", "ethnicityMiddleEastern",
	"bodyTypePetite", "bodyTypeAthletic", "bodyTypeCurvy", "bodyTypeBBW",
	"specificsBigTits", "specificsBigAss", "specificsBigCock",
	"hairColorBlonde", "hairColorBlack", "hairColorRed",
	"subcultureGoth", "subcultureRomantic", "subcultureAnime", "subcultureStudent",
] as const;

/** 二级标签 slug：语言组（tagLanguage 前缀，实测在线数>0 的有效 slug）/
 * Second-level tag slugs — language group (tagLanguage-prefixed; only slugs
 * verified to return nonzero counts against the live API) */
const LANG_SLUGS = [
	"tagLanguageChinese", "tagLanguageJapanese", "tagLanguageKorean",
	"tagLanguageTaiwanese", "tagLanguageVietnamese", "tagLanguageThai",
	"tagLanguageFilipino",
	"tagLanguageHindi", "tagLanguageBengali",
	"tagLanguageUkrainian", "tagLanguageRomanian", "tagLanguageFrench",
	"tagLanguageItalian", "tagLanguageTurkish", "tagLanguageSpanish",
	"tagLanguageHungarian", "tagLanguageDutch", "tagLanguagePolish",
	"tagLanguageCzech", "tagLanguagePortuguese", "tagLanguageGreek",
] as const;

/** 直播状态 → 状态点颜色 / Live status → status-dot color */
function statusColor(status: string): string {
	if (status === "public") return "bg-green-500";
	if (status === "groupShow" || status === "ticketShow") return "bg-amber-500";
	if (status === "private" || status === "p2p" || status === "spy")
		return "bg-violet-500";
	return "bg-muted-foreground";
}

/** 原始 status → i18n key（stripchat.status.*）；未收录值回退原文 /
 * Raw status → i18n key (stripchat.status.*); unlisted values fall back to raw */
const STATUS_KEYS: Record<string, string> = {
	public: "stripchat.status.public",
	private: "stripchat.status.private",
	virtualPrivate: "stripchat.status.virtualPrivate",
	groupShow: "stripchat.status.groupShow",
	ticketShow: "stripchat.status.ticketShow",
	p2p: "stripchat.status.p2p",
	p2pVoice: "stripchat.status.p2pVoice",
	spy: "stripchat.status.spy",
	idle: "stripchat.status.idle",
	off: "stripchat.status.off",
};

function statusLabel(status: string): string {
	const key = STATUS_KEYS[status];
	return key ? t(key) : status || "—";
}

// ─── 状态 / State ───────────────────────────────────────────────────────────

const primaryTag = ref<(typeof PRIMARY_TAGS)[number]>("girls");
const tag = ref<string | null>(null);
const sortBy = ref<"stripRanking" | "trending">("stripRanking");

const models = ref<ScModel[]>([]);
const loading = ref(false);
const loadingMore = ref(false);
const hasMore = ref(true);
const error = ref<string | null>(null);
const sentinel = ref<HTMLElement | null>(null);
const addingSet = ref<Set<string>>(new Set());
let observer: IntersectionObserver | null = null;

// ─── 隐私模式（本页独立键，与主页互不影响）/ Privacy mode (page-specific key, independent from the homepage) ───

type PrivacyMode = "off" | "live" | "all";
const PRIVACY_MODE_KEY = "sc.privacyMode.stripchat";
const privacyMode = ref<PrivacyMode>(
	(() => {
		// 未设置/非法值默认仅直播画面模糊（本页全部是在线卡片，live/all 等效）
		// Unset/invalid defaults to live-only blur (every card here is live, so
		// live/all are equivalent on this page)
		const v = localStorage.getItem(PRIVACY_MODE_KEY);
		return v === "off" || v === "all" ? v : "live";
	})(),
);
watch(privacyMode, (v) => localStorage.setItem(PRIVACY_MODE_KEY, v));

/** 已手动解除模糊的卡片（会话级，切过滤/刷新自然重置）/
 * Cards manually unblurred (session-scoped; resets on filter change) */
const unblurredSet = ref<Set<number>>(new Set());

/** 该卡片当前是否模糊（本页皆在线：live/all 均生效，off 不生效）/
 * Whether this card is blurred (all cards here are live: live/all both apply,
 * off disables) */
function shouldBlur(m: ScModel): boolean {
	return privacyMode.value !== "off" && !unblurredSet.value.has(m.model_id);
}

/** 隐私模式是否作用于本页（off 时连眼睛按钮都不显示）/
 * Whether privacy mode applies on this page (no eye button at all when off) */
const privacyApplies = computed(() => privacyMode.value !== "off");

function toggleBlur(m: ScModel) {
	const next = new Set(unblurredSet.value);
	if (next.has(m.model_id)) next.delete(m.model_id);
	else next.add(m.model_id);
	unblurredSet.value = next;
}

// ─── 缩略图 TLD 竞速（与主页 useFastThumbnail 同思路，但全列表共享一次结果）/
// Thumbnail TLD race (same idea as the homepage useFastThumbnail, but one
// winner shared by the whole list) ───

const CDN_TLDS = ["doppiocdn.net", "doppiocdn.com", "doppiocdn.org", "doppiocdn.live"];
/** 竞速胜出的 thumbs 主机 / Winning thumbs host from the TLD race */
const thumbsTld = ref<string | null>(null);

/** 用首张预览图竞速各 TLD，先加载成功者为赢家（全失败保持原样）/
 * Race all TLDs with the first preview URL — first to load wins
 * (keeps the original host when every TLD fails) */
function raceThumbsTld(sampleUrl: string) {
	if (thumbsTld.value) return;
	CDN_TLDS.forEach((tld) => {
		const img = new Image();
		img.onload = () => {
			if (!thumbsTld.value) thumbsTld.value = tld;
		};
		img.src = sampleUrl.replace(/doppiocdn\.[a-z]+/, `doppiocdn.${tld}`);
	});
}

/** 卡片预览地址：竞速完成后把 thumbs 主机替换成赢家 TLD /
 * Card preview URL: after the race, rewrite the thumbs host to the winner TLD */
function previewSrc(m: ScModel): string | null {
	const p = m.preview ?? m.avatar;
	if (!p) return null;
	if (thumbsTld.value && p.includes("doppiocdn.")) {
		return p.replace(/doppiocdn\.[a-z]+/, `doppiocdn.${thumbsTld.value}`);
	}
	return p;
}

// ─── 缩略图 / Thumbnails ─────────────────────────────────────────────────────

// ─── 加载 / Loading ─────────────────────────────────────────────────────────

async function loadPage(reset: boolean) {
	if (loadingMore.value || loading.value) return;
	if (!reset && !hasMore.value) return;
	if (reset) {
		loading.value = true;
		error.value = null;
	} else {
		loadingMore.value = true;
	}
	const offset = reset ? 0 : models.value.length;
	try {
		// 参数用 camelCase：Tauri invoke 自动转 snake_case 匹配 Rust 参数；
		// web 端由 COMMAND_MAP 显式映射到 snake_case query 参数
		// Args are camelCase: Tauri invoke auto-converts to snake_case for Rust
		// params; the web side maps them explicitly to snake_case query params
		// via COMMAND_MAP
		const res = await call<ScModelsResp>("list_sc_models", {
			primaryTag: primaryTag.value,
			tag: tag.value,
			sortBy: sortBy.value,
			offset,
			limit: PAGE_SIZE,
		});
		const list = res.models ?? [];
		models.value = reset ? list : [...models.value, ...list];
		// 返回不足一页即到底（上游 filteredCount 有封顶，不作依据）
		// Fewer than a page returned = end reached (upstream filteredCount is
		// capped, so it can't be trusted as the stop signal)
		hasMore.value = list.length >= PAGE_SIZE;
		// 首批预览图到位后启动 TLD 竞速（仅首次生效）
		// Kick off the TLD race once the first previews arrive (first call wins)
		const sample = list.find((m) => m.preview?.includes("doppiocdn."))?.preview;
		if (sample) raceThumbsTld(sample);
	} catch (e) {
		error.value = e instanceof Error ? e.message : String(e);
		if (reset) models.value = [];
		hasMore.value = false;
	} finally {
		loading.value = false;
		loadingMore.value = false;
	}
}

/** 过滤条件变化 → 重置并从头加载 / Filter changed → reset and reload from scratch */
watch([primaryTag, tag, sortBy], () => {
	hasMore.value = true;
	loadPage(true);
});

// ─── 添加 / Add ─────────────────────────────────────────────────────────────

function isAdded(username: string): boolean {
	return streamersStore.streamers.some(
		(s) => s.username.toLowerCase() === username.toLowerCase(),
	);
}

async function addToRecord(username: string) {
	if (addingSet.value.has(username)) return;
	addingSet.value.add(username);
	try {
		await streamersStore.addStreamers([username]);
		toast(t("stripchat.addedToast", { username }), "success");
	} catch (e) {
		toast(e instanceof Error ? e.message : String(e), "error");
	} finally {
		addingSet.value.delete(username);
	}
}

// ─── 系统播放器播放转发流 / Play relay stream in the system player ──────────

/** 正在唤起播放器的卡片（防连点重复唤起）/ Cards currently launching a player (prevents double-click duplicates) */
const playingSet = ref<Set<string>>(new Set());

/** 点卡片：启动本地 /stream 转发监听并用系统播放器打开该主播的转发流 /
 * Card click: lazily start the local /stream listener and open the streamer's
 * relay stream in the system player */
async function playRelay(m: ScModel) {
	if (playingSet.value.has(m.username)) return;
	playingSet.value.add(m.username);
	try {
		await call<{ url: string; player: string }>("open_relay_stream", {
			username: m.username,
		});
	} catch (e) {
		const msg = e instanceof Error ? e.message : String(e);
		// 未找到播放器时给安装提示，其余错误原样透出 /
		// show an install hint when no player was found; surface other errors as-is
		toast(
			msg === "no_player"
				? t("stripchat.noPlayer")
				: t("stripchat.playFailed", { message: msg }),
			"error",
		);
	} finally {
		playingSet.value.delete(m.username);
	}
}

// ─── 无限滚动 / Infinite scroll ─────────────────────────────────────────────

onMounted(() => {
	loadPage(true);
	observer = new IntersectionObserver(
		(entries) => {
			if (entries[0]?.isIntersecting) loadPage(false);
		},
		{ rootMargin: "600px" },
	);
	if (sentinel.value) observer.observe(sentinel.value);
});

onUnmounted(() => observer?.disconnect());
</script>

<template>
	<div class="flex h-full min-h-0 flex-col gap-4 overflow-y-auto px-6 pt-6 pb-6">
		<!-- 页头：窄屏（<640px）改纵向——标题行在上、控件逐行全宽在下，
			标题不被挤压；宽屏保持左右布局 / Header: vertical below 640 px —
			title on top, controls stacked full-width below so the title isn't
			squeezed; wide screens keep the side-by-side layout -->
		<header class="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("stripchat.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{ t("stripchat.description") }}
				</p>
			</div>
			<div class="flex flex-col items-stretch gap-2 sm:flex-row sm:items-center">
				<Select v-model="privacyMode">
					<SelectTrigger class="w-full cursor-pointer sm:w-auto sm:shrink-0" :title="t('home.privacy.label')">
						<SelectValue />
					</SelectTrigger>
					<SelectContent>
						<SelectItem value="off" class="cursor-pointer">{{ t("home.privacy.off") }}</SelectItem>
						<SelectItem value="live" class="cursor-pointer">{{ t("home.privacy.live") }}</SelectItem>
						<SelectItem value="all" class="cursor-pointer">{{ t("home.privacy.all") }}</SelectItem>
					</SelectContent>
				</Select>
				<Select v-model="sortBy">
					<SelectTrigger class="w-full cursor-pointer gap-2 sm:w-auto sm:shrink-0">
						<SelectValue />
					</SelectTrigger>
					<SelectContent>
						<SelectItem value="stripRanking" class="cursor-pointer">
							{{ t("stripchat.sort.stripRanking") }}
						</SelectItem>
						<SelectItem value="trending" class="cursor-pointer">
							{{ t("stripchat.sort.trending") }}
						</SelectItem>
					</SelectContent>
				</Select>
			</div>
		</header>

		<!-- 一级分类页签 / Primary-category tabs -->
		<div class="flex gap-1 border-b border-border shrink-0">
			<Button
				v-for="pt in PRIMARY_TAGS"
				:key="pt"
				variant="ghost"
				class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px rounded-none cursor-pointer"
				:class="
					primaryTag === pt
						? 'border-primary text-foreground'
						: 'border-transparent text-muted-foreground hover:text-foreground'
				"
				@click="primaryTag = pt"
			>
				{{ t(`stripchat.tabs.${pt}`) }}
			</Button>
		</div>

		<!-- 二级标签（flex-wrap 换行平铺——桌面端横滚不易用，全部 chip 直接可见可点）
		     Second-level tags (flex-wrap layout — horizontal scrolling is awkward on
		     desktop, so all chips stay directly visible and clickable) -->
		<div class="flex shrink-0 flex-col gap-1.5 pb-1">
			<!-- 特征组 / Traits group -->
			<div class="flex flex-wrap items-center gap-1.5">
				<span class="text-xs text-muted-foreground">{{ t("stripchat.groups.traits") }}</span>
				<button
					class="shrink-0 cursor-pointer rounded-full border px-3 py-1 text-xs transition-colors"
					:class="
						tag === null
							? 'border-primary bg-primary/10 text-primary'
							: 'border-border text-muted-foreground hover:border-primary/50 hover:text-foreground'
					"
					@click="tag = null"
				>
					{{ t("stripchat.tags.all") }}
				</button>
				<button
					v-for="slug in TAG_SLUGS"
					:key="slug"
					class="shrink-0 cursor-pointer rounded-full border px-3 py-1 text-xs transition-colors"
					:class="
						tag === slug
							? 'border-primary bg-primary/10 text-primary'
							: 'border-border text-muted-foreground hover:border-primary/50 hover:text-foreground'
					"
					@click="tag = tag === slug ? null : slug"
				>
					{{ t(`stripchat.tags.${slug}`) }}
				</button>
			</div>
			<!-- 语言组 / Language group -->
			<div class="flex flex-wrap items-center gap-1.5">
				<span class="text-xs text-muted-foreground">{{ t("stripchat.groups.langs") }}</span>
				<button
					v-for="slug in LANG_SLUGS"
					:key="slug"
					class="shrink-0 cursor-pointer rounded-full border px-3 py-1 text-xs transition-colors"
					:class="
						tag === slug
							? 'border-primary bg-primary/10 text-primary'
							: 'border-border text-muted-foreground hover:border-primary/50 hover:text-foreground'
					"
					@click="tag = tag === slug ? null : slug"
				>
					{{ t(`stripchat.tags.${slug}`) }}
				</button>
			</div>
		</div>

		<!-- 错误 / Error -->
		<p v-if="error" class="text-sm text-destructive">
			{{ t("stripchat.error", { message: error }) }}
		</p>

		<!-- 初次加载 / Initial loading -->
		<div
			v-if="loading"
			class="flex flex-1 items-center justify-center text-muted-foreground"
		>
			<Loader2 class="h-6 w-6 animate-spin" />
		</div>

		<!-- 空态 / Empty -->
		<div
			v-else-if="models.length === 0 && !error"
			class="flex flex-1 items-center justify-center text-sm text-muted-foreground"
		>
			{{ t("stripchat.empty") }}
		</div>

		<!-- 网格 / Grid -->
		<div
			v-else
			class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3"
		>
			<div
				v-for="m in models"
				:key="`${m.model_id}-${m.username}`"
				class="group relative cursor-pointer overflow-hidden rounded-lg border bg-card transition-shadow hover:shadow-md"
				:title="t('stripchat.play')"
				@click="playRelay(m)"
			>
				<!-- 预览图：contextmenu 仅在模糊时拦截（防止保存被模糊的原图），
					未模糊时保留正常菜单 / Preview image: context menu is suppressed
					only while blurred (keeps the original from being saved); the
					normal menu still works when visible -->
				<div
					class="relative aspect-video overflow-hidden bg-muted select-none"
					@contextmenu="shouldBlur(m) && $event.preventDefault()"
				>
					<!-- 模糊容器：blur-2xl + scale-110 掩盖羽化边缘 / Blur container: blur-2xl + scale-110 hides feathered edges -->
					<div
						class="absolute inset-0 transition-[filter,transform] duration-300"
						:class="shouldBlur(m) ? 'blur-2xl scale-110' : ''"
					>
						<img
							v-if="previewSrc(m)"
							:src="previewSrc(m)!"
							:alt="m.username"
							loading="lazy"
							class="h-full w-full object-cover [-webkit-touch-callout:none]"
						/>
						<div
							v-else
							class="flex h-full w-full items-center justify-center text-muted-foreground"
						>
							<Users class="h-8 w-8 opacity-40" />
						</div>
					</div>



					<!-- 播放浮层：悬停浮现的中央播放键；pointer-events-none 不拦截
						点击（整卡可点），唤起期间转圈 / Play overlay: a center play
						button fading in on hover; pointer-events-none keeps the whole
						card clickable, spinner shows while the player is launching -->
					<div
						class="pointer-events-none absolute inset-0 z-10 flex items-center justify-center opacity-0 transition-opacity group-hover:opacity-100"
					>
						<span class="flex size-12 items-center justify-center rounded-full bg-black/60 text-white">
							<Loader2
								v-if="playingSet.has(m.username)"
								class="size-6 animate-spin"
							/>
							<Play v-else class="size-6 fill-current" />
						</span>
					</div>

					<!-- 隐私模式眼睛按钮：模糊适用时常驻，点击在模糊/清晰之间切换；
						位于添加按钮左侧（右下角已被占用）/
					     Privacy eye button: pinned while privacy mode applies; click
					     toggles blur on/off; sits left of the add button (bottom-right is
					     taken) -->
					<button
						v-if="privacyApplies"
						type="button"
						class="absolute bottom-2 right-11 z-10 size-7 rounded-md bg-black/55 text-white/90 flex items-center justify-center opacity-100 transition-opacity hover:bg-black/75 cursor-pointer"
						:title="shouldBlur(m) ? t('streamerCard.revealImage') : t('streamerCard.blurImage')"
						@click.stop.prevent="toggleBlur(m)"
					>
						<Eye v-if="shouldBlur(m)" class="size-4" />
						<EyeOff v-else class="size-4" />
					</button>

					<!-- 左上：状态点 + 状态文字 / Top-left: status dot + label -->
					<div
						class="absolute left-2 top-2 flex items-center gap-1.5 rounded bg-black/60 px-1.5 py-0.5 text-[11px] text-white"
					>
						<span
							class="h-1.5 w-1.5 rounded-full"
							:class="statusColor(m.status)"
						/>
						{{ statusLabel(m.status) }}
					</div>

					<!-- 右上：徽标 / Top-right: badges -->
					<div class="absolute right-2 top-2 flex gap-1">
						<span
							v-if="m.has_toy"
							class="flex items-center gap-0.5 rounded bg-pink-600/90 px-1.5 py-0.5 text-[10px] font-semibold text-white"
							:title="t('stripchat.toy')"
						><Vibrate class="h-3 w-3" />TOY</span>
						<span
							v-if="m.is_new"
							class="rounded bg-emerald-600/90 px-1.5 py-0.5 text-[10px] font-semibold text-white"
						>NEW</span>
						<span
							v-if="m.is_hd"
							class="rounded bg-black/60 px-1.5 py-0.5 text-[10px] font-semibold text-white"
						>HD</span>
						<span
							v-if="m.is_vr"
							class="rounded bg-violet-600/90 px-1.5 py-0.5 text-[10px] font-semibold text-white"
						>VR</span>
					</div>

					<!-- 底部信息条 / Bottom info bar -->
					<div
						class="absolute inset-x-0 bottom-0 flex items-end justify-between gap-2 bg-gradient-to-t from-black/80 via-black/40 to-transparent px-2.5 pb-2 pt-8"
					>
						<div class="min-w-0">
							<p class="truncate text-sm font-semibold text-white">
								{{ m.username }}
							</p>
							<p class="flex items-center gap-1 text-[11px] text-white/80">
								<Eye class="h-3 w-3" />
								{{ m.viewers.toLocaleString() }}
								<span v-if="m.country" class="uppercase">
									· {{ m.country }}
								</span>
							</p>
						</div>
						<Button
							v-if="!isAdded(m.username)"
							size="icon"
							class="h-7 w-7 shrink-0 rounded-full"
							:title="t('stripchat.add')"
							:disabled="addingSet.has(m.username)"
							@click.stop="addToRecord(m.username)"
						>
							<Loader2
								v-if="addingSet.has(m.username)"
								class="h-3.5 w-3.5 animate-spin"
							/>
							<Plus v-else class="h-3.5 w-3.5" />
						</Button>
						<span
							v-else
							class="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-emerald-600/80 text-white"
							:title="t('stripchat.added')"
						>
							<Check class="h-3.5 w-3.5" />
						</span>
					</div>
				</div>
			</div>
		</div>

		<!-- 无限滚动哨兵 / Infinite-scroll sentinel -->
		<div ref="sentinel" class="flex h-10 shrink-0 items-center justify-center">
			<Loader2 v-if="loadingMore" class="h-5 w-5 animate-spin text-muted-foreground" />
			<span
				v-else-if="!hasMore && models.length > 0"
				class="text-xs text-muted-foreground"
			>
				{{ t("stripchat.end") }}
			</span>
		</div>
	</div>
</template>
