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
	import { useStreamersStore } from "@/stores/streamers";

	const { t } = useI18n();
	const streamersStore = useStreamersStore();

	/** 是否运行在 Tauri 桌面模式（relay 功能仅 server 模式可用）*/
	const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

	// 按用户名查找主播条目 / Find streamer entry by username
	function findStreamer(username: string) {
		return streamersStore.streamers.find(s => s.username === username) ?? null;
	}

	// 主播状态文字：在线显示状态，离线显示"离线" / Streamer status label
	function streamerStatusLabel(username: string): string {
		const s = findStreamer(username);
		if (!s) return t("streamerCard.offline");
		return s.is_online ? s.status : t("streamerCard.offline");
	}

	// 主播状态 Badge 内联样式 / Streamer status badge inline style
	function streamerStatusStyle(username: string): Record<string, string> {
		const s = findStreamer(username);
		if (!s?.is_online) {
			return { backgroundColor: "rgb(39 39 42)", color: "rgb(161 161 170)", borderColor: "transparent" };
		}
		if (s.status === "公开秀") {
			return { backgroundColor: "rgb(20 83 45)", color: "rgb(134 239 172)", borderColor: "transparent" };
		}
		return { backgroundColor: "rgb(120 53 15)", color: "rgb(252 211 77)", borderColor: "transparent" };
	}

	// Rust serde snake_case enum 序列化格式：
	// 无字段 variant → 字符串，如 "live" | "connecting"
	// 有字段 variant → 对象，如 { "offline": { "status": "..." } } | { "error": { "message": "..." } }
	type RawStreamState =
		| "connecting"
		| "live"
		| { offline: { status: string } }
		| { error: { message: string } };

	interface StreamState {
		type: "connecting" | "live" | "offline" | "error";
		status?: string;
		message?: string;
	}

	function parseStreamState(raw: RawStreamState): StreamState {
		if (raw === "connecting") return { type: "connecting" };
		if (raw === "live") return { type: "live" };
		if (typeof raw === "object" && "offline" in raw) return { type: "offline", status: raw.offline.status };
		if (typeof raw === "object" && "error" in raw) return { type: "error", message: raw.error.message };
		return { type: "offline" };
	}

	interface RelaySession {
		username: string;
		stream_state: RawStreamState;
		active_connections: number;
		uptime_secs: number;
		created_at_ms: number;
		stream_url: string;
	}

	// 解析后的会话（stream_state 已转换为统一格式）/ Parsed session with normalized stream_state
	type ParsedSession = Omit<RelaySession, "stream_state"> & { stream_state: StreamState };

	// 解析后的会话列表（stream_state 已转换为统一格式）
	const sessions = ref<ParsedSession[]>([]);
	const loading = ref(true);
	const copiedMap = ref<Record<string, boolean>>({});
	// 本地时钟 tick，每秒递增，用于驱动运行时间的响应式更新
	// Local clock tick, increments every second to drive reactive uptime updates
	const nowMs = ref(Date.now());
	let pollTimer: ReturnType<typeof setInterval> | null = null;
	let tickTimer: ReturnType<typeof setInterval> | null = null;

	async function fetchSessions() {
		try {
			const raw = await call<RelaySession[]>("list_relay_sessions");
			sessions.value = raw.map(s => ({ ...s, stream_state: parseStreamState(s.stream_state) }));
		} catch {
			// 静默失败 / Fail silently
		} finally {
			loading.value = false;
		}
	}

	// 根据 created_at_ms 实时计算运行时长（秒），每秒更新
	// Compute live uptime in seconds from created_at_ms, updated every second
	function liveUptime(session: ParsedSession): number {
		if (session.created_at_ms > 0) {
			return Math.max(0, Math.floor((nowMs.value - session.created_at_ms) / 1000));
		}
		// 回退到服务端值（兼容旧数据）/ Fall back to server value (backward compat)
		return session.uptime_secs;
	}

	function formatUptime(secs: number): string {
		if (secs < 60) return `${secs}s`;
		if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
		const h = Math.floor(secs / 3600);
		const m = Math.floor((secs % 3600) / 60);
		return `${h}h ${m}m`;
	}

	function getStreamUrl(session: ParsedSession): string {
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

	function stateVariant(state: StreamState): Record<string, string> {
		switch (state.type) {
			case "live":       return { backgroundColor: "rgb(20 83 45)",   color: "rgb(134 239 172)", borderColor: "transparent" };
			case "connecting": return { backgroundColor: "rgb(23 37 84)",   color: "rgb(147 197 253)", borderColor: "transparent" };
			case "error":      return { backgroundColor: "rgb(127 29 29)",  color: "rgb(252 165 165)", borderColor: "transparent" };
			default:           return { backgroundColor: "rgb(39 39 42)",   color: "rgb(161 161 170)", borderColor: "transparent" };
		}
	}

	function stateLabel(state: StreamState): string {
		switch (state.type) {
			case "live": return t("relay.state.live");
			case "offline": return state.status || t("relay.state.offline");
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
		if (isTauri) return; // desktop 模式无 relay 功能，不启动轮询
		fetchSessions();
		// 确保主播列表数据已加载（用于显示主播真实状态）
		// Ensure streamer list is loaded (for displaying real streamer status)
		if (streamersStore.streamers.length === 0) {
			void streamersStore.fetchStreamers();
		}
		// 每 5 秒轮询一次会话列表（仅更新状态/连接数，运行时间由本地计时器驱动）
		// Poll session list every 5s (only updates state/connections; uptime driven by local timer)
		pollTimer = setInterval(fetchSessions, 5000);
		// 每秒更新本地时钟，驱动运行时间实时刷新
		// Update local clock every second to drive live uptime refresh
		tickTimer = setInterval(() => { nowMs.value = Date.now(); }, 1000);
	});

	onUnmounted(() => {
		if (pollTimer) clearInterval(pollTimer);
		if (tickTimer) clearInterval(tickTimer);
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

		<!-- Desktop 模式不支持转发流 / Relay not available in desktop mode -->
		<div
			v-if="isTauri"
			class="rounded-lg border border-zinc-800 bg-zinc-900/40 px-4 py-8 text-center flex flex-col items-center gap-2"
		>
			<Radio class="size-8 opacity-20" />
			<p class="text-sm text-muted-foreground">{{ t("relay.desktopUnsupported") }}</p>
		</div>

		<template v-else>
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
			<p class="text-xs">{{ t("relay.noSessionsHint", { modelname: "{modelname}" }) }}</p>
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
						<div class="flex items-center gap-1.5 shrink-0">
							<!-- 主播真实状态（来自主播列表）/ Streamer real status (from streamer list) -->
							<Badge
								v-if="findStreamer(session.username)"
								variant="outline"
								:style="streamerStatusStyle(session.username)"
								class="text-xs"
							>
								{{ streamerStatusLabel(session.username) }}
							</Badge>
							<!-- 转发流内部状态 / Relay stream internal state -->
							<Badge variant="outline" :style="stateVariant(session.stream_state)" class="text-xs">
								{{ stateLabel(session.stream_state) }}
							</Badge>
						</div>
					</div>

					<!-- 统计信息 / Stats -->
					<div class="flex items-center gap-4 text-xs text-muted-foreground">
						<span class="flex items-center gap-1">
							<Radio class="size-3" />
							{{ t("relay.connections", { n: session.active_connections }) }}
						</span>
						<span>{{ t("relay.uptime", { t: formatUptime(liveUptime(session)) }) }}</span>
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
		</template><!-- end v-else (server mode) -->
	</div>
</template>
