/**
 * i18n 初始化模块（桌面版）/ i18n Initialization Module (Desktop)
 *
 * 与服务器版的区别：通过 Tauri invoke 而非 fetch 加载 locale 数据。
 * Difference from server version: loads locale data via Tauri invoke instead of fetch.
 */

import { createI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import zhCN from "./locales/zh-CN";
import enUS from "./locales/en-US";

export type MessageSchema = typeof zhCN;

const savedLocale = "zh-CN";

/** 可用语言条目 / Available locale entry */
export interface LocaleEntry {
	code: string;
	name: string;
}

/**
 * 获取服务器支持的语言列表（通过 Tauri invoke）。
 * Fetch available locales via Tauri invoke.
 */
export async function fetchAvailableLocales(): Promise<LocaleEntry[]> {
	try {
		const data = await invoke<LocaleEntry[]>("list_locales");
		if (Array.isArray(data) && data.length > 0) return data;
		return builtinLocales();
	} catch {
		return builtinLocales();
	}
}

function builtinLocales(): LocaleEntry[] {
	return [
		{ code: "zh-CN", name: "简体中文" },
		{ code: "en-US", name: "English" },
	];
}

export interface LoadLocaleResult {
	modules: Record<string, unknown>;
	warning?: string;
}

/**
 * 从后端加载指定语言的完整 locale 数据（通过 Tauri invoke）。
 * Load the full locale data for the given locale code via Tauri invoke.
 */
export async function loadLocaleFromServer(
	localeCode: string,
): Promise<LoadLocaleResult> {
	try {
		const data = await invoke<Record<string, unknown>>("get_locale", {
			localeCode,
		});

		if (data.app && typeof data.app === "object") {
			if (!i18n.global.availableLocales.includes(localeCode as never)) {
				i18n.global.setLocaleMessage(localeCode as never, data.app as Record<string, unknown>);
			} else {
				i18n.global.mergeLocaleMessage(localeCode as never, data.app as Record<string, unknown>);
			}
		}

		return {
			modules: (data.modules as Record<string, unknown>) ?? {},
			warning: typeof data.warning === "string" ? data.warning : undefined,
		};
	} catch {
		return { modules: {} };
	}
}

const i18n = createI18n<false>({
	legacy: false,
	locale: savedLocale,
	fallbackLocale: "zh-CN",
	messages: {
		"zh-CN": zhCN,
		"en-US": enUS,
	},
});

export default i18n;
