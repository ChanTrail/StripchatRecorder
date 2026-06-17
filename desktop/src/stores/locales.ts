/**
 * 可用语言列表 Store / Available Locales Store
 *
 * 集中管理从后端获取的可用语言列表，供 SetupView、SettingsView 共享。
 * 需要在 App.vue 的 onMounted 里调用 setup() 来完成事件监听注册。
 *
 * Centrally manages the available locale list fetched from the backend,
 * shared by SetupView and SettingsView.
 * Call setup() in App.vue's onMounted to register event listeners properly.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { fetchAvailableLocales, type LocaleEntry } from "@/i18n";
import { on } from "@/lib/api";

export const useLocalesStore = defineStore("locales", () => {
	/** 可用语言列表 / Available locale list */
	const locales = ref<LocaleEntry[]>([]);
	/** 是否已完成首次加载 / Whether the initial load has completed */
	const loaded = ref(false);

	/**
	 * 从后端拉取最新的可用语言列表，有变化时才更新。
	 * Fetch the latest available locale list; only update if it changed.
	 */
	async function refresh() {
		const latest = await fetchAvailableLocales();
		const latestJson = JSON.stringify(latest.map((l) => l.code).sort());
		const currentJson = JSON.stringify(locales.value.map((l) => l.code).sort());
		if (latestJson !== currentJson) {
			locales.value = latest;
		}
		loaded.value = true;
	}

	/**
	 * 注册事件监听。必须在 App.vue 的 onMounted 里 await 调用，
	 * 确保 Tauri webview 已就绪（desktop 模式）或 SSE 已连接（server 模式）。
	 *
	 * Register event listeners. Must be awaited in App.vue's onMounted
	 * to ensure Tauri webview is ready (desktop) or SSE is connected (server).
	 */
	async function setupListeners() {
		await on("locale-files-changed", () => {
			refresh();
		});
	}

	return { locales, loaded, refresh, setupListeners };
});
