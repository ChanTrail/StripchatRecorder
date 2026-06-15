/**
 * i18n 初始化模块 / i18n Initialization Module
 *
 * 语言数据加载优先级：
 * 1. 从后端 /api/locale/{code} 加载（读取 <exe_dir>/locale/app/{code}.json），允许用户自定义覆盖
 * 2. 内置 TS 翻译（zh-CN / en-US）作为初始消息和 fallback，确保首屏不闪烁
 *
 * 可用语言列表通过 /api/locales 动态获取，无需修改前端代码即可支持新语言。
 *
 * Locale data loading priority:
 * 1. Load from backend /api/locale/{code}, allowing user customization
 * 2. Built-in TS translations (zh-CN / en-US) as initial messages and fallback
 *
 * Available locale list is fetched dynamically from /api/locales —
 * adding a new language requires no frontend code changes.
 */

import { createI18n } from "vue-i18n";
import zhCN from "./locales/zh-CN";
import enUS from "./locales/en-US";

export type MessageSchema = typeof zhCN;

const savedLocale = "zh-CN"; // 初始值，启动后由 App.vue 从后端 settings 同步覆盖

/** 可用语言条目（从 /api/locales 获取）/ Available locale entry (from /api/locales) */
export interface LocaleEntry {
	/** BCP 47 语言代码 / BCP 47 locale code */
	code: string;
	/** 该语言的自身显示名称 / Native display name */
	name: string;
}

/**
 * 获取服务器支持的语言列表。
 * Fetch the list of available locales from the server.
 */
export async function fetchAvailableLocales(): Promise<LocaleEntry[]> {
	try {
		const res = await fetch("/api/locales");
		if (!res.ok) return builtinLocales();
		const data = await res.json();
		if (Array.isArray(data) && data.length > 0) return data as LocaleEntry[];
		return builtinLocales();
	} catch {
		return builtinLocales();
	}
}

/** 内置 fallback 语言列表（后端不可用时使用）/ Built-in fallback locale list */
function builtinLocales(): LocaleEntry[] {
	return [
		{ code: "zh-CN", name: "简体中文" },
		{ code: "en-US", name: "English" },
	];
}

/** 加载 locale 的返回结果 / Result of loading a locale */
export interface LoadLocaleResult {
	/** 模块翻译数据映射（moduleId -> {name, description, params}）/ Module translation map */
	modules: Record<string, unknown>;
	/**
	 * 若语言文件存在但校验失败，此字段为错误描述；否则为 undefined。
	 * Set when the locale file exists but fails validation; otherwise undefined.
	 */
	warning?: string;
}

/**
 * 从后端 API 获取指定语言的完整 locale 数据，
 * 动态注册到 vue-i18n（若尚未注册），并深度合并覆盖内置消息。
 * 同时返回模块翻译数据和可能的文件校验警告。
 *
 * Fetch the full locale data from the backend for the given locale code,
 * dynamically register it in vue-i18n if not already registered,
 * and deep-merge to override built-in messages.
 * Returns module translation overrides and any file validation warning.
 *
 * @param localeCode - BCP 47 语言标签 / BCP 47 language tag
 * @returns LoadLocaleResult，失败时 modules 为空对象 / LoadLocaleResult, modules is {} on failure
 */
export async function loadLocaleFromServer(
	localeCode: string,
): Promise<LoadLocaleResult> {
	try {
		const res = await fetch(`/api/locale/${encodeURIComponent(localeCode)}`);
		if (!res.ok) return { modules: {} };
		const data = await res.json();

		if (data.app && typeof data.app === "object") {
			// 若 vue-i18n 尚未注册该语言则先用空对象注册，再合并
			// Register with empty object first if locale not yet known, then merge
			if (!i18n.global.availableLocales.includes(localeCode as never)) {
				i18n.global.setLocaleMessage(localeCode as never, data.app);
			} else {
				i18n.global.mergeLocaleMessage(localeCode as never, data.app);
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

// vue-i18n 实例：用宽泛的 string 类型避免硬编码语言列表，
// 内置 zh-CN / en-US 作为初始消息确保首屏无闪烁。
//
// vue-i18n instance: use loose string type to avoid hardcoding locale list.
// Built-in zh-CN / en-US provide initial messages to prevent first-frame flash.
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
