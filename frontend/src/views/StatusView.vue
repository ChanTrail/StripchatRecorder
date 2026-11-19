<!--
    服务状态页 / Service Status View

    参考 edgetunnel 管理页的延迟测试设计：对每条目持续采样（探测一轮 →
    间隔 ~1.8s → 下一轮），滚动窗口保留 16 个样本，卡片内渲染 ECG 风格
    实时曲线，延时数字与曲线同步更新。

    - 前端组：浏览器直连 camgirlfinder、缩略图 img.doppiocdn 跨 TLD
      竞速、离线直播间图 static-proxy.strpst.com（均为浏览器真实走的链路）
    - 后端组：SC API（生产通道含 ECH/镜像）、SC 浏览页、GitHub
    - CDN 组：各 doppiocdn 域按角色分组并标注提供商

    Inspired by the edgetunnel admin latency test: samples are collected
    continuously (one probe round → ~1.8s pause → next round), a rolling
    window keeps 16 samples per item, each card renders a live ECG-style
    graph and the latency figure updates together with the graph.

    - frontend group: the browser's direct link to camgirlfinder, the
      thumbnail img.doppiocdn cross-TLD race, and the offline room image
      static-proxy.strpst.com (the real browser paths)
    - backend group: SC API (production channel), SC browse page, GitHub
    - CDN group: doppiocdn domains by role with provider labels
-->
<script setup lang="ts">
	import { ref, computed, onMounted, onUnmounted } from "vue";
	import { call } from "@/lib/api";
	import { THUMB_CDN_TLDS } from "@/composables/useFastThumbnail";
	import { Badge } from "@/components/ui/badge";
	import { Card, CardContent } from "@/components/ui/card";
	import { Loader2, Globe, Activity } from "@lucide/vue";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();

	/** camgirlfinder API 根（与 FinderView 同址）/ camgirlfinder API root (same as FinderView) */
	const CGF_API = "https://api.camgirlfinder.net";

	/** 一条探测腿的结果（普通或 ECH）/ one probe leg's outcome (plain or ECH) */
	interface LegStat {
		/** 可达 / reachable */
		ok: boolean;
		/** 耗时毫秒 / elapsed ms */
		latency_ms: number | null;
		/** 详情（HTTP 状态或错误摘要；悬停可见）/ detail (HTTP status or error summary; on hover) */
		detail: string;
	}

	/** 后端返回的探测条目：普通腿与 ECH 腿并列（对齐分片下载双腿竞速）/
	 * Probe item as returned by the backend: plain + ECH legs side by side
	 * (matching the segment download's dual-leg race) */
	interface StatusItem {
		/** 稳定 id（i18n 键后缀 statusPage.items.*）/ stable id (i18n key suffix) */
		id: string;
		/** 分组 / group id */
		group: "backend" | "cdn" | "frontend";
		/** CDN 条目角色 / CDN item role */
		role?: string;
		/** 实际请求的 URL / effective probed URL */
		url: string;
		/** 目标主机 / target host */
		host: string;
		/** 提供商键 / provider key */
		provider: string;
		/** 普通腿（非 ECH 通道；有代理时经代理）/ plain leg (non-ECH
		 * channel — rides the configured proxy when set) */
		direct: LegStat;
		/** ECH 腿（ECH 未启用/不适用时缺席——隐藏该腿）/
		 * ECH leg (absent when ECH is off or not applicable — the leg is hidden) */
		ech?: LegStat | null;
		/** 一次性探测项（镜像条目只测一轮）/ one-shot item (mirror entries are probed once) */
		once?: boolean;
	}

	interface ServiceStatusResp {
		elapsed_ms: number;
		items: StatusItem[];
	}

	/** 每条目滚动样本数（曲线横轴点数）/ rolling samples per item (graph points) */
	const SAMPLE_COUNT = 16;
	/** 探测轮间隔（上一轮结束后起算）/ delay between probe rounds (after the previous finishes) */
	const LOOP_DELAY_MS = 1800;
	/** 曲线纵轴封顶毫秒数（>=此值的点贴顶）/ graph y-axis cap in ms (points above clip at top) */
	const LAT_CAP_MS = 3000;
	const GRAPH_W = 400;
	const GRAPH_H = 60;
	const GRAPH_PAD = 6;
	/** 失败样本的落点高度（贴底基线）/ y position of failed samples (on the baseline) */
	const FAIL_Y = GRAPH_H - 5;

	const loading = ref(true);
	const running = ref(false);
	/** 后端条目持久表（id|host → 条目）；镜像条目在后续轮缺席时保留首轮
	 * 结果冻结展示 / persistent backend item table (id|host → item);
	 * mirror items keep their first-round result frozen when absent from
	 * later rounds */
	const itemMap = ref<Record<string, StatusItem>>({});
	const cgfItem = ref<StatusItem | null>(null);
	/** 浏览器侧缩略图竞速条目 / browser-side thumbnail race item */
	const thumbItem = ref<StatusItem | null>(null);
	/** 浏览器侧离线直播间图条目 / browser-side offline room-image item */
	const previewItem = ref<StatusItem | null>(null);
	const elapsedMs = ref(0);
	const lastRunAt = ref<Date | null>(null);
	/** 每条目滚动样本窗：d=普通腿、e=ECH 腿；>=0 毫秒，-1 失败 /
	 * rolling per-item sample windows: d=plain leg, e=ECH leg; ms, or -1
	 * for failure */
	const samples = ref<Record<string, { d: number[]; e: number[] }>>({});
	let roundInFlight = false;

	/** 条目唯一键（CDN 条目跨 TLD 共用 id，须含主机名）/ unique item key
	 * (CDN items share ids across TLDs — the host must be part of the key) */
	function keyOf(item: StatusItem): string {
		return `${item.id}|${item.host}`;
	}

	/** 浏览器侧探测 camgirlfinder（Finder 功能真实链路）/
	 * Browser-side probe of camgirlfinder (the real path the Finder uses) */
	async function probeCgf(): Promise<StatusItem> {
		const url = `${CGF_API}/`;
		const t0 = performance.now();
		try {
			const res = await fetch(url, {
				mode: "cors",
				signal: AbortSignal.timeout(10000),
			});
			return {
				id: "cgf",
				group: "frontend",
				url,
				host: new URL(url).host,
				provider: "",
				direct: {
					ok: res.status < 500,
					latency_ms: Math.round(performance.now() - t0),
					detail: `HTTP ${res.status}`,
				},
				ech: null,
			};
		} catch (e) {
			return {
				id: "cgf",
				group: "frontend",
				url,
				host: new URL(url).host,
				provider: "",
				direct: {
					ok: false,
					latency_ms: null,
					detail: String(e).slice(0, 160),
				},
				ech: null,
			};
		}
	}

	/** 浏览器侧探测离线直播间图：主播离线时卡片显示 previewUrl
	 * （static-proxy.strpst.com，API 返回的直链，useFastThumbnail 不
	 * 匹配竞速 TLD 故原样直连，无竞速）；no-cors 完成 HTTP 交换即算通 /
	 * Browser-side probe of the offline room image: offline cards show
	 * previewUrl (static-proxy.strpst.com — an API-returned direct link;
	 * useFastThumbnail matches no racing TLD so it loads as-is, no race);
	 * a completed no-cors exchange counts as reachable */
	async function probePreview(): Promise<StatusItem> {
		const host = "static-proxy.strpst.com";
		const url = `https://${host}/`;
		const t0 = performance.now();
		try {
			await fetch(url, {
				mode: "no-cors",
				signal: AbortSignal.timeout(10000),
			});
			return {
				id: "preview",
				group: "frontend",
				url,
				host,
				provider: "",
				direct: {
					ok: true,
					latency_ms: Math.round(performance.now() - t0),
					detail: host,
				},
				ech: null,
			};
		} catch (e) {
			return {
				id: "preview",
				group: "frontend",
				url,
				host,
				provider: "",
				direct: {
					ok: false,
					latency_ms: null,
					detail: String(e).slice(0, 160),
				},
				ech: null,
			};
		}
	}

	/** 浏览器侧探测缩略图：与 useFastThumbnail 同构的跨 TLD 竞速——
	 * 对 img.doppiocdn.{TLD} 并行发 no-cors GET，首个完成 HTTP 交换的
	 * 应答为胜（生产取最快加载成功的那一路；no-cors 读不到状态码，
	 * 交换能完成即算通）。条目 host 固定为通配形态以保证样本窗键稳定，
	 * 胜出的实际域写入 url/detail。
	 * Browser-side thumbnail probe: the same cross-TLD race as
	 * useFastThumbnail — fires a no-cors GET at every img.doppiocdn.{TLD}
	 * in parallel, first completed HTTP exchange wins (production takes
	 * the fastest successful load; no-cors hides the status so a
	 * completed exchange counts as reachable). The item's host is pinned
	 * to the wildcard form so the sample-window key stays stable; the
	 * winning domain lands in url/detail. */
	async function probeThumb(): Promise<StatusItem> {
		const host = "img.doppiocdn.*";
		const legs = THUMB_CDN_TLDS.map(async (tld) => {
			const url = `https://img.${tld}/`;
			const t0 = performance.now();
			await fetch(url, {
				mode: "no-cors",
				signal: AbortSignal.timeout(10000),
			});
			return { url, ms: Math.round(performance.now() - t0) };
		});
		try {
			const win = await Promise.any(legs);
			return {
				id: "thumb",
				group: "frontend",
				url: win.url,
				host,
				provider: "",
				direct: {
					ok: true,
					latency_ms: win.ms,
					detail: new URL(win.url).host,
				},
				ech: null,
			};
		} catch {
			return {
				id: "thumb",
				group: "frontend",
				url: `https://img.${THUMB_CDN_TLDS[0]}/`,
				host,
				provider: "",
				direct: {
					ok: false,
					latency_ms: null,
					detail: `all ${THUMB_CDN_TLDS.length} raced TLDs failed`,
				},
				ech: null,
			};
		}
	}

	/** 腿结果 → 样本值（>=0 毫秒，-1 失败）/ leg outcome → sample value
	 * (ms, or -1 for failure) */
	function legSample(l: LegStat): number {
		return l.ok && l.latency_ms !== null ? l.latency_ms : -1;
	}

	/** 样本入窗（滚动裁剪到 SAMPLE_COUNT）；一次性（镜像）条目只保留
	 * 首个样本；ECH 腿仅在条目返回 ech 对象时累积 /
	 * push a sample into the rolling window (trimmed to SAMPLE_COUNT);
	 * one-shot (mirror) items keep their first sample only; the ECH window
	 * grows only while the item reports an ech leg */
	function pushSample(item: StatusItem) {
		const s = (samples.value[keyOf(item)] ??= { d: [], e: [] });
		if (item.once && s.d.length >= 1) return;
		s.d.push(legSample(item.direct));
		if (s.d.length > SAMPLE_COUNT) s.d.shift();
		if (item.ech) {
			s.e.push(legSample(item.ech));
			if (s.e.length > SAMPLE_COUNT) s.e.shift();
		}
	}

	/** 首轮全量探测完成标记：拿到响应后轮次起改走 skip_mirrored，镜像
	 * 条目只测一次省流量 / set once the first full round got a response —
	 * later rounds ride skip_mirrored so mirrored items are probed just
	 * once (saving traffic) */
	let didFullRound = false;

	/** 执行一轮并行探测并把结果入样本窗 / run one parallel probe round and
	 * feed every result into the rolling sample windows */
	async function runRound() {
		if (roundInFlight || !running.value) return;
		roundInFlight = true;
		try {
			// 前端自测与后端端点并行 / the browser-side probes and the backend
			// endpoint run in parallel
			const [cgf, thumb, preview, resp] = await Promise.all([
				probeCgf(),
				probeThumb(),
				probePreview(),
				call<ServiceStatusResp>(
					"service_status",
					didFullRound ? { skip_mirrored: 1 } : undefined,
				).catch((e) => {
					console.error("service_status failed:", e);
					return null;
				}),
			]);
			if (!running.value) return;
			cgfItem.value = cgf;
			thumbItem.value = thumb;
			previewItem.value = preview;
			if (resp) {
				didFullRound = true;
				// 合并而非替换：缺席条目（被跳过的镜像项）保留旧值冻结展示 /
				// merge instead of replace: absent items (skipped mirrors)
				// keep their previous values frozen on screen
				for (const item of resp.items) {
					// 提供商首个非空识别即永久粘住：同一网络环境下应答
					// 厂商不会更换，而按轮重算遇到失败轮会回落空值导致
					// 徽标闪动 / the provider sticks at its first non-empty
					// identification: under one network environment the
					// answering vendor never changes, while recomputing
					// per round falls back to "" on failed rounds and the
					// badge flickers
					const prev = itemMap.value[keyOf(item)];
					if (prev?.provider) {
						item.provider = prev.provider;
					}
					itemMap.value[keyOf(item)] = item;
					pushSample(item);
				}
			} else {
				// 端点失败：给已知条目记失败样本，曲线前进显示"探测失败"而非
				// 冻结旧数据（一次性镜像条目仍冻结；ECH 腿仅在条目曾带 ech
				// 对象时同步记失败）/ endpoint failure: feed a failure sample
				// to every known item so graphs keep moving (honest "probe
				// failed") instead of freezing stale data; one-shot mirror
				// items stay frozen, and the ECH window only advances for
				// items that carried an ech leg
				for (const [k, item] of Object.entries(itemMap.value)) {
					if (item.once) continue;
					const s = (samples.value[k] ??= { d: [], e: [] });
					s.d.push(-1);
					if (s.d.length > SAMPLE_COUNT) s.d.shift();
					if (item.ech) {
						s.e.push(-1);
						if (s.e.length > SAMPLE_COUNT) s.e.shift();
					}
				}
			}
			elapsedMs.value = resp?.elapsed_ms ?? 0;
			lastRunAt.value = new Date();
			pushSample(cgf);
			pushSample(thumb);
			pushSample(preview);
			loading.value = false;
		} finally {
			roundInFlight = false;
		}
	}

	/** 持续采样循环：探测一轮 → 间隔 → 下一轮 / continuous sampling loop:
	 * probe a round → delay → next round */
	async function loop() {
		while (running.value) {
			await runRound();
			if (!running.value) break;
			await new Promise((r) => setTimeout(r, LOOP_DELAY_MS));
		}
	}

	/** 分组展示：前端 → 后端服务 → CDN 域 / display groups: frontend → backend → cdn */
	const groups = computed(() => {
		const all = Object.values(itemMap.value);
		const front = [cgfItem.value, thumbItem.value, previewItem.value].filter(
			(i): i is StatusItem => i !== null,
		);
		return [
			{ id: "frontend", items: front },
			{ id: "backend", items: all.filter((i) => i.group === "backend") },
			{ id: "cdn", items: all.filter((i) => i.group === "cdn") },
		];
	});

	/** CDN 角色排序 / CDN role display order */
	const CDN_ROLE_ORDER = ["playlist", "media", "node"];

	/** CDN 组内按角色再分小节（列表域/分片域/…），节内自适应网格排各域名 /
	 * the CDN group is sub-sectioned by role (playlist/segments/…); each
	 * subsection lays its domains out in an auto-fill grid */
	const cdnRoles = computed(() => {
		const byRole = new Map<string, StatusItem[]>();
		for (const it of Object.values(itemMap.value)) {
			if (it.group !== "cdn") continue;
			const r = it.role || "other";
			const arr = byRole.get(r) ?? [];
			arr.push(it);
			byRole.set(r, arr);
		}
		const ordered = CDN_ROLE_ORDER.filter((r) => byRole.has(r)).map(
			(r) => ({ role: r, items: byRole.get(r)! }),
		);
		// 未识别角色排在已知之后 / unrecognized roles trail the known ones
		for (const [r, items] of byRole) {
			if (!CDN_ROLE_ORDER.includes(r)) ordered.push({ role: r, items });
		}
		return ordered;
	});

	/** 腿键：d=普通，e=ECH / leg key: d=plain, e=ECH */
	type LegKey = "d" | "e";

	/** 条目某腿的展示行（ECH 腿缺席时不出现）/ per-item leg rows (the ECH
	 * row is absent when the item carries no ech leg) */
	function legViews(item: StatusItem): { key: LegKey; label: string }[] {
		const legs = [{ key: "d" as LegKey, label: t("statusPage.legs.direct") }];
		if (item.ech) legs.push({ key: "e", label: "ECH" });
		return legs;
	}

	/** 条目某腿的样本窗 / one leg's sample window */
	function windowOf(item: StatusItem, leg: LegKey): number[] {
		return samples.value[keyOf(item)]?.[leg] ?? [];
	}

	/** 窗口内有效样本的截尾均值（>5 样本去最高最低）；无有效样本返回 -1 /
	 * Trimmed mean of valid samples in the window (min/max dropped when >5);
	 * -1 when no valid sample exists */
	function avgLatency(item: StatusItem, leg: LegKey): number {
		const arr = windowOf(item, leg).filter((l) => l >= 0);
		if (!arr.length) return -1;
		const s = [...arr].sort((a, b) => a - b);
		const t = s.length > 5 ? s.slice(1, -1) : s;
		return Math.round(t.reduce((a, b) => a + b, 0) / t.length);
	}

	/** 延时档位（文本着色用）/ latency tier (for text coloring) */
	function latencyClass(ms: number): string {
		if (ms < 0) return "text-red-400";
		if (ms < 800) return "text-green-400";
		if (ms < 3000) return "text-amber-300";
		return "text-red-300";
	}

	/** 延时档位对应色值（SVG 描边/光标用）/ latency tier color value
	 * (for SVG stroke/cursor) */
	function tierColorHex(ms: number): string {
		if (ms < 0) return "#f87171";
		if (ms < 800) return "#4ade80";
		if (ms < 3000) return "#fcd34d";
		return "#fca5a5";
	}

	/** 样本窗 → 曲线坐标点：延时映射 0..LAT_CAP 到纵轴（越高越大），失败点
	 * 贴底基线 / sample window → graph points: latency maps 0..LAT_CAP onto
	 * the y axis (higher = higher up); failures sit on the baseline */
	function graphPoints(item: StatusItem, leg: LegKey): { x: number; y: number }[] {
		const arr = windowOf(item, leg);
		const step = GRAPH_W / (SAMPLE_COUNT - 1);
		return arr.map((l, i) => ({
			x: i * step,
			y:
				l < 0
					? FAIL_Y
					: GRAPH_H -
						GRAPH_PAD -
						(Math.min(l, LAT_CAP_MS) / LAT_CAP_MS) *
							(GRAPH_H - 2 * GRAPH_PAD),
		}));
	}

	/** 二次贝塞尔平滑的曲线路径 / Bezier-smoothed graph path */
	function graphPath(item: StatusItem, leg: LegKey): string {
		const pts = graphPoints(item, leg);
		if (!pts.length) return "";
		let d = `M${pts[0].x},${pts[0].y}`;
		for (let i = 0; i < pts.length - 1; i++) {
			const mx = (pts[i].x + pts[i + 1].x) / 2;
			const my = (pts[i].y + pts[i + 1].y) / 2;
			d += ` Q${pts[i].x},${pts[i].y} ${mx},${my}`;
		}
		const last = pts[pts.length - 1];
		d += ` L${last.x},${last.y}`;
		return d;
	}

	/** 末点光标位置（无样本返回 null）/ cursor position of the last point
	 * (null when empty) */
	function cursorPos(item: StatusItem, leg: LegKey): { x: number; y: number } | null {
		const pts = graphPoints(item, leg);
		return pts.length ? pts[pts.length - 1] : null;
	}

	/** 标题状态点颜色：对齐生产竞速的「最佳线路」语义——两腿取均值延时
	 * 更低者对应的档位色（生产下载会走更快的那条，卡片状态自然以它为
	 * 准）；失败/缺席腿按无穷大不参与比较；两腿皆失败为红 /
	 * headline dot color: mirrors production racing's "best route"
	 * semantics — the lower of the two legs' average latencies picks the
	 * tier color (downloads ride the faster leg, so the card's status is
	 * defined by it); failed/absent legs count as +∞ and never win;
	 * both legs failed → red */
	function headColor(item: StatusItem): string {
		const d = avgLatency(item, "d");
		const e = avgLatency(item, "e");
		const best = Math.min(d < 0 ? Infinity : d, e < 0 ? Infinity : e);
		return tierColorHex(best === Infinity ? -1 : best);
	}

	/** 卡片悬停提示：目标 URL + 各腿详情（失败摘要也在此）/
	 * card hover tooltip: target URL plus each leg's detail (failure
	 * summaries live here) */
	function titleOf(item: StatusItem): string {
		const parts = [item.direct.ok ? item.url : item.direct.detail];
		if (item.ech) parts.push(`ECH: ${item.ech.detail}`);
		return parts.join("\n");
	}

	/** 提供商徽标文案 / provider badge label */
	function providerLabel(p: string): string {
		return p ? t(`statusPage.providers.${p}`) : "";
	}

	onMounted(() => {
		running.value = true;
		void loop();
	});

	onUnmounted(() => {
		running.value = false;
	});
</script>

<template>
	<div class="flex flex-col gap-5 px-6 pt-6 pb-6">
		<header>
			<h1 class="text-xl font-bold mb-0.5">{{ t("statusPage.title") }}</h1>
			<p class="text-sm text-muted-foreground">
				{{ t("statusPage.subtitle") }}
				<span v-if="running && lastRunAt" class="ml-2 text-xs opacity-70 inline-flex items-center gap-1">
					<Activity class="size-3 animate-pulse text-green-400" />
					{{ t("statusPage.live") }}
				</span>
			</p>
		</header>

		<!-- 分组 / Groups -->
		<template v-for="g in groups" :key="g.id">
			<!-- 前端/后端组：卡片网格（与 CDN 卡片同款）/
			     frontend & backend groups: card grid (same card style as CDN) -->
			<section v-if="g.id !== 'cdn'" class="flex flex-col gap-2">
				<h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
					{{ t(`statusPage.groups.${g.id}`) }}
				</h2>

				<div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-3">
					<Card
						v-for="item in g.items"
						:key="item.id + item.host"
						class="overflow-hidden py-0"
						:title="titleOf(item)"
					>
						<CardContent class="p-3.5 flex flex-col gap-2">
							<!-- 名称行：状态点 + 名称 + 徽标 / title row: dot + name + badges -->
							<div class="flex items-center gap-2">
								<span
									class="size-2 rounded-full shrink-0"
									:class="{ 'bg-zinc-500 animate-pulse': loading }"
									:style="loading ? undefined : { backgroundColor: headColor(item) }"
								/>
								<span class="font-medium text-sm truncate">
									{{ t(`statusPage.items.${item.id}`) }}
								</span>
								<Badge
									v-if="providerLabel(item.provider)"
									variant="secondary"
									class="text-[10px] px-1.5 py-0 h-4 font-normal shrink-0"
								>
									{{ providerLabel(item.provider) }}
								</Badge>
								<Badge
									v-if="item.once"
									variant="outline"
									class="text-[10px] px-1.5 py-0 h-4 font-normal shrink-0"
								>
									{{ t("statusPage.mirror") }}
								</Badge>
							</div>
							<!-- 主机名（单行截断，完整名在悬停提示）/
							     host (single line truncated, full name on hover) -->
							<div
								class="font-mono text-xs text-muted-foreground truncate"
								:title="item.host"
							>
								{{ item.host }}
							</div>
							<!-- 普通 / ECH 双腿：各一条实时曲线 + 延时（条目不带
							     ech 腿时仅显示普通腿）/ plain + ECH legs: one live
							     graph + latency each (plain only when the item
							     carries no ech leg) -->
							<div
								class="grid gap-3"
								:class="item.ech ? 'grid-cols-2' : 'grid-cols-1'"
							>
								<div
									v-for="leg in legViews(item)"
									:key="leg.key"
									class="flex flex-col gap-1 min-w-0"
								>
									<span class="text-[10px] uppercase tracking-wide text-muted-foreground">
										{{ leg.label }}
									</span>
									<svg
										class="w-full h-10"
										viewBox="0 0 400 60"
										preserveAspectRatio="none"
									>
										<path
											:d="`M0,${FAIL_Y} L${GRAPH_W},${FAIL_Y}`"
											fill="none"
											stroke="currentColor"
											stroke-width="1"
											class="text-border/60"
										/>
										<path
											:d="graphPath(item, leg.key)"
											fill="none"
											:stroke="tierColorHex(avgLatency(item, leg.key))"
											stroke-width="2"
											stroke-linecap="round"
										/>
										<circle
											v-if="cursorPos(item, leg.key)"
											:cx="cursorPos(item, leg.key)!.x"
											:cy="cursorPos(item, leg.key)!.y"
											r="3.5"
											:fill="tierColorHex(avgLatency(item, leg.key))"
										/>
									</svg>
									<div
										class="font-mono text-sm"
										:class="latencyClass(avgLatency(item, leg.key))"
									>
										<template v-if="avgLatency(item, leg.key) >= 0">
											{{ avgLatency(item, leg.key) }}<span class="text-xs">ms</span>
										</template>
										<span v-else-if="!loading" class="text-xs">—</span>
										<span v-else class="text-xs">…</span>
									</div>
								</div>
							</div>
						</CardContent>
					</Card>
				</div>

				<!-- 探测中占位 / probing placeholder -->
				<div
					v-if="loading && g.items.length === 0"
					class="flex items-center gap-2 px-1 py-2 text-sm text-muted-foreground"
				>
					<Loader2 class="size-3.5 animate-spin" />
					{{ t("statusPage.probing") }}
				</div>
				<!-- 组空态 / empty group -->
				<div
					v-else-if="!loading && g.items.length === 0"
					class="px-1 py-2 text-sm text-muted-foreground"
				>
					{{ t("statusPage.empty") }}
				</div>
			</section>

			<!-- CDN 组：先按角色分小节，节内自适应网格排各域名 /
			     CDN group: role subsections first, domains in an auto-fill
			     grid inside each -->
			<section v-else class="flex flex-col gap-3">
				<h2 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide">
					{{ t("statusPage.groups.cdn") }}
				</h2>

				<div
					v-for="r in cdnRoles"
					:key="r.role"
					class="flex flex-col gap-2"
				>
					<h3 class="text-xs font-medium text-muted-foreground">
						{{ t(`statusPage.roles.${r.role}`) }}
					</h3>
					<div class="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-3">
						<Card
							v-for="item in r.items"
							:key="item.id + item.host"
							class="overflow-hidden py-0"
							:title="titleOf(item)"
						>
							<CardContent class="p-3.5 flex flex-col gap-2">
								<!-- 标题行：状态点 + 域名（单行截断）+ 提供商 /
								     title row: dot + domain (single line) + provider -->
								<div class="flex items-center gap-2">
									<span
										class="size-2 rounded-full shrink-0"
										:class="{ 'bg-zinc-500 animate-pulse': loading }"
										:style="loading ? undefined : { backgroundColor: headColor(item) }"
									/>
									<span
										class="font-mono text-sm font-medium truncate flex-1 min-w-0"
										:title="item.host"
									>
										{{ item.host }}
									</span>
									<Badge
										v-if="providerLabel(item.provider)"
										variant="secondary"
										class="text-[10px] px-1.5 py-0 h-4 font-normal shrink-0"
									>
										{{ providerLabel(item.provider) }}
									</Badge>
									<!-- 节点分片条目只抽测一个节点主机，徽标标明 /
									     the node-segment item probes a single node
									     host — badge marks it as a sample -->
									<Badge
										v-if="item.id === 'cdnNode'"
										variant="outline"
										class="text-[10px] px-1.5 py-0 h-4 font-normal shrink-0"
										:title="t('statusPage.sampleHint')"
									>
										{{ t("statusPage.sample") }}
									</Badge>
								</div>
								<!-- 条目角色名（占位与后端卡片的主机行对齐，保证
								     所有卡片等高）/ item role name — occupies the
								     same slot as the backend cards' host row so all
								     cards share one height -->
								<div class="text-xs text-muted-foreground truncate">
									{{ t(`statusPage.items.${item.id}`) }}
								</div>
								<!-- 普通 / ECH 双腿：各一条实时曲线 + 延时 /
								     plain + ECH legs: one live graph + latency each -->
								<div
									class="grid gap-3"
									:class="item.ech ? 'grid-cols-2' : 'grid-cols-1'"
								>
									<div
										v-for="leg in legViews(item)"
										:key="leg.key"
										class="flex flex-col gap-1 min-w-0"
									>
										<span class="text-[10px] uppercase tracking-wide text-muted-foreground">
											{{ leg.label }}
										</span>
										<svg
											class="w-full h-10"
											viewBox="0 0 400 60"
											preserveAspectRatio="none"
										>
											<path
												:d="`M0,${FAIL_Y} L${GRAPH_W},${FAIL_Y}`"
												fill="none"
												stroke="currentColor"
												stroke-width="1"
												class="text-border/60"
											/>
											<path
												:d="graphPath(item, leg.key)"
												fill="none"
												:stroke="tierColorHex(avgLatency(item, leg.key))"
												stroke-width="2"
												stroke-linecap="round"
											/>
											<circle
												v-if="cursorPos(item, leg.key)"
												:cx="cursorPos(item, leg.key)!.x"
												:cy="cursorPos(item, leg.key)!.y"
												r="3.5"
												:fill="tierColorHex(avgLatency(item, leg.key))"
											/>
										</svg>
										<div
											class="font-mono text-sm"
											:class="latencyClass(avgLatency(item, leg.key))"
										>
											<template v-if="avgLatency(item, leg.key) >= 0">
												{{ avgLatency(item, leg.key) }}<span class="text-xs">ms</span>
											</template>
											<span v-else-if="!loading" class="text-xs">—</span>
											<span v-else class="text-xs">…</span>
										</div>
									</div>
								</div>
							</CardContent>
						</Card>
					</div>
				</div>

				<!-- 探测中占位 / probing placeholder -->
				<div
					v-if="loading && cdnRoles.length === 0"
					class="flex items-center gap-2 px-1 py-2 text-sm text-muted-foreground"
				>
					<Loader2 class="size-3.5 animate-spin" />
					{{ t("statusPage.probing") }}
				</div>
				<div
					v-else-if="!loading && cdnRoles.length === 0"
					class="px-1 py-2 text-sm text-muted-foreground"
				>
					{{ t("statusPage.empty") }}
				</div>
			</section>
		</template>

		<!-- 页脚说明 / footnote -->
		<p class="text-xs text-muted-foreground/70 flex items-center gap-1.5">
			<Globe class="size-3.5" />
			{{ t("statusPage.footnote") }}
		</p>
	</div>
</template>
