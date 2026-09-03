<!--
    应用根组件 / Application Root Component

    提供侧边栏导航和主内容区域的整体布局。
    负责：
    - 跟随系统主题自动切换深色/浅色模式
    - 监听 ffmpeg-missing 事件并显示警告
    - 监听 SSE 断开/重连事件，重连后自动刷新页面
    - 监听 startup-warnings 事件，处理不存在的主播和孤立的后处理记录

    Provides the overall layout with sidebar navigation and main content area.
    Responsible for:
    - Auto dark/light mode following system theme
    - Listening for ffmpeg-missing events and showing warnings
    - Listening for SSE disconnect/reconnect events, auto-reloading on reconnect
    - Listening for startup-warnings to handle non-existent streamers and orphaned post-processing records
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, ref } from "vue";
	import { RouterView, useRouter, useRoute } from "vue-router";
	import NotifyLayer from "./components/NotifyLayer.vue";
	import DirectoryBrowserDialog from "./components/DirectoryBrowserDialog.vue";
	import { Button } from "@/components/ui/button";
	import { call, on, onSseReconnect, onSseDisconnect } from "@/lib/api";
	import {
		Users, Video, Clapperboard, Radio, Search, Settings, Info, LogOut,
		ChevronsLeft, ChevronsRight,
	} from "@lucide/vue";
	import { useNotify } from "@/composables/useNotify";
	import { toast as sonnerToast } from "vue-sonner";
	import { useStreamersStore } from "@/stores/streamers";
	import { useI18n } from "vue-i18n";
	import { useScrollbar } from "@/composables/useScrollbar";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";
	import { useAuthStore } from "@/stores/auth";

	const router = useRouter();
	const route = useRoute();
	const { toast, confirm } = useNotify();
	const streamersStore = useStreamersStore();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();
	const authStore = useAuthStore();

	const mainScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(mainScrollEl);

	/**
	 * 侧边栏导航项配置。
	 *
	 * 注：早前这里的注释写的是"桌面版无流转发"，但这份代码是 frontend/（Web/Server
	 * 版）而不是 desktop/——流转发（backend/src/relay.rs）是纯 Server 端功能，
	 * RelayView.vue、对应的 /relay 路由（见 router/index.ts）、以及 relay.* 的
	 * 中英文翻译此前都完整存在，只是这个导航数组里缺了这一项，导致页面在实际存在、
	 * 路由能正常跳转的情况下，用户在侧边栏里根本看不到入口。
	 *
	 * Sidebar navigation items configuration.
	 *
	 * Note: this used to be commented "desktop: no relay", but this file lives under
	 * frontend/ (the Web/Server build), not desktop/ — relay streaming
	 * (backend/src/relay.rs) is a server-only feature. RelayView.vue, its /relay route
	 * (see router/index.ts), and the relay.* zh-CN/en-US translations were all already
	 * complete; this array was simply missing the entry, so the page existed and the
	 * route worked fine, but users had no way to find it from the sidebar.
	 */
	const navItems = [
		{ to: "/", labelKey: "nav.streamers", icon: Users },
		{ to: "/recordings", labelKey: "nav.recordings", icon: Video },
		{ to: "/postprocess", labelKey: "nav.postprocess", icon: Clapperboard },
		{ to: "/relay", labelKey: "nav.relay", icon: Radio },
		{ to: "/finder", labelKey: "nav.finder", icon: Search },
		{ to: "/settings", labelKey: "nav.settings", icon: Settings },
		{ to: "/about", labelKey: "nav.about", icon: Info },
	];

	/** 侧边栏是否折叠 / Whether the sidebar is collapsed */
	const sidebarCollapsed = ref(false);

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

	async function handleLogout() {
		await authStore.logout();
		router.push("/login");
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

		// 初始加载可用语言列表
		// Load available locales
		await localesStore.refresh();
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
	<!-- 全局布局过渡：setup/login 页面与主页面之间的切换 / Global layout transition between setup/login and main -->
	<Transition name="layout" mode="out-in">

		<!-- setup / login 页面：全屏无侧边栏 / Setup / login page: full-screen without sidebar -->
		<div v-if="route.path === '/setup' || route.path === '/login'" key="setup" class="contents">
			<RouterView v-slot="{ Component }">
				<Transition name="page" mode="out-in">
					<component :is="Component" :key="route.path" />
				</Transition>
			</RouterView>
			<NotifyLayer />
			<DirectoryBrowserDialog />
		</div>

		<!-- 正常布局：侧边栏 + 内容区 / Normal layout: sidebar + content -->
		<div v-else key="main" class="flex h-screen overflow-hidden">
			<aside
				class="shrink-0 bg-sidebar border-r border-sidebar-border flex flex-col p-3 gap-1 transition-[width] duration-200 ease-in-out overflow-hidden"
				:class="sidebarCollapsed ? 'w-14' : 'w-52'"
			>
				<!-- 品牌区 / Brand area -->
				<div class="flex items-center gap-2 px-1 py-4 mb-1 border-b border-sidebar-border min-w-0">
					<img src="/icon.png" alt="icon" class="w-5 h-5 shrink-0" />
					<span
						class="text-sm font-bold text-sidebar-foreground truncate transition-[opacity,width] duration-200 ease-in-out"
						:class="sidebarCollapsed ? 'opacity-0 w-0' : 'opacity-100'"
					>StripchatRecorder</span>
				</div>

				<!-- 导航项 / Nav items -->
				<nav class="flex flex-col gap-0.5">
					<Button
						v-for="item in navItems"
						:key="item.to"
						variant="ghost"
						class="w-full text-sm font-normal px-2 transition-[justify-content]"
						:class="[
							sidebarCollapsed ? 'justify-center gap-0' : 'justify-start gap-2',
							route.path === item.to
								? 'bg-sidebar-accent text-sidebar-accent-foreground font-semibold'
								: 'text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50'
						]"
						:title="sidebarCollapsed ? t(item.labelKey) : undefined"
						@click="router.push(item.to)"
					>
						<component :is="item.icon" class="size-4 shrink-0" />
						<span v-show="!sidebarCollapsed" class="truncate">{{ t(item.labelKey) }}</span>
					</Button>
				</nav>

				<!-- 底部：退出 + 折叠按钮 / Bottom: logout + collapse button -->
				<div class="mt-auto pt-2 border-t border-sidebar-border flex flex-col gap-0.5">
					<Button
						variant="ghost"
						class="w-full text-sm font-normal px-2 text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50"
						:class="sidebarCollapsed ? 'justify-center gap-0' : 'justify-start gap-2'"
						:title="sidebarCollapsed ? t('login.logout') : undefined"
						@click="handleLogout"
					>
						<LogOut class="size-4 shrink-0" />
						<span v-show="!sidebarCollapsed" class="truncate">{{ t("login.logout") }}</span>
					</Button>
					<Button
						variant="ghost"
						class="w-full text-sm font-normal px-2 text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50"
						:class="sidebarCollapsed ? 'justify-center gap-0' : 'justify-start gap-2'"
						:title="sidebarCollapsed ? t('nav.expand') : t('nav.collapse')"
						@click="sidebarCollapsed = !sidebarCollapsed"
					>
						<ChevronsLeft v-if="!sidebarCollapsed" class="size-4 shrink-0" />
						<ChevronsRight v-else class="size-4 shrink-0" />
						<span v-show="!sidebarCollapsed" class="truncate">{{ t("nav.collapse") }}</span>
					</Button>
				</div>
			</aside>
			<main class="flex-1 overflow-hidden">
				<div ref="mainScrollEl" class="h-full overflow-y-auto scrollbar-overlay">
					<RouterView v-slot="{ Component }">
						<Transition name="page" mode="out-in">
							<component :is="Component" :key="route.path" />
						</Transition>
					</RouterView>
				</div>
			</main>
			<NotifyLayer />
			<DirectoryBrowserDialog />
		</div>

	</Transition>
</template>
