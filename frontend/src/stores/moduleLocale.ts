/**
 * 模块翻译状态管理 Store / Module Translation State Management Store
 *
 * 存储从服务器 /api/locale/{code} 接口加载的模块翻译覆盖数据。
 * postprocess store 使用此数据（而非 --describe 中的 i18n 字段）来翻译模块名称、
 * 描述和参数标签，允许用户在 locale/modules/<id>/{code}.json 中自定义翻译。
 *
 * Stores module translation override data loaded from the server's /api/locale/{code} endpoint.
 * The postprocess store uses this data (instead of the --describe i18n field) to translate
 * module names, descriptions, and parameter labels, allowing users to customize translations
 * in locale/modules/<id>/{code}.json.
 */

import { defineStore } from "pinia";
import { ref } from "vue";

/** 单个模块的翻译数据 / Translation data for a single module */
export interface ModuleLocaleData {
	/** 翻译后的模块名称 / Translated module name */
	name?: string;
	/** 翻译后的模块描述 / Translated module description */
	description?: string;
	/** 参数翻译（key -> {label}）/ Parameter translations (key -> {label}) */
	params?: Record<string, { label?: string }>;
}

export const useModuleLocaleStore = defineStore("moduleLocale", () => {
	/**
	 * 当前语言下的模块翻译映射：moduleId -> 翻译数据
	 * Module translation map for the current locale: moduleId -> translation data
	 */
	const locales = ref<Record<string, ModuleLocaleData>>({});

	/** 当前加载的语言代码 / Currently loaded locale code */
	const currentLocale = ref<string>("");

	/**
	 * 设置指定语言的模块翻译数据（由 App.vue 在加载 locale JSON 后调用）。
	 * Set module translation data for the given locale (called by App.vue after loading locale JSON).
	 *
	 * @param localeCode - BCP 47 语言标签 / BCP 47 language tag
	 * @param data - 模块翻译数据映射 / Module translation data map
	 */
	function setLocales(
		localeCode: string,
		data: Record<string, unknown>,
	) {
		currentLocale.value = localeCode;
		locales.value = data as Record<string, ModuleLocaleData>;
	}

	/**
	 * 获取指定模块在当前语言下的翻译数据。
	 * Get the translation data for a specific module in the current locale.
	 *
	 * @param moduleId - 模块唯一 ID / Module unique ID
	 * @returns 翻译数据，若不存在则返回 undefined
	 *          Translation data, or undefined if not found
	 */
	function getModuleLocale(moduleId: string): ModuleLocaleData | undefined {
		return locales.value[moduleId];
	}

	return {
		locales,
		currentLocale,
		setLocales,
		getModuleLocale,
	};
});
