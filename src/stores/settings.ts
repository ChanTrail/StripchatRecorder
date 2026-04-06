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
	/** 录制文件输出目录 / Recording output directory */
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
	/** 最大并发录制数（0 = 不限制）/ Max concurrent recordings (0 = unlimited) */
	max_concurrent: number;
	/** 录制片段合并格式（"mp4" 或 "mkv"）/ Recording segment merge format ("mp4" or "mkv") */
	merge_format: string;
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
		max_concurrent: 0,
		merge_format: "mp4",
	});
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
	 * 打开系统目录选择对话框（仅 Tauri 模式支持）。
	 * Open the system directory picker dialog (Tauri mode only).
	 *
	 * @returns 选择的目录路径，取消则返回 null / Selected directory path, or null if cancelled
	 */
	async function pickOutputDir(): Promise<string | null> {
		return call<string | null>("pick_output_dir");
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
		});
	}

	return {
		settings,
		loading,
		saved,
		isSavingLocally,
		fetchSettings,
		saveSettings,
		pickOutputDir,
		initListeners,
	};
});
