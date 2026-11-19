/**
 * FFmpeg 下载/安装进度 toast / FFmpeg Download/Install Progress Toast
 *
 * 订阅后端的 `ffmpeg-progress` 事件，用一条固定 id 的 sonner toast 实时显示
 * 下载/安装进度（同一 id 复用更新而不是堆叠新 toast）。
 * `kind === "install"` 的安装类任务不弹 toast——由全局强制进度窗口
 * （FFmpegDownloadDialog）展示；这里只处理 `kind === "update"` 的自动更新。
 * - downloading：标题随 kind 区分"下载/更新"，description 为进度条组件
 *   （含百分比与已下载/总大小文本；total 未知时只显示已下载大小并脉冲动画）
 * - installing：同一进度条组件（后端解压回调按 0–100% 发 `Installing{pct}`；
 *   pct 未知时脉冲）。两形态都传 `classes.content = "grow"`——sonner 的
 *   `data-content` 是按内容收缩的 flex item，不撑满 toast 行宽会导致
 *   `w-full` 进度条比文本还窄
 * - done：success toast（5 秒自动消失）
 * - error / idle：仅关闭 toast——后端已推送持久化错误通知
 *   （通知面板与通知 toast 都会展示），不再重复弹错误 toast
 *
 * `setupFfmpegProgressToast` 注册事件监听后还会拉一次 `get_ffmpeg_status`，
 * 让晚接入的客户端能恢复一个正在进行中的下载显示；返回取消订阅函数。
 *
 * Subscribes to the backend `ffmpeg-progress` event and shows the download/install
 * progress in one fixed-id sonner toast (reusing the id updates in place instead
 * of stacking toasts). `kind === "install"` tasks never toast — the global
 * forced window (FFmpegDownloadDialog) shows them; only `kind === "update"`
 * auto-updates are handled here.
 * - downloading: title varies by kind ("downloading"/"updating"), description is
 *   a progress-bar component (percent + downloaded/total text; with unknown total
 *   only the downloaded size shows, with a pulse animation)
 * - installing: the same progress-bar component (the backend extraction
 *   callback emits `Installing{pct}` 0–100; pulses when pct is unknown).
 *   Both shapes pass `classes.content = "grow"` — sonner's `data-content`
 *   is a content-sized flex item that doesn't stretch to the toast row,
 *   which would shrink the `w-full` bar below the text width
 * - done: success toast (auto-dismiss after 5 s)
 * - error / idle: only dismisses the toast — the backend already pushes a
 *   persistent error notification (shown by both the panel and notification
 *   toasts), so no second error toast
 *
 * `setupFfmpegProgressToast` registers the listener, then fetches
 * `get_ffmpeg_status` once so late-connecting clients can restore an in-progress
 * download display; returns the unsubscribe function.
 */

import { h, reactive } from "vue";
import { toast as sonnerToast } from "vue-sonner";
import { call, on } from "@/lib/api";
import { formatSize } from "@/utils/format";

/** toast 固定 id（复用同一 toast 更新，避免堆叠）/ Fixed toast id (reuse to update in place) */
const TOAST_ID = "ffmpeg-progress";
/**
 * installing/done 形态的 toast id（与 downloading 形状不同——无下载
 * 尺寸文本/无 description）。vue-sonner 在挂载时量一次高度写入 --initial-height，
 * 悬停展开时强制采用该值；同 id 从「带进度条的高 toast」更新成「无
 * description 的矮 toast」后初始高度不会复测，悬停即上下膨胀。因此
 * 形状变化必须换新 id，让新 toast 重新测量。
 *
 * Toast ids for the installing/done shapes (different from downloading —
 * no size text / no description). vue-sonner measures height once at mount
 * into --initial-height and forces that value on hover-expand; updating
 * the same id from a tall progress-bar toast to a short description-less
 * one never re-measures, so hovering inflates it back to the stale
 * height. A new shape therefore gets a new id so it measures fresh.
 */
const INSTALLING_TOAST_ID = "ffmpeg-progress-installing";
const DONE_TOAST_ID = "ffmpeg-progress-done";
/** 全部 toast 形态 id（终态/清理时统一 dismiss）/ All toast-shape ids (dismissed together on terminal states) */
const ALL_TOAST_IDS = [TOAST_ID, INSTALLING_TOAST_ID, DONE_TOAST_ID];

/**
 * `ffmpeg-progress` 事件载荷（与后端 `FfmpegProgress` 的 serde 输出一致；
 * `{"state":"idle"}` 之外的状态都带 `kind`）。
 * Payload of the `ffmpeg-progress` event (matches the backend `FfmpegProgress`
 * serde output; every non-`idle` state carries `kind`).
 */
interface FfmpegProgressPayload {
	state: "idle" | "downloading" | "installing" | "done" | "error";
	kind?: "install" | "update";
	downloaded?: number;
	total?: number;
	pct?: number | null;
	message?: string;
}

/** 进度条组件读取的共享状态（组件身份稳定，只改数据不重建）。排版与强制
 * 进度窗口一致：左 `大小 · 速度`，右百分比 / Shared state read by the
 * progress-bar component (stable component identity — data changes, no
 * remount). Layout mirrors the forced progress window: `size · speed` on
 * the left, percentage on the right */
const bar = reactive({
	pct: null as number | null,
	size: "",
	speed: "",
	pctText: "",
});

/** 速度采样点：相邻事件的字节数与时间戳（与强制进度窗口同算法）/ Speed
 * sample: bytes + timestamp of the previous event (same algorithm as the
 * forced progress window) */
let lastSample: { bytes: number; at: number } | null = null;

/**
 * 根据下载事件更新进度条状态：左 `41.2 MB / 110.5 MB · 5.2 MB/s`、右 `37%`；
 * total 为 0（大小未知）时只显示已下载大小并启用脉冲动画。
 * Update the progress-bar state from a download event: `41.2 MB /
 * 110.5 MB · 5.2 MB/s` on the left, `37%` on the right; when total is 0
 * (unknown size) only the downloaded size shows and the bar pulses.
 */
function updateBar(p: FfmpegProgressPayload) {
	const downloaded = p.downloaded ?? 0;
	const total = p.total ?? 0;
	bar.pct = total > 0 ? (p.pct ?? null) : null;
	bar.size =
		total > 0 ? `${formatSize(downloaded)} / ${formatSize(total)}` : formatSize(downloaded);
	bar.pctText = bar.pct !== null ? `${bar.pct}%` : "";
	// 速度 = 相邻事件的字节差/时间差（事件约 200ms 一跳）/ Speed = byte/time
	// delta between consecutive events (~200 ms apart)
	const now = Date.now();
	if (lastSample && downloaded > lastSample.bytes) {
		const dt = (now - lastSample.at) / 1000;
		if (dt > 0.05) {
			bar.speed = `${formatSize((downloaded - lastSample.bytes) / dt)}/s`;
		}
	}
	lastSample = { bytes: downloaded, at: now };
}

/**
 * 进度条函数组件：创建一次后身份不再变化（读取模块级 reactive 状态），
 * Vue 不会在每次 toast 更新时重新挂载。
 * Functional progress-bar component: created once and never changes identity
 * (reads the module-level reactive state), so Vue doesn't remount it on each
 * toast update.
 */
const ProgressBody = () =>
	h("div", { class: "w-full" }, [
		h("div", { class: "h-1.5 w-full rounded-full bg-muted overflow-hidden mt-1.5" }, [
			h("div", {
				class: [
					"h-full bg-primary transition-all",
					bar.pct === null ? "w-full animate-pulse" : "",
				],
				style: bar.pct === null ? undefined : { width: `${bar.pct}%` },
			}),
		]),
		h(
			"div",
			{ class: "flex items-baseline justify-between text-xs text-muted-foreground tabular-nums" },
			[
				h("span", bar.speed ? `${bar.size} · ${bar.speed}` : bar.size),
				h("span", bar.pctText),
			],
		),
	]);

/**
 * 注册 `ffmpeg-progress` 事件监听并恢复进行中的状态，返回取消订阅函数。
 * 在 App 根组件 `onMounted` 末尾调用、`onUnmounted` 中调用返回值。
 *
 * Register the `ffmpeg-progress` listener and restore in-progress state; returns
 * the unsubscribe function. Call at the end of the root component's `onMounted`
 * and invoke the returned function in `onUnmounted`.
 *
 * @param t - vue-i18n 的翻译函数 / the vue-i18n translate function
 * @returns 取消订阅函数 / unsubscribe function
 */
export async function setupFfmpegProgressToast(
	t: (key: string, named?: Record<string, unknown>) => string,
): Promise<() => void> {
	const apply = (p: FfmpegProgressPayload) => {
		// install 类下载由全局强制进度窗口（FFmpegDownloadDialog）展示，
		// 不走 toast；update 自动更新仍用 toast / Install-kind downloads are
		// shown by the global forced window (FFmpegDownloadDialog), never a
		// toast; update auto-updates still use the toast
		if (p.kind === "install") return;
		// 离开下载态即丢弃速度基线，新一轮下载不会吃到陈旧采样点 /
		// Drop the speed baseline outside the downloading state so a fresh
		// run can't compute against a stale sample
		if (p.state !== "downloading") lastSample = null;
		switch (p.state) {
			case "downloading":
				updateBar(p);
				// 下载中形状不变，同 id 复用以平滑更新；顺带清理其它形态的残留
				// Same shape while downloading — reuse the id for smooth in-place
				// updates; clear any leftovers of the other shapes
				sonnerToast.dismiss(INSTALLING_TOAST_ID);
				sonnerToast.dismiss(DONE_TOAST_ID);
				sonnerToast.loading(
					p.kind === "update" ? t("ffmpeg.updating") : t("ffmpeg.downloading"),
					// content 是收缩的 flex item → grow 撑满行宽，否则 w-full 的
					// 进度条比下面的文本还窄 / data-content is a content-sized flex
					// item — grow stretches it so the w-full bar isn't narrower
					// than the text below
					{
						id: TOAST_ID,
						duration: Infinity,
						description: ProgressBody,
						// toast 类供 style.css 钉高——展开/折叠同高，hover 不再
						// 抖动 / toast class lets style.css pin the height —
						// identical collapsed/expanded height, no hover jitter
						classes: { toast: "ffmpeg-progress-toast", content: "grow" },
					},
				);
				break;
			case "installing":
				// 换形状（无尺寸文本的进度条）→ 收起旧 id 换新的，避免悬停展开
				// 卡旧高度；解压回调发 Installing{pct}，pct 未知时脉冲
				// Shape change (progress bar without size text) → dismiss the old
				// id and use a new one so hover-expand doesn't reuse the stale
				// height; the extraction callback emits Installing{pct}, pulse
				// when pct is unknown
				bar.pct = p.pct ?? null;
				bar.size = "";
				bar.speed = "";
				bar.pctText = p.pct != null ? `${p.pct}%` : "";
				sonnerToast.dismiss(TOAST_ID);
				sonnerToast.dismiss(DONE_TOAST_ID);
				sonnerToast.loading(t("ffmpeg.installing"), {
					id: INSTALLING_TOAST_ID,
					duration: Infinity,
					description: ProgressBody,
					classes: { toast: "ffmpeg-progress-toast", content: "grow" },
				});
				break;
			case "done":
				sonnerToast.dismiss(TOAST_ID);
				sonnerToast.dismiss(INSTALLING_TOAST_ID);
				sonnerToast.success(
					p.kind === "update" ? t("ffmpeg.updated") : t("ffmpeg.installed"),
					{ id: DONE_TOAST_ID, description: undefined, duration: 5000 },
				);
				break;
			case "error":
			case "idle":
				// 后端已通过通知推送错误详情，这里只收起进度 toast
				// The backend already pushed the error details via notifications;
				// just dismiss the progress toast
				for (const id of ALL_TOAST_IDS) sonnerToast.dismiss(id);
				break;
		}
	};

	const unlisten = await on("ffmpeg-progress", (payload) =>
		apply(payload as FfmpegProgressPayload),
	);

	// 晚接入的客户端：拉一次当前状态，恢复正在进行中的下载/安装显示
	// Late-connecting clients: fetch current state once to restore an in-progress
	// download/install display
	try {
		const current = await call<FfmpegProgressPayload>("get_ffmpeg_status");
		if (current.state === "downloading" || current.state === "installing") {
			apply(current);
		}
	} catch {
		// 状态接口失败时静默忽略：只是少一次恢复显示
		// Silently ignore status-endpoint failures: only a missed restore
	}

	return unlisten;
}
