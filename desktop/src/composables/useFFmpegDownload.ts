/**
 * FFmpeg 下载触发与构建选择记忆 / FFmpeg download trigger + build-choice memory
 *
 * 统一的手动下载入口（Setup 向导、FFmpeg 首选项向导、设置页下载确认、
 * 全局 `ffmpeg-choice-needed` 选择对话框与失败重试共用）：把本次构建选择
 * （平台轨 × 变体）记入模块级变量后调用 `download_ffmpeg`；
 * `lastFFmpegDownloadChoice` 供 `FFmpegDownloadDialog` 的"重试"沿用同一
 * 选择——否则重试不带参数时会按"缺省 → 宿主默认轨 + 静态"解析，静默
 * 换掉用户已选的版本。
 *
 * The single entry for manual downloads (shared by the setup wizard, the
 * FFmpeg preference wizard, the settings-page download confirm, the global
 * `ffmpeg-choice-needed` pick-a-build dialog and error retries): records the
 * build choice (platform track × variant) in a module-level slot, then calls
 * `download_ffmpeg`. `lastFFmpegDownloadChoice` lets the Retry button reuse
 * the same choice — a parameterless retry would otherwise resolve
 * "absent → host default track + static" and silently swap the user's pick.
 */

import { call } from "@/lib/api";

/** 下载构建选择（对应后端 `FFmpegDownloadChoice`；字段缺省时后端自动解析）/ Download build choice (mirrors backend `FFmpegDownloadChoice`; absent fields auto-resolve server-side) */
export interface FFmpegDownloadChoiceArgs {
	/** 平台轨标识（如 `linux-x86_64-legacy`）/ Platform track id (e.g. `linux-x86_64-legacy`) */
	platform?: string;
	/** 变体后缀（`""`/`-shared`/`-debug`/`-shared-debug`）/ Variant suffix (`""`/`-shared`/`-debug`/`-shared-debug`) */
	variant?: string;
}

/** 最近一次手动触发的构建选择（重试沿用；null = 无记录/未显式选择）/ Last manually triggered build choice (reused by Retry; null = none recorded / nothing explicit) */
let lastChoice: FFmpegDownloadChoiceArgs | null = null;

/**
 * 触发 tools 目录的 FFmpeg 下载安装，并记录构建选择供重试沿用。
 * `platform` 空串归一为 `undefined`（后端按缺省解析）；`variant` 的空串
 * 是「静态构建」的显式取值（后端白名单成员），必须原样下发——若同样
 * 丢弃，`Some("")` 变 `None`，后端会回落到已安装标记的变体，「换回
 * 静态版」将静默重装当前构建而非切换。完全未传 choice 时不覆盖既有
 * 记录——兜底重试仍能沿用上一次的选择。
 *
 * Trigger a tools-directory FFmpeg download/install, recording the build
 * choice for retries. An empty `platform` normalizes to `undefined`
 * (auto-resolved server-side), but an empty `variant` is the explicit
 * "static build" value (an allowlist member) and must be sent as-is —
 * dropping it too would turn `Some("")` into `None`, making the backend
 * fall back to the installed marker's variant, so "switch back to
 * static" would silently reinstall the current build instead. Calling
 * without a choice keeps the previous record so a fallback retry still
 * reuses it.
 *
 * @param choice - 可选构建选择 / Optional build choice
 */
export async function requestFFmpegDownload(choice?: FFmpegDownloadChoiceArgs): Promise<void> {
	// 任一字段已提供（variant 含显式空串）即记为用户选择；全缺省/空对象 →
	// 清记录按缺省解析 / Any provided field (incl. an explicit empty variant)
	// records the user's pick; fully absent/empty object clears it for auto-resolution
	if (choice && (choice.platform || choice.variant != null)) {
		lastChoice = { platform: choice.platform, variant: choice.variant };
	} else if (choice) {
		// 显式传了空对象 = 明确要求缺省解析，清掉旧记录 /
		// An explicit empty object = intentionally auto-resolve; drop the record
		lastChoice = null;
	}
	await call("download_ffmpeg", {
		platform: choice?.platform || undefined,
		// variant 空串 = 显式选择静态构建，原样下发；缺席才交后端解析 /
		// empty variant = explicit static pick, sent as-is; only an absent
		// field goes to server-side resolution
		variant: choice?.variant,
	});
}

/**
 * 返回最近一次手动触发的构建选择（无记录时为 `undefined`）。
 * Return the last manually triggered build choice (`undefined` when none).
 */
export function lastFFmpegDownloadChoice(): FFmpegDownloadChoiceArgs | undefined {
	return lastChoice ?? undefined;
}
