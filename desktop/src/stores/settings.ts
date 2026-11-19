/**
 * 应用设置状态管理 Store / Application Settings State Management Store
 *
 * 管理录制器的全局配置，包括输出目录、轮询间隔、代理设置、并发数和合并格式。
 * 支持多客户端实时同步：当其他客户端修改设置时，通过事件自动更新本地状态。
 *
 * Manages global recorder configuration including output directory, poll interval,
 * proxy settings, concurrency, and merge format.
 * Supports real-time multi-client sync: automatically updates local state when
 * other clients modify settings via events.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { call, on } from "@/lib/api";

/** 应用设置数据结构 / Application settings data structure */
export interface Settings {
	/** TS 分片流输出目录 / TS segment stream output directory */
	output_dir: string;
	/** 主播状态轮询间隔（秒）/ Streamer status poll interval (seconds) */
	poll_interval_secs: number;
	/** 是否默认开启自动录制 / Whether auto-record is enabled by default */
	auto_record: boolean;
	/** Stripchat API 代理地址 / Stripchat API proxy URL */
	api_proxy_url: string | null;
	/** CDN 缩略图代理地址 / CDN thumbnail proxy URL */
	cdn_proxy_url: string | null;
	/** Stripchat 镜像站地址 / Stripchat mirror site URL */
	sc_mirror_url: string | null;
	/** Stripchat 镜像站协议（"https" 或 "http"）/ Mirror site scheme ("https" or "http") */
	sc_mirror_scheme: string;
	/** StripChat 浏览页专用代理（与 api_proxy_url 相互独立）/ Dedicated proxy for the StripChat browse page (independent from api_proxy_url) */
	sc_browse_proxy_url: string | null;
	/** StripChat 浏览页专用镜像站（与 sc_mirror_url 相互独立）/ Dedicated mirror for the StripChat browse page (independent from sc_mirror_url) */
	sc_browse_mirror_url: string | null;
	/** 每文件录制时长上限（秒，0 = 不限制）/ Per-file recording duration in seconds (0 = unlimited) */
	max_recording_duration_secs: number;
	/** 首选录制分辨率高度（0 = 原始/最高画质）/ Preferred recording resolution height (0 = original/highest quality) */
	preferred_resolution: number;
	/** 首选分辨率缺失时的回退方向 / Fallback direction when preferred resolution is unavailable */
	resolution_preference: "lower" | "higher";
	/** 首选视频编码（"h264" / "H265" / "AV1"，空串 = 不指定）/ Preferred video codec ("h264" / "H265" / "AV1", empty = unspecified) */
	preferred_codec: string;
	/** 后处理 tmp 目录最大占用（GB，0 = 不限制）/ Max tmp dir size in GB (0 = unlimited) */
	max_tmp_dir_gb: number;
	/** 界面语言 / UI language */
	language: string;
	/** Mouflon Keys 同步 URL / Mouflon Keys sync URL */
	mouflon_sync_url: string | null;
	/** Mouflon Keys 同步鉴权 Token / Mouflon Keys sync auth token */
	mouflon_sync_token: string | null;
	/** 社区模块代理地址 / Community module proxy URL */
	community_proxy_url: string | null;
	/** 社区模块镜像站地址 / Community module mirror URL */
	community_mirror_url: string | null;
	/** 用户是否已接受社区模块使用条款 / Whether the user has accepted the community module terms */
	community_terms_accepted: boolean;
	/** 首次启动向导是否已完成 / Whether the first-launch setup wizard has been completed */
	setup_done: boolean;
	/** 后处理最大并发数（0 = 自动 = CPU 逻辑核心数 × 2；≥1 = 固定，上限 CPU × 2）/ Max concurrent post-processing tasks (0 = auto = logical CPU count × 2; ≥1 = fixed, capped at CPU × 2) */
	max_pp_concurrent: number;
	/** 是否检查预发布（beta/rc）版本更新 / Whether to check for pre-release (beta/rc) updates */
	check_prerelease: boolean;
	/** 是否在空闲时自动更新下载到 tools 的 FFmpeg（默认开启）/ Whether to auto-update the FFmpeg copy downloaded into tools while idle (on by default) */
	ffmpeg_auto_update: boolean;
	/** FFmpeg 首选项（0 = 系统环境变量，1 = tools 文件夹，null = 未设置 → 前端弹出首选项向导）/ FFmpeg preference (0 = system PATH, 1 = tools folder, null = unset → frontend shows the preference wizard) */
	ffmpeg_preference: number | null;
	/** camgirlfinder 代理地址 / camgirlfinder proxy URL */
	cgf_proxy_url: string | null;
	/** 是否启用 ECH（SC API/CDN 请求的 TLS ClientHello 加密）/ Whether ECH is enabled (TLS ClientHello encryption for SC API/CDN requests) */
	ech_enabled: boolean;
}

/** Mouflon 密钥存储结构（含时间戳）/ Mouflon key store (with timestamps) */
export interface MouflonKeysStore {
	/** pkey -> pdkey 密钥对 / pkey -> pdkey key pairs */
	keys: Record<string, string>;
	/** pkey -> keystream（十六进制），没有 pdkey 时用它直接解密 / pkey -> keystream (hex), used directly when there is no pdkey */
	keystreams: Record<string, string>;
	/** 最近一次同步时数据源返回的内容哈希（仅用于比对是否变化）/ Content hash returned by the source at the last sync (comparison only) */
	synced_hash: string | null;
	/** 最近一次自动同步写入变更的本地时间（RFC 3339）/ Local time of the last auto-sync that applied a change (RFC 3339) */
	auto_synced_at: string | null;
	/** 最近一次手动操作时间（RFC 3339）/ Timestamp of last manual key change (RFC 3339) */
	manual_updated_at: string | null;
}

export const useSettingsStore = defineStore("settings", () => {
	/** 当前设置值 / Current settings values */
	const settings = ref<Settings>({
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
		resolution_preference: "lower",
		preferred_codec: "",
		max_tmp_dir_gb: 50,
		language: "zh-CN",
		mouflon_sync_url: null,
		mouflon_sync_token: null,
		community_proxy_url: null,
		community_mirror_url: null,
		community_terms_accepted: false,
		setup_done: false,
		max_pp_concurrent: 0,
		check_prerelease: false,
		ffmpeg_auto_update: true,
		ffmpeg_preference: null,
		cgf_proxy_url: null,
		ech_enabled: false,
	});
	/** 设置是否已从后端成功加载（区分"未加载"与"已加载但取默认值"）/ Whether settings have been loaded from the backend (distinguishes "not loaded" from "loaded with defaults") */
	const loaded = ref(false);
	/** 是否正在加载 / Whether loading */
	const loading = ref(false);
	/** 保存成功后短暂显示的状态标志 / Flag briefly set to true after successful save */
	const saved = ref(false);
	/** 是否正在本地保存（用于过滤自身触发的 settings-updated 事件）/ Whether saving locally (to filter self-triggered settings-updated events) */
	const isSavingLocally = ref(false);
	/** 事件监听器是否已初始化（防止重复注册）/ Whether event listeners are initialized (prevents duplicate registration) */
	let listenersReady = false;

	/**
	 * 从后端获取当前设置。
	 * Fetch current settings from the backend.
	 */
	async function fetchSettings() {
		loading.value = true;
		try {
			settings.value = await call<Settings>("get_settings");
			loaded.value = true;
		} finally {
			loading.value = false;
		}
	}

	/**
	 * 保存设置到后端，并在 2 秒内显示保存成功状态。
	 * Save settings to the backend and briefly show a saved indicator for 2 seconds.
	 *
	 * @param s - 要保存的设置对象 / Settings object to save
	 */
	async function saveSettings(s: Settings) {
		isSavingLocally.value = true;
		try {
			await call("save_settings_cmd", { newSettings: s });
			settings.value = s;
			saved.value = true;
			setTimeout(() => (saved.value = false), 2000);
		} finally {
			// 延迟 500ms 后清除本地保存标志，确保事件过滤窗口足够
			// Clear local saving flag after 500ms to ensure event filter window is sufficient
			setTimeout(() => {
				isSavingLocally.value = false;
			}, 500);
		}
	}

	/**
	 * 初始化设置更新事件监听器（只执行一次）。
	 * Initialize settings update event listener (executed only once).
	 */
	async function initListeners() {
		if (listenersReady) return;
		listenersReady = true;
		await on("settings-updated", (payload) => {
			// 本地保存时忽略自身触发的事件 / Ignore self-triggered events during local save
			if (isSavingLocally.value) return;
			settings.value = payload as Settings;
			loaded.value = true;
		});
	}

	return {
		settings,
		loaded,
		loading,
		saved,
		isSavingLocally,
		fetchSettings,
		saveSettings,
		initListeners,
	};
});
