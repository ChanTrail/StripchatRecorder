<!--
    应用设置页面 / Application Settings View

    与 frontend 版对齐，新增：
    - 目录浏览器按钮（useDirectoryBrowser）
    - sc_mirror_scheme 协议选择
    - preferred_resolution / resolution_preference
    - max_recording_duration_secs
    - max_pp_concurrent（SystemStore 动态上限）
    - community_proxy_url / community_mirror_url
    - showToken 眼睛图标
    - sticky header + section observer
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, reactive, ref, watch, nextTick, computed } from "vue";
	import { call, on } from "@/lib/api";
	import { useSettingsStore, type Settings, type MouflonKeysStore } from "../stores/settings";
	import { useSystemStore } from "../stores/system";
	import { useNotify } from "../composables/useNotify";
	import { useMobileLayout } from "../composables/useMobileLayout";
	import { useDirectoryBrowser } from "@/composables/useDirectoryBrowser";
	import { Button } from "@/components/ui/button";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import { Switch } from "@/components/ui/switch";
	import SchemeUrlInput from "@/components/SchemeUrlInput.vue";
	import DurationHmsInput from "@/components/DurationHmsInput.vue";
	import {
		NumberField,
		NumberFieldContent,
		NumberFieldDecrement,
		NumberFieldIncrement,
		NumberFieldInput,
	} from "@/components/ui/number-field";
	import { useI18n } from "vue-i18n";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
	} from "@/components/ui/select";
	import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
	import { FolderOpen, Eye, EyeOff, Check, Loader2 } from "@lucide/vue";

	const store = useSettingsStore();
	const systemStore = useSystemStore();
	const { toast, confirm } = useNotify();
	/** 移动端（UA 判定）下字段行上下堆叠 / UA-detected mobile: stack field rows vertically */
	const { isMobile } = useMobileLayout();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();

	const resolutionDirections = ["lower", "higher"] as const;

	/** 可选的首选视频编码：value 即请求中 preferredVideoCodec 的取值，label 为界面显示名
	 * Selectable preferred video codecs: value is what goes into the preferredVideoCodec request
	 * parameter, label is the display name */
	const videoCodecs = [
		{ value: "h264", label: "H.264" },
		{ value: "H265", label: "H.265" },
		{ value: "AV1", label: "AV1" },
	] as const;

	/** 下拉框当前值：不区分大小写匹配设置里的编码，空或不认识的值显示为"默认"
	 * (SelectItem 的 value 不能为空串，故用 "default" 表示未指定)
	 * Current dropdown value: matches the stored codec case-insensitively; empty or unknown shows
	 * as "default" (a SelectItem value cannot be an empty string, so "default" stands for unspecified) */
	const codecSelectValue = computed(() => {
		const stored = (form.preferred_codec ?? "").trim().toLowerCase();
		return videoCodecs.find((c) => c.value.toLowerCase() === stored)?.value ?? "default";
	});

	/** 当前运行的是 beta 版本（版本号以 `-beta` 结尾）时为 true，此时强制开启 beta 更新检查。
	 * True when running a beta build; forces check_prerelease on. */
	const isBeta = computed(() => __APP_VERSION__.endsWith("-beta"));

	async function setLocale(lang: string) {
		const { modules: moduleLocales, warning } = await loadLocaleFromServer(lang);
		locale.value = lang;
		localStorage.setItem("locale", lang);
		form.language = lang;
		await saveWithIndicator();
		moduleLocaleStore.setLocales(lang, moduleLocales);
		if (warning) {
			toast(t("settings.localeFileInvalid", { file: `${lang}.json` }) + ": " + warning, "warning");
		}
	}

	const form = reactive<Settings>({
		output_dir: "",
		poll_interval_secs: 30,
		auto_record: true,
		api_proxy_url: null,
		cdn_proxy_url: null,
		sc_mirror_url: null,
		sc_mirror_scheme: "https",
		sc_browse_proxy_url: null,
		sc_browse_mirror_url: null,
		max_recording_duration_secs: 0,
		preferred_resolution: 0,
		resolution_preference: "lower" as "lower" | "higher",
		preferred_codec: "",
		max_tmp_dir_gb: 50,
		language: "zh-CN",
		mouflon_sync_url: null,
		mouflon_sync_token: null,
		community_proxy_url: null,
		community_mirror_url: null,
		community_terms_accepted: false,
		setup_done: true,
		max_pp_concurrent: 0,
		check_prerelease: false,
		ffmpeg_auto_update: true,
		ffmpeg_preference: null,
		cgf_proxy_url: null,
		ech_enabled: false,
	});

	const originalOutputDir = ref("");
	const originalApiProxy = ref<string | null>(null);
	const originalCdnProxy = ref<string | null>(null);
	const originalScMirror = ref<string | null>(null);
	const originalMouflonSyncUrl = ref<string | null>(null);
	const originalMouflonSyncToken = ref<string | null>(null);
	const originalCommunityProxy = ref<string | null>(null);
	const originalCommunityMirror = ref<string | null>(null);
	const originalCgfProxy = ref<string | null>(null);
	const originalScBrowseProxy = ref<string | null>(null);
	const originalScBrowseMirror = ref<string | null>(null);
	let initialized = false;

	const unlisteners: (() => void)[] = [];

	onMounted(async () => {
		await store.initListeners();
		await Promise.all([store.fetchSettings(), systemStore.fetchSystemInfo()]);
		Object.assign(form, store.settings);
		originalOutputDir.value = form.output_dir;
		originalApiProxy.value = form.api_proxy_url;
		originalCdnProxy.value = form.cdn_proxy_url;
		originalScMirror.value = form.sc_mirror_url;
		originalMouflonSyncUrl.value = form.mouflon_sync_url;
		originalMouflonSyncToken.value = form.mouflon_sync_token;
		originalCommunityProxy.value = form.community_proxy_url;
		originalCommunityMirror.value = form.community_mirror_url;
		originalCgfProxy.value = form.cgf_proxy_url;
		originalScBrowseProxy.value = form.sc_browse_proxy_url;
		originalScBrowseMirror.value = form.sc_browse_mirror_url;
		await nextTick();
		initialized = true;
		await loadKeys();
		// 探测两位置 FFmpeg 版本供首选项展示（后台异步，不阻塞页面）/ Probe FFmpeg versions at both locations for the preference display (background, non-blocking)
		void loadFfmpegLocations();

		unlisteners.push(
			await on("mouflon-keys-updated", (payload) => {
				const s = payload as MouflonKeysStore;
				mouflonStore.value = s;
				toast(t("settings.mouflonUpdatedByOther"), "info");
			}),
		);

		// 待完成的 tools 下载：install done 时才把 tools 写入首选项（auto-save
		// watcher 持久化）；error 保持 pending 供重试 / Pending tools download:
		// the tools preference is committed only on the install `done` event
		// (the auto-save watcher persists it); `error` keeps it pending for retry
		unlisteners.push(
			await on("ffmpeg-progress", (payload) => {
				const p = payload as FfmpegProgressLite;
				if (p.kind === "install" && p.state === "done") {
					// tools 安装完成后刷新版本显示（新二进制版本可能不同）/ Refresh versions after a tools install completes (the new binaries may differ)
					void loadFfmpegLocations();
					if (pendingToolsDownload.value) {
						commitFfmpegPreference(1);
					}
				}
			}),
		);
	});

	onUnmounted(() => {
		unlisteners.forEach((fn) => fn());
		sectionObserver?.disconnect();
		clearTimeout(saveTimer);
		clearTimeout(savedHideTimer);
	});

	// ── 自动保存（防抖 + 标题栏状态指示）/ Auto-save (debounced + header status) ──
	/** 正在执行自动保存 / An auto-save is in flight */
	const autoSaving = ref(false);
	/** 至少完成过一次自动保存（用于显示"已自动保存"）/ At least one auto-save succeeded */
	const autoSaved = ref(false);
	/** "已保存"指示自动隐藏计时器（每次保存重置）/ Auto-hide timer for the "saved" chip (reset on each save) */
	let savedHideTimer: ReturnType<typeof setTimeout> | undefined;
	/** 防抖计时器 / Debounce timer */
	let saveTimer: ReturnType<typeof setTimeout> | undefined;
	/** 是否有待保存的变更（含保存进行中又来了新变更）/ Pending changes (incl. edits arriving mid-save) */
	let dirtyPending = false;

	/** 防抖调度自动保存 / Schedule a debounced auto-save */
	function scheduleAutosave() {
		if (!initialized) return;
		dirtyPending = true;
		clearTimeout(saveTimer);
		saveTimer = setTimeout(() => void flushAutosave(), 600);
	}

	/** 执行自动保存：成功点亮"已保存"指示，失败弹错误 toast /
	 *  Run the auto-save: light the "saved" indicator on success, toast on error */
	async function flushAutosave() {
		if (!dirtyPending) return;
		dirtyPending = false;
		await saveWithIndicator();
		// 保存进行中又来了变更 → 再补一轮防抖保存 /
		// Changes arrived during the save → schedule another round
		if (dirtyPending) scheduleAutosave();
	}

	/**
	 * 保存当前表单并在标题栏显示状态（保存中转圈 → 成功亮"已保存"），失败弹错误 toast。
	 * Save the current form with the header status indicator (spinner → "saved"),
	 * toasting on failure. Returns whether the save succeeded.
	 */
	async function saveWithIndicator(): Promise<boolean> {
		autoSaving.value = true;
		try {
			await store.saveSettings({ ...form });
			autoSaved.value = true;
			// 3 秒后淡出"已保存"，连续保存时重置计时 / Fade the "saved" chip after 3s; consecutive saves reset the timer
			clearTimeout(savedHideTimer);
			savedHideTimer = setTimeout(() => (autoSaved.value = false), 3000);
			return true;
		} catch (e: unknown) {
			toast(t("settings.saveFailed") + ": " + String(e), "error");
			return false;
		} finally {
			autoSaving.value = false;
		}
	}

	watch(
		() => ({
			poll_interval_secs: form.poll_interval_secs,
			auto_record: form.auto_record,
			max_recording_duration_secs: form.max_recording_duration_secs,
			preferred_resolution: form.preferred_resolution,
			resolution_preference: form.resolution_preference,
			preferred_codec: form.preferred_codec,
			max_tmp_dir_gb: form.max_tmp_dir_gb,
			sc_mirror_scheme: form.sc_mirror_scheme,
			max_pp_concurrent: form.max_pp_concurrent,
			check_prerelease: form.check_prerelease,
			ffmpeg_auto_update: form.ffmpeg_auto_update,
			ffmpeg_preference: form.ffmpeg_preference,
			ech_enabled: form.ech_enabled,
		}),
		scheduleAutosave,
		{ deep: true },
	);

	// 监听 ffmpeg 相关设置被本端其他入口（如首选项向导）改动后同步到表单；
	// 本页自己的保存因值相同直接跳过，不会触发上面的自动保存
	// Sync ffmpeg settings changed elsewhere in this client (e.g. the preference
	// wizard) into the form; saves from this page are skipped because the values
	// already match, so the auto-save watcher above never re-fires
	watch(
		() => [store.settings.ffmpeg_preference, store.settings.ffmpeg_auto_update] as const,
		([pref, auto]) => {
			if (!initialized) return;
			if (pref === form.ffmpeg_preference && auto === form.ffmpeg_auto_update) return;
			initialized = false;
			form.ffmpeg_preference = pref;
			form.ffmpeg_auto_update = auto;
			nextTick(() => {
				initialized = true;
			});
		},
	);

	/**
	 * FFmpeg 首选项变更处理：自动更新随首选项联动——选择系统环境变量（0）
	 * 时 `ffmpeg_auto_update` 写为 false（PATH 中的 FFmpeg 由用户自管），
	 * 选回 tools（1）时自动写为 true，使配置文件与实际行为一致。
	 * Handle FFmpeg preference changes: auto-update follows the preference —
	 * choosing the system PATH (0) writes `ffmpeg_auto_update: false` (the
	 * user's own FFmpeg is theirs to manage), and switching back to tools (1)
	 * writes `true`, keeping the config file consistent with behavior.
	 *
	 * @param v - Select 组件发出的新值 / New value emitted by the Select
	 */
	/** FFmpeg 位置探测结果（与后端 `FfmpegLocations` 对应的精简字段）/ Minimal probe fields mirroring the backend `FfmpegLocations` payload */
	interface FfmpegLocationProbeLite {
		/** ffmpeg 是否存在 / Whether ffmpeg exists */
		ffmpeg: boolean;
		/** ffprobe 是否存在 / Whether ffprobe exists */
		ffprobe: boolean;
		/** ffplay 是否存在 / Whether ffplay exists */
		ffplay: boolean;
		/** `ffmpeg -version` 的版本号字段（缺失/失败为 null）/ Version token from `ffmpeg -version` (null when absent/failed) */
		ffmpeg_version: string | null;
	}
	/** 两个位置的探测结果 / Probe results for both locations */
	interface FfmpegLocationsLite {
		/** 系统 PATH 探测结果 / System PATH probe result */
		system: FfmpegLocationProbeLite;
		/** tools 目录探测结果 / Tools directory probe result */
		tools: FfmpegLocationProbeLite;
	}

	/** 位置是否组件齐全（ffmpeg + ffprobe + ffplay 都在）/ Whether a location has the full component set (ffmpeg + ffprobe + ffplay) */
	const locationComplete = (p: FfmpegLocationProbeLite) => p.ffmpeg && p.ffprobe && p.ffplay;

	/** 两位置的探测结果（进入页面即拉取，供首选项 Select 展示各位置版本号）/ Probe results for both locations (fetched on mount to show per-location versions in the preference Select) */
	const ffmpegLocations = ref<FfmpegLocationsLite | null>(null);
	/** 探测两位置组件与版本；失败静默——版本号仅作展示，缺省退化为纯选项名 /
	 *  Probe both locations for components and versions; failures are silent —
	 *  versions are display-only and the options degrade to bare labels */
	async function loadFfmpegLocations() {
		try {
			ffmpegLocations.value = await call<FfmpegLocationsLite>("get_ffmpeg_locations");
		} catch {
			/* 静默失败 / fail silently */
		}
	}

	/**
	 * FFmpeg 首选项 Select 的强制重挂载计数器。
	 * reka-ui Select 在点击选项时内部即更新显示状态；当提交值与当前值相同
	 * （如取消/回滚后恢复为原值）prop 不变，内部状态不会回同步，显示会卡在
	 * 未提交的选项上。每次提交后递增 key 强制重挂载，使显示始终等于已提交值。
	 * Force-remount counter for the FFmpeg preference Select.
	 * reka-ui Select updates its display state internally on click; when the
	 * committed value equals the current one (e.g. reverts after cancel) the
	 * prop never changes so the internal state never re-syncs and the display
	 * stays stuck on the uncommitted option. Bumping this key after every
	 * commit remounts the Select so its display always equals the committed value.
	 */
	const ffmpegPrefSelectKey = ref(0);
	/** 待完成的 tools 下载：确认"下载"后置位，install `done` 事件时才把 tools
	 * 写入首选项（失败/取消不污染配置）/ Pending tools download: set after the
	 * Download confirm; the tools preference is only written when the install
	 * `done` event arrives (failures/cancels never touch the config) */
	const pendingToolsDownload = ref(false);
	/** ffmpeg-progress 事件载荷（此处只需 state/kind）/ ffmpeg-progress payload (only state/kind needed here) */
	interface FfmpegProgressLite {
		state: string;
		kind?: string;
	}

	/**
	 * 提交 FFmpeg 首选项并联动自动更新：选择系统环境变量（0）时 `ffmpeg_auto_update`
	 * 写为 false（PATH 中的 FFmpeg 由用户自管），选回 tools（1）时自动写为 true，
	 * 使配置文件与实际行为一致；写表单即触发上方自动保存 watcher 持久化。
	 * Commit the FFmpeg preference with auto-update linkage: choosing the system
	 * PATH (0) writes `ffmpeg_auto_update: false` (the user's own FFmpeg is theirs
	 * to manage) and switching to tools (1) writes `true`, keeping the config file
	 * consistent with behavior; mutating the form triggers the auto-save watcher.
	 *
	 * @param n - 首选项值（0=系统 PATH，1=tools 文件夹）/ Preference value (0=system PATH, 1=tools folder)
	 */
	function commitFfmpegPreference(n: number) {
		pendingToolsDownload.value = false;
		form.ffmpeg_preference = n;
		form.ffmpeg_auto_update = n !== 0;
		// 强制 Select 重挂载，显示与提交值对齐（同值提交时 prop 不变，reka 内部状态不会自行回同步）
		// Force-remount the Select so its display matches the committed value (same-value commits never re-sync reka's internal state)
		ffmpegPrefSelectKey.value += 1;
	}

	/**
	 * FFmpeg 首选项变更处理：提交前先探测目标位置的组件完整性——
	 * - 选 PATH 但组件不齐 → 弹单确认框告知后自动改回 tools；
	 * - 选 tools 但组件不齐 → 询问是否下载：确认→保存 tools 并触发下载（全局强制进度窗口），取消→改回 PATH；
	 * - 组件齐全或探测失败 → 按原样提交（探测失败不阻塞选择）。
	 * FFmpeg preference change handler: the target location is probed for a
	 * complete component set before committing —
	 * - PATH selected but incomplete → notify via a confirm-only dialog, then auto-revert to tools;
	 * - tools selected but incomplete → ask whether to download: confirm commits tools and triggers the download (global forced progress window), cancel reverts to PATH;
	 * - complete set or probe failure → committed as-is (a failed probe never blocks the choice).
	 *
	 * @param v - Select 组件发出的新值 / New value emitted by the Select
	 */
	async function onFfmpegPreferenceChange(v: unknown) {
		const n = Number(v);
		if (!Number.isFinite(n)) return;
		// 探测两个位置的组件完整性；失败时 probe 为 null，按原样提交
		// Probe both locations; on failure probe stays null and the choice is committed as-is
		let probe: FfmpegLocationsLite | null = null;
		try {
			probe = await call<FfmpegLocationsLite>("get_ffmpeg_locations");
		} catch {
			probe = null;
		}
		if (probe && n === 0 && !locationComplete(probe.system)) {
			// PATH 缺组件 → 仅可确认，确认后自动改回 tools
			// PATH incomplete → confirm-only dialog, then auto-revert to tools
			await confirm({
				title: t("settings.ffmpegPreference.pathMissingTitle"),
				message: t("settings.ffmpegPreference.pathMissingDesc"),
				confirmText: t("common.confirm"),
				hideCancelButton: true,
			});
			commitFfmpegPreference(1);
			return;
		}
		if (probe && n === 1 && !locationComplete(probe.tools)) {
			// tools 缺组件 → 询问是否下载；确认→下载，取消→改回 PATH
			// tools incomplete → ask whether to download; confirm downloads, cancel reverts to PATH
			const wantDownload = await confirm({
				title: t("settings.ffmpegPreference.toolsMissingTitle"),
				message: t("settings.ffmpegPreference.toolsMissingDesc"),
				confirmText: t("settings.ffmpegPreference.download"),
				cancelText: t("common.cancel"),
			});
			if (!wantDownload) {
				commitFfmpegPreference(0);
				return;
			}
			// 安装完成才写首选项：先标记待下载 + 把 Select 显示弹回已提交值
			// （未提交前仍显示旧值），install done 事件里提交 tools。
			// 手动下载不再按 PATH 首选项拦截（明确意图），无需先落配置。
			// Persist the preference only after install: mark the pending
			// download and snap the Select back to the committed value (the old
			// value still shows until then); the install `done` event commits
			// tools. The manual download no longer gates on the PATH
			// preference (an explicit click is intent), so no pre-save is needed.
			pendingToolsDownload.value = true;
			ffmpegPrefSelectKey.value += 1;
			try {
				await call("download_ffmpeg");
			} catch {
				pendingToolsDownload.value = false;
				toast(t("settings.ffmpegPreference.downloadStartFailed"), "error");
			}
			return;
		}
		commitFfmpegPreference(n);
	}

	watch(
		() => store.settings,
		(newSettings) => {
			if (!initialized || store.isSavingLocally) return;
			initialized = false;
			Object.assign(form, newSettings);
			originalOutputDir.value = newSettings.output_dir;
			originalApiProxy.value = newSettings.api_proxy_url;
			originalCdnProxy.value = newSettings.cdn_proxy_url;
			originalScMirror.value = newSettings.sc_mirror_url;
			originalMouflonSyncUrl.value = newSettings.mouflon_sync_url;
			originalMouflonSyncToken.value = newSettings.mouflon_sync_token;
			originalCommunityProxy.value = newSettings.community_proxy_url;
			originalCommunityMirror.value = newSettings.community_mirror_url;
			originalCgfProxy.value = newSettings.cgf_proxy_url;
			originalScBrowseProxy.value = newSettings.sc_browse_proxy_url;
			originalScBrowseMirror.value = newSettings.sc_browse_mirror_url;
			nextTick(() => { initialized = true; });
			toast(t("settings.updatedByOther"), "info");
		},
		{ deep: true },
	);

	async function saveProxy(
		field: "api_proxy_url" | "cdn_proxy_url" | "sc_mirror_url" | "mouflon_sync_url" | "mouflon_sync_token" | "community_proxy_url" | "community_mirror_url" | "cgf_proxy_url" | "sc_browse_proxy_url" | "sc_browse_mirror_url",
	) {
		if (!initialized) return;
		const originalMap = {
			api_proxy_url: originalApiProxy,
			cdn_proxy_url: originalCdnProxy,
			sc_mirror_url: originalScMirror,
			mouflon_sync_url: originalMouflonSyncUrl,
			mouflon_sync_token: originalMouflonSyncToken,
			community_proxy_url: originalCommunityProxy,
			community_mirror_url: originalCommunityMirror,
			cgf_proxy_url: originalCgfProxy,
			sc_browse_proxy_url: originalScBrowseProxy,
			sc_browse_mirror_url: originalScBrowseMirror,
		};
		const original = originalMap[field];
		if (form[field] === original.value) return;
		// 保存成功才更新基线值，失败留待下次失焦重试（标题栏指示已覆盖反馈）
		// Update baseline only on success; failure retries on next blur (header chip covers feedback)
		if (await saveWithIndicator()) original.value = form[field];
	}

	async function saveOutputDir() {
		if (!initialized) return;
		if (form.output_dir === originalOutputDir.value) return;
		const ok = await confirm({
			title: t("settings.outputDir.changeTitle"),
			message: t("settings.outputDir.changeMessage", { dir: form.output_dir }),
			confirmText: t("settings.outputDir.changeConfirm"),
		});
		if (ok) {
			if (await saveWithIndicator()) originalOutputDir.value = form.output_dir;
		} else {
			form.output_dir = originalOutputDir.value;
		}
	}

	const { open: openDirectoryBrowser } = useDirectoryBrowser();

	function browseOutputDir() {
		openDirectoryBrowser(form.output_dir, (picked) => {
			form.output_dir = picked;
			saveOutputDir();
		});
	}

	const mouflonStore = ref<MouflonKeysStore>({
		keys: {},
		keystreams: {},
		synced_hash: null,
		auto_synced_at: null,
		manual_updated_at: null,
	});

	/** 密钥表的行：pdkey 表与 keystream 表中出现过的所有 pkey，按 pkey 排序
	 * Rows of the key table: every pkey that appears in the pdkey or keystream table, sorted by pkey */
	const mouflonRows = computed(() => {
		const keys = mouflonStore.value.keys ?? {};
		const keystreams = mouflonStore.value.keystreams ?? {};
		return [...new Set([...Object.keys(keys), ...Object.keys(keystreams)])]
			.sort()
			.map((pkey) => ({ pkey, pdkey: keys[pkey] ?? "", keystream: keystreams[pkey] ?? "" }));
	});

	const newPkey = ref("");
	const newPdkey = ref("");
	const newKeystream = ref("");
	const keyError = ref("");
	const syncing = ref(false);
	const showToken = ref(false);

	async function loadKeys() {
		mouflonStore.value = await call<MouflonKeysStore>("list_mouflon_keys");
	}

	async function addKey() {
		keyError.value = "";
		const pkey = newPkey.value.trim();
		const pdkey = newPdkey.value.trim();
		const keystream = newKeystream.value.trim();
		// pkey 必填，pdkey 与 keystream 至少填一个 / pkey is required, plus at least one of pdkey and keystream
		if (!pkey || (!pdkey && !keystream)) { keyError.value = t("settings.keyError.needKey"); return; }
		try {
			await call("add_mouflon_key", {
				pkey,
				pdkey: pdkey || undefined,
				keystream: keystream || undefined,
			});
			newPkey.value = "";
			newPdkey.value = "";
			newKeystream.value = "";
			await loadKeys();
		} catch (e: unknown) {
			keyError.value = String(e);
		}
	}

	async function removeKey(pkey: string) {
		await call("remove_mouflon_key", { pkey });
		await loadKeys();
	}

	async function syncKeys() {
		syncing.value = true;
		try {
			const updated = await call<boolean>("sync_mouflon_keys");
			await loadKeys();
			toast(updated ? t("settings.mouflonSyncDone") : t("settings.mouflonSyncUpToDate"), "success");
		} catch (e: unknown) {
			toast(t("settings.mouflonSyncFailed", { error: String(e) }), "error");
		} finally {
			syncing.value = false;
		}
	}

	function formatTs(ts: string | null): string {
		if (!ts) return t("settings.mouflonNever");
		return new Date(ts).toLocaleString();
	}

	// ── section 追踪 / Section tracking ──────────────────────────────────────
	const SECTION_KEYS = ["language", "recording", "network", "mouflonKeys"] as const;
	type SectionKey = (typeof SECTION_KEYS)[number];
	const activeSection = ref<SectionKey | null>(null);
	const sectionRefs = ref<Partial<Record<SectionKey, HTMLElement>>>({});
	let sectionObserver: IntersectionObserver | null = null;

	function initSectionObserver() {
		if (sectionObserver) sectionObserver.disconnect();
		sectionObserver = new IntersectionObserver(
			(entries) => {
				for (const entry of entries) {
					const key = entry.target.getAttribute("data-section") as SectionKey;
					if (entry.isIntersecting) activeSection.value = key;
				}
			},
			{ rootMargin: "-110px 0px -80% 0px", threshold: 0 },
		);
		for (const key of SECTION_KEYS) {
			const el = sectionRefs.value[key];
			if (el) sectionObserver.observe(el);
		}
	}

	/** 点击分区 pill 平滑滚动到对应 section（立即高亮，observer 随后确认）/
	 *  Scroll smoothly to a section on pill click (highlight at once; observer confirms) */
	function scrollToSection(key: SectionKey) {
		activeSection.value = key;
		sectionRefs.value[key]?.scrollIntoView({ behavior: "smooth", block: "start" });
	}

	watch(() => store.loading, (loading) => {
		if (!loading) nextTick(initSectionObserver);
	}, { immediate: true });
</script>

<template>
	<div class="flex flex-col">
		<!-- sticky 标题区 / Sticky header -->
		<header class="bg-background sticky top-0 z-20 px-6 border-b shrink-0 pt-6 pb-3">
			<div class="flex items-center justify-between gap-4">
				<h1 class="text-xl font-bold">{{ t("settings.title") }}</h1>
				<!-- 自动保存状态指示：保存中转圈、保存过显示对勾 / Auto-save
					status: spinner while saving, check after a successful save -->
				<span
					class="flex items-center gap-1.5 text-xs text-muted-foreground h-4 shrink-0 transition-opacity duration-500"
					:class="{ 'opacity-0': !autoSaving && !autoSaved }"
				>
					<Loader2 v-if="autoSaving" class="size-3.5 animate-spin" />
					<Check
						v-else-if="autoSaved"
						class="size-3.5 text-green-600 dark:text-green-500"
					/>
					{{ autoSaving ? t("settings.saving") : autoSaved ? t("settings.saved") : "" }}
				</span>
			</div>
			<!-- 分区锚点导航 / Section anchor navigation -->
			<nav class="mt-2.5 flex items-center gap-1 overflow-x-auto">
				<button
					v-for="key in SECTION_KEYS"
					:key="key"
					type="button"
					class="shrink-0 rounded-full px-2.5 py-1 text-xs font-medium transition-colors"
					:class="
						activeSection === key
							? 'bg-primary text-primary-foreground'
							: 'text-muted-foreground hover:bg-accent hover:text-foreground'
					"
					@click="scrollToSection(key)"
				>
					{{ t(`settings.sections.${key}`) }}
				</button>
			</nav>
		</header>

		<!-- 左右两栏布局：左侧设置名+描述，右侧控件（移动端 UA 下上下堆叠）/
			Two-column layout: name+description left, control right (stacks on mobile UA) -->
		<div class="flex flex-col gap-5 px-6 pt-5 pb-6">
		<div v-if="store.loading" class="text-muted-foreground">{{ t("settings.loading") }}</div>

		<section v-else class="flex flex-col gap-7">

			<!-- 语言 / Language -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.language = (el as HTMLElement) }"
					data-section="language"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b scroll-mt-28"
				>{{ t("settings.sections.language") }}</h2>
				<div class="flex items-center justify-between gap-4">
					<Label>{{ t("settings.language.label") }}</Label>
					<Select class="shrink-0" :model-value="String(locale)" @update:model-value="(v) => v && setLocale(String(v))">
						<SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
						<SelectContent>
							<SelectItem v-for="loc in localesStore.locales" :key="loc.code" :value="loc.code">
								{{ loc.name }}
							</SelectItem>
						</SelectContent>
					</Select>
				</div>
			</section>

			<!-- 录制设置 / Recording -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.recording = (el as HTMLElement) }"
					data-section="recording"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b scroll-mt-28"
				>{{ t("settings.sections.recording") }}</h2>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.outputDir.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.outputDir.hint") }}</p>
					</div>
					<div :class="isMobile ? 'flex w-full items-center gap-1.5' : 'flex w-96 shrink-0 items-center gap-1.5'">
						<Input
							v-model="form.output_dir"
							:placeholder="t('settings.outputDir.placeholder')"
							autocomplete="off"
							class="flex-1"
							@keyup.enter="saveOutputDir"
							@blur="saveOutputDir"
						/>
						<Button type="button" variant="outline" size="icon" :title="t('settings.outputDir.pick')" @click="browseOutputDir">
							<FolderOpen class="size-4" />
						</Button>
					</div>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.preferredResolution.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.preferredResolution.hint") }}</p>
					</div>
					<Select
						class="shrink-0"
						:model-value="String(form.preferred_resolution)"
						@update:model-value="form.preferred_resolution = Number($event ?? 0)"
					>
						<SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
						<SelectContent>
							<SelectItem value="0">{{ t("settings.preferredResolution.original") }}</SelectItem>
							<SelectItem v-for="res in [2160, 1440, 1080, 720, 540, 480, 360, 240]" :key="res" :value="String(res)">
								{{ res }}p
							</SelectItem>
						</SelectContent>
					</Select>
				</div>

				<div v-if="form.preferred_resolution > 0" class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.resolutionPreference.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.resolutionPreference.hint") }}</p>
					</div>
					<RadioGroup
						:model-value="form.resolution_preference"
						class="flex shrink-0 flex-row gap-4"
						@update:model-value="(v) => v && (form.resolution_preference = v as 'lower' | 'higher')"
					>
						<div v-for="dir in resolutionDirections" :key="dir" class="flex items-center gap-2">
							<RadioGroupItem :id="`resolution-${dir}`" :value="dir" />
							<Label :for="`resolution-${dir}`" class="cursor-pointer">
								{{ t(`settings.resolutionPreference.${dir}`) }}
							</Label>
						</div>
					</RadioGroup>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.preferredCodec.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.preferredCodec.hint") }}</p>
					</div>
					<Select
						class="shrink-0"
						:model-value="codecSelectValue"
						@update:model-value="form.preferred_codec = $event === 'default' ? '' : String($event)"
					>
						<SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
						<SelectContent>
							<SelectItem value="default">{{ t("settings.preferredCodec.default") }}</SelectItem>
							<SelectItem v-for="c in videoCodecs" :key="c.value" :value="c.value">{{ c.label }}</SelectItem>
						</SelectContent>
					</Select>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.recordingDuration.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.recordingDuration.hint") }}</p>
					</div>
					<!-- 时/分/秒单控件：显示 HH:MM:SS，点击弹出三列滚轮选择器 /
						Single H/M/S control: displays HH:MM:SS, opens a three-column
						scroll picker on click -->
					<DurationHmsInput v-model="form.max_recording_duration_secs" class="w-40 shrink-0" />
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.maxPpConcurrent.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.maxPpConcurrent.hint") }}</p>
					</div>
					<NumberField
						:model-value="form.max_pp_concurrent"
						:min="0"
						:max="systemStore.maxPpConcurrentCap > 0 ? systemStore.maxPpConcurrentCap : undefined"
						class="w-40 shrink-0"
						@update:model-value="(v) => v !== undefined && (form.max_pp_concurrent = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
				</div>

				<div class="flex items-center justify-between gap-4">
					<Label>{{ t("settings.pollInterval.label") }}</Label>
					<NumberField
						:model-value="form.poll_interval_secs"
						:min="10" :max="300"
						class="w-40 shrink-0"
						@update:model-value="(v) => v !== undefined && (form.poll_interval_secs = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.maxTmpDirGb.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.maxTmpDirGb.hint") }}</p>
					</div>
					<NumberField
						:model-value="form.max_tmp_dir_gb"
						:min="0" :step="0.5"
						class="w-40 shrink-0"
						@update:model-value="(v) => v !== undefined && (form.max_tmp_dir_gb = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label
							for="check-prerelease"
							:class="isBeta ? 'cursor-default opacity-60' : 'cursor-pointer'"
						>
							{{ t("settings.checkPrerelease.label") }}
						</Label>
						<p class="text-xs text-muted-foreground">
							<!-- 不用 <template v-if>：template 元素嵌在 <p> 里会让解析器吞掉 </p>，
								导致其后整页 DOM 错嵌（span 是合法 phrasing content）/
								No <template v-if> here: a <template> inside <p> makes the parser
								swallow the </p> and mis-nest the rest of the page -->
							<span v-if="isBeta">{{ t("settings.checkPrerelease.hintBeta") }}</span>
							<span v-else>{{ t("settings.checkPrerelease.hint") }}</span>
						</p>
					</div>
					<Switch
						id="check-prerelease"
						class="shrink-0"
						:model-value="isBeta ? true : form.check_prerelease"
						:disabled="isBeta"
						@update:model-value="(v) => !isBeta && (form.check_prerelease = !!v)"
					/>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.ffmpegPreference.label") }}</Label>
						<p class="text-xs text-muted-foreground">
							{{ t("settings.ffmpegPreference.hint") }}
						</p>
						<!-- 选中系统环境变量时提示自动更新被禁用；始终渲染（invisible 占位）避免提示出现时下方内容上跳 / Warn that auto-update is disabled while the system PATH is chosen; always rendered (invisible placeholder) so content below doesn't jump when it appears -->
						<p
							class="text-xs text-amber-600 dark:text-amber-500"
							:class="form.ffmpeg_preference === 0 ? '' : 'invisible'"
							:aria-hidden="form.ffmpeg_preference !== 0"
						>
							{{ t("settings.ffmpegPreference.systemNoAutoUpdate") }}
						</p>
					</div>
					<Select
						:key="ffmpegPrefSelectKey"
						class="shrink-0"
						:model-value="form.ffmpeg_preference == null ? undefined : String(form.ffmpeg_preference)"
						@update:model-value="onFfmpegPreferenceChange"
					>
						<SelectTrigger class="w-48">
							<SelectValue :placeholder="t('settings.ffmpegPreference.unset')" />
						</SelectTrigger>
						<SelectContent>
							<SelectItem value="0">
								{{ t("settings.ffmpegPreference.system") }}
								<!-- 版本放 #extra：仅下拉项显示，不回显到折叠的触发按钮 / Version in #extra: dropdown-only, never echoed into the collapsed trigger -->
								<template #extra>
									<!-- 该位置的 ffmpeg 版本；无 ffmpeg 显示未安装；探测未完成/失败不显示 / The location's ffmpeg version; "not installed" when ffmpeg is absent; nothing while probing or on failure -->
									<span v-if="ffmpegLocations?.system.ffmpeg_version" class="font-mono text-muted-foreground">（{{ ffmpegLocations.system.ffmpeg_version }}）</span>
									<span v-else-if="ffmpegLocations && !ffmpegLocations.system.ffmpeg" class="text-muted-foreground">（{{ t("settings.ffmpegPreference.notInstalled") }}）</span>
								</template>
							</SelectItem>
							<SelectItem value="1">
								{{ t("settings.ffmpegPreference.tools") }}
								<template #extra>
									<span v-if="ffmpegLocations?.tools.ffmpeg_version" class="font-mono text-muted-foreground">（{{ ffmpegLocations.tools.ffmpeg_version }}）</span>
									<span v-else-if="ffmpegLocations && !ffmpegLocations.tools.ffmpeg" class="text-muted-foreground">（{{ t("settings.ffmpegPreference.notInstalled") }}）</span>
								</template>
							</SelectItem>
						</SelectContent>
					</Select>
				</div>

				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label for="ffmpeg-auto-update" class="cursor-pointer">
							{{ t("settings.ffmpegAutoUpdate.label") }}
						</Label>
						<p class="text-xs text-muted-foreground">
							{{ t("settings.ffmpegAutoUpdate.hint") }}
						</p>
					</div>
					<!-- 选用系统 PATH 时 FFmpeg 由用户自管：开关禁用且显示为关（不改存储值，切回 tools 恢复）/ While the system PATH is chosen the user's own FFmpeg is theirs to manage: the switch is disabled and shows off (stored value untouched, restored when switching back to tools) -->
					<Switch
						id="ffmpeg-auto-update"
						class="shrink-0"
						:model-value="form.ffmpeg_preference === 0 ? false : form.ffmpeg_auto_update"
						:disabled="form.ffmpeg_preference === 0"
						@update:model-value="(v) => (form.ffmpeg_auto_update = !!v)"
					/>
				</div>
			</section>

			<!-- 网络 / Network -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.network = (el as HTMLElement) }"
					data-section="network"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b scroll-mt-28"
				>{{ t("settings.sections.network") }}</h2>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.apiProxy.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.apiProxy.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="proxy"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.api_proxy_url"
						:placeholder="t('settings.apiProxy.placeholder')"
						@update:model-value="form.api_proxy_url = $event"
						@commit="saveProxy('api_proxy_url')"
					/>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.scMirror.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.scMirror.hint") }}</p>
					</div>
					<div :class="isMobile ? 'flex w-full items-center gap-1.5' : 'flex w-96 shrink-0 items-center gap-1.5'">
						<Select
							:model-value="form.sc_mirror_scheme"
							class="w-28 shrink-0"
							@update:model-value="(v) => v && (form.sc_mirror_scheme = String(v))"
						>
							<SelectTrigger class="w-28 shrink-0"><SelectValue /></SelectTrigger>
							<SelectContent>
								<SelectItem value="https">https</SelectItem>
								<SelectItem value="http">http</SelectItem>
							</SelectContent>
						</Select>
						<Input
							:model-value="form.sc_mirror_url ?? ''"
							:placeholder="t('settings.scMirror.placeholder')"
							autocomplete="off"
							class="flex-1"
							@update:model-value="form.sc_mirror_url = ($event as string) || null"
							@keyup.enter="saveProxy('sc_mirror_url')"
							@blur="saveProxy('sc_mirror_url')"
						/>
					</div>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.scBrowseProxy.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.scBrowseProxy.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="proxy"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.sc_browse_proxy_url"
						:placeholder="t('settings.scBrowseProxy.placeholder')"
						@update:model-value="form.sc_browse_proxy_url = $event"
						@commit="saveProxy('sc_browse_proxy_url')"
					/>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.scBrowseMirror.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.scBrowseMirror.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="mirror"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.sc_browse_mirror_url"
						:placeholder="t('settings.scBrowseMirror.placeholder')"
						@update:model-value="form.sc_browse_mirror_url = $event"
						@commit="saveProxy('sc_browse_mirror_url')"
					/>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.cdnProxy.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.cdnProxy.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="proxy"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.cdn_proxy_url"
						:placeholder="t('settings.cdnProxy.placeholder')"
						@update:model-value="form.cdn_proxy_url = $event"
						@commit="saveProxy('cdn_proxy_url')"
					/>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.communityProxy.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.communityProxy.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="proxy"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.community_proxy_url"
						:placeholder="t('settings.communityProxy.placeholder')"
						@update:model-value="form.community_proxy_url = $event"
						@commit="saveProxy('community_proxy_url')"
					/>
				</div>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.communityMirror.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.communityMirror.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="mirror"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.community_mirror_url"
						:placeholder="t('settings.communityMirror.placeholder')"
						@update:model-value="form.community_mirror_url = $event"
						@commit="saveProxy('community_mirror_url')"
					/>
				</div>
				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label>{{ t("settings.cgfProxy.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.cgfProxy.hint") }}</p>
					</div>
					<SchemeUrlInput
						kind="proxy"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						:model-value="form.cgf_proxy_url"
						:placeholder="t('settings.cgfProxy.placeholder')"
						@update:model-value="form.cgf_proxy_url = $event"
						@commit="saveProxy('cgf_proxy_url')"
					/>
				</div>
				<div class="flex items-center justify-between gap-4">
					<div class="flex min-w-0 flex-col gap-0.5">
						<Label for="ech-enabled" class="cursor-pointer">{{ t("settings.echEnabled.label") }}</Label>
						<p class="text-xs text-muted-foreground">{{ t("settings.echEnabled.hint") }}</p>
					</div>
					<Switch
						id="ech-enabled"
						class="shrink-0"
						:model-value="form.ech_enabled"
						@update:model-value="(v) => (form.ech_enabled = !!v)"
					/>
				</div>
			</section>

			<!-- Mouflon Keys -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.mouflonKeys = (el as HTMLElement) }"
					data-section="mouflonKeys"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b scroll-mt-28"
				>{{ t("settings.sections.mouflonKeys") }}</h2>
				<p class="text-xs text-muted-foreground leading-relaxed">
					{{ t("settings.mouflonKeysDesc") }}
					<code class="bg-muted px-1 py-0.5 rounded text-xs font-mono">pkey → pdkey</code>
				</p>

				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<Label class="min-w-0 flex-1">{{ t("settings.mouflonSyncUrl.label") }}</Label>
					<Input
						:model-value="form.mouflon_sync_url ?? ''"
						:placeholder="t('settings.mouflonSyncUrl.placeholder')"
						autocomplete="url"
						:class="isMobile ? 'w-full' : 'w-96 shrink-0'"
						@update:model-value="form.mouflon_sync_url = ($event as string) || null"
						@keyup.enter="saveProxy('mouflon_sync_url')"
						@blur="saveProxy('mouflon_sync_url')"
					/>
				</div>
				<div :class="isMobile ? 'flex flex-col gap-1.5' : 'flex items-center justify-between gap-6'">
					<Label class="min-w-0 flex-1">{{ t("settings.mouflonSyncToken.label") }}</Label>
					<div :class="isMobile ? 'flex w-full items-center gap-1.5' : 'flex w-96 shrink-0 items-center gap-1.5'">
						<Input
							:model-value="form.mouflon_sync_token ?? ''"
							:placeholder="t('settings.mouflonSyncToken.placeholder')"
							:type="showToken ? 'text' : 'password'"
							autocomplete="off"
							class="flex-1"
							@update:model-value="form.mouflon_sync_token = ($event as string) || null"
							@keyup.enter="saveProxy('mouflon_sync_token')"
							@blur="saveProxy('mouflon_sync_token')"
						/>
						<Button
							type="button"
							variant="outline"
							size="icon"
							:title="showToken ? t('common.hide') : t('common.show')"
							@click="showToken = !showToken"
						>
							<EyeOff v-if="showToken" class="size-4" />
							<Eye v-else class="size-4" />
						</Button>
					</div>
				</div>

				<div class="flex items-center justify-between gap-4 text-xs text-muted-foreground">
					<div class="flex flex-col gap-0.5">
						<span>{{ t("settings.mouflonAutoSyncedAt") }}{{ formatTs(mouflonStore.auto_synced_at) }}</span>
						<span>{{ t("settings.mouflonManualUpdatedAt") }}{{ formatTs(mouflonStore.manual_updated_at) }}</span>
					</div>
					<Button type="button" variant="outline" size="sm" :disabled="syncing || !form.mouflon_sync_url" @click="syncKeys">
						{{ syncing ? t("settings.mouflonSyncing") : t("settings.mouflonSync") }}
					</Button>
				</div>

				<table v-if="mouflonRows.length" class="w-full text-xs border-collapse">
					<thead>
						<tr>
							<th class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold">{{ t("settings.mouflonTable.pkey") }}</th>
							<th class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold">{{ t("settings.mouflonTable.pdkey") }}</th>
							<th class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold">{{ t("settings.mouflonTable.keystream") }}</th>
							<th class="border-b"></th>
						</tr>
					</thead>
					<tbody>
						<tr v-for="row in mouflonRows" :key="row.pkey">
							<td class="px-2 py-1.5 border-b font-mono">{{ row.pkey }}</td>
							<td class="px-2 py-1.5 border-b font-mono max-w-48 truncate" :title="row.pdkey">{{ row.pdkey || "-" }}</td>
							<td class="px-2 py-1.5 border-b font-mono max-w-48 truncate" :title="row.keystream">{{ row.keystream || "-" }}</td>
							<td class="px-2 py-1.5 border-b">
								<Button type="button" variant="destructive" size="sm" class="h-6 text-xs px-2" @click="removeKey(row.pkey)">
									{{ t("common.delete") }}
								</Button>
							</td>
						</tr>
					</tbody>
				</table>
				<p v-else class="text-xs text-muted-foreground">{{ t("settings.noKeys") }}</p>

				<!-- flex-wrap：窄屏时输入框/按钮换行防横向溢出 / flex-wrap lets inputs and the button wrap on narrow screens to prevent horizontal overflow -->
				<div class="flex flex-wrap gap-2 items-center">
					<Input v-model="newPkey" placeholder="pkey" autocomplete="off" class="min-w-24 flex-1 font-mono text-xs" />
					<Input v-model="newPdkey" placeholder="pdkey" autocomplete="off" class="min-w-24 flex-2 font-mono text-xs" />
					<Input v-model="newKeystream" placeholder="keystream (hex)" autocomplete="off" class="min-w-24 flex-2 font-mono text-xs" />
					<Button type="button" variant="outline" @click="addKey">{{ t("settings.addKey") }}</Button>
				</div>
				<p class="text-xs text-muted-foreground">{{ t("settings.mouflonKeystreamHint") }}</p>
				<p v-if="keyError" class="text-xs text-destructive">{{ keyError }}</p>
			</section>
		</section>
		</div>
	</div>
</template>

