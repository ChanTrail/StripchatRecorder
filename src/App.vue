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
	import { onMounted, onUnmounted } from "vue";
	import { RouterView, useRouter, useRoute } from "vue-router";
	import NotifyLayer from "./components/NotifyLayer.vue";
	import { Button } from "@/components/ui/button";
	import { call, on, onSseReconnect, onSseDisconnect } from "@/lib/api";
	import { useNotify } from "@/composables/useNotify";
	import { toast as sonnerToast } from "vue-sonner";
	import { useStreamersStore } from "@/stores/streamers";

	const router = useRouter();
	const route = useRoute();
	const { toast, confirm } = useNotify();
	const streamersStore = useStreamersStore();

	/** 侧边栏导航项配置 / Sidebar navigation items configuration */
	const navItems = [
		{ to: "/", label: "主播列表" },
		{ to: "/recordings", label: "录制文件" },
		{ to: "/postprocess", label: "后处理" },
		{ to: "/settings", label: "设置" },
	];

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
			await confirm({
				title: "发现不存在的主播",
				message: `以下主播账号不存在，将从列表中删除\n\n${w.missing_streamers.join("\n")}`,
				confirmText: "已知晓并删除",
				danger: true,
				hideCancelButton: true,
			});
			for (const username of w.missing_streamers) {
				await streamersStore.removeStreamer(username).catch(() => {});
			}
			toast(`已删除 ${w.missing_streamers.length} 个不存在的主播`, "success");
		}

		if (w.missing_pp_results.length > 0) {
			await confirm({
				title: "发现已删除文件的后处理记录",
				message: `以下文件已不存在，但仍有后处理记录，将进行清理\n\n${w.missing_pp_results.map((p) => p.split(/[\\/]/).pop()).join("\n")}`,
				confirmText: "已知晓并清理",
				hideCancelButton: true,
			});
			await call("remove_missing_pp_results", {
				paths: w.missing_pp_results,
			}).catch(() => {});
			toast(`已清理 ${w.missing_pp_results.length} 条后处理记录`, "success");
		}
	}

	onMounted(async () => {
		// 初始化主题并监听系统主题变化 / Initialize theme and listen for system theme changes
		applyTheme(mq.matches);
		mq.addEventListener("change", onThemeChange);

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
			sonnerToast.info(`已重新连接到服务器，${remaining} 秒后刷新页面…`, {
				id,
				duration: (COUNTDOWN + 1) * 1000,
			});
			const timer = setInterval(() => {
				remaining--;
				if (remaining > 0) {
					sonnerToast.info(`已重新连接到服务器，${remaining} 秒后刷新页面…`, {
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
			toast("与服务器的连接已断开，正在尝试重连…", "warning");
		});

		// 监听启动警告 / Listen for startup warnings
		unlistenWarnings = await on("startup-warnings", handleStartupWarnings);
	});

	onUnmounted(() => {
		// 清理所有事件监听器 / Clean up all event listeners
		mq.removeEventListener("change", onThemeChange);
		unlistenFfmpeg?.();
		unlistenReconnect?.();
		unlistenDisconnect?.();
		unlistenWarnings?.();
	});
</script>

<template>
	<div class="flex h-screen overflow-hidden">
		<aside
			class="w-44 shrink-0 bg-sidebar border-r border-sidebar-border flex flex-col p-3 gap-1"
		>
			<div
				class="flex items-center gap-2 px-1 py-4 mb-1 border-b border-sidebar-border"
			>
				<span class="w-2.5 h-2.5 rounded-full bg-destructive shrink-0" />
				<span class="text-sm font-bold text-sidebar-foreground"
					>StripchatRecorder</span
				>
			</div>
			<nav class="flex flex-col gap-0.5">
				<Button
					v-for="item in navItems"
					:key="item.to"
					variant="ghost"
					class="w-full justify-start text-sm font-normal"
					:class="
						route.path === item.to
							? 'bg-sidebar-accent text-sidebar-accent-foreground font-semibold'
							: 'text-sidebar-foreground/70 hover:text-sidebar-foreground hover:bg-sidebar-accent/50'
					"
					@click="router.push(item.to)"
				>
					{{ item.label }}
				</Button>
			</nav>
		</aside>
		<main class="flex-1 overflow-y-auto p-6">
			<RouterView />
		</main>
	</div>
	<NotifyLayer />
</template>
