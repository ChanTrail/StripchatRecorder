<!--
    转发流监控页面 / Relay Stream Monitor View

    展示所有已建立连接的转发流状态，包括：
    - 主播名、流状态（直播中/离线/连接中/错误）
    - 活跃连接数、运行时长
    - 流地址（可复制）

    Displays the status of all active relay streams, including:
    - Streamer name, stream state (live/offline/connecting/error)
    - Active connections, uptime
    - Stream URL (copyable)
-->
<script setup lang="ts">
	import { ref, onMounted, onUnmounted, computed } from "vue";
	import { call } from "@/lib/api";
	import { Badge } from "@/components/ui/badge";
	import { Button } from "@/components/ui/button";
	import { Card, CardContent } from "@/components/ui/card";
	import { Copy, Check, Radio, Wifi, WifiOff, AlertCircle, Loader } from "lucide-vue-next";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();

	interface StreamState {
		type: "connecting" | "live" | "offline" | "error";
		status?: string;
		message?: string;
	}

	interface RelaySession {
		username: string;
		stream_state: StreamState;
		active_connections: number;
		uptime_secs: number;
		stream_url: string;
	}

	const sessions = ref<RelaySession[]>([]);
	const loading = ref(true);
	const copiedMap = ref<Record<string, boolean>>({});
	let pollTimer: ReturnType<typeof setInterval> | null = null;

	async function fetchSessions() {
		try {
			sessions.value = await call<RelaySession[]>("list_relay_sessions");
		} catch {
			// 静默失败 / Fail silently
		} finally {
			loading.value = false;
		}
	}

	function formatUptime(secs: number): string {
		if (secs < 60) return `${secs}s`;
		if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
		const h = Math.floor(secs / 3600);
		const m = Math.floor((secs % 3600) / 60);
		return `${h}h ${m}m`;
	}

	function getStreamUrl(session: RelaySession): string {
		return `${window.location.origin}${session.stream_url}`;
	}

	async function copyUrl(username: string, url: string) {
		try {
			await navigator.clipboard.writeText(url);
			copiedMap.value[username] = true;
			setTimeout(() => {
				copiedMap.value[username] = false;
			}, 2000);
		} catch {}
	}

	function stateVariant(state: StreamState): string {
		switch (state.type) {
			case "live": return "bg-green-900 text-green-300 border-transparent";
			case "offline": return "bg-zinc-800 text-zinc-400 border-transparent";
			case "connecting": return "bg-blue-900 text-blue-300 border-transparent";
			case "error": return "bg-red-900 text-red-300 border-transparent";
			default: return "bg-zinc-800 text-zinc-400 border-transparent";
		}
	}

	function stateLabel(state: StreamState): string {
		switch (state.type) {
			case "live": return t("relay.state.live");
			case "offline": return state.status ? `${t("relay.state.offline")} · ${state.status}` : t("relay.state.offline");
			case "connecting": return t("relay.state.connecting");
			case "error": return t("relay.state.error");
			default: return state.type;
		}
	}

	const totalConnections = computed(() =>
		sessions.value.reduce((sum, s) => sum + s.active_connections, 0)
	);
	const exampleUrl = computed(() => `${window.location.origin}/stream/{modelname}`);

	onMounted(() => {
		fetchSessions();
		// 每 3 秒刷新一次 / Refresh every 3 seconds
		pollTimer = setInterval(fetchSessions, 3000);
	});

	onUnmounted(() => {
		if (pollTimer) clearInterval(pollTimer);
	});
</script>

<template>
	<div class="flex flex-col gap-5">
		<header class="flex items-start justify-between">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("relay.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{ t("relay.subtitle", { streams: sessions.length, connections: totalConnections }) }}
				</p>
			</div>
		</header>

		<!-- 说明卡片 / Info card -->
		<div class="rounded-lg border border-blue-900/40 bg-blue-950/20 px-4 py-3 text-sm text-blue-300/80">
			<p>{{ t("relay.hint") }}</p>
			<p class="mt-1 font-mono text-xs text-blue-400/60">
				{{ exampleUrl }}
			</p>
		</div>

		<div v-if="loading" class="text-center text-muted-foreground py-16">
			{{ t("common.loading") }}
		</div>

		<div
			v-else-if="sessions.length === 0"
			class="text-center text-muted-foreground py-16 flex flex-col items-center gap-2"
		>
			<Radio class="size-8 opacity-20" />
			<p>{{ t("relay.noSessions") }}</p>
			<p class="text-xs">{{ t("relay.noSessionsHint") }}</p>
		</div>

		<div
			v-else
			class="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-3.5"
		>
			<Card
				v-for="session in [...sessions].sort((a, b) => a.username.localeCompare(b.username))"
				:key="session.username"
				class="overflow-hidden py-0"
				:class="{
					'border-green-900/50': session.stream_state.type === 'live',
					'border-blue-900/50': session.stream_state.type === 'connecting',
					'border-red-900/50': session.stream_state.type === 'error',
				}"
			>
				<CardContent class="p-4 flex flex-col gap-3">
					<!-- 主播名 + 状态 / Username + state -->
					<div class="flex items-center justify-between gap-2">
						<div class="flex items-center gap-2 min-w-0">
							<!-- 状态图标 / State icon -->
							<component
								:is="session.stream_state.type === 'live' ? Wifi
									: session.stream_state.type === 'connecting' ? Loader
									: session.stream_state.type === 'error' ? AlertCircle
									: WifiOff"
								class="size-4 shrink-0"
								:class="{
									'text-green-400 animate-pulse': session.stream_state.type === 'live',
									'text-blue-400 animate-spin': session.stream_state.type === 'connecting',
									'text-red-400': session.stream_state.type === 'error',
									'text-zinc-500': session.stream_state.type === 'offline',
								}"
							/>
							<span class="font-semibold text-sm truncate">{{ session.username }}</span>
						</div>
						<Badge :class="stateVariant(session.stream_state)" class="shrink-0 text-xs">
							{{ stateLabel(session.stream_state) }}
						</Badge>
					</div>

					<!-- 统计信息 / Stats -->
					<div class="flex items-center gap-4 text-xs text-muted-foreground">
						<span class="flex items-center gap-1">
							<Radio class="size-3" />
							{{ t("relay.connections", { n: session.active_connections }) }}
						</span>
						<span>{{ t("relay.uptime", { t: formatUptime(session.uptime_secs) }) }}</span>
					</div>

					<!-- 流地址 + 复制按钮 / Stream URL + copy button -->
					<div class="flex items-center gap-2">
						<div
							class="flex-1 text-xs font-mono text-blue-400/70 bg-blue-950/20 rounded px-2 py-1.5 truncate select-all"
							:title="getStreamUrl(session)"
						>
							{{ getStreamUrl(session) }}
						</div>
						<Button
							size="sm"
							variant="ghost"
							class="shrink-0 px-2 h-7 text-muted-foreground hover:text-blue-300"
							:title="t('relay.copyUrl')"
							@click="copyUrl(session.username, getStreamUrl(session))"
						>
							<Check v-if="copiedMap[session.username]" class="size-3.5 text-green-400" />
							<Copy v-else class="size-3.5" />
						</Button>
					</div>
				</CardContent>
			</Card>
		</div>
	</div>
</template>
