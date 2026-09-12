<!--
    应用根组件 / Application Root Component

    提供侧边栏导航和主内容区域的整体布局。
    负责：
    - 跟随系统主题自动切换深色/浅色模式
    - 监听 ffmpeg-missing 事件并显示警告
    - 监听 SSE 断开/重连事件，重连后自动刷新页面
    - 监听 startup-warnings 事件，处理不存在的主播和孤立的后处理记录
    - 窄屏（<768px）下用底部标签栏替代侧边栏；"主播查找" 折叠进"主播列表"标签，
      通过一个仅在移动端显示的分段控件切换

    Provides the overall layout with sidebar navigation and main content area.
    Responsible for:
    - Auto dark/light mode following system theme
    - Listening for ffmpeg-missing events and showing warnings
    - Listening for SSE disconnect/reconnect events, auto-reloading on reconnect
    - Listening for startup-warnings to handle non-existent streamers and orphaned post-processing records
    - Below 768px, a bottom tab bar replaces the sidebar; Finder folds into the Streamers tab
      via a mobile-only segmented control
-->
<script setup lang="ts">
	import { computed, onMounted, onUnmounted, ref } from "vue";
	import { RouterView, useRouter, useRoute } from "vue-router";
	import NotifyLayer from "./components/NotifyLayer.vue";
	import { Button } from "@/components/ui/button";
	import { Users, Video, Wand2, Radio, Search, Settings } from "@lucide/vue";
	import { call, on, onSseReconnect, onSseDisconnect } from "@/lib/api";
	import { useNotify } from "@/composables/useNotify";
	import { toast as sonnerToast } from "vue-sonner";
	import { useStreamersStore } from "@/stores/streamers";
	import { useI18n } from "vue-i18n";
	import { useScrollbar } from "@/composables/useScrollbar";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";

	const router = useRouter();
	const route = useRoute();
	const { toast, confirm } = useNotify();
	const streamersStore = useStreamersStore();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();

	const mainScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(mainScrollEl);

	async function setLocale(lang: string) {
		const { modules: moduleLocales } = await loadLocaleFromServer(lang);
		locale.value = lang;
		moduleLocaleStore.setLocales(lang, moduleLocales);
		try {
			const settings = await call<Record<string, unknown>>("get_settings");
			await call("save_settings_cmd", { newSettings: { ...settings, language: lang } });
		} catch { /* non-critical */ }
	}

	/** 侧边栏导航项配置 / Sidebar navigation items configuration */
	const navItems = [
		{ to: "/", labelKey: "nav.streamers", icon: Users },
		{ to: "/recordings", labelKey: "nav.recordings", icon: Video },
		{ to: "/postprocess", labelKey: "nav.postprocess", icon: Wand2 },
		{ to: "/relay", labelKey: "nav.relay", icon: Radio },
		{ to: "/finder", labelKey: "nav.finder", icon: Search },
		{ to: "/settings", labelKey: "nav.settings", icon: Settings },
	];

	/**
	 * 底部标签栏项目（移动端）：不含"主播查找"，它折叠进"主播列表"标签。
	 * Bottom tab bar items (mobile): excludes Finder, which folds into the Streamers tab.
	 */
	const bottomNavItems = navItems.filter((item) => item.to !== "/finder");

	/**
	 * 判断底部标签是否处于激活状态。"主播列表"标签在 /finder 路由下也视为激活，
	 * 因为主播查找是折叠进该标签的子视图。
	 *
	 * Whether a bottom tab is active. The Streamers tab is also considered active on the
	 * /finder route, since Finder is a folded-in sub-view of that tab.
	 */
	function isTabActive(to: string): boolean {
		if (to === "/") return route.path === "/" || route.path === "/finder";
		return route.path === to;
	}

	/** 是否显示"主播列表 / 主播查找"分段控件（仅这两个路由下）/ Whether to show the Streamers/Finder segmented control */
	const showStreamersSubnav = computed(
		() => route.path === "/" || route.path === "/finder",
	);

	/**
	 * 根据参数切换文档根元素的 dark 类，实现深色/浅色主题切换。
	 * Toggle the dark class on the document root element for dark/light theme switching.
	 *
	 * @param dark - 是否应用深色主题 / Whether to apply dark theme
	 */
	function applyTheme(dark: boolean) {
		document.documentElement.classList.toggle("dark", dark);
	}

	// 监听系统主题变化 / Listen for system theme changes
	const mq = window.matchMedia("(prefers-color-scheme: dark)");
	function onThemeChange(e: MediaQueryListEvent) {
		applyTheme(e.matches);
	}

	// 事件取消订阅函数 / Event unsubscribe functions
	let unlistenFfmpeg: (() => void) | null = null;
	let unlistenReconnect: (() => void) | null = null;
	let unlistenDisconnect: (() => void) | null = null;
	let unlistenWarnings: (() => void) | null = null;
	let unlistenLocaleWarnings: (() => void) | null = null;

	/**
	 * 处理启动时的警告事件：
	 * 1. 不存在的主播账号 -> 提示用户并自动删除
	 * 2. 孤立的后处理记录（对应文件已删除）-> 提示用户并清理
	 *
	 * Handle startup warning events:
	 * 1. Non-existent streamer accounts -> prompt user and auto-delete
	 * 2. Orphaned post-processing records (files deleted) -> prompt user and clean up
	 */
	async function handleStartupWarnings(payload: unknown) {
		const w = payload as {
			missing_streamers: string[];
			missing_pp_results: string[];
		};

		if (w.missing_streamers.length > 0) {
			const ok = await confirm({
				title: t("notify.missingStreamers.title"),
				message: t("notify.missingStreamers.message", { list: w.missing_streamers.join("\n") }),
				confirmText: t("notify.missingStreamers.confirm"),
				cancelText: t("notify.missingStreamers.ignore"),
				danger: true,
			});
			if (ok) {
				for (const username of w.missing_streamers) {
					await streamersStore.removeStreamer(username).catch(() => {});
				}
				toast(t("notify.missingStreamers.done", { count: w.missing_streamers.length }), "success");
			}
		}

		if (w.missing_pp_results.length > 0) {
			const ok = await confirm({
				title: t("notify.missingPpResults.title"),
				message: t("notify.missingPpResults.message", { list: w.missing_pp_results.map((p) => p.split(/[\\/]/).pop()).join("\n") }),
				confirmText: t("notify.missingPpResults.confirm"),
				cancelText: t("notify.missingPpResults.ignore"),
			});
			if (ok) {
				await call("remove_missing_pp_results", {
					paths: w.missing_pp_results,
				}).catch(() => {});
				toast(t("notify.missingPpResults.done", { count: w.missing_pp_results.length }), "success");
			}
		}
	}

	onMounted(async () => {
		// 初始化主题并监听系统主题变化 / Initialize theme and listen for system theme changes
		applyTheme(mq.matches);
		mq.addEventListener("change", onThemeChange);

		// 从后端同步语言设置，先加载消息再切换 locale
		// Sync language from backend, load messages before switching locale
		try {
			const settings = await call<{ language?: string }>("get_settings");
			if (settings?.language) {
				// 先加载该语言的消息，再切换 locale，保证首屏就用正确语言渲染
				// Load messages for the language first, then switch locale,
				// so the first render already uses the correct language
				const { modules: moduleLocales } = await loadLocaleFromServer(settings.language);
				locale.value = settings.language;
				moduleLocaleStore.setLocales(settings.language, moduleLocales);
			} else {
				// 无自定义语言，仍加载默认 locale 的服务器覆盖（模块翻译等）
				// No custom language, still load server overrides for the default locale
				const { modules: moduleLocales } = await loadLocaleFromServer(locale.value);
				moduleLocaleStore.setLocales(locale.value, moduleLocales);
			}
		} catch {
			// 后端未就绪时加载当前 locale 的消息作为 fallback
			// Backend not ready: load current locale messages as fallback
			const { modules: moduleLocales } = await loadLocaleFromServer(locale.value);
			moduleLocaleStore.setLocales(locale.value, moduleLocales);
		}

		// 监听 ffmpeg 缺失警告 / Listen for ffmpeg missing warning
		unlistenFfmpeg = await on("ffmpeg-missing", (payload) => {
			const p = payload as { message: string };
			toast(p.message, "warning");
		});

		// SSE 重连后倒计时 3 秒刷新页面，确保状态与服务器同步
		// After SSE reconnect, countdown 3 seconds then reload to sync state with server
		unlistenReconnect = onSseReconnect(() => {
			const COUNTDOWN = 3;
			let remaining = COUNTDOWN;
			const id = "reconnect-reload";
			sonnerToast.info(t("notify.reconnected", { n: remaining }), {
				id,
				duration: (COUNTDOWN + 1) * 1000,
			});
			const timer = setInterval(() => {
				remaining--;
				if (remaining > 0) {
					sonnerToast.info(t("notify.reconnected", { n: remaining }), {
						id,
						duration: (remaining + 1) * 1000,
					});
				} else {
					clearInterval(timer);
					window.location.reload();
				}
			}, 1000);
		});

		// 监听 SSE 断开连接 / Listen for SSE disconnect
		unlistenDisconnect = onSseDisconnect(() => {
			toast(t("notify.disconnected"), "warning");
		});

		// 监听启动警告 / Listen for startup warnings
		unlistenWarnings = await on("startup-warnings", handleStartupWarnings);

		// 监听自定义语言文件校验警告 / Listen for custom locale file validation warnings
		unlistenLocaleWarnings = await on(
			"locale-warnings",
			(payload) => {
				const items = payload as Array<{ path: string; reason: string }>;
				for (const item of items) {
					const file = item.path.replace(/\\/g, "/").split("/").pop() ?? item.path;
					toast(`${t("settings.localeFileInvalid", { file })}: ${item.reason}`, "warning");
				}
			},
		);

		// 初始加载可用语言列表 / Initial load of available locales
		await localesStore.refresh();

		// locale-files-changed 事件已在 localesStore 内部监听，无需在此重复注册
		// locale-files-changed is already listened inside localesStore; no need to register here
	});

	onUnmounted(() => {
		// 清理所有事件监听器 / Clean up all event listeners
		mq.removeEventListener("change", onThemeChange);
		unlistenFfmpeg?.();
		unlistenReconnect?.();
		unlistenDisconnect?.();
		unlistenWarnings?.();
		unlistenLocaleWarnings?.();
	});
</script>

<template>
	<!-- 全局布局过渡：setup 页面与主页面之间的切换 / Global layout transition between setup and main -->
	<Transition name="layout" mode="out-in">

		<!-- setup 页面：全屏无侧边栏 / Setup page: full-screen without sidebar -->
		<div v-if="route.path === '/setup'" key="setup" class="contents">
			<RouterView v-slot="{ Component }">
				<Transition name="page" mode="out-in">
					<component :is="Component" :key="route.path" />
				</Transition>
			</RouterView>
			<NotifyLayer />
		</div>

		<!-- 正常布局：侧边栏 + 内容区 / Normal layout: sidebar + content -->
		<div v-else key="main" class="flex h-screen overflow-hidden">
			<aside
				class="hidden md:flex w-52 shrink-0 bg-sidebar border-r border-sidebar-border flex-col p-3 gap-1"
			>
				<div
					class="flex items-center gap-2.5 px-1.5 py-4 mb-2 border-b border-sidebar-border"
				>
					<span
						class="flex items-center justify-center size-7 rounded-md bg-primary text-primary-foreground shrink-0"
					>
						<Video class="size-4" />
					</span>
					<span class="text-sm font-bold text-sidebar-foreground leading-tight truncate"
						>StripchatRecorder</span
					>
				</div>
				<nav class="flex flex-col gap-0.5">
					<Button
						v-for="item in navItems"
						:key="item.to"
						variant="ghost"
						class="w-full justify-start gap-2.5 text-sm font-normal rounded-l-none border-l-2 border-transparent pl-2.5"
						:class="
							route.path === item.to
								? 'border-l-primary bg-sidebar-accent text-sidebar-accent-foreground font-semibold'
								: 'text-sidebar-foreground/65 hover:text-sidebar-foreground hover:bg-sidebar-accent/40'
						"
						@click="router.push(item.to)"
					>
						<component :is="item.icon" class="size-4 shrink-0" />
						<span class="truncate">{{ t(item.labelKey) }}</span>
					</Button>
				</nav>
				<div class="mt-auto pt-2 px-1.5 flex items-center justify-between">
					<span class="text-[11px] text-sidebar-foreground/40">v0.3.5</span>
					<select
						:value="String(locale)"
						class="text-[11px] bg-transparent text-sidebar-foreground/50 hover:text-sidebar-foreground cursor-pointer outline-none border-none appearance-none pr-1"
						@change="setLocale(($event.target as HTMLSelectElement).value)"
					>
						<option
							v-for="loc in localesStore.locales"
							:key="loc.code"
							:value="loc.code"
						>{{ loc.name }}</option>
					</select>
				</div>
			</aside>
			<main class="flex-1 overflow-hidden flex flex-col">
				<!-- 移动端分段控件：主播列表 / 主播查找（折叠导航）/ Mobile segmented control: Streamers / Finder (folded nav) -->
				<div v-if="showStreamersSubnav" class="md:hidden flex gap-1.5 px-4 pt-3 pb-1 shrink-0">
					<button
						v-for="seg in [{ to: '/', labelKey: 'nav.streamers' }, { to: '/finder', labelKey: 'nav.finder' }]"
						:key="seg.to"
						type="button"
						class="flex-1 rounded-md py-2 text-sm font-medium transition-colors"
						:class="
							route.path === seg.to
								? 'bg-primary/12 text-primary font-semibold'
								: 'text-muted-foreground hover:text-foreground'
						"
						@click="router.push(seg.to)"
					>
						{{ t(seg.labelKey) }}
					</button>
				</div>
				<div
					ref="mainScrollEl"
					class="flex-1 overflow-y-scroll p-4 pb-[calc(4.5rem+env(safe-area-inset-bottom))] md:p-6 scrollbar-overlay"
				>
					<RouterView v-slot="{ Component }">
						<Transition name="page" mode="out-in">
							<component :is="Component" :key="route.path" />
						</Transition>
					</RouterView>
				</div>
			</main>

			<!-- 底部标签栏（移动端）/ Bottom tab bar (mobile) -->
			<nav
				class="md:hidden fixed inset-x-0 bottom-0 z-30 flex bg-sidebar border-t border-sidebar-border"
				style="padding-bottom: env(safe-area-inset-bottom)"
			>
				<button
					v-for="item in bottomNavItems"
					:key="item.to"
					type="button"
					class="flex-1 flex flex-col items-center justify-center gap-0.5 py-2 min-h-14 transition-colors"
					:class="isTabActive(item.to) ? 'text-primary' : 'text-sidebar-foreground/55'"
					@click="router.push(item.to)"
				>
					<component
						:is="item.icon"
						class="size-5 shrink-0"
						:stroke-width="isTabActive(item.to) ? 2.25 : 1.75"
					/>
					<span
						class="text-[11px] leading-none"
						:class="isTabActive(item.to) ? 'font-semibold' : 'font-normal'"
						>{{ t(item.labelKey) }}</span
					>
				</button>
			</nav>

			<NotifyLayer />
		</div>

	</Transition>
</template>
