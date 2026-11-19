<!--
    资源占用页 / Resource Usage View

    轮询 /api/process-usage（桌面版为同名 Tauri 命令）获取本程序进程及其
    全部后代（录制 ffmpeg、后处理模块、模块唤起的 ffmpeg 等）的 CPU/内存/
    磁盘/GPU 快照与系统网络速率。后端单次采集约 450ms（CPU 差分窗口），
    这里采用「请求 → 间隔 → 请求」的节奏串行轮询，不产生并发请求。

    布局：页头与汇总卡固定，进程表在自身区域内滚动（表头吸附），
    底部固定迷你趋势图（CPU/内存/磁盘/网络四路滚动窗口）。

    Polls /api/process-usage (same-named Tauri command on desktop) for a
    CPU/memory/disk/GPU snapshot of this process plus every descendant,
    plus system network rates. One backend collection takes ~450 ms (the
    CPU differential window), so we poll serially — request → interval →
    request — never overlapping.

    Layout: header + summary cards stay put, the process table scrolls
    inside its own region (sticky header), and a fixed bottom strip holds
    rolling mini-charts (CPU/memory/disk/network).
-->
<script setup lang="ts">
	import { ref, computed, onMounted, onUnmounted } from "vue";
	import { call } from "@/lib/api";
	import { Badge } from "@/components/ui/badge";
	import { Card, CardContent } from "@/components/ui/card";
	import {
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow,
	} from "@/components/ui/table";
	import {
		Activity, Cpu, Gpu, MemoryStick, HardDrive, Boxes, ArrowDownUp, Loader2,
	} from "@lucide/vue";
	import { useI18n } from "vue-i18n";
	import { formatSize, formatDuration } from "@/utils/format";

	const { t } = useI18n();

	/** 轮询间隔（后端单次采集 ~450ms，此处为响应后的停顿）/
	 * Poll interval (backend collection ~450 ms; this is the pause AFTER a response) */
	const LOOP_DELAY_MS = 1600;
	/** 趋势图滚动窗口样本数（~2s/样本 ≈ 2 分钟）/
	 * Rolling chart window size (~2 s/sample ≈ 2 minutes) */
	const HISTORY_MAX = 60;

	/** 进程角色 / Process role */
	type Role = "main" | "module" | "ffmpeg" | "other";

	/** 单个进程的资源占用条目 / One process' usage entry */
	interface ProcEntry {
		pid: number;
		ppid: number;
		name: string;
		exe: string;
		cmd: string;
		role: Role;
		depth: number;
		/** 100% = 单核满载 / 100% = one fully-loaded core */
		cpu_pct: number;
		mem_bytes: number;
		/** 磁盘读/写速率（字节/秒）/ Disk read/write rates (bytes/s) */
		disk_read_bps: number;
		disk_write_bps: number;
		/** GPU 引擎利用率（null = 不支持/无活动）/ GPU engine % (null = unsupported/idle) */
		gpu_pct: number | null;
		/** GPU 显存字节（null = 不可得）/ GPU VRAM bytes (null = unavailable) */
		gpu_mem_bytes: number | null;
		run_secs: number;
	}

	/** 后端返回的完整快照 / Snapshot as returned by the backend */
	interface UsageSnapshot {
		self_pid: number;
		cpu_count: number;
		sample_ms: number;
		collected_at: number;
		total_cpu_pct: number;
		total_mem_bytes: number;
		total_disk_read_bps: number;
		total_disk_write_bps: number;
		/** 全树 GPU 引擎利用率合计（null = 无 GPU 数据）/
		 * Tree GPU engine % total (null = no GPU data) */
		total_gpu_pct: number | null;
		/** 全树显存合计（null = 不可得）/ Tree VRAM total (null = unavailable) */
		total_gpu_mem_bytes: number | null;
		/** 系统网络下行/上行速率（字节/秒，整机口径）/
		 * System network RX/TX rates (bytes/s, whole-machine scope) */
		net_rx_bps: number;
		net_tx_bps: number;
		processes: ProcEntry[];
	}

	/** 趋势图单个采样点 / One chart history sample */
	interface HistorySample {
		/** 占全部核心的 CPU% / CPU% of all cores */
		cpu: number;
		/** 内存字节 / Memory bytes */
		mem: number;
		/** 全树 GPU 引擎利用率合计 / Tree GPU engine % total */
		gpu: number;
		/** 磁盘读/写速率 / Disk read/write rates */
		diskR: number;
		diskW: number;
		/** 网络收/发速率 / Network rx/tx rates */
		netR: number;
		netT: number;
	}

	const snapshot = ref<UsageSnapshot | null>(null);
	const history = ref<HistorySample[]>([]);
	const error = ref<string | null>(null);
	const running = ref(false);
	const lastRunAt = ref<number | null>(null);
	let timer: ReturnType<typeof setTimeout> | null = null;

	/** 角色徽标配色 / Role badge colors */
	const roleClass: Record<Role, string> = {
		main:   "text-sky-400 border-sky-500/40 bg-sky-500/10",
		module: "text-violet-400 border-violet-500/40 bg-violet-500/10",
		ffmpeg: "text-amber-400 border-amber-500/40 bg-amber-500/10",
		other:  "text-zinc-400 border-zinc-500/40 bg-zinc-500/10",
	};

	/** 进程数 / Process count */
	const procCount = computed(() => snapshot.value?.processes.length ?? 0);
	/** 总 CPU 占全部核心的比例（0–100）/ Tree CPU as share of all cores (0–100) */
	const totalCpuShare = computed(() => {
		const s = snapshot.value;
		if (!s || !s.cpu_count) return 0;
		return s.total_cpu_pct / s.cpu_count;
	});

	/** GPU 单元格悬停文本：附上显存（若有）/ GPU cell tooltip: append VRAM when known */
	function gpuTitle(p: ProcEntry): string {
		const parts = [t("usagePage.table.gpuHint") as string];
		if (p.gpu_mem_bytes != null) {
			parts.push(`${t("usagePage.table.vram")}: ${formatSize(p.gpu_mem_bytes)}`);
		}
		return parts.join(" · ");
	}

	/** 拉取一轮数据并把要点压入趋势缓冲 / Fetch one round and push chart sample */
	async function loop() {
		if (!running.value) return;
		try {
			const s = await call<UsageSnapshot>("process_usage");
			snapshot.value = s;
			error.value = null;
			lastRunAt.value = Date.now();
			history.value.push({
				cpu: s.cpu_count ? s.total_cpu_pct / s.cpu_count : 0,
				mem: s.total_mem_bytes,
				// null（平台不支持/全树无 GPU 活动）按 0 画线，数值仍显示 — /
				// null (unsupported/idle) draws as 0; the label still shows —
				gpu: s.total_gpu_pct ?? 0,
				diskR: s.total_disk_read_bps,
				diskW: s.total_disk_write_bps,
				netR: s.net_rx_bps,
				netT: s.net_tx_bps,
			});
			if (history.value.length > HISTORY_MAX) {
				history.value.splice(0, history.value.length - HISTORY_MAX);
			}
		} catch (e) {
			error.value = e instanceof Error ? e.message : String(e);
		}
		if (running.value) {
			timer = setTimeout(() => void loop(), LOOP_DELAY_MS);
		}
	}

	/** 生成 SVG 折线 points（按窗口内最大值自缩放）/
	 * Build an SVG polyline `points` string (autoscaled to the window max) */
	function sparkPoints(values: number[], w = 100, h = 26): string {
		if (values.length === 0) return "";
		const max = Math.max(...values, 1e-6);
		const step = values.length > 1 ? w / (values.length - 1) : 0;
		return values
			.map((v, i) => `${(i * step).toFixed(1)},${(h - (v / max) * (h - 1) - 0.5).toFixed(1)}`)
			.join(" ");
	}

	/** 取某字段的历史序列 / Field extractor for the history buffer */
	function series<K extends keyof HistorySample>(k: K): number[] {
		return history.value.map((s) => s[k]);
	}

	/** 格式化最后更新时刻 / Format the last-update clock time */
	const lastRunText = computed(() => {
		if (!lastRunAt.value) return "";
		return new Date(lastRunAt.value).toLocaleTimeString();
	});

	onMounted(() => {
		running.value = true;
		void loop();
	});

	onUnmounted(() => {
		running.value = false;
		if (timer) clearTimeout(timer);
	});
</script>

<template>
	<!-- h-full + 自滚动：页头/卡片/图表固定，仅表格区域滚动 /
	     h-full + self-scrolling: header/cards/chart pinned, only the table scrolls -->
	<div class="flex h-full flex-col gap-4 px-6 pt-6 pb-4 overflow-hidden">
		<header class="shrink-0">
			<h1 class="text-xl font-bold mb-0.5">{{ t("usagePage.title") }}</h1>
			<p class="text-sm text-muted-foreground">
				{{ t("usagePage.subtitle") }}
				<span v-if="running && lastRunAt" class="ml-2 text-xs opacity-70 inline-flex items-center gap-1">
					<Activity class="size-3 animate-pulse text-green-400" />
					{{ t("usagePage.live") }} · {{ lastRunText }}
				</span>
			</p>
		</header>

		<!-- 汇总卡片 / Summary cards -->
		<div class="shrink-0 grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3">
			<Card class="overflow-hidden py-0" :title="t('usagePage.table.cpuHint')">
				<CardContent class="p-3.5 flex items-center gap-3">
					<Cpu class="size-5 text-sky-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.totalCpu") }}</div>
						<div class="text-lg font-semibold tabular-nums leading-tight">
							{{ snapshot ? snapshot.total_cpu_pct.toFixed(1) + "%" : "—" }}
							<span v-if="snapshot" class="text-xs font-normal text-muted-foreground">
								{{ t("usagePage.summary.ofAllCores", { n: snapshot.cpu_count, pct: totalCpuShare.toFixed(1) }) }}
							</span>
						</div>
					</div>
				</CardContent>
			</Card>
			<Card class="overflow-hidden py-0" :title="t('usagePage.summary.gpuHint')">
				<CardContent class="p-3.5 flex items-center gap-3">
					<Gpu class="size-5 text-rose-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.gpu") }}</div>
						<div class="text-lg font-semibold tabular-nums leading-tight">
							{{ snapshot?.total_gpu_pct != null ? snapshot.total_gpu_pct.toFixed(1) + "%" : "—" }}
							<span v-if="snapshot?.total_gpu_mem_bytes != null" class="text-xs font-normal text-muted-foreground">
								{{ t("usagePage.table.vram") }} {{ formatSize(snapshot.total_gpu_mem_bytes!) }}
							</span>
						</div>
					</div>
				</CardContent>
			</Card>
			<Card class="overflow-hidden py-0">
				<CardContent class="p-3.5 flex items-center gap-3">
					<MemoryStick class="size-5 text-emerald-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.totalMem") }}</div>
						<div class="text-lg font-semibold tabular-nums leading-tight">
							{{ snapshot ? formatSize(snapshot.total_mem_bytes) : "—" }}
						</div>
					</div>
				</CardContent>
			</Card>
			<Card class="overflow-hidden py-0" :title="t('usagePage.table.diskHint')">
				<CardContent class="p-3.5 flex items-center gap-3">
					<HardDrive class="size-5 text-amber-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.disk") }}</div>
						<div class="text-sm font-semibold tabular-nums leading-tight whitespace-nowrap">
							<template v-if="snapshot">
								<span class="text-emerald-400">{{ t("usagePage.read") }} {{ formatSize(snapshot.total_disk_read_bps) }}/s</span>
								<span class="text-sky-400 ml-2">{{ t("usagePage.write") }} {{ formatSize(snapshot.total_disk_write_bps) }}/s</span>
							</template>
							<template v-else>—</template>
						</div>
					</div>
				</CardContent>
			</Card>
			<!-- 系统网络：OS 不按进程归属流量，只有整机口径，故如实标注 /
			     System network: the OS doesn't attribute traffic per-process,
			     so it's shown as whole-machine and labelled as such -->
			<Card class="overflow-hidden py-0" :title="t('usagePage.summary.netHint')">
				<CardContent class="p-3.5 flex items-center gap-3">
					<ArrowDownUp class="size-5 text-cyan-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.net") }}</div>
						<div class="text-sm font-semibold tabular-nums leading-tight whitespace-nowrap">
							<template v-if="snapshot">
								<span class="text-emerald-400">↓{{ formatSize(snapshot.net_rx_bps) }}/s</span>
								<span class="text-sky-400 ml-2">↑{{ formatSize(snapshot.net_tx_bps) }}/s</span>
							</template>
							<template v-else>—</template>
						</div>
					</div>
				</CardContent>
			</Card>
			<Card class="overflow-hidden py-0">
				<CardContent class="p-3.5 flex items-center gap-3">
					<Boxes class="size-5 text-violet-400 shrink-0" />
					<div class="min-w-0">
						<div class="text-xs text-muted-foreground">{{ t("usagePage.summary.procCount") }}</div>
						<div class="text-lg font-semibold tabular-nums leading-tight">
							{{ snapshot ? procCount : "—" }}
						</div>
					</div>
				</CardContent>
			</Card>
		</div>

		<!-- 错误提示（保留旧数据继续轮询）/ Error note (keeps old data, keeps polling) -->
		<p v-if="error" class="shrink-0 text-sm text-destructive">
			{{ t("usagePage.error", { msg: error }) }}
		</p>

		<!-- 进程表（独立滚动区，表头吸附）/
		     Process table (own scroll region, sticky header) -->
		<Card class="flex-1 min-h-0 overflow-hidden py-0">
			<div class="h-full overflow-y-auto">
				<Table>
					<TableHeader class="sticky top-0 z-10">
						<TableRow class="bg-card hover:bg-card">
							<TableHead class="w-[280px]">{{ t("usagePage.table.process") }}</TableHead>
							<TableHead class="w-20">{{ t("usagePage.table.pid") }}</TableHead>
							<TableHead class="w-20">{{ t("usagePage.table.role") }}</TableHead>
							<TableHead class="w-24" :title="t('usagePage.table.cpuHint')">
								{{ t("usagePage.table.cpu") }}
							</TableHead>
							<TableHead class="w-24" :title="t('usagePage.table.gpuHint')">
								{{ t("usagePage.table.gpu") }}
							</TableHead>
							<TableHead class="w-44" :title="t('usagePage.table.diskHint')">
								{{ t("usagePage.table.disk") }}
							</TableHead>
							<TableHead class="w-28">{{ t("usagePage.table.mem") }}</TableHead>
							<TableHead class="w-24">{{ t("usagePage.table.uptime") }}</TableHead>
							<TableHead>{{ t("usagePage.table.cmd") }}</TableHead>
						</TableRow>
					</TableHeader>
					<TableBody>
						<!-- 初次加载 / Initial load -->
						<TableRow v-if="!snapshot && !error">
							<TableCell colspan="9" class="text-center text-muted-foreground py-10">
								<Loader2 class="size-4 animate-spin inline-block mr-2 -mt-0.5" />
								{{ t("usagePage.loading") }}
							</TableCell>
						</TableRow>
						<TableRow v-else-if="snapshot && snapshot.processes.length === 0">
							<TableCell colspan="9" class="text-center text-muted-foreground py-10">
								{{ t("usagePage.empty") }}
							</TableCell>
						</TableRow>
						<TableRow
							v-for="p in snapshot?.processes ?? []"
							:key="p.pid"
							:class="{ 'bg-muted/40': p.role === 'main' }"
						>
							<!-- 进程名（按树深度缩进）/ Name (indented by tree depth) -->
							<TableCell>
								<div class="flex items-center gap-2 min-w-0 w-[280px]">
									<span
										class="shrink-0 border-l-2 border-border/60"
										:style="{ marginLeft: `${p.depth * 16}px`, height: '14px', width: '0' }"
									/>
									<span class="font-medium text-sm truncate" :title="p.exe || p.name">
										{{ p.name }}
									</span>
								</div>
							</TableCell>
							<TableCell class="tabular-nums text-sm text-muted-foreground">{{ p.pid }}</TableCell>
							<TableCell>
								<Badge variant="outline" :class="roleClass[p.role] ?? roleClass.other">
									{{ t(`usagePage.roles.${p.role}`) }}
								</Badge>
							</TableCell>
							<TableCell class="tabular-nums text-sm">{{ p.cpu_pct.toFixed(1) }}%</TableCell>
							<!-- GPU：无数据时 —，悬停见显存（若有）/
							     GPU: em-dash when unavailable, VRAM in tooltip when known -->
							<TableCell class="tabular-nums text-sm" :title="gpuTitle(p)">
								<template v-if="p.gpu_pct != null">{{ p.gpu_pct.toFixed(1) }}%</template>
								<span v-else class="text-muted-foreground">—</span>
							</TableCell>
							<!-- 磁盘读/写分列 / Disk read/write split -->
							<TableCell class="tabular-nums text-sm whitespace-nowrap">
								<span class="text-emerald-400">{{ t("usagePage.read") }} {{ formatSize(p.disk_read_bps) }}/s</span>
								<span class="text-sky-400 ml-1.5">{{ t("usagePage.write") }} {{ formatSize(p.disk_write_bps) }}/s</span>
							</TableCell>
							<TableCell class="tabular-nums text-sm">{{ formatSize(p.mem_bytes) }}</TableCell>
							<TableCell class="tabular-nums text-sm text-muted-foreground">
								{{ formatDuration(p.run_secs) }}
							</TableCell>
							<TableCell class="max-w-0">
								<span
									v-if="p.cmd"
									class="font-mono text-xs text-muted-foreground truncate block"
									:title="p.cmd"
								>{{ p.cmd }}</span>
							</TableCell>
						</TableRow>
					</TableBody>
				</Table>
			</div>
		</Card>

		<!-- 底部固定趋势图（滚动窗口 ~2 分钟）/
		     Pinned bottom chart strip (~2 min rolling window) -->
		<Card class="shrink-0 overflow-hidden py-0">
			<CardContent class="p-3 grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-x-4 gap-y-2">
				<!-- CPU -->
				<div class="min-w-0">
					<div class="flex items-baseline justify-between gap-2 text-xs">
						<span class="text-muted-foreground">CPU</span>
						<span class="tabular-nums font-medium text-sky-400">
							{{ snapshot ? totalCpuShare.toFixed(1) + "%" : "—" }}
						</span>
					</div>
					<svg viewBox="0 0 100 26" preserveAspectRatio="none" class="w-full h-7 text-sky-400">
						<polyline fill="none" stroke="currentColor" stroke-width="1.5"
							:points="sparkPoints(series('cpu'))" />
					</svg>
				</div>
				<!-- GPU -->
				<div class="min-w-0">
					<div class="flex items-baseline justify-between gap-2 text-xs">
						<span class="text-muted-foreground">GPU</span>
						<span class="tabular-nums font-medium text-rose-400">
							{{ snapshot?.total_gpu_pct != null ? snapshot.total_gpu_pct.toFixed(1) + "%" : "—" }}
						</span>
					</div>
					<svg viewBox="0 0 100 26" preserveAspectRatio="none" class="w-full h-7 text-rose-400">
						<polyline fill="none" stroke="currentColor" stroke-width="1.5"
							:points="sparkPoints(series('gpu'))" />
					</svg>
				</div>
				<!-- 内存 / Memory -->
				<div class="min-w-0">
					<div class="flex items-baseline justify-between gap-2 text-xs">
						<span class="text-muted-foreground">{{ t("usagePage.chart.mem") }}</span>
						<span class="tabular-nums font-medium text-violet-400">
							{{ snapshot ? formatSize(snapshot.total_mem_bytes) : "—" }}
						</span>
					</div>
					<svg viewBox="0 0 100 26" preserveAspectRatio="none" class="w-full h-7 text-violet-400">
						<polyline fill="none" stroke="currentColor" stroke-width="1.5"
							:points="sparkPoints(series('mem'))" />
					</svg>
				</div>
				<!-- 磁盘读/写 / Disk read/write -->
				<div class="min-w-0">
					<div class="flex items-baseline justify-between gap-2 text-xs">
						<span class="text-muted-foreground">{{ t("usagePage.chart.disk") }}</span>
						<span class="tabular-nums font-medium whitespace-nowrap">
							<span class="text-emerald-400">{{ t("usagePage.read") }} {{ snapshot ? formatSize(snapshot.total_disk_read_bps) + "/s" : "—" }}</span>
							<span class="text-sky-400 ml-1">{{ t("usagePage.write") }} {{ snapshot ? formatSize(snapshot.total_disk_write_bps) + "/s" : "—" }}</span>
						</span>
					</div>
					<svg viewBox="0 0 100 26" preserveAspectRatio="none" class="w-full h-7">
						<polyline fill="none" stroke="#34d399" stroke-width="1.5"
							:points="sparkPoints(series('diskR'))" />
						<polyline fill="none" stroke="#38bdf8" stroke-width="1.5"
							:points="sparkPoints(series('diskW'))" />
					</svg>
				</div>
				<!-- 网络收/发 / Network rx/tx -->
				<div class="min-w-0">
					<div class="flex items-baseline justify-between gap-2 text-xs">
						<span class="text-muted-foreground">{{ t("usagePage.chart.net") }}</span>
						<span class="tabular-nums font-medium whitespace-nowrap">
							<span class="text-emerald-400">↓{{ snapshot ? formatSize(snapshot.net_rx_bps) + "/s" : "—" }}</span>
							<span class="text-sky-400 ml-1">↑{{ snapshot ? formatSize(snapshot.net_tx_bps) + "/s" : "—" }}</span>
						</span>
					</div>
					<svg viewBox="0 0 100 26" preserveAspectRatio="none" class="w-full h-7">
						<polyline fill="none" stroke="#34d399" stroke-width="1.5"
							:points="sparkPoints(series('netR'))" />
						<polyline fill="none" stroke="#38bdf8" stroke-width="1.5"
							:points="sparkPoints(series('netT'))" />
					</svg>
				</div>
			</CardContent>
		</Card>
	</div>
</template>
