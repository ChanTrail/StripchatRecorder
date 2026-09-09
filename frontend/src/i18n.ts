/**
 * i18n 初始化模块 / i18n Initialization Module
 *
 * 翻译数据完全来自后端 /api/locale/{code}（读取 <exe_dir>/locale/app/{code}.json）。
 * 前端不内置任何 fallback 消息，启动时同步阻塞加载后才挂载 Vue。
 *
 * All translation data comes from the backend /api/locale/{code}.
 * No built-in fallback messages in the frontend; Vue is mounted only after
 * the locale data is loaded synchronously at startup.
 */

import { createI18n } from "vue-i18n";

// MessageSchema 从后端 JSON 的结构推导，运行时类型安全
// MessageSchema derived from backend JSON structure for runtime type safety
export type MessageSchema = Record<string, unknown>;

const savedLocale = localStorage.getItem("locale") || "zh-CN";

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

/** 内置语言列表（/api/locales 不可用时的备用）/ Fallback locale list when /api/locales is unavailable */
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
 * 从后端 API 获取指定语言的完整 locale 数据并注册到 vue-i18n。
 *
 * Fetch the full locale data from the backend for the given locale code
 * and register it in vue-i18n.
 *
 * @param localeCode - BCP 47 语言标签 / BCP 47 language tag
 * @returns LoadLocaleResult，失败时 modules 为空对象 / modules is {} on failure
 */
export async function loadLocaleFromServer(
	localeCode: string,
): Promise<LoadLocaleResult> {
	try {
		const res = await fetch(`/api/locale/${encodeURIComponent(localeCode)}`);
		if (!res.ok) return { modules: {} };
		const data = await res.json();

		if (data.app && typeof data.app === "object") {
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

// vue-i18n 实例（空消息，由 initI18n 填充）
// vue-i18n instance with empty messages, populated by initI18n()
const i18n = createI18n<false>({
	legacy: false,
	locale: savedLocale,
	fallbackLocale: false,
	messages: {},
	missing: (_locale, key) => key, // 键缺失时直接返回键名，避免控制台警告
});

/**
 * 在 Vue 挂载前调用：从后端加载当前语言数据。
 *
 * 语言优先级：
 * 1. 后端 /api/settings 的 language 字段（无需登录，即使缓存被清空也能获取正确语言）
 * 2. localStorage 缓存值（后端请求失败时使用）
 * 3. 硬编码 fallback "zh-CN"
 *
 * 取到后端语言后同步写入 localStorage，保持一致。
 *
 * Call before Vue mounts: load locale data from the backend.
 *
 * Language priority:
 * 1. backend /api/settings language field (no auth needed; survives cache clear)
 * 2. localStorage cached value (fallback when backend request fails)
 * 3. hardcoded fallback "zh-CN"
 *
 * The resolved locale is written back to localStorage to keep it in sync.
 */
export async function initI18n(): Promise<void> {
	let resolvedLocale = savedLocale; // localStorage 或 "zh-CN" / localStorage or "zh-CN"

	// 尝试从后端读取语言设置（/api/settings 不需要 token，setup/login 阶段也可用）
	// Try reading language from backend (/api/settings needs no token, works during setup/login)
	try {
		const res = await fetch("/api/settings");
		if (res.ok) {
			const data = await res.json() as { language?: string };
			if (data.language && typeof data.language === "string") {
				resolvedLocale = data.language;
				// 同步写回 localStorage，后续切换语言时仍能读到正确值
				// Write back to localStorage so subsequent locale switches read the right value
				localStorage.setItem("locale", resolvedLocale);
			}
		}
	} catch {
		// 后端未就绪（冷启动）时静默使用 localStorage/fallback
		// Backend not ready (cold start): silently use localStorage/fallback
	}

	await loadLocaleFromServer(resolvedLocale);
	i18n.global.locale.value = resolvedLocale as never;
}

export default i18n;
