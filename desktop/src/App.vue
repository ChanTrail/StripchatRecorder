<!--
    应用根组件（桌面版）/ Application Root Component (Desktop)

    提供侧边栏导航和主内容区域的整体布局。
    负责：
    - 跟随系统主题自动切换深色/浅色模式
    - 监听 ffmpeg-missing 事件并显示警告
    - 监听 startup-warnings 事件，通知面板处理孤立记录
    - 侧边栏可折叠（持久化到 localStorage）
    - 移动端底部 Tab 栏 + 抽屉导航
    - 全局通知面板（Bell 图标，合并前端内存通知 + 后端持久化通知）
    - 全局 DirectoryBrowserDialog 单例挂载

    Provides the overall layout with sidebar navigation and main content area.
    Responsible for:
    - Auto dark/light mode following system theme
    - Listening for ffmpeg-missing events and showing warnings
    - Listening for startup-warnings events, notification panel for orphaned records
    - Collapsible sidebar (persisted to localStorage)
    - Mobile bottom tab bar + drawer navigation
    - Global notification panel (Bell icon, merging frontend + backend notifications)
    - Global DirectoryBrowserDialog singleton mount
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, ref, computed } from "vue";
	import { RouterView, useRouter, useRoute } from "vue-router";
	import NotifyLayer from "./components/NotifyLayer.vue";
	import DirectoryBrowserDialog from "./components/DirectoryBrowserDialog.vue";
	import { Button } from "@/components/ui/button";
	import { call, on, onSseReconnect } from "@/lib/api";
	import {
		Users, Video, Clapperboard, Search, Settings, Info,
		ChevronsLeft, ChevronsRight, Bell, Store, X,
		Menu,
	} from "@lucide/vue";
	import {
		useNotify, notify,
		frontendNotifications, dismissFrontendNotification, dismissAllFrontendNotifications,
		type FrontendNotification,
	} from "@/composables/useNotify";
	import { useStreamersStore } from "@/stores/streamers";
	import { useI18n } from "vue-i18n";
	import { useScrollbar } from "@/composables/useScrollbar";
	import { useMobileLayout } from "@/composables/useMobileLayout";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";
	import { useNotificationsStore, type Notification } from "@/stores/notifications";
	import {
		Dialog, DialogContent, DialogHeader, DialogTitle,
	} from "@/components/ui/dialog";
	import { toast as sonnerToast } from "vue-sonner";

	const router = useRouter();
	const route = useRoute();
	const { toast, confirm } = useNotify();
	const streamersStore = useStreamersStore();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();
	const notificationsStore = useNotificationsStore();

	/**
	 * 合并后端持久化通知与前端内存通知，按时间倒序排列。
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
			return tb - ta;
		});
	});

	const totalNotificationCount = computed(
		() => notificationsStore.unreadCount + frontendNotifications.value.length,
	);

	const notificationPanelOpen = ref(false);
	const notificationScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(notificationScrollEl);

	const mainScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(mainScrollEl);

	const { isMobile } = useMobileLayout();

	/**
	 * 侧边栏导航项配置（桌面版无流转发）。
	 * Sidebar navigation items (desktop has no relay).
	 */
	const navItems = [
		{ to: "/",            labelKey: "nav.streamers",   icon: Users },
		{ to: "/recordings",  labelKey: "nav.recordings",  icon: Video },
		{ to: "/postprocess", labelKey: "nav.postprocess", icon: Clapperboard },
		{ to: "/finder",      labelKey: "nav.finder",      icon: Search },
		{ to: "/community",   labelKey: "nav.community",   icon: Store },
		{ to: "/settings",    labelKey: "nav.settings",    icon: Settings },
		{ to: "/about",       labelKey: "nav.about",       icon: Info },
	];

	/** 侧边栏是否折叠 / Whether the sidebar is collapsed */
	const SIDEBAR_KEY = "sidebar_collapsed";
	const sidebarCollapsed = ref(localStorage.getItem(SIDEBAR_KEY) === "1");

	function toggleSidebar() {
		sidebarCollapsed.value = !sidebarCollapsed.value;
		localStorage.setItem(SIDEBAR_KEY, sidebarCollapsed.value ? "1" : "0");
	}

	/** 移动端抽屉是否打开 / Whether the mobile drawer is open */
	const mobileDrawerOpen = ref(false);

	/** 移动端底部 Tab 栏的固定 4 项 */
	const bottomTabItems = [
		{ to: "/",           labelKey: "nav.streamers",  icon: Users },
		{ to: "/recordings", labelKey: "nav.recordings", icon: Video },
		{ to: "/finder",     labelKey: "nav.finder",     icon: Search },
		{ to: "/settings",   labelKey: "nav.settings",   icon: Settings },
	];

	function mobileNavTo(path: string) {
		router.push(path);
		mobileDrawerOpen.value = false;
	}

	function applyTheme(dark: boolean) {
		document.documentElement.classList.toggle("dark", dark);
	}

	const mq = window.matchMedia("(prefers-color-scheme: dark)");
	function onThemeChange(e: MediaQueryListEvent) {
		applyTheme(e.matches);
	}

	let unlistenFfmpeg: (() => void) | null = null;
	let unlistenReconnect: (() => void) | null = null;
	let unlistenWarnings: (() => void) | null = null;
	let unlistenLocaleWarnings: (() => void) | null = null;
	let unlistenNotification: (() => void) | null = null;

	/**
	 * 执行通知面板中的操作按钮。
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
			} else if (action_type === "view_update") {
				router.push("/about");
			}
		} catch {
			// 静默失败
		}
		await notificationsStore.markRead([n.id]);
	}

	/**
	 * 渲染后端通知消息：若有 message_key 则查 i18n，否则返回 message。
	 */
	function renderNotificationMessage(n: Notification): string {
		if (n.message_key) {
			const args = n.message_args ?? {};
			const translated = t(n.message_key, args as Record<string, string | number>);
			if (translated && translated !== n.message_key) return translated;
		}
		return n.message;
	}

	function notificationLevelClass(level: Notification["level"]): string {
		return level === "error"
			? "text-destructive"
			: level === "warning"
			? "text-yellow-500 dark:text-yellow-400"
			: "text-muted-foreground";
	}

	function frontendNotificationClass(type: FrontendNotification["type"]): string {
		return type === "error"
			? "text-destructive"
			: type === "warning"
			? "text-yellow-500 dark:text-yellow-400"
			: type === "success"
			? "text-green-500"
			: "text-muted-foreground";
	}

	/**
	 * 处理启动时的警告事件（孤立的后处理记录等）。
	 * 桌面版用通知面板代替直接弹窗（与 frontend 对齐），
	 * 但 missing_streamers 仍优先弹窗（确认风险高）。
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
				await call("remove_missing_pp_results", { paths: w.missing_pp_results }).catch(() => {});
				toast(t("notify.missingPpResults.done", { count: w.missing_pp_results.length }), "success");
			}
		}
	}

	onMounted(async () => {
		applyTheme(mq.matches);
		mq.addEventListener("change", onThemeChange);

		// 从后端同步语言设置 / Sync language from backend
		try {
			const settings = await call<{ language?: string }>("get_settings");
			if (settings?.language) {
				const { modules: moduleLocales } = await loadLocaleFromServer(settings.language);
				locale.value = settings.language;
				localStorage.setItem("locale", settings.language);
				moduleLocaleStore.setLocales(settings.language, moduleLocales);
			} else {
				const { modules: moduleLocales } = await loadLocaleFromServer(locale.value);
				moduleLocaleStore.setLocales(locale.value, moduleLocales);
			}
		} catch {
			const { modules: moduleLocales } = await loadLocaleFromServer(locale.value);
			moduleLocaleStore.setLocales(locale.value, moduleLocales);
		}

		// 拉取后端通知 / Fetch backend notifications
		await notificationsStore.fetch();

		// 监听 ffmpeg 缺失警告 / Listen for ffmpeg missing warning
		unlistenFfmpeg = await on("ffmpeg-missing", (payload) => {
			const p = payload as { message: string };
			notify(p.message, "warning");
		});

		// Tauri 版无 SSE 重连概念；保留接口兼容的空操作回调。
		// 若后续接入 Tauri updater，可在此处处理重连逻辑。
		unlistenReconnect = onSseReconnect(() => {
			// Tauri mode: no-op reconnect handler
		});

		// 监听启动警告 / Listen for startup warnings
		unlistenWarnings = await on("startup-warnings", handleStartupWarnings);

		// 监听自定义语言文件校验警告 / Listen for locale validation warnings
		unlistenLocaleWarnings = await on("locale-warnings", (payload) => {
			const items = payload as Array<{ path: string; reason: string }>;
			for (const item of items) {
				const file = item.path.replace(/\\/g, "/").split("/").pop() ?? item.path;
				toast(`${t("settings.localeFileInvalid", { file })}: ${item.reason}`, "warning");
			}
		});

		// 监听后端推送的新通知 / Listen for new backend notifications
		unlistenNotification = await on("notification-created", (payload) => {
			const n = payload as Notification;
			notificationsStore.append(n, true);
		});

		// 监听启动扫描完成信号，重新拉取通知列表（只加面板，不弹 toast）
		// On startup scan done, re-fetch notification list (panel only, no toast)
		await on("startup-scan-done", () => {
			void notificationsStore.fetch();
		});

		await localesStore.refresh();
		await localesStore.setupListeners();
	});

	onUnmounted(() => {
		mq.removeEventListener("change", onThemeChange);
		unlistenFfmpeg?.();
		unlistenReconnect?.();
		unlistenWarnings?.();
		unlistenLocaleWarnings?.();
		unlistenNotification?.();
	});
</script>

<template>
	<Transition name="layout" mode="out-in">

		<!-- setup 页面 / Setup page -->
		<div v-if="route.path === '/setup'" key="setup" class="contents">
			<RouterView v-slot="{ Component }">
				<Transition name="page" mode="out-in">
					<component :is="Component" :key="route.path" />
				</Transition>
			</RouterView>
			<NotifyLayer />
		</div>

		<!-- 正常布局 / Normal layout -->
		<div v-else key="main" class="flex h-screen overflow-hidden">

			<!-- ── 桌面端侧边栏 / Desktop sidebar ── -->
			<aside
				v-if="!isMobile"
				class="shrink-0 bg-sidebar border-r border-sidebar-border flex flex-col transition-all duration-200"
				:class="sidebarCollapsed ? 'w-14' : 'w-44'"
			>
				<!-- Logo 区域 / Logo area -->
				<div
					class="flex items-center px-3 py-4 mb-1 border-b border-sidebar-border"
					:class="sidebarCollapsed ? 'justify-center' : 'gap-2'"
				>
					<img src="/icon.png" alt="logo" class="w-5 h-5 rounded shrink-0" />
					<span
						v-if="!sidebarCollapsed"
						class="text-sm font-bold text-sidebar-foreground truncate"
					>StripchatRecorder</span>
				</div>

				<!-- 导航项 / Navigation items -->
				<nav class="flex flex-col gap-0.5 flex-1 px-2">
					<button
						v-for="item in navItems"
						:key="item.to"
						class="flex items-center rounded-md px-2 py-1.5 text-sm transition-colors w-full"
						:class="[
							sidebarCollapsed ? 'justify-center' : 'gap-2',
							route.path === item.to
								? 'bg-sidebar-accent text-sidebar-accent-foreground font-semibold'
								: 'text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50',
						]"
						:title="sidebarCollapsed ? t(item.labelKey) : undefined"
						@click="router.push(item.to)"
					>
						<component :is="item.icon" class="size-4 shrink-0" />
						<span v-if="!sidebarCollapsed" class="truncate">{{ t(item.labelKey) }}</span>
					</button>
				</nav>

				<!-- 通知按钮 / Notification button -->
				<div class="px-2 pb-2">
					<button
						class="flex items-center rounded-md px-2 py-1.5 text-sm transition-colors w-full relative text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50"
						:class="sidebarCollapsed ? 'justify-center' : 'gap-2'"
						:title="sidebarCollapsed ? t('nav.notifications') : undefined"
						@click="notificationPanelOpen = true"
					>
						<Bell class="size-4 shrink-0" />
						<span v-if="!sidebarCollapsed" class="truncate">{{ t("nav.notifications") }}</span>
						<span
							v-if="totalNotificationCount > 0"
							class="absolute top-0.5 right-0.5 min-w-4 h-4 rounded-full bg-destructive text-[10px] text-white flex items-center justify-center px-0.5"
						>{{ totalNotificationCount > 99 ? "99+" : totalNotificationCount }}</span>
					</button>
				</div>

				<!-- 折叠按钮 / Collapse toggle -->
				<div class="px-2 pb-3">
					<button
						class="flex items-center justify-center w-full rounded-md py-1.5 text-sidebar-foreground/50 hover:text-sidebar-foreground hover:bg-sidebar-accent/50 transition-colors"
						:title="sidebarCollapsed ? t('nav.expand') : t('nav.collapse')"
						@click="toggleSidebar"
					>
						<ChevronsLeft v-if="!sidebarCollapsed" class="size-4" />
						<ChevronsRight v-else class="size-4" />
					</button>
				</div>
			</aside>

			<!-- 主内容区 / Main content area -->
			<div class="flex-1 overflow-hidden flex flex-col">

				<!-- 移动端顶部栏 / Mobile top bar -->
				<div v-if="isMobile" class="flex items-center justify-between px-4 py-3 border-b shrink-0 bg-background">
					<div class="flex items-center gap-2">
						<img src="/icon.png" alt="logo" class="w-6 h-6 rounded" />
						<span class="text-sm font-bold">StripchatRecorder</span>
					</div>
					<div class="flex items-center gap-1">
						<button
							class="relative p-2 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
							@click="notificationPanelOpen = true"
						>
							<Bell class="size-5" />
							<span
								v-if="totalNotificationCount > 0"
								class="absolute top-1 right-1 min-w-3.5 h-3.5 rounded-full bg-destructive text-[9px] text-white flex items-center justify-center px-0.5"
							>{{ totalNotificationCount > 99 ? "99+" : totalNotificationCount }}</span>
						</button>
						<button
							class="p-2 rounded-md text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
							@click="mobileDrawerOpen = true"
						>
							<Menu class="size-5" />
						</button>
					</div>
				</div>

				<main class="flex-1 overflow-hidden" :class="isMobile ? 'pb-16' : ''">
					<div ref="mainScrollEl" class="h-full overflow-y-scroll scrollbar-overlay">
						<RouterView v-slot="{ Component }">
							<Transition name="page" mode="out-in">
								<component :is="Component" :key="route.path" />
							</Transition>
						</RouterView>
					</div>
				</main>

				<!-- 移动端底部 Tab 栏 / Mobile bottom tab bar -->
				<div
					v-if="isMobile"
					class="fixed bottom-0 inset-x-0 z-20 bg-background border-t flex items-stretch h-16 pb-[env(safe-area-inset-bottom,0px)]"
				>
					<button
						v-for="item in bottomTabItems"
						:key="item.to"
						class="flex-1 flex flex-col items-center justify-center gap-0.5 text-[10px] transition-colors"
						:class="route.path === item.to ? 'text-primary' : 'text-muted-foreground'"
						@click="router.push(item.to)"
					>
						<component :is="item.icon" class="size-5" />
						<span>{{ t(item.labelKey) }}</span>
					</button>
					<button
						class="flex-1 flex flex-col items-center justify-center gap-0.5 text-[10px] text-muted-foreground"
						@click="mobileDrawerOpen = true"
					>
						<Menu class="size-5" />
						<span>{{ t("nav.more") }}</span>
					</button>
				</div>
			</div>

			<!-- 移动端全导航抽屉 / Mobile full-nav drawer -->
			<Transition
				enter-active-class="transition-opacity duration-200"
				enter-from-class="opacity-0"
				enter-to-class="opacity-100"
				leave-active-class="transition-opacity duration-200"
				leave-from-class="opacity-100"
				leave-to-class="opacity-0"
			>
				<div
					v-if="mobileDrawerOpen"
					class="fixed inset-0 z-40 bg-black/40"
					@click="mobileDrawerOpen = false"
				/>
			</Transition>
			<Transition
				enter-active-class="transition-transform duration-250 ease-out"
				enter-from-class="-translate-x-full"
				enter-to-class="translate-x-0"
				leave-active-class="transition-transform duration-200 ease-in"
				leave-from-class="translate-x-0"
				leave-to-class="-translate-x-full"
			>
				<div
					v-if="mobileDrawerOpen"
					class="fixed left-0 top-0 bottom-0 z-50 w-64 bg-background border-r flex flex-col"
				>
					<div class="flex items-center gap-2 px-4 py-5 border-b">
						<img src="/icon.png" alt="logo" class="w-7 h-7 rounded" />
						<span class="font-bold text-sm">StripchatRecorder</span>
					</div>
					<nav class="flex flex-col gap-0.5 flex-1 overflow-y-auto px-2 py-2">
						<button
							v-for="item in navItems"
							:key="item.to"
							class="flex items-center gap-3 px-3 py-2.5 rounded-md text-sm transition-colors w-full text-left"
							:class="route.path === item.to
								? 'bg-accent text-accent-foreground font-semibold'
								: 'text-muted-foreground hover:text-foreground hover:bg-accent/50'"
							@click="mobileNavTo(item.to)"
						>
							<component :is="item.icon" class="size-4 shrink-0" />
							{{ t(item.labelKey) }}
						</button>
					</nav>
				</div>
			</Transition>

			<!-- 通知面板 Dialog / Notification panel Dialog -->
			<Dialog :open="notificationPanelOpen" @update:open="notificationPanelOpen = $event">
				<DialogContent class="flex flex-col p-0 gap-0 max-w-sm w-full" style="max-height: 80vh">
					<DialogHeader class="px-4 pt-4 pb-3 border-b shrink-0">
						<div class="flex items-center justify-between">
							<DialogTitle class="text-sm font-semibold">{{ t("nav.notifications") }}</DialogTitle>
							<div class="flex items-center gap-2">
								<button
									v-if="totalNotificationCount > 0"
									class="text-xs text-muted-foreground hover:text-foreground transition-colors"
									@click="notificationsStore.markAllRead(); dismissAllFrontendNotifications()"
								>
									{{ t("notifications.markAllRead") }}
								</button>
								<button
									class="p-0.5 rounded text-muted-foreground hover:text-foreground transition-colors"
									@click="notificationPanelOpen = false"
								>
									<X class="size-4" />
								</button>
							</div>
						</div>
					</DialogHeader>

					<div ref="notificationScrollEl" class="flex-1 min-h-0 overflow-y-auto scrollbar-overlay">
						<div v-if="mergedNotifications.length === 0" class="flex flex-col items-center justify-center py-12 text-muted-foreground text-sm gap-2">
							<Bell class="size-8 opacity-30" />
							<span>{{ t("notifications.empty") }}</span>
						</div>
						<div v-else class="flex flex-col divide-y">
							<div
								v-for="item in mergedNotifications"
								:key="item.source === 'backend' ? `b-${item.data.id}` : `f-${item.data.id}`"
								class="px-4 py-3 text-xs"
							>
								<template v-if="item.source === 'backend'">
									<p :class="notificationLevelClass(item.data.level)">
										{{ renderNotificationMessage(item.data) }}
									</p>
									<div class="flex items-center justify-between mt-1.5 gap-2">
										<span class="text-muted-foreground/60">
											{{ new Date(item.data.created_at).toLocaleString() }}
										</span>
										<div class="flex items-center gap-2 shrink-0">
											<button
												v-if="item.data.action"
												class="text-primary hover:underline"
												@click="executeNotificationAction(item.data)"
											>
												{{ t(`notifications.action.${item.data.action.action_type}`) }}
											</button>
											<button
												class="text-muted-foreground hover:text-foreground"
												@click="notificationsStore.markRead([item.data.id])"
											>
												<X class="size-3" />
											</button>
										</div>
									</div>
								</template>
								<template v-else>
									<p :class="frontendNotificationClass(item.data.type)">
										{{ item.data.message }}
									</p>
									<div class="flex items-center justify-between mt-1.5 gap-2">
										<span class="text-muted-foreground/60">
											{{ item.data.timestamp.toLocaleString() }}
										</span>
										<button
											class="text-muted-foreground hover:text-foreground shrink-0"
											@click="dismissFrontendNotification(item.data.id)"
										>
											<X class="size-3" />
										</button>
									</div>
								</template>
							</div>
						</div>
					</div>
				</DialogContent>
			</Dialog>

			<!-- 全局目录浏览器 / Global directory browser -->
			<DirectoryBrowserDialog />

			<NotifyLayer />
		</div>

	</Transition>
</template>
