<!--
    FFmpeg 下载/安装强制进度窗口 / FFmpeg Download/Install Forced Progress Dialog

    挂载于 App 根组件：订阅 `ffmpeg-progress` 事件，凡 `kind === "install"`
    的下载/安装任务（启动自动安装、首选项向导"下载"按钮、手动触发）都以
    强制窗口展示进度——不可跳过（无关闭按钮、Esc/遮罩点击被拦截），刷新
    页面后经 `get_ffmpeg_status` 恢复进行中的任务显示。
    - downloading：进度条 + `37% · a / b` 文本（total 未知时 1/3 宽脉冲）
    - installing：解压进度条（后端经 `Installing{pct}` 实时汇报）
    - done：成功提示 1.5 秒后自动关闭
    - error：错误信息 + 重试/关闭按钮
    标题随阶段同步切换（下载 / 校验并安装 / 安装完成 / 下载失败）。
    `kind === "update"` 的自动更新仍走全局进度 toast（非侵入），本窗口不显示。

    Mounted in the app root: subscribes to `ffmpeg-progress` events and shows
    every `kind === "install"` download/install task (startup auto-install,
    the preference wizard's Download button, manual triggers) in a forced
    window — non-dismissible (no close button, Esc/overlay clicks blocked),
    restored after a page refresh via `get_ffmpeg_status`.
    - downloading: progress bar + `37% · a / b` (1/3-wide pulse for unknown total)
    - installing: extraction progress bar (backend reports `Installing{pct}`)
    - done: success shown 1.5 s then auto-close
    - error: message + retry/close buttons
    The title switches with the phase (download / verify & install / done / failed).
    `kind === "update"` auto-updates keep using the global progress toast
    (non-intrusive) and are not shown here.
-->

<script setup lang="ts">
	import { computed, onMounted, onUnmounted, ref } from "vue";
	import { useRoute } from "vue-router";
	import { useI18n } from "vue-i18n";
	import { call, on, onSseReconnect } from "@/lib/api";
	import { formatSize } from "@/utils/format";
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import { Button } from "@/components/ui/button";
	import { AlertCircle, Check, Loader2 } from "@lucide/vue";

	const { t } = useI18n();
	const route = useRoute();

	/** ffmpeg-progress 事件载荷（与后端 `FfmpegProgress` 的 serde 输出一致）/ `ffmpeg-progress` event payload (matches the backend `FfmpegProgress` serde output) */
	interface FfmpegProgressPayload {
		state: "idle" | "downloading" | "installing" | "done" | "error";
		kind?: "install" | "update";
		downloaded?: number;
		total?: number;
		pct?: number | null;
		message?: string;
	}

	/** 当前展示的进度；非 null 时窗口显示 / Progress currently shown; the window is visible while non-null */
	const progress = ref<FfmpegProgressPayload | null>(null);
	// setup 向导页不弹强制窗口——进度由向导步骤内联展示 /
	// No forced window on the /setup wizard page — progress renders inline in the step
	const open = computed(() => progress.value !== null && route.path !== "/setup");

	/** 标题随阶段同步：下载 / 校验并安装 / 安装完成 / 下载失败 /
	 * Title synced with the phase: download / verify & install / done / failed */
	const titleKey = computed(() => {
		switch (progress.value?.state) {
			case "installing":
				return "ffmpeg.downloadDialog.titleInstalling";
			case "done":
				return "ffmpeg.downloadDialog.titleDone";
			case "error":
				return "ffmpeg.downloadDialog.titleFailed";
			default:
				return "ffmpeg.downloadDialog.title";
		}
	});

	/** 下载速度文本（`5.2 MB/s`）；非下载态清空 / Download speed text (`5.2 MB/s`); cleared outside the downloading state */
	const speedText = ref("");
	/** 速度采样点：相邻事件的字节数与时间戳 / Speed sample: bytes + timestamp of the previous event */
	let lastSample: { bytes: number; at: number } | null = null;

	/** ffmpeg-progress 事件的取消订阅函数 / Unsubscriber for the ffmpeg-progress event */
	let unlistenProgress: (() => void) | null = null;
	/** SSE 重连回调的取消注册函数 / Unsubscriber for the SSE-reconnect callback */
	let unlistenReconnect: (() => void) | null = null;
	/** 完成态的自动关闭定时器 / Auto-close timer for the done state */
	let doneTimer: ReturnType<typeof setTimeout> | null = null;

	/** 下载进度条：`pct` 驱动宽度（0–100）；`indeterminate` 为总大小未知且
	 * 已有数据时的 1/3 宽脉冲；尚未收到任何数据（准备中）显示空轨道即 0%。
	 * 文本为 `37% · a / b` 或已下载大小。
	 * Download bar: `pct` drives the width (0–100); `indeterminate` is a 1/3-wide
	 * pulse for real downloads with unknown total; with no bytes yet (preparing)
	 * the track stays empty at 0%. Text is `37% · a / b` or just the downloaded size. */
	const downloadBar = computed(() => {
		const p = progress.value;
		const empty = {
			pct: 0 as number | null,
			indeterminate: false,
			size: "",
			pctText: "",
		};
		if (!p || p.state !== "downloading") return empty;
		const downloaded = p.downloaded ?? 0;
		const total = p.total ?? 0;
		if (total > 0) {
			const pct = p.pct ?? 0;
			return {
				pct,
				indeterminate: false,
				size: `${formatSize(downloaded)} / ${formatSize(total)}`,
				pctText: `${pct}%`,
			};
		}
		// 总大小未知：无已下载量 = 准备中显示 0%；有数据 = 不确定态脉冲 /
		// Unknown total: no bytes yet = preparing → 0%; bytes flowing = indeterminate pulse
		if (downloaded === 0) return empty;
		return {
			pct: null,
			indeterminate: true,
			size: formatSize(downloaded),
			pctText: "",
		};
	});

	onMounted(async () => {
		unlistenProgress = await on("ffmpeg-progress", (payload) =>
			applyProgress(payload as FfmpegProgressPayload),
		);
		unlistenReconnect = onSseReconnect(() => void restoreStatus());
		await restoreStatus();
	});

	onUnmounted(() => {
		unlistenProgress?.();
		unlistenReconnect?.();
		if (doneTimer) clearTimeout(doneTimer);
	});

	/**
	 * 处理 ffmpeg-progress 事件；只接管 `kind === "install"` 的任务
	 * （update 交给全局 toast）。终态由事件驱动：Done 1.5 秒后自动关闭，
	 * Error 展示重试/关闭按钮，Idle 保持关闭。
	 * Handle a ffmpeg-progress event; only `kind === "install"` tasks are
	 * taken over (updates stay on the global toast). Terminal states are
	 * event-driven: Done auto-closes after 1.5 s, Error shows retry/close,
	 * Idle keeps the window closed.
	 *
	 * @param p - 事件载荷 / Event payload
	 */
	function applyProgress(p: FfmpegProgressPayload) {
		if (p.kind !== "install") return;
		switch (p.state) {
			case "downloading": {
				if (doneTimer) {
					clearTimeout(doneTimer);
					doneTimer = null;
				}
				// 速度 = 相邻事件的字节差/时间差（事件约 200ms 一跳）/
				// Speed = byte/time delta between events (~200 ms apart)
				const bytes = p.downloaded ?? 0;
				const now = Date.now();
				if (lastSample && bytes > lastSample.bytes) {
					const dt = (now - lastSample.at) / 1000;
					if (dt > 0.05) {
						speedText.value = `${formatSize((bytes - lastSample.bytes) / dt)}/s`;
					}
				}
				lastSample = { bytes, at: now };
				progress.value = p;
				break;
			}
			case "installing":
			case "error":
				if (doneTimer) {
					clearTimeout(doneTimer);
					doneTimer = null;
				}
				speedText.value = "";
				lastSample = null;
				progress.value = p;
				break;
			case "done":
				progress.value = p;
				if (doneTimer) clearTimeout(doneTimer);
				doneTimer = setTimeout(() => {
					doneTimer = null;
					progress.value = null;
				}, 1500);
				break;
			case "idle":
				break;
		}
	}

	/**
	 * 拉取当前进度状态，恢复进行中的安装任务显示（页面刷新、SSE 重连时调用）。
	 * Fetch the current progress to restore an in-flight install task (page
	 * refresh, SSE reconnect).
	 */
	async function restoreStatus() {
		try {
			const cur = await call<FfmpegProgressPayload>("get_ffmpeg_status");
			if (cur.state === "downloading" || cur.state === "installing") {
				applyProgress(cur);
			}
		} catch {
			// 状态接口失败忽略：等下一个 SSE 事件 / Ignore: wait for the next SSE event
		}
	}

	/**
	 * 下载失败后的重试：重新触发后台安装，SSE 事件随后覆盖占位状态。
	 * Retry after a failed download: re-triggers the background install; SSE
	 * events then overwrite the placeholder state.
	 */
	async function retry() {
		progress.value = { state: "downloading", kind: "install", downloaded: 0, total: 0, pct: null };
		try {
			await call("download_ffmpeg");
		} catch {
			progress.value = {
				state: "error",
				kind: "install",
				message: t("ffmpeg.downloadDialog.retryFailed"),
			};
		}
	}

	/**
	 * 错误态关闭窗口（后端终态已把进度重置为 Idle，这里只清本地状态）。
	 * Close the window on the error state (the backend already reset progress
	 * to Idle — this only clears local state).
	 */
	function close() {
		if (doneTimer) {
			clearTimeout(doneTimer);
			doneTimer = null;
		}
		speedText.value = "";
		lastSample = null;
		progress.value = null;
	}

	/**
	 * 阻止对话框关闭（Esc / 点击遮罩）：错误态请用"关闭"按钮。
	 * Block dialog dismissal (Esc / overlay click): on error use the Close button.
	 *
	 * @param e - 待拦截的键盘/指针事件 / Keyboard or pointer event to intercept
	 */
	function blockDismiss(e: Event | KeyboardEvent) {
		e.preventDefault();
	}
</script>

<template>
	<Dialog :open="open">
		<DialogContent
			class="sm:max-w-md focus:outline-none [&>button:last-child]:hidden"
			@escape-key-down="blockDismiss"
			@pointer-down-outside="blockDismiss"
			@interact-outside="blockDismiss"
		>
			<DialogHeader>
				<DialogTitle>{{ t(titleKey) }}</DialogTitle>
				<DialogDescription>{{ t("ffmpeg.downloadDialog.desc") }}</DialogDescription>
			</DialogHeader>

			<!-- 状态体：min-h 按下载/安装态的高度预留，done 态不再收缩高度 /
				State body: min-h reserves the downloading/installing height so the
				done state doesn't shrink the dialog -->
			<div class="flex flex-col gap-3 py-1 min-h-[72px]">
				<template v-if="progress?.state === 'downloading'">
					<div class="flex items-center gap-2 text-sm">
						<Loader2 class="size-4 animate-spin text-primary" />
						{{ t("ffmpeg.downloading") }}
					</div>
					<div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
						<div
							class="h-full bg-primary transition-all"
							:class="downloadBar.indeterminate ? 'w-1/3 animate-pulse' : ''"
							:style="downloadBar.indeterminate ? undefined : { width: `${downloadBar.pct}%` }"
						/>
					</div>
					<p class="flex min-h-4 items-baseline justify-between text-xs tabular-nums text-muted-foreground">
						<span>{{ downloadBar.size }}{{ speedText ? " · " + speedText : "" }}</span>
						<span>{{ downloadBar.pctText }}</span>
					</p>
				</template>
				<template v-else-if="progress?.state === 'installing'">
					<div class="flex items-center gap-2 text-sm">
						<Loader2 class="size-4 animate-spin text-primary" />
						{{ t("ffmpeg.installing") }}
					</div>
					<div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
						<div
							class="h-full bg-primary transition-all"
							:style="{ width: `${progress.pct ?? 0}%` }"
						/>
					</div>
					<p class="flex min-h-4 items-baseline justify-end text-xs tabular-nums text-muted-foreground">
						<span>{{ progress.pct != null ? `${progress.pct}%` : "" }}</span>
					</p>
				</template>
				<div
					v-else-if="progress?.state === 'done'"
					class="flex items-center gap-2 text-sm text-green-600 dark:text-green-500"
				>
					<Check class="size-4" />
					{{ t("ffmpeg.installed") }}
				</div>
				<template v-else-if="progress?.state === 'error'">
					<div class="flex items-center gap-2 text-sm text-destructive">
						<AlertCircle class="size-4 shrink-0" />
						{{ t("ffmpeg.downloadDialog.failed") }}
					</div>
					<p v-if="progress.message" class="max-h-24 overflow-y-auto break-all text-xs text-muted-foreground">
						{{ progress.message }}
					</p>
				</template>
			</div>

			<DialogFooter
				v-if="progress?.state === 'error'"
				class="flex items-center justify-end gap-2 sm:justify-end"
			>
				<Button variant="ghost" size="sm" @click="close">
					{{ t("ffmpeg.downloadDialog.close") }}
				</Button>
				<Button @click="retry">
					{{ t("ffmpeg.downloadDialog.retry") }}
				</Button>
			</DialogFooter>
		</DialogContent>
	</Dialog>
</template>
