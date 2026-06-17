/**
 * 可用语言列表 Store / Available Locales Store
 *
 * 集中管理从后端获取的可用语言列表，供 SetupView、SettingsView 共享。
 * store 初始化时自动订阅 `locale-files-changed` 事件，文件变化后立即刷新。
 *
 * Centrally manages the available locale list fetched from the backend,
 * shared by SetupView and SettingsView.
 * Automatically subscribes to `locale-files-changed` on init to refresh on file changes.
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
	 * 从后端拉取最新的可用语言列表。
	 * Fetch the latest available locale list from the backend.
	 */
	async function refresh() {
		locales.value = await fetchAvailableLocales();
		loaded.value = true;
	}

	// store 创建时立即订阅文件变化事件，不依赖 App.vue 的 onMounted 时序
	// Subscribe to file change events when store is created, independent of App.vue's onMounted timing
	on("locale-files-changed", () => {
		refresh();
	});

	return { locales, loaded, refresh };
});
