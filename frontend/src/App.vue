<!--
    应用根组件 / Application Root Component

    提供侧边栏导航和主内容区域的整体布局。
    负责：
    - 跟随系统主题自动切换深色/浅色模式
    - 监听 ffmpeg-missing 事件并显示警告
    - 监听 SSE 断开/重连事件，重连后自动刷新页面
    - 监听 startup-warnings 事件，处理孤立的后处理记录

    Provides the overall layout with sidebar navigation and main content area.
    Responsible for:
    - Auto dark/light mode following system theme
    - Listening for ffmpeg-missing events and showing warnings
    - Listening for SSE disconnect/reconnect events, auto-reloading on reconnect
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, ref, computed } from "vue";
	import { RouterView, useRouter, useRoute } from "vue-router";
	import NotifyLayer from "./components/NotifyLayer.vue";
	import DirectoryBrowserDialog from "./components/DirectoryBrowserDialog.vue";
	import { Button } from "@/components/ui/button";
	import { call, on, onSseReconnect, onSseDisconnect } from "@/lib/api";
	import {
		Users, Video, Clapperboard, Radio, Search, Settings, Info, LogOut,
		ChevronsLeft, ChevronsRight, Bell, Store, X, WifiOff, Loader2,
	} from "@lucide/vue";
	import {
		useNotify, notify,
		frontendNotifications, dismissFrontendNotification, dismissAllFrontendNotifications,
		type FrontendNotification,
	} from "@/composables/useNotify";
	import { useStreamersStore } from "@/stores/streamers";
	import { useI18n } from "vue-i18n";
	import { useScrollbar } from "@/composables/useScrollbar";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";
	import { useAuthStore } from "@/stores/auth";
	import { useNotificationsStore, type Notification } from "@/stores/notifications";
	import {
		Dialog, DialogContent, DialogHeader, DialogTitle,
	} from "@/components/ui/dialog";

	const router = useRouter();
	const route = useRoute();
	const { toast } = useNotify();
	const streamersStore = useStreamersStore();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();
	const authStore = useAuthStore();

	const notificationsStore = useNotificationsStore();

	/**
	 * 合并后端持久化通知与前端内存通知，按时间倒序排列。
	 * 后端通知使用 created_at 字段；前端通知使用 timestamp 字段。
	 *
	 * Merge backend persistent notifications and frontend in-memory notifications,
	 * sorted by time descending.
	 */
	type MergedNotification =
		| { source: "backend"; data: Notification }
		| { source: "frontend"; data: FrontendNotification };

	const mergedNotifications = computed<MergedNotification[]>(() => {
		const backend: MergedNotification[] = notificationsStore.notifications.map((n) => ({
			source: "backend",
			data: n,
		}));
		const frontend: MergedNotification[] = frontendNotifications.value.map((n) => ({
			source: "frontend",
			data: n,
		}));
		return [...backend, ...frontend].sort((a, b) => {
			const ta = a.source === "backend"
				? new Date(a.data.created_at).getTime()
				: a.data.timestamp.getTime();
			const tb = b.source === "backend"
				? new Date(b.data.created_at).getTime()
				: b.data.timestamp.getTime();
			return tb - ta; // 倒序：最新在上 / descending: newest first
		});
	});

	/** 面板通知总数（后端 + 前端内存）/ Total panel notification count (backend + frontend) */
	const totalNotificationCount = computed(
		() => notificationsStore.unreadCount + frontendNotifications.value.length,
	);

	/** 通知面板是否打开 / Whether the notification panel is open */
	const notificationPanelOpen = ref(false);

	/** SSE 是否已断开连接（显示断连遮罩）/ Whether SSE is disconnected (shows overlay) */
	const isDisconnected = ref(false);
	const notificationScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(notificationScrollEl);

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
		{ to: "/community", labelKey: "nav.community", icon: Store },
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
	let unlistenLocaleWarnings: (() => void) | null = null;
	let unlistenNotification: (() => void) | null = null;
	/**
	 * 执行通知面板中的操作按钮（删除主播、清理孤立记录等）。
	 * Execute an action button in the notification panel.
	 */
	async function executeNotificationAction(n: Notification) {
		if (!n.action) return;
		const { action_type, targets } = n.action;
		try {
			if (action_type === "remove_streamers") {
				for (const username of targets) {
					await streamersStore.removeStreamer(username).catch(() => {});
				}
				toast(t("notify.missingStreamers.done", { count: targets.length }), "success");
			}
		} catch {
			// 静默失败
		}
		// 操作完成后标记通知已读
		await notificationsStore.markRead([n.id]);
	}

	async function handleLogout() {
		await authStore.logout();
		router.push("/login");
	}

	function notificationLevelClass(level: Notification["level"]): string {
		return level === "error"
			? "text-destructive"
			: level === "warning"
			? "text-yellow-500 dark:text-yellow-400"
			: "text-muted-foreground";
	}

	/** 前端内存通知的颜色类（ToastType → CSS class）/ Color class for frontend in-memory notifications */
	function frontendNotificationClass(type: FrontendNotification["type"]): string {
		return type === "error"
			? "text-destructive"
			: type === "warning"
			? "text-yellow-500 dark:text-yellow-400"
			: type === "success"
			? "text-green-600 dark:text-green-400"
			: "text-muted-foreground";
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
			notify(p.message, "warning");
		});

		// SSE 重连后倒计时 3 秒刷新页面，确保状态与服务器同步
		// After SSE reconnect, countdown 3 seconds then reload to sync state with server
		unlistenReconnect = onSseReconnect(() => {
			// 重连成功，立即刷新页面恢复状态 / Reconnected: reload immediately to restore state
			window.location.reload();
		});

		// 监听 SSE 断开连接 / Listen for SSE disconnect
		unlistenDisconnect = onSseDisconnect(() => {
			toast(t("notify.disconnected"), "warning");
			isDisconnected.value = true;
		});

		// 监听自定义语言文件校验警告 / Listen for custom locale file validation warnings
		unlistenLocaleWarnings = await on(
			"locale-warnings",
			(payload) => {
				const items = payload as Array<{ path: string; reason: string }>;
				for (const item of items) {
					const file = item.path.replace(/\\/g, "/").split("/").pop() ?? item.path;
					notify(`${t("settings.localeFileInvalid", { file })}: ${item.reason}`, "warning");
				}
			},
		);

		// 初始加载可用语言列表
		// Load available locales
		await localesStore.refresh();

		// 首次加载通知列表 / Fetch notifications on first load
		await notificationsStore.fetch();

		// 监听实时新通知（用户在线时：同时追加面板 + 弹 toast）
		// Listen for real-time new notifications (user online: panel + toast)
		unlistenNotification = await on("notification-created", (payload) => {
			notificationsStore.append(payload as Notification, true);
		});
	});

	onUnmounted(() => {
		// 清理所有事件监听器 / Clean up all event listeners
		mq.removeEventListener("change", onThemeChange);
		unlistenFfmpeg?.();
		unlistenReconnect?.();
		unlistenDisconnect?.();
		unlistenLocaleWarnings?.();
		unlistenNotification?.();
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

				<!-- 底部：通知 + 退出 + 折叠按钮 / Bottom: notifications + logout + collapse button -->
				<div class="mt-auto pt-2 border-t border-sidebar-border flex flex-col gap-0.5">
					<!-- 通知按钮 / Notification button -->
					<Button
						variant="ghost"
						class="w-full text-sm font-normal px-2 text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50 relative"
						:class="sidebarCollapsed ? 'justify-center gap-0' : 'justify-start gap-2'"
						:title="sidebarCollapsed ? t('nav.notifications') : undefined"
						@click="notificationPanelOpen = true"
					>
						<span class="relative shrink-0">
							<Bell class="size-4" />
							<span
								v-if="totalNotificationCount > 0"
								class="absolute -top-1.5 -right-1.5 min-w-4 h-4 rounded-full bg-destructive text-destructive-foreground text-[10px] font-bold flex items-center justify-center px-0.5 leading-none"
							>
								{{ totalNotificationCount > 99 ? "99+" : totalNotificationCount }}
							</span>
						</span>
						<span v-show="!sidebarCollapsed" class="truncate">{{ t("nav.notifications") }}</span>
					</Button>
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

			<!-- 通知面板 / Notification panel -->
			<Dialog :open="notificationPanelOpen" @update:open="(v) => (notificationPanelOpen = v)">
				<DialogContent class="sm:max-w-md flex flex-col">
					<DialogHeader class="shrink-0">
						<DialogTitle>{{ t("nav.notifications") }}</DialogTitle>
					</DialogHeader>

					<!-- 全部已读操作行 / Mark-all-read action row -->
					<div v-if="totalNotificationCount > 0" class="shrink-0 flex justify-end -mt-1">
						<Button
							variant="outline"
							size="sm"
							class="h-7 px-3 text-xs"
							@click="notificationsStore.markAllRead(); dismissAllFrontendNotifications()"
						>
							{{ t("notifications.markAllRead") }}
						</Button>
					</div>

					<!-- 合并通知列表 / Merged notification list -->
					<!-- 超过 5 条时固定高度并开启滚动；5 条及以内自然撑开 dialog -->
					<!-- Scrolls when > 5 items; expands naturally otherwise -->
					<div
						v-if="mergedNotifications.length > 0"
						ref="notificationScrollEl"
						class="flex flex-col gap-2 pr-1"
						:class="mergedNotifications.length > 5
							? 'overflow-y-auto scrollbar-overlay max-h-[60vh]'
							: 'overflow-visible'"
					>
						<!-- 后端持久化通知（带 action 按钮）/ Backend persistent notification (with action button) -->
						<template v-for="item in mergedNotifications" :key="item.source === 'backend' ? `b-${item.data.id}` : `f-${item.data.id}`">
							<div
								v-if="item.source === 'backend'"
								class="rounded-lg border px-3 py-2.5 flex flex-col gap-1.5"
							>
								<div class="flex items-start justify-between gap-2">
									<p class="text-sm leading-snug flex-1" :class="notificationLevelClass(item.data.level)">
										{{ item.data.message }}
									</p>
									<Button
										variant="ghost"
										size="sm"
										class="h-6 w-6 p-0 shrink-0 text-muted-foreground hover:text-foreground"
										@click="notificationsStore.markRead([item.data.id])"
									>
										<X class="size-3.5" />
									</Button>
								</div>
								<div class="flex items-center justify-between gap-2">
									<p class="text-xs text-muted-foreground">
										{{ new Date(item.data.created_at).toLocaleString() }}
									</p>
									<Button
										v-if="item.data.action"
										variant="destructive"
										size="sm"
										class="h-6 text-xs px-2"
										@click="executeNotificationAction(item.data)"
									>
										{{ t(`notifications.action.${item.data.action.action_type}`) }}
									</Button>
								</div>
							</div>

							<!-- 前端内存通知（纯展示，刷新后消失）/ Frontend in-memory notification (display only, cleared on refresh) -->
							<div
								v-else
								class="rounded-lg border px-3 py-2.5 flex flex-col gap-1.5"
							>
								<div class="flex items-start justify-between gap-2">
									<p class="text-sm leading-snug flex-1" :class="frontendNotificationClass(item.data.type)">
										{{ item.data.message }}
									</p>
									<Button
										variant="ghost"
										size="sm"
										class="h-6 w-6 p-0 shrink-0 text-muted-foreground hover:text-foreground"
										@click="dismissFrontendNotification(item.data.id)"
									>
										<X class="size-3.5" />
									</Button>
								</div>
								<p class="text-xs text-muted-foreground">
									{{ item.data.timestamp.toLocaleString() }}
								</p>
							</div>
						</template>
					</div>

					<!-- 空状态 / Empty state -->
					<div
						v-else
						class="flex items-center justify-center text-sm text-muted-foreground py-8"
					>
						{{ t("notifications.empty") }}
					</div>
				</DialogContent>
			</Dialog>
		</div>

	</Transition>

	<!-- 断连全屏遮罩（叠在所有内容上）/ Disconnect full-screen overlay -->
	<Transition
		enter-active-class="transition-opacity duration-300"
		enter-from-class="opacity-0"
		enter-to-class="opacity-100"
		leave-active-class="transition-opacity duration-300"
		leave-from-class="opacity-100"
		leave-to-class="opacity-0"
	>
		<div
			v-if="isDisconnected"
			class="fixed inset-0 z-50 flex flex-col items-center justify-center gap-6 bg-background/95 backdrop-blur-sm text-center px-6"
		>
			<div class="relative flex items-center justify-center">
				<Loader2 class="size-16 animate-spin text-muted-foreground/25" />
				<WifiOff class="absolute size-7 text-muted-foreground" />
			</div>
			<div class="flex flex-col gap-2 max-w-sm">
				<h1 class="text-lg font-semibold">{{ t("notify.disconnectedPage.title") }}</h1>
				<p class="text-sm text-muted-foreground leading-relaxed">
					{{ t("notify.disconnectedPage.subtitle") }}
				</p>
				<p class="text-xs text-muted-foreground/50 mt-1">
					{{ t("notify.disconnectedPage.tip") }}
				</p>
			</div>
		</div>
	</Transition>

</template>
