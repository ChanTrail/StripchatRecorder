<!--
    FFmpeg 首选项向导对话框（版本升级的兼容路径）/ FFmpeg Preference Wizard Dialog (upgrade compatibility path)

    当设置项 ffmpeg_preference 不存在（null）时自动弹出——这是旧版本升级到引入
    首选项的新版本后的兼容路径（全新安装的选择在 Setup 页处理）：先调用后端探测
    system（PATH）与 tools 两个位置中 ffmpeg/ffprobe/ffplay 的存在情况与 ffmpeg
    版本；两个位置各自渲染为可点选的卡片（选中项边框高亮），
    点击卡片即选择 0（系统环境变量）或 1（tools 文件夹），确认后写入设置。
    预选规则：组件齐全的位置优先（都齐全时 tools 优先）；都不齐全时选组件更
    多的一侧（数量相同选 tools）。
    对话框为必选：隐藏关闭按钮并拦截 Esc / 遮罩点击，设置写入后自动关闭。
    若所选位置缺少任一组件会给出提示，但不阻止选择（用户可自行补齐或重新检测）。
    选 tools 但组件不完整时确认按钮变为"下载"：先触发后台下载，等安装完成
    （install done 事件）才把首选项落盘——失败路径不会留下 tools 配置；
    下载/安装进度由全局强制进度窗口（FFmpegDownloadDialog）接管展示。

    Pops up automatically when the ffmpeg_preference setting is absent (null) — this
    is the compatibility path shown after upgrading from an older version that had no
    preference (fresh installs are handled on the Setup page): it probes
    ffmpeg/ffprobe/ffplay presence and the ffmpeg version on the system PATH and
    in the tools dir via the backend, always listing the results. Each location
    is rendered as a clickable card (the selected one gets a highlighted
    border); clicking a card picks 0 (system PATH) or 1 (tools folder) and
    confirming writes the setting. Preselection: a complete location wins (tools
    preferred when both are complete); when neither is complete, the side with
    more components is picked (ties go to tools). The dialog is mandatory: the
    close button is hidden and Esc / overlay clicks are blocked; it closes
    itself once the setting has been written. When the selected location lacks
    any component a hint is shown, but the choice is not blocked (the user can
    fill it later or re-probe). When tools is selected but incomplete the
    confirm button becomes Download: the background download is triggered
    first and the preference is only persisted once the install lands (the
    install `done` event) — failures never leave a tools preference behind;
    the global forced progress window (FFmpegDownloadDialog) shows the
    download/install progress meanwhile.
-->
<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { useI18n } from "vue-i18n";
import { call, on } from "@/lib/api";
import { useSettingsStore } from "@/stores/settings";
import { useNotify } from "@/composables/useNotify";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import FFmpegBuildSelect from "@/components/FFmpegBuildSelect.vue";
import { AlertCircle, Check, Info, Loader2, RefreshCw, X } from "@lucide/vue";

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
	/** 系统 PATH 探测结果 / System PATH probe result */
	system: FFmpegLocationProbe;
	/** tools 目录探测结果 / Tools directory probe result */
	tools: FFmpegLocationProbe;
	/** 可下载构建选项（不支持下载的平台为 null）/ Downloadable build options (null on unsupported platforms) */
	download_options?: FFmpegDownloadOptions | null;
}

const { t } = useI18n();
const route = useRoute();
const settingsStore = useSettingsStore();
const { toast } = useNotify();

/** 对话框是否打开 / Whether the dialog is open */
const open = ref(false);
/** 探测结果（null = 尚未探测或探测失败）/ Probe result (null = not probed or probe failed) */
const locations = ref<FFmpegLocations | null>(null);
/** 是否正在探测 / Whether a probe is in flight */
const checking = ref(false);
/** 最近一次探测是否失败 / Whether the last probe failed */
const checkFailed = ref(false);
/** 用户当前选择（"0" = 系统环境变量，"1" = tools 文件夹，"" = 未选）/ Current selection ("0" = system PATH, "1" = tools folder, "" = unset) */
const selected = ref<"0" | "1" | "">("");
/** 是否正在保存 / Whether the choice is being saved */
const saving = ref(false);
/** 待完成的 tools 下载：点"下载"后置位，install `done` 事件才把首选项
 * 落盘（失败/重试期间不污染配置）/ Pending tools download: set when
 * Download is clicked; the preference is only persisted on the install
 * `done` event (failures/retries never touch the config) */
const downloadPending = ref(false);
/** 下载构建选择：平台轨（默认取 options 首元素即宿主默认轨）/ Download build pick: platform track (defaults to first option = host default) */
const downloadPlatform = ref("");
/** 下载构建选择：变体后缀（默认 `""` = 静态）/ Download build pick: variant suffix (defaults to `""` = static) */
const downloadVariant = ref("");
/** ffmpeg-progress 事件载荷（此处只需 state/kind）/ ffmpeg-progress payload (only state/kind needed here) */
interface FFmpegProgressLite {
	state: string;
	kind?: string;
}
/** ffmpeg-progress 事件取消订阅 / Unsubscriber for ffmpeg-progress */
let unlistenProgress: (() => void) | null = null;

/** 当前选中位置的探测结果（未选择或无结果时为 null）/ Probe result of the selected location (null when unselected or there is no result) */
const selectedLocation = computed<FFmpegLocationProbe | null>(() => {
	if (!locations.value || selected.value === "") return null;
	return selected.value === "1" ? locations.value.tools : locations.value.system;
});

/** 取路径的父目录部分（同时兼容 \ 与 / 分隔符）/ Parent-dir part of a path (handles both \ and / separators) */
const dirname = (p: string | null | undefined) => p?.replace(/[\\/][^\\/]*$/, "") ?? null;

/** 系统卡标题旁显示的目录：优先 ffmpeg 所在目录，缺失时依次回退 ffprobe / ffplay（PATH 下通常同处一个 bin 目录；分处不同目录时只展示首个命中的）
 * Directory shown next to the system card title: the ffmpeg dir first, falling back to ffprobe's then ffplay's (PATH binaries usually share one bin dir; when they differ only the first hit is shown) */
const systemDir = computed(() => {
	const s = locations.value?.system;
	if (!s) return null;
	return dirname(s.ffmpeg_path) ?? dirname(s.ffprobe_path) ?? dirname(s.ffplay_path);
});

/** 位置是否组件齐全（ffmpeg + ffprobe + ffplay 都在）/ Whether a location has the full set (ffmpeg + ffprobe + ffplay) */
const locationComplete = (p: FFmpegLocationProbe) => p.ffmpeg && p.ffprobe && p.ffplay;

/** 所选位置缺少任一组件时显示提醒（不阻止选择，用户可自行补齐或重新检测）/ Warn when the selected location lacks any component (does not block the choice — the user can fill it later or re-probe) */
const showIncompleteWarning = computed(
	() => selectedLocation.value != null && !locationComplete(selectedLocation.value),
);

/** 选中 tools 且其组件不完整时，确认按钮变为"下载"（触发下载，install done 才落首选项，进度由全局强制窗口展示）/ When tools is selected but its components are incomplete, the confirm button becomes Download (triggers the download; the preference is persisted on install done, progress shown by the global forced window) */
const isDownload = computed(
	() =>
		selected.value === "1" &&
		locations.value != null &&
		!locationComplete(locations.value.tools),
);

/**
 * 是否应该显示向导：设置已加载、首次启动向导已完成且 ffmpeg_preference 尚未设置。
 * Whether the wizard should show: settings loaded, first-launch setup done,
 * and ffmpeg_preference not set yet.
 */
const shouldShow = computed(
	() =>
		settingsStore.loaded &&
		settingsStore.settings.setup_done &&
		settingsStore.settings.ffmpeg_preference == null,
);

watch(
	shouldShow,
	(v) => {
		if (v) {
			open.value = true;
			void probe();
		} else {
			open.value = false;
		}
	},
	{ immediate: true },
);

// 未登录时 get_settings 会失败：路由变化（登录后跳转）时再尝试加载一次
// get_settings fails while logged out: retry loading on navigation (post-login redirect)
watch(
	() => route.path,
	() => {
		if (!settingsStore.loaded) void loadSettings();
	},
);

onMounted(async () => {
	if (!settingsStore.loaded) void loadSettings();
	// 待完成的 tools 下载：安装完成后才把首选项落盘（auto-save 由
	// saveSettings 负责）/ Pending tools download: persist the preference
	// only after the install lands
	unlistenProgress = await on("ffmpeg-progress", (payload) => {
		const p = payload as FFmpegProgressLite;
		if (p.kind === "install" && p.state === "done" && downloadPending.value) {
			void persistChoice();
		}
	});
});

onUnmounted(() => {
	unlistenProgress?.();
});

/**
 * 加载设置（静默失败：未登录或后端未就绪时不打扰用户）。
 * Load settings (fails silently — no noise while logged out or backend not ready).
 */
async function loadSettings() {
	try {
		await settingsStore.fetchSettings();
	} catch {
		// 忽略：loaded 仍为 false，等待下次触发 / Ignore: loaded stays false until the next trigger
	}
}

/**
 * 调用后端探测两个位置的 ffmpeg/ffprobe/ffplay 存在情况与版本，并按结果预选：
 * 组件齐全的位置优先（两侧都齐全时 tools 优先）；都不齐全时选组件更多的一侧
 * （数量相同选 tools——缺失的组件可由自动下载补齐）。
 * Probe both locations via the backend and preselect from the result: a
 * complete location wins (tools preferred when both are complete); when
 * neither is complete, the side with more components is picked (ties go to
 * tools — its missing pieces can be filled by the auto-download).
 */
async function probe() {
	checking.value = true;
	checkFailed.value = false;
	try {
		const res = await call<FFmpegLocations>("get_ffmpeg_locations");
		locations.value = res;
		// 构建选择回填宿主默认轨（用户已手动改过则不动）/ Default the build
		// pick to the host's default track (leave a manual pick untouched)
		if (!downloadPlatform.value && res.download_options?.platforms.length) {
			downloadPlatform.value = res.download_options.platforms[0];
		}
		if (selected.value === "") {
			const count = (p: FFmpegLocationProbe) =>
				[p.ffmpeg, p.ffprobe, p.ffplay].filter(Boolean).length;
			if (locationComplete(res.tools)) selected.value = "1";
			else if (locationComplete(res.system)) selected.value = "0";
			else selected.value = count(res.tools) >= count(res.system) ? "1" : "0";
		}
	} catch {
		locations.value = null;
		checkFailed.value = true;
	} finally {
		checking.value = false;
	}
}

/**
 * 把用户选择（0 = 系统环境变量，1 = tools 文件夹）写入设置；自动更新随
 * 选择联动——PATH 写 false、tools 写 true，使配置文件与实际行为一致。
 * settings 更新后 shouldShow 变为 false，watch 自动关闭对话框。
 * Write the user's choice (0 = system PATH, 1 = tools folder) into settings;
 * auto-update follows the choice — `false` for PATH, `true` for tools — so
 * the config file reflects actual behavior. Once the update lands,
 * shouldShow flips to false and the watcher closes the dialog.
 */
/**
 * 把当前选择写入设置（`ffmpeg_preference` + 联动的 `ffmpeg_auto_update`）；
 * 写入后 `shouldShow` 变 false，watch 自动关闭对话框。
 * Persist the current selection (`ffmpeg_preference` + the linked
 * `ffmpeg_auto_update`); once it lands `shouldShow` flips false and the
 * watcher closes the dialog.
 */
async function persistChoice() {
	if (selected.value === "") return;
	downloadPending.value = false;
	try {
		await settingsStore.saveSettings({
			...settingsStore.settings,
			ffmpeg_preference: Number(selected.value),
			// 自动更新随首选项联动：PATH 关、tools 开 /
			// Auto-update follows the preference: off for PATH, on for tools
			ffmpeg_auto_update: selected.value !== "0",
		});
	} catch {
		toast(t("ffmpeg.preferenceWizard.saveFailed"), "error");
	}
}

async function confirm() {
	if (selected.value === "" || saving.value) return;
	saving.value = true;
	try {
		if (isDownload.value) {
			// 安装完成才写首选项：先标记待下载再触发，install done 事件里
			// 落盘；进度由全局强制窗口（FFmpegDownloadDialog）展示。
			// 手动下载不按 PATH 首选项拦截（明确意图），无需先保存。
			// Persist the preference only after install: mark the pending
			// download and trigger it — the install `done` event persists the
			// choice; progress is shown by the global forced window
			// (FFmpegDownloadDialog). The manual download is not gated on the
			// PATH preference (explicit intent), so no pre-save is needed.
			downloadPending.value = true;
			try {
				await call("download_ffmpeg", {
					platform: downloadPlatform.value || undefined,
					variant: downloadVariant.value || undefined,
				});
			} catch {
				downloadPending.value = false;
				toast(t("ffmpeg.preferenceWizard.downloadStartFailed"), "error");
			}
		} else {
			await persistChoice();
		}
	} finally {
		saving.value = false;
	}
}

/**
 * 阻止对话框在未选择时关闭（Esc / 点击遮罩）。
 * Prevent the dialog from closing before a choice is made (Esc / overlay click).
 *
 * @param e - reka-ui 派发的事件 / Event dispatched by reka-ui
 */
function preventClose(e: Event) {
	e.preventDefault();
}
</script>

<template>
	<Dialog
		:open="open"
		@update:open="
			(v) => {
				if (v) open = true;
			}
		"
	>
		<DialogContent
			class="focus:outline-none sm:max-w-lg"
			:show-close-button="false"
			@escape-key-down="preventClose"
			@interact-outside="preventClose"
		>
			<DialogHeader>
				<DialogTitle>{{ t("ffmpeg.preferenceWizard.title") }}</DialogTitle>
				<DialogDescription>{{ t("ffmpeg.preferenceWizard.desc") }}</DialogDescription>
			</DialogHeader>

			<!-- 检测结果：无论找到与否都列出两个位置，点击整张卡片即选择该来源（选中高亮）/ Probe results: both locations listed regardless of outcome; clicking a card selects that source (highlighted) -->
			<!-- 探测区：min-h 预留两张位置卡片的高度，checking/失败/结果三态切换时
				对话框高度不跳变（未就绪状态垂直居中）/ Probe area: min-h reserves the
				two location cards' height so switching between checking / failed /
				results never resizes the dialog (pending states are centered) -->
			<div class="flex min-h-[230px] flex-col justify-center">
				<div
					v-if="checking && !locations"
					class="flex items-center justify-center gap-2 py-4 text-sm text-muted-foreground"
				>
					<Loader2 class="size-4 animate-spin" />
					{{ t("ffmpeg.preferenceWizard.checking") }}
				</div>
				<div
					v-else-if="checkFailed && !locations"
					class="py-4 text-center text-sm text-destructive"
				>
					{{ t("ffmpeg.preferenceWizard.checkFailed") }}
				</div>
				<div
					v-else-if="locations"
					class="flex flex-col gap-2"
					role="radiogroup"
					:aria-label="t('ffmpeg.preferenceWizard.title')"
				>
					<!-- 系统环境变量：点击卡片任意位置选择，边框高亮标识选中态 / System PATH: click anywhere on the card to select; the highlighted border marks it -->
					<div
						role="radio"
						tabindex="0"
						:aria-checked="selected === '0'"
						class="cursor-pointer rounded-lg border px-3 py-2.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60"
						:class="
							selected === '0'
								? 'border-primary ring-1 ring-primary'
								: 'hover:border-muted-foreground/50'
						"
						@click="selected = '0'"
						@keydown.enter.prevent="selected = '0'"
						@keydown.space.prevent="selected = '0'"
					>
						<p class="min-w-0 text-sm font-medium">
							{{ t("ffmpeg.preferenceWizard.systemPath") }}
							<span
								v-if="systemDir"
								class="ml-1 break-all text-xs font-normal text-muted-foreground"
								>（{{ systemDir }}）</span
							>
						</p>
						<!-- 子项：组件名（+版本号）后跟 ✓/✗ / Rows: component name (+ version) followed by ✓/✗ -->
						<div class="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffmpeg</span>
							<span
								v-if="locations.system.ffmpeg_version"
								class="font-mono text-foreground/80"
								>{{ locations.system.ffmpeg_version }}</span
							>
							<Check
								v-if="locations.system.ffmpeg"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<div class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffprobe</span>
							<span
								v-if="locations.system.ffprobe_version"
								class="font-mono text-foreground/80"
								>{{ locations.system.ffprobe_version }}</span
							>
							<Check
								v-if="locations.system.ffprobe"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<div class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffplay</span>
							<span
								v-if="locations.system.ffplay_version"
								class="font-mono text-foreground/80"
								>{{ locations.system.ffplay_version }}</span
							>
							<Check
								v-if="locations.system.ffplay"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<!-- 选择 PATH 即禁用 FFmpeg 自动更新的提示 / Note that choosing PATH disables FFmpeg auto-update -->
						<div
							class="mt-1.5 flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-500"
						>
							<Info class="size-3.5 shrink-0" />
							{{ t("settings.ffmpegPreference.systemNoAutoUpdate") }}
						</div>
					</div>

					<!-- tools 文件夹 / tools folder -->
					<div
						role="radio"
						tabindex="0"
						:aria-checked="selected === '1'"
						class="cursor-pointer rounded-lg border px-3 py-2.5 outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring/60"
						:class="
							selected === '1'
								? 'border-primary ring-1 ring-primary'
								: 'hover:border-muted-foreground/50'
						"
						@click="selected = '1'"
						@keydown.enter.prevent="selected = '1'"
						@keydown.space.prevent="selected = '1'"
					>
						<p class="min-w-0 text-sm font-medium">
							{{ t("ffmpeg.preferenceWizard.toolsDir") }}
							<span class="ml-1 break-all text-xs font-normal text-muted-foreground"
								>（{{ locations.tools_dir }}）</span
							>
						</p>
						<!-- tools 各文件都在标题所示的同一目录内 / tools files all live in the directory shown in the title -->
						<div class="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffmpeg</span>
							<span
								v-if="locations.tools.ffmpeg_version"
								class="font-mono text-foreground/80"
								>{{ locations.tools.ffmpeg_version }}</span
							>
							<Check
								v-if="locations.tools.ffmpeg"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<div class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffprobe</span>
							<span
								v-if="locations.tools.ffprobe_version"
								class="font-mono text-foreground/80"
								>{{ locations.tools.ffprobe_version }}</span
							>
							<Check
								v-if="locations.tools.ffprobe"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<div class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
							<span class="font-mono">ffplay</span>
							<span
								v-if="locations.tools.ffplay_version"
								class="font-mono text-foreground/80"
								>{{ locations.tools.ffplay_version }}</span
							>
							<Check
								v-if="locations.tools.ffplay"
								class="size-3.5 shrink-0 text-green-500"
							/>
							<X v-else class="size-3.5 shrink-0 text-destructive" />
						</div>
						<!-- 推荐提示：tools 副本由程序自动更新维护 / Recommendation: the tools copy is auto-updated by the program -->
						<div
							class="mt-1.5 flex items-center gap-1.5 text-xs text-emerald-600 dark:text-emerald-500"
						>
							<Info class="size-3.5 shrink-0" />
							{{ t("ffmpeg.preferenceWizard.toolsRecommended") }}
						</div>
					</div>
				</div>

				<!-- 下载构建选择（选中 tools 且不齐 + 主机支持下载时出现）/
				Build selection (shown when tools is picked but incomplete and the
				host supports downloads) -->
				<FFmpegBuildSelect
					v-if="locations && isDownload && locations.download_options"
					v-model:platform="downloadPlatform"
					v-model:variant="downloadVariant"
					:download-options="locations.download_options"
				/>
			</div>

			<!-- 所选位置不完整时给出提示；始终渲染（invisible 占位）避免出现时对话框高度跳动 / Hint for an incomplete selection; always rendered (invisible placeholder) so the dialog height doesn't jump when it appears -->
			<p
				class="flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-500"
				:class="showIncompleteWarning ? '' : 'invisible'"
				:aria-hidden="!showIncompleteWarning"
			>
				<AlertCircle class="size-3.5 shrink-0" />
				{{ t("ffmpeg.preferenceWizard.incompleteSelection") }}
			</p>

			<DialogFooter class="flex items-center justify-between sm:justify-between">
				<Button variant="ghost" size="sm" :disabled="checking" @click="probe">
					<RefreshCw :class="['size-3.5', checking ? 'animate-spin' : '']" />
					{{ t("ffmpeg.preferenceWizard.retry") }}
				</Button>
				<Button :disabled="selected === '' || saving || checking" @click="confirm">
					<Loader2 v-if="saving" class="size-3.5 animate-spin" />
					{{
						isDownload
							? t("ffmpeg.preferenceWizard.download")
							: t("ffmpeg.preferenceWizard.confirm")
					}}
				</Button>
			</DialogFooter>
		</DialogContent>
	</Dialog>
</template>
