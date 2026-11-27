<!--
    首次启动配置向导 / First-launch Setup Wizard

    在 setup_done = false 时显示，引导用户完成基础配置：
    Step 1 — 语言选择
    Step 2 — 录制输出目录
    Step 3 — 网络代理（可选）
    Step 4 — FFmpeg 检查（选择来源位置，tools 缺失可触发下载）
    完成后将 setup_done 置为 true 并跳转到主页。

    Shown when setup_done = false. Guides the user through basic configuration:
    Step 1 — Language
    Step 2 — Recording output directory
    Step 3 — Network proxies (optional)
    Step 4 — FFmpeg check (pick the source location; download when tools is incomplete)
    On finish, sets setup_done = true and navigates to the home page.
-->
<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import { useSettingsStore } from "@/stores/settings";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import SchemeUrlInput from "@/components/SchemeUrlInput.vue";
import FFmpegBuildSelect from "@/components/FFmpegBuildSelect.vue";
import { requestFFmpegDownload } from "@/composables/useFFmpegDownload";
import { Switch } from "@/components/ui/switch";
import { loadLocaleFromServer } from "@/i18n";
import { useModuleLocaleStore } from "@/stores/moduleLocale";
import { useLocalesStore } from "@/stores/locales";
import { useDirectoryBrowser } from "@/composables/useDirectoryBrowser";
import { useScrollbar } from "@/composables/useScrollbar";
import { call, on } from "@/lib/api";
import { formatSize } from "@/utils/format";
import { FolderOpen, Check, X, Loader2, RefreshCw, Info, AlertCircle } from "@lucide/vue";

const router = useRouter();
const { t, locale } = useI18n();
const store = useSettingsStore();
const moduleLocaleStore = useModuleLocaleStore();
const localesStore = useLocalesStore();

// ── 步骤控制 / Step control ──────────────────────────────────────────────────
const TOTAL_STEPS = 4;
/** FFmpeg 检查所在的步骤序号 / Step number of the FFmpeg check */
const FFMPEG_STEP = 4;
const step = ref(1);
const saving = ref(false);
const error = ref("");

/** 步骤切换方向，用于决定滑动动画方向 / Step direction for slide animation */
const stepTransition = ref("step-forward");

/** 步骤滚动容器：挂 overlay 滚动条（滚动时短暂显示的细条）/ Step scroll container — wired to the overlay scrollbar (thin bar shown briefly while scrolling) */
const stepScrollEl = ref<HTMLElement | null>(null);
useScrollbar(stepScrollEl);

// ── 表单字段 / Form fields ───────────────────────────────────────────────────
const language = ref("zh-CN");
const outputDir = ref("");
// 代理 / 社区加速站为完整 URL（scheme://地址），由 SchemeUrlInput 拆分协议与地址；
// Stripchat 镜像站的协议单独存储（sc_mirror_scheme），与设置页一致
// Proxies / community mirror hold a full URL (scheme://address), split into scheme and address
// by SchemeUrlInput; the Stripchat mirror stores its scheme separately (sc_mirror_scheme),
// same as the settings page
const apiProxy = ref<string | null>(null);
const scMirror = ref("");
const scMirrorScheme = ref("https");
const cdnProxy = ref<string | null>(null);
const communityProxy = ref<string | null>(null);
const communityMirror = ref<string | null>(null);
const cgfProxy = ref<string | null>(null);
// 浏览页（StripChat 列表）独立的代理/镜像与全局 ECH 开关，与设置页同字段 /
// Browse-page (StripChat list) scoped proxy/mirror and the global ECH switch,
// same fields as the settings page
const scBrowseProxy = ref<string | null>(null);
const scBrowseMirror = ref<string | null>(null);
const echEnabled = ref(false);

// ── FFmpeg 检查 / FFmpeg check ───────────────────────────────────────────────
/** 单个位置的探测结果（与后端 `FFmpegLocationProbe` 一致）/ Probe result for one location (matches backend `FFmpegLocationProbe`) */
interface FFmpegLocationProbe {
	/** 是否找到 ffmpeg / Whether ffmpeg was found */
	ffmpeg: boolean;
	/** 是否找到 ffprobe / Whether ffprobe was found */
	ffprobe: boolean;
	/** 是否找到 ffplay / Whether ffplay was found */
	ffplay: boolean;
	/** ffmpeg 的完整路径 / Full ffmpeg path */
	ffmpeg_path: string | null;
	/** ffprobe 的完整路径 / Full ffprobe path */
	ffprobe_path: string | null;
	/** ffplay 的完整路径 / Full ffplay path */
	ffplay_path: string | null;
	/** ffmpeg -version 的版本号字段 / Version token from `ffmpeg -version` */
	ffmpeg_version: string | null;
	/** ffprobe -version 的版本号字段 / Version token from `ffprobe -version` */
	ffprobe_version: string | null;
	/** ffplay -version 的版本号字段 / Version token from `ffplay -version` */
	ffplay_version: string | null;
}
/** 当前主机可下载的构建选项（与后端 `FFmpegDownloadOptions` 一致）/ Downloadable build choices for this host (matches backend `FFmpegDownloadOptions`) */
interface FFmpegDownloadOptions {
	/** 可选平台轨（首元素 = 宿主默认轨）/ Selectable platform tracks (first = host default) */
	platforms: string[];
	/** 可选变体后缀（`""` = 静态）/ Selectable variant suffixes (`""` = static) */
	variants: string[];
}
/** 两个位置的探测结果汇总（与后端 `FFmpegLocations` 一致）/ Combined probe results (matches backend `FFmpegLocations`) */
interface FFmpegLocations {
	/** tools 目录的绝对路径 / Absolute tools directory path */
	tools_dir: string;
	/** 系统环境变量 / System PATH */
	system: FFmpegLocationProbe;
	/** 程序目录下 tools/ / tools dir next to the binary */
	tools: FFmpegLocationProbe;
	/** 可下载构建选项（不支持下载的平台为 null）/ Downloadable build options (null on unsupported platforms) */
	download_options?: FFmpegDownloadOptions | null;
}
/** 下载构建选择：平台轨（默认取 options 首元素即宿主默认轨）/ Download build pick: platform track (defaults to the first option = host default) */
const downloadPlatform = ref("");
/** 下载构建选择：变体后缀（默认 `""` = 静态）/ Download build pick: variant suffix (defaults to `""` = static) */
const downloadVariant = ref("");
const ffmpegLocations = ref<FFmpegLocations | null>(null);
const ffmpegChecking = ref(false);
const ffmpegCheckFailed = ref(false);
/** 用户选择的来源："0" = 系统 PATH，"1" = tools，"" = 未选择 / User's pick: "0" = system PATH, "1" = tools, "" = unselected */
const ffmpegSelected = ref<"0" | "1" | "">("");
const ffmpegSaving = ref(false);

/** 某位置的三个组件是否齐全 / Whether a location has all three components */
const locationComplete = (p: FFmpegLocationProbe) => p.ffmpeg && p.ffprobe && p.ffplay;
/** 所选位置的探测结果 / Probe result of the selected location */
const ffmpegSelectedLoc = computed<FFmpegLocationProbe | null>(() => {
	if (!ffmpegLocations.value || ffmpegSelected.value === "") return null;
	return ffmpegSelected.value === "0"
		? ffmpegLocations.value.system
		: ffmpegLocations.value.tools;
});
/** 所选位置是否齐全 / Whether the selected location is complete */
const ffmpegSelectedComplete = computed(() =>
	ffmpegSelectedLoc.value ? locationComplete(ffmpegSelectedLoc.value) : false,
);
/** 选中 tools 但组件不齐：按钮变「下载」/ tools selected but incomplete: the finish button morphs into Download */
const ffmpegNeedsDownload = computed(
	() =>
		ffmpegSelected.value === "1" &&
		!!ffmpegLocations.value &&
		!locationComplete(ffmpegLocations.value.tools),
);
/** 取路径的父目录部分（同时兼容 \ 与 / 分隔符）/ Parent-dir part of a path (handles both \ and / separators) */
const dirname = (p: string | null | undefined) => p?.replace(/[\\/][^\\/]*$/, "") ?? null;
/** 系统卡标题旁显示的目录：优先 ffmpeg 所在目录，缺失时依次回退 ffprobe / ffplay
 * Directory shown next to the system card title: ffmpeg's dir first, then ffprobe's, then ffplay's */
const ffmpegSystemDir = computed(() => {
	const s = ffmpegLocations.value?.system;
	if (!s) return null;
	return dirname(s.ffmpeg_path) ?? dirname(s.ffprobe_path) ?? dirname(s.ffplay_path);
});
/** ffmpeg-progress 事件载荷（与后端 `FFmpegProgress` 的 serde 输出一致）/ `ffmpeg-progress` payload (matches backend `FFmpegProgress` serde output) */
interface FFmpegProgressPayload {
	state: "idle" | "downloading" | "installing" | "done" | "error";
	kind?: "install" | "update";
	downloaded?: number;
	total?: number;
	pct?: number | null;
	message?: string;
}
/** 当前 install 类进度（非 null 时步骤内联显示进度条）/ Current install progress (rendered inline in the step while non-null) */
const ffmpegProgress = ref<FFmpegProgressPayload | null>(null);
/** 下载速度文本（相邻事件字节差/时间差）/ Download speed text (byte/time delta between events) */
const ffmpegSpeed = ref("");
/** 速度采样点 / Speed sample point */
let ffmpegLastSample: { bytes: number; at: number } | null = null;
/** 下载/安装进行中 / Whether a download/install is in flight */
const ffmpegInFlight = computed(
	() =>
		ffmpegProgress.value?.state === "downloading" ||
		ffmpegProgress.value?.state === "installing",
);
/** 下载进度条数据：`pct` 驱动宽度；total 未知且有数据时 1/3 宽脉冲 /
 * Download bar data: `pct` drives the width; 1/3-wide pulse while bytes flow with an unknown total */
const ffmpegDownloadBar = computed(() => {
	const p = ffmpegProgress.value;
	const empty = { pct: 0 as number | null, indeterminate: false, size: "", pctText: "" };
	if (!p || p.state !== "downloading") return empty;
	const downloaded = p.downloaded ?? 0;
	const total = p.total ?? 0;
	if (total > 0) {
		const pct = p.pct ?? 0;
		return {
			pct,
			indeterminate: false,
			size: `${formatSize(downloaded)} / ${formatSize(total)}`,
			pctText: `${pct}%`,
		};
	}
	if (downloaded === 0) return empty;
	return { pct: null, indeterminate: true, size: formatSize(downloaded), pctText: "" };
});
let unlistenFFmpegProgress: (() => void) | null = null;

/**
 * 探测两个位置的 ffmpeg 组件并预选：完整位置优先（tools 优先），都不完整
 * 时取组件更多的一侧（平票选 tools——缺失的组件可由下载补齐）。
 * Probe both locations and preselect: a complete location wins (tools
 * preferred); otherwise the side with more components (ties go to tools —
 * missing pieces can be filled by the download).
 */
async function probeFFmpeg() {
	ffmpegChecking.value = true;
	ffmpegCheckFailed.value = false;
	try {
		const res = await call<FFmpegLocations>("get_ffmpeg_locations");
		ffmpegLocations.value = res;
		// 构建选择回填宿主默认轨（用户已手动改过则不动）/ Default the build
		// pick to the host's default track (leave a manual pick untouched)
		if (!downloadPlatform.value && res.download_options?.platforms.length) {
			downloadPlatform.value = res.download_options.platforms[0];
		}
		if (ffmpegSelected.value === "") {
			const count = (p: FFmpegLocationProbe) =>
				[p.ffmpeg, p.ffprobe, p.ffplay].filter(Boolean).length;
			if (locationComplete(res.tools)) ffmpegSelected.value = "1";
			else if (locationComplete(res.system)) ffmpegSelected.value = "0";
			else ffmpegSelected.value = count(res.tools) >= count(res.system) ? "1" : "0";
		}
	} catch {
		ffmpegLocations.value = null;
		ffmpegCheckFailed.value = true;
	} finally {
		ffmpegChecking.value = false;
	}
}

/**
 * 触发后台下载，进度在步骤内联展示（setup 期间全局强制窗口被抑制）。
 * 首选项不落盘——install `done` 事件里才持久化所选位置（见
 * `applyFFmpegProgress`）；手动下载不按 PATH 首选项拦截（明确意图）。
 * Trigger the background download; progress renders inline in the step
 * (the global forced window is suppressed during setup). The preference is
 * NOT persisted here — the install `done` event persists the selected
 * location (see `applyFFmpegProgress`); a manual download is not gated on
 * the PATH preference (explicit intent).
 */
async function startFFmpegDownload() {
	if (ffmpegSaving.value) return;
	ffmpegSaving.value = true;
	error.value = "";
	try {
		await requestFFmpegDownload({
			platform: downloadPlatform.value,
			variant: downloadVariant.value,
		});
	} catch {
		error.value = t("ffmpeg.preferenceWizard.downloadStartFailed");
	} finally {
		ffmpegSaving.value = false;
	}
}

/**
 * 草稿实时持久化：向导字段边填边写（不置 `setup_done`；失败只提示不阻塞）。
 * 步骤 3 的代理在进入 FFmpeg 下载前即落盘，下载请求才能携带代理。
 * Live-draft persistence: wizard fields are written as they're filled
 * (`setup_done` untouched; a failure only shows an error, never blocks).
 * Step-3 proxies land on disk before the FFmpeg download starts, so the
 * download request carries them.
 */
let saveTail: Promise<void> = Promise.resolve();
let draftDirty = false;
let draftRunning = false;

/**
 * 把一次设置写入串到保存队列尾部。job 执行时才展开 `store.settings`，
 * 并发的旧快照不会覆盖已落盘的更新值（如 ffmpeg_preference）。
 * Chain a settings write onto the save queue. The job spreads `store.settings`
 * at run time so a stale in-flight snapshot can't overwrite newer values
 * (e.g. ffmpeg_preference).
 */
function enqueueSave(job: () => Promise<void>): Promise<void> {
	saveTail = saveTail.then(job).catch(() => {});
	return saveTail;
}

async function persistDraft() {
	draftDirty = true;
	if (draftRunning) {
		// 已有草稿保存在排队/执行：标脏即可，排空循环会带上最新值；await
		// 队列保证返回时最新字段已落盘 /
		// A draft save is queued/running: the drain loop picks up the latest
		// values; awaiting the tail guarantees they've landed when we return
		await saveTail;
		return;
	}
	draftRunning = true;
	try {
		await enqueueSave(async () => {
			while (draftDirty) {
				draftDirty = false;
				try {
					await store.saveSettings({
						...store.settings,
						language: language.value,
						// 目录为空时保留已存值（步骤 2 校验放行前可能短暂为空）/
						// Keep the stored dir when the field is empty (can briefly
						// be empty before step-2 validation passes)
						output_dir: outputDir.value.trim() || store.settings.output_dir,
						api_proxy_url: apiProxy.value,
						sc_mirror_url: scMirror.value.trim() || null,
						sc_mirror_scheme: scMirrorScheme.value,
						cdn_proxy_url: cdnProxy.value,
						community_proxy_url: communityProxy.value,
						community_mirror_url: communityMirror.value,
						cgf_proxy_url: cgfProxy.value,
						sc_browse_proxy_url: scBrowseProxy.value,
						sc_browse_mirror_url: scBrowseMirror.value,
						ech_enabled: echEnabled.value,
					});
				} catch {
					error.value = t("settings.saveFailed");
					// 失败即停止排空，等下一次触发 / Stop draining on failure; the next trigger retries
					break;
				}
			}
		});
	} finally {
		draftRunning = false;
	}
}

/**
 * 安装完成后把当前选择的来源写入设置（`ffmpeg_preference` + 联动的
 * `ffmpeg_auto_update`）；写入失败只提示，不影响 finish() 的最终落盘。
 * Persist the selected source after the install lands (`ffmpeg_preference`
 * + the linked `ffmpeg_auto_update`); a failure only shows an error — the
 * final finish() payload still carries the choice.
 */
async function persistFFmpegChoice() {
	if (ffmpegSelected.value === "") return;
	await enqueueSave(async () => {
		try {
			await store.saveSettings({
				...store.settings,
				ffmpeg_preference: Number(ffmpegSelected.value),
				// 自动更新随首选项联动：PATH 关、tools 开 /
				// Auto-update follows the preference: off for PATH, on for tools
				ffmpeg_auto_update: ffmpegSelected.value === "1",
			});
		} catch {
			error.value = t("ffmpeg.preferenceWizard.saveFailed");
		}
	});
}

/**
 * 处理 ffmpeg-progress 事件：只接管 `kind === "install"` 的任务，进度在步骤
 * 内联展示（setup 期间全局强制窗口已抑制）。`done` 时清进度并重新探测。
 * Handle a ffmpeg-progress event: only `kind === "install"` tasks are taken —
 * progress renders inline in the step (the global forced window is suppressed
 * during setup). `done` clears progress and re-probes.
 *
 * @param p - 事件载荷 / Event payload
 */
function applyFFmpegProgress(p: FFmpegProgressPayload) {
	if (p.kind !== "install") return;
	switch (p.state) {
		case "downloading": {
			// 速度 = 相邻事件的字节差/时间差（事件约 200ms 一跳）/
			// Speed = byte/time delta between events (~200 ms apart)
			const bytes = p.downloaded ?? 0;
			const now = Date.now();
			if (ffmpegLastSample && bytes > ffmpegLastSample.bytes) {
				const dt = (now - ffmpegLastSample.at) / 1000;
				if (dt > 0.05)
					ffmpegSpeed.value = `${formatSize((bytes - ffmpegLastSample.bytes) / dt)}/s`;
			}
			ffmpegLastSample = { bytes, at: now };
			ffmpegProgress.value = p;
			break;
		}
		case "installing":
		case "error":
			ffmpegSpeed.value = "";
			ffmpegLastSample = null;
			ffmpegProgress.value = p;
			break;
		case "done":
			ffmpegProgress.value = null;
			ffmpegSpeed.value = "";
			ffmpegLastSample = null;
			// 安装落地后才把所选来源写入配置（失败路径不会留下 tools 首选项）
			// Persist the selected source only now that the install landed —
			// failed paths never leave a tools preference behind
			void persistFFmpegChoice();
			void probeFFmpeg();
			break;
		case "idle":
			ffmpegProgress.value = null;
			break;
	}
}

/**
 * 拉取当前进度状态，恢复进行中的安装任务显示（刷新页面中途恢复用）。
 * Fetch the current progress to restore an in-flight install display (page
 * refresh mid-download).
 */
async function restoreFFmpegStatus() {
	try {
		const cur = await call<FFmpegProgressPayload>("get_ffmpeg_status");
		if (cur.state === "downloading" || cur.state === "installing") {
			applyFFmpegProgress(cur);
		}
	} catch {
		// 状态接口失败忽略：等下一个 SSE 事件 / Ignore: wait for the next SSE event
	}
}

/** 可用语言列表（从共享 store 读取，由 App.vue 统一维护）
 * Available locales (from shared store, maintained by App.vue) */
// 直接使用 store 的 ref，保持响应式；不能赋给普通变量（会丢失响应式）
// Use the store's ref directly to keep reactivity; assigning to a plain variable breaks it

onMounted(async () => {
	// App.vue 可能还未完成 onMounted（例如首次配置路由直接到 /setup），
	// 确保语言列表已加载；若已加载则 refresh() 内部会快速返回已有结果。
	// App.vue may not have finished its onMounted yet (e.g. first-launch routing to /setup).
	// Ensure the locale list is loaded; if already loaded, refresh() returns quickly.
	if (!localesStore.loaded) {
		await localesStore.refresh();
	}
	await store.fetchSettings();
	const s = store.settings;
	language.value = s.language || "zh-CN";
	locale.value = language.value;
	outputDir.value = s.output_dir || "";
	apiProxy.value = s.api_proxy_url || null;
	scMirror.value = s.sc_mirror_url || "";
	scMirrorScheme.value = s.sc_mirror_scheme || "https";
	cdnProxy.value = s.cdn_proxy_url || null;
	communityProxy.value = s.community_proxy_url || null;
	communityMirror.value = s.community_mirror_url || null;
	cgfProxy.value = s.cgf_proxy_url || null;
	scBrowseProxy.value = s.sc_browse_proxy_url || null;
	scBrowseMirror.value = s.sc_browse_mirror_url || null;
	echEnabled.value = s.ech_enabled;
	// FFmpeg 探测（供 Step 4 卡片）；订阅 install 类进度事件：downloading/
	// installing 内联显示进度条，done 重新探测使 tools 卡片变绿 /
	// Probe FFmpeg locations (for the Step 4 cards); subscribe to install-kind
	// progress events: downloading/installing render an inline bar, `done`
	// re-probes so the tools card turns complete
	void probeFFmpeg();
	void restoreFFmpegStatus();
	unlistenFFmpegProgress = await on("ffmpeg-progress", (payload) =>
		applyFFmpegProgress(payload as FFmpegProgressPayload),
	);
});

onUnmounted(() => {
	unlistenFFmpegProgress?.();
});

// ── 目录浏览器 / Directory browser ───────────────────────────────────────────
const { open: openDirectoryBrowser } = useDirectoryBrowser();
function browseOutputDir() {
	openDirectoryBrowser(outputDir.value, (picked) => {
		outputDir.value = picked;
		// 目录选定即落盘 / Persist right after picking
		void persistDraft();
	});
}

// ── 语言切换 / Language switch ───────────────────────────────────────────────
async function setLanguage(lang: string) {
	language.value = lang;
	// 先加载消息，再切换 locale，避免 vue-i18n 在消息就绪前以 fallback 语言渲染
	// Load messages first, then switch locale to avoid vue-i18n rendering with fallback
	const { modules: moduleLocales, warning } = await loadLocaleFromServer(lang);
	locale.value = lang;
	localStorage.setItem("locale", lang);
	moduleLocaleStore.setLocales(lang, moduleLocales);
	if (warning) {
		// SetupView 没有 toast，用 error ref 展示
		error.value = t("settings.localeFileInvalid", { file: `${lang}.json` }) + ": " + warning;
	}
	// 语言选择立即落盘 / Persist the language choice immediately
	void persistDraft();
}

// 选择来源即持久化——但仅限组件齐全的位置：tools 缺件时等安装落地
// （persistFFmpegChoice 在 done 分支负责），失败路径不留 tools 首选项。
// Persist the source choice on select — only when the location is complete:
// incomplete tools waits for the install to land (persistFFmpegChoice runs on
// `done`), so failed paths never leave a tools preference behind.
watch(ffmpegSelected, (v) => {
	if (v !== "" && ffmpegSelectedComplete.value) void persistFFmpegChoice();
});

// ── 步骤校验 / Step validation ───────────────────────────────────────────────
const canNext = computed(() => {
	if (step.value === 2) return outputDir.value.trim().length > 0;
	// FFmpeg 步骤：必须已选择且所选位置组件齐全（tools 不完整时走下载按钮）；
	// 探测本身彻底失败时放行——首选项留空，进入主页后由首选项向导兜底
	// FFmpeg step: a complete selected location is required (incomplete tools goes
	// through the download button); a total probe failure lets the user through —
	// the preference stays unset and the preference wizard picks it up after setup
	if (step.value === FFMPEG_STEP) {
		if (ffmpegCheckFailed.value && !ffmpegLocations.value) return true;
		return ffmpegSelected.value !== "" && ffmpegSelectedComplete.value;
	}
	return true;
});

async function next() {
	error.value = "";
	if (step.value < TOTAL_STEPS) {
		// 先落盘当前步字段再前进（步骤 3 的代理落盘后 FFmpeg 下载才能携带）/
		// Persist the current step's fields before advancing (step-3 proxies must
		// be on disk before the FFmpeg download can carry them)
		await persistDraft();
		stepTransition.value = "step-forward";
		step.value++;
		// 进入 FFmpeg 步骤时刷新探测（用户可能刚装好组件）/ Refresh the probe
		// when entering the FFmpeg step (components may have been installed since)
		if (step.value === FFMPEG_STEP) void probeFFmpeg();
	}
}

function back() {
	error.value = "";
	if (step.value > 1) {
		// 回退同样实时落盘，保住本步的修改 / Persist on stepping back too, keeping this step's edits
		void persistDraft();
		stepTransition.value = "step-back";
		step.value--;
	}
}

// ── 完成向导 / Finish wizard ─────────────────────────────────────────────────
async function finish() {
	if (!outputDir.value.trim()) {
		error.value = t("setup.outputDirRequired");
		stepTransition.value = "step-back";
		step.value = 2;
		return;
	}
	saving.value = true;
	error.value = "";
	try {
		// 走串行队列：否则飞行中的草稿快照会后到并覆盖
		// setup_done/ffmpeg_preference /
		// Through the serial queue — otherwise an in-flight draft snapshot can
		// land later and overwrite setup_done/ffmpeg_preference
		let saveErr: unknown = null;
		await enqueueSave(async () => {
			try {
				await store.saveSettings({
					...store.settings,
					language: language.value,
					output_dir: outputDir.value.trim(),
					api_proxy_url: apiProxy.value,
					sc_mirror_url: scMirror.value.trim() || null,
					sc_mirror_scheme: scMirrorScheme.value,
					cdn_proxy_url: cdnProxy.value,
					community_proxy_url: communityProxy.value,
					community_mirror_url: communityMirror.value,
					cgf_proxy_url: cgfProxy.value,
					sc_browse_proxy_url: scBrowseProxy.value,
					sc_browse_mirror_url: scBrowseMirror.value,
					ech_enabled: echEnabled.value,
					// FFmpeg 来源选择（Step 4）；自动更新随首选项联动：PATH 关、tools 开 /
					// FFmpeg source choice (Step 4); auto-update follows it: off for PATH, on for tools
					ffmpeg_preference:
						ffmpegSelected.value === "" ? null : Number(ffmpegSelected.value),
					ffmpeg_auto_update: ffmpegSelected.value === "1",
					setup_done: true,
				});
			} catch (e) {
				saveErr = e;
			}
		});
		if (saveErr) throw saveErr;
		await router.replace("/");
	} catch (e: unknown) {
		error.value = String(e);
	} finally {
		saving.value = false;
	}
}
</script>

<template>
	<!-- 页面锁定视口高并铺满窗口：步骤区内部滚动，错误提示与导航按钮常驻底部，
		超长步骤不再把按钮顶出视口 / Page locked to the viewport and filling the
		window: the step area scrolls internally while the error line and nav
		buttons stay pinned at the bottom — tall steps can no longer push them
		off-screen -->
	<div data-setup-shell class="flex h-dvh overflow-hidden bg-background p-6">
		<div
			class="flex h-full min-h-0 w-full min-w-0 flex-col gap-10 lg:flex-row lg:gap-14"
		>
			<!-- 左栏：品牌、标题与纵向步骤指示 / Left column: brand, title and vertical step indicator -->
			<div class="flex shrink-0 flex-col gap-8 lg:w-60">
				<div class="flex flex-col gap-1.5">
					<div class="flex items-center gap-2.5">
						<img src="/icon.png" alt="icon" class="w-6 h-6 shrink-0" />
						<span class="text-lg font-bold">StripchatRecorder</span>
					</div>
					<h1 class="text-2xl font-bold mt-1">{{ t("setup.title") }}</h1>
					<p class="text-sm text-muted-foreground">{{ t("setup.subtitle") }}</p>
				</div>

				<!-- 纵向步骤指示器：圆点 + 步骤名 + 连接线 / Vertical step indicator: dot + step name + connector -->
				<div class="flex flex-col">
					<template v-for="i in TOTAL_STEPS" :key="i">
						<div class="flex items-center gap-3">
							<div
								class="flex items-center justify-center w-7 h-7 rounded-full text-xs font-semibold transition-colors duration-200 shrink-0"
								:class="
									i < step
										? 'bg-primary text-primary-foreground'
										: i === step
											? 'bg-primary text-primary-foreground ring-2 ring-primary/30'
											: 'bg-muted text-muted-foreground'
								"
							>
								<svg
									v-if="i < step"
									class="w-3.5 h-3.5"
									viewBox="0 0 24 24"
									fill="none"
									stroke="currentColor"
									stroke-width="3"
								>
									<polyline points="20 6 9 17 4 12" />
								</svg>
								<span v-else>{{ i }}</span>
							</div>
							<span
								class="text-sm transition-colors duration-200"
								:class="
									i === step
										? 'font-medium text-foreground'
										: i < step
											? 'text-foreground/70'
											: 'text-muted-foreground'
								"
							>
								{{ t(`setup.step${i}.title`) }}
							</span>
						</div>
						<div
							v-if="i < TOTAL_STEPS"
							class="w-px h-6 ml-[13.5px] transition-colors duration-200"
							:class="i < step ? 'bg-primary' : 'bg-border'"
						/>
					</template>
				</div>
			</div>

			<!-- 右栏：步骤内容、错误提示与导航按钮 / Right column: step content, error and navigation -->
			<div class="flex-1 flex flex-col gap-6 min-w-0 min-h-0">
				<!-- 步骤内容（带方向感知的滑动过渡）：高度被外层卡片固定，内容
				超高时内部纵向滚动，导航按钮始终可见；scrollbar-gutter 预留
				滚动条槽位避免切换步骤时横向跳动；x 向隐藏防止滑动过渡露边 /
				Step content (directional slide transition): bounded by the
				card's fixed height so over-tall content scrolls internally and
				nav buttons stay visible; scrollbar-gutter reserves the scrollbar
				lane to avoid width jitter between steps; x is hidden so the
				slide transition never bleeds -->
				<div
					ref="stepScrollEl"
					class="min-h-0 flex-1 overflow-y-auto overflow-x-hidden [scrollbar-gutter:stable] scrollbar-overlay"
				>
					<Transition :name="stepTransition" mode="out-in">
						<div :key="step" class="flex flex-col gap-6">
							<!-- Step 1: 语言 / Language -->
							<template v-if="step === 1">
								<div class="flex flex-col gap-1.5">
									<h2 class="text-base font-semibold">
										{{ t("setup.step1.title") }}
									</h2>
									<p class="text-sm text-muted-foreground">
										{{ t("setup.step1.desc") }}
									</p>
								</div>
								<RadioGroup
									:model-value="language"
									class="flex flex-col gap-3"
									@update:model-value="(v) => v && setLanguage(String(v))"
								>
									<label
										v-for="loc in localesStore.locales"
										:key="loc.code"
										class="flex items-center gap-3 rounded-lg border p-4 cursor-pointer transition-colors"
										:class="
											language === loc.code
												? 'border-primary bg-primary/5'
												: 'border-border hover:border-primary/50'
										"
									>
										<RadioGroupItem :value="loc.code" />
										<span class="font-medium">{{ loc.name }}</span>
									</label>
								</RadioGroup>
							</template>

							<!-- Step 2: 输出目录 / Output directory -->
							<template v-if="step === 2">
								<div class="flex flex-col gap-1.5">
									<h2 class="text-base font-semibold">
										{{ t("setup.step2.title") }}
									</h2>
									<p class="text-sm text-muted-foreground">
										{{ t("setup.step2.desc") }}
									</p>
								</div>
								<div class="flex flex-col gap-2">
									<Label>{{ t("settings.outputDir.label") }}</Label>
									<div class="flex items-center gap-1.5">
										<Input
											v-model="outputDir"
											:placeholder="t('setup.step2.placeholder')"
											autocomplete="off"
											autofocus
											class="flex-1"
											@keyup.enter="persistDraft"
											@blur="persistDraft"
										/>
										<Button
											type="button"
											variant="outline"
											size="icon"
											:title="t('settings.outputDir.pick')"
											@click="browseOutputDir"
										>
											<FolderOpen class="size-4" />
										</Button>
									</div>
									<p class="text-xs text-muted-foreground">
										{{ t("setup.step2.hint") }}
									</p>
								</div>
							</template>

							<!-- Step 3: 网络代理（可选）/ Network proxies (optional) -->
							<template v-if="step === 3">
								<div class="flex flex-col gap-1.5">
									<h2 class="text-base font-semibold">
										{{ t("setup.step3.title") }}
									</h2>
									<p class="text-sm text-muted-foreground">
										{{ t("setup.step3.desc") }}
									</p>
								</div>
								<div class="flex flex-col gap-4">
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.apiProxy.label") }}</Label>
										<SchemeUrlInput
											v-model="apiProxy"
											kind="proxy"
											:placeholder="t('settings.apiProxy.placeholder')"
											@commit="persistDraft"
										/>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.scMirror.label") }}</Label>
										<div class="flex items-center gap-1.5">
											<Select
												:model-value="scMirrorScheme"
												@update:model-value="
													(v) => {
														if (v) {
															scMirrorScheme = String(v);
															void persistDraft();
														}
													}
												"
											>
												<SelectTrigger class="w-28 shrink-0"
													><SelectValue
												/></SelectTrigger>
												<SelectContent>
													<SelectItem value="https">https</SelectItem>
													<SelectItem value="http">http</SelectItem>
												</SelectContent>
											</Select>
											<Input
												v-model="scMirror"
												:placeholder="t('settings.scMirror.placeholder')"
												autocomplete="off"
												class="flex-1"
												@keyup.enter="persistDraft"
												@blur="persistDraft"
											/>
										</div>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.scBrowseProxy.label") }}</Label>
										<SchemeUrlInput
											v-model="scBrowseProxy"
											kind="proxy"
											:placeholder="t('settings.scBrowseProxy.placeholder')"
											@commit="persistDraft"
										/>
										<p class="text-xs text-muted-foreground">
											{{ t("settings.scBrowseProxy.hint") }}
										</p>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.scBrowseMirror.label") }}</Label>
										<SchemeUrlInput
											v-model="scBrowseMirror"
											kind="mirror"
											:placeholder="t('settings.scBrowseMirror.placeholder')"
											@commit="persistDraft"
										/>
										<p class="text-xs text-muted-foreground">
											{{ t("settings.scBrowseMirror.hint") }}
										</p>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.cdnProxy.label") }}</Label>
										<SchemeUrlInput
											v-model="cdnProxy"
											kind="proxy"
											:placeholder="t('settings.cdnProxy.placeholder')"
											@commit="persistDraft"
										/>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.communityProxy.label") }}</Label>
										<SchemeUrlInput
											v-model="communityProxy"
											kind="proxy"
											:placeholder="t('settings.communityProxy.placeholder')"
											@commit="persistDraft"
										/>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.communityMirror.label") }}</Label>
										<SchemeUrlInput
											v-model="communityMirror"
											kind="mirror"
											:placeholder="t('settings.communityMirror.placeholder')"
											@commit="persistDraft"
										/>
										<p class="text-xs text-muted-foreground">
											{{ t("settings.communityMirror.hint") }}
										</p>
									</div>
									<div class="flex flex-col gap-1.5">
										<Label>{{ t("settings.cgfProxy.label") }}</Label>
										<SchemeUrlInput
											v-model="cgfProxy"
											kind="proxy"
											:placeholder="t('settings.cgfProxy.placeholder')"
											@commit="persistDraft"
										/>
										<p class="text-xs text-muted-foreground">
											{{ t("settings.cgfProxy.hint") }}
										</p>
									</div>
									<!-- ECH 全局开关：立即落盘，浏览页/录制的 SC API 请求统一生效 /
									Global ECH switch: persisted immediately, applies to both browse-page
									and recording SC API requests -->
									<div class="flex items-center justify-between gap-4">
										<div class="flex min-w-0 flex-col gap-0.5">
											<Label for="setup-ech-enabled" class="cursor-pointer">{{
												t("settings.echEnabled.label")
											}}</Label>
											<p class="text-xs text-muted-foreground">
												{{ t("settings.echEnabled.hint") }}
											</p>
										</div>
										<Switch
											id="setup-ech-enabled"
											class="shrink-0"
											:model-value="echEnabled"
											@update:model-value="
												(v) => {
													echEnabled = !!v;
													void persistDraft();
												}
											"
										/>
									</div>
								</div>
							</template>

							<!-- Step 4: FFmpeg 检查（与首选项向导同源逻辑：探测两个位置、选择来源、
							tools 不齐可触发下载）/ FFmpeg check (same logic as the preference
							wizard: probe both locations, pick a source, download when tools
							is incomplete) -->
							<template v-if="step === 4">
								<div class="flex flex-col gap-1.5">
									<h2 class="text-base font-semibold">
										{{ t("setup.step4.title") }}
									</h2>
									<p class="text-sm text-muted-foreground">
										{{ t("setup.step4.desc") }}
									</p>
								</div>

								<!-- 探测区：三态（检测中/失败/结果）同高占位，切换不跳变 / Probe area:
								the three states share a reserved height so switching never jumps -->
								<div class="flex min-h-[230px] flex-col justify-center">
									<div
										v-if="ffmpegChecking && !ffmpegLocations"
										class="flex items-center justify-center gap-2 py-4 text-sm text-muted-foreground"
									>
										<Loader2 class="size-4 animate-spin" />
										{{ t("ffmpeg.preferenceWizard.checking") }}
									</div>
									<div
										v-else-if="ffmpegCheckFailed && !ffmpegLocations"
										class="flex flex-col items-center gap-3 py-4"
									>
										<p class="text-sm text-destructive">
											{{ t("ffmpeg.preferenceWizard.checkFailed") }}
										</p>
										<Button variant="ghost" size="sm" @click="probeFFmpeg">
											<RefreshCw class="size-3.5" />
											{{ t("ffmpeg.preferenceWizard.retry") }}
										</Button>
									</div>
									<div
										v-else-if="ffmpegLocations"
										class="flex flex-col gap-2"
										role="radiogroup"
										:aria-label="t('setup.step4.title')"
									>
										<!-- 系统环境变量 / System PATH -->
										<div
											role="radio"
											tabindex="0"
											:aria-checked="ffmpegSelected === '0'"
											class="cursor-pointer rounded-lg border px-3 py-2.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60"
											:class="
												ffmpegSelected === '0'
													? 'border-primary ring-1 ring-primary'
													: 'hover:border-muted-foreground/50'
											"
											@click="ffmpegSelected = '0'"
											@keydown.enter.prevent="ffmpegSelected = '0'"
											@keydown.space.prevent="ffmpegSelected = '0'"
										>
											<p class="min-w-0 text-sm font-medium">
												{{ t("ffmpeg.preferenceWizard.systemPath") }}
												<span
													v-if="ffmpegSystemDir"
													class="ml-1 break-all text-xs font-normal text-muted-foreground"
													>（{{ ffmpegSystemDir }}）</span
												>
											</p>
											<div
												class="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffmpeg</span>
												<span
													v-if="ffmpegLocations.system.ffmpeg_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.system.ffmpeg_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.system.ffmpeg"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffprobe</span>
												<span
													v-if="ffmpegLocations.system.ffprobe_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.system.ffprobe_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.system.ffprobe"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffplay</span>
												<span
													v-if="ffmpegLocations.system.ffplay_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.system.ffplay_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.system.ffplay"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1.5 flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-500"
											>
												<Info class="size-3.5 shrink-0" />
												{{
													t(
														"settings.ffmpegPreference.systemNoAutoUpdate",
													)
												}}
											</div>
										</div>

										<!-- tools 文件夹 / tools folder -->
										<div
											role="radio"
											tabindex="0"
											:aria-checked="ffmpegSelected === '1'"
											class="cursor-pointer rounded-lg border px-3 py-2.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60"
											:class="
												ffmpegSelected === '1'
													? 'border-primary ring-1 ring-primary'
													: 'hover:border-muted-foreground/50'
											"
											@click="ffmpegSelected = '1'"
											@keydown.enter.prevent="ffmpegSelected = '1'"
											@keydown.space.prevent="ffmpegSelected = '1'"
										>
											<p class="min-w-0 text-sm font-medium">
												{{ t("ffmpeg.preferenceWizard.toolsDir") }}
												<span
													class="ml-1 break-all text-xs font-normal text-muted-foreground"
													>（{{ ffmpegLocations.tools_dir }}）</span
												>
											</p>
											<div
												class="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffmpeg</span>
												<span
													v-if="ffmpegLocations.tools.ffmpeg_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.tools.ffmpeg_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.tools.ffmpeg"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffprobe</span>
												<span
													v-if="ffmpegLocations.tools.ffprobe_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.tools.ffprobe_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.tools.ffprobe"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
											>
												<span class="font-mono">ffplay</span>
												<span
													v-if="ffmpegLocations.tools.ffplay_version"
													class="font-mono text-foreground/80"
													>{{
														ffmpegLocations.tools.ffplay_version
													}}</span
												>
												<Check
													v-if="ffmpegLocations.tools.ffplay"
													class="size-3.5 shrink-0 text-green-500"
												/>
												<X
													v-else
													class="size-3.5 shrink-0 text-destructive"
												/>
											</div>
											<div
												class="mt-1.5 flex items-center gap-1.5 text-xs text-emerald-600 dark:text-emerald-500"
											>
												<Info class="size-3.5 shrink-0" />
												{{ t("ffmpeg.preferenceWizard.toolsRecommended") }}
											</div>
										</div>
									</div>
								</div>

								<!-- 下载构建选择（选中 tools 且不齐 + 主机支持下载时出现）/
								Build selection (shown when tools is picked but incomplete and the
								host supports downloads) -->
								<FFmpegBuildSelect
									v-if="ffmpegNeedsDownload && ffmpegLocations?.download_options"
									v-model:platform="downloadPlatform"
									v-model:variant="downloadVariant"
									:download-options="ffmpegLocations.download_options"
								/>

								<!-- 所选位置不完整时的提示（invisible 常驻占位避免布局跳动）/ Hint for
								an incomplete selection (always rendered so the layout doesn't jump) -->
								<p
									class="flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-500"
									:class="
										ffmpegSelected && !ffmpegSelectedComplete ? '' : 'invisible'
									"
									:aria-hidden="!(ffmpegSelected && !ffmpegSelectedComplete)"
								>
									<AlertCircle class="size-3.5 shrink-0" />
									{{ t("ffmpeg.preferenceWizard.incompleteSelection") }}
								</p>

								<!-- 下载/安装进度：setup 期间内联显示（全局强制窗口被抑制）/
								Download/install progress: rendered inline during setup
								(the global forced window is suppressed) -->
								<div v-if="ffmpegProgress" class="flex flex-col gap-3">
									<template v-if="ffmpegProgress.state === 'downloading'">
										<div class="flex items-center gap-2 text-sm">
											<Loader2 class="size-4 animate-spin text-primary" />
											{{ t("ffmpeg.downloading") }}
										</div>
										<div
											class="h-1.5 w-full overflow-hidden rounded-full bg-muted"
										>
											<div
												class="h-full bg-primary transition-all"
												:class="
													ffmpegDownloadBar.indeterminate
														? 'w-1/3 animate-pulse'
														: ''
												"
												:style="
													ffmpegDownloadBar.indeterminate
														? undefined
														: { width: `${ffmpegDownloadBar.pct}%` }
												"
											/>
										</div>
										<p
											class="flex min-h-4 items-baseline justify-between text-xs tabular-nums text-muted-foreground"
										>
											<span
												>{{ ffmpegDownloadBar.size
												}}{{ ffmpegSpeed ? " · " + ffmpegSpeed : "" }}</span
											>
											<span>{{ ffmpegDownloadBar.pctText }}</span>
										</p>
									</template>
									<template v-else-if="ffmpegProgress.state === 'installing'">
										<div class="flex items-center gap-2 text-sm">
											<Loader2 class="size-4 animate-spin text-primary" />
											{{ t("ffmpeg.installing") }}
										</div>
										<div
											class="h-1.5 w-full overflow-hidden rounded-full bg-muted"
										>
											<div
												class="h-full bg-primary transition-all"
												:style="{ width: `${ffmpegProgress.pct ?? 0}%` }"
											/>
										</div>
										<p
											class="flex min-h-4 items-baseline justify-end text-xs tabular-nums text-muted-foreground"
										>
											<span>{{
												ffmpegProgress.pct != null
													? `${ffmpegProgress.pct}%`
													: ""
											}}</span>
										</p>
									</template>
									<template v-else-if="ffmpegProgress.state === 'error'">
										<div
											class="flex items-center gap-2 text-sm text-destructive"
										>
											<AlertCircle class="size-4 shrink-0" />
											{{ t("ffmpeg.downloadDialog.failed") }}
										</div>
										<p
											v-if="ffmpegProgress.message"
											class="max-h-24 overflow-y-auto break-all text-xs text-muted-foreground"
										>
											{{ ffmpegProgress.message }}
										</p>
									</template>
								</div>
							</template>
						</div>
					</Transition>
				</div>

				<!-- 错误提示（常驻占位避免导航按钮上下跳动）/ Error message (always rendered so the nav buttons don't shift when it appears) -->
				<p class="text-sm text-destructive -mt-2 min-h-5">{{ error }}</p>

				<!-- 导航按钮 / Navigation buttons -->
				<div class="flex items-center justify-between">
					<Button variant="ghost" :disabled="step === 1" @click="back">
						{{ t("setup.back") }}
					</Button>

					<div class="flex items-center gap-3">
						<!-- FFmpeg 步骤且所选 tools 不完整：「完成」按钮变「下载」，触发后台
						下载（进度由全局强制窗口展示）/ On the FFmpeg step with incomplete
						tools selected the finish button becomes Download, triggering the
						background download (progress shown by the global forced window) -->
						<Button
							v-if="step === FFMPEG_STEP && ffmpegNeedsDownload"
							:disabled="ffmpegSaving || ffmpegInFlight"
							@click="startFFmpegDownload"
						>
							<Loader2
								v-if="ffmpegSaving || ffmpegInFlight"
								class="size-3.5 animate-spin"
							/>
							{{ t("ffmpeg.preferenceWizard.download") }}
						</Button>
						<Button v-else-if="step < TOTAL_STEPS" :disabled="!canNext" @click="next">
							{{ t("setup.next") }}
						</Button>
						<!-- v-else-if 与上面构成互斥：FFmpeg 步（末步）tools 缺件时只显示「下载」，不再同时露出灰掉的「完成」/ v-else-if keeps the chain exclusive: on the last (FFmpeg) step with incomplete tools only Download shows — no ghosted Finish beside it -->
						<Button
							v-else-if="step === TOTAL_STEPS"
							:disabled="saving || !canNext"
							@click="finish"
						>
							{{ saving ? t("setup.saving") : t("setup.finish") }}
						</Button>
					</div>
				</div>
			</div>
		</div>
	</div>
</template>
