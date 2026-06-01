/**
 * 通用工具函数 / General Utility Functions
 *
 * 提供 Tailwind CSS 类名合并工具，结合 clsx 和 tailwind-merge 实现智能去重合并。
 * Provides Tailwind CSS class name merging utility, combining clsx and tailwind-merge
 * for intelligent deduplication and merging.
 */

import type { ClassValue } from "clsx";
import { clsx } from "clsx";
import { twMerge } from "tailwind-merge";

/**
 * 合并 Tailwind CSS 类名，自动处理冲突和重复。
 * Merges Tailwind CSS class names, automatically handling conflicts and duplicates.
 *
 * @param inputs - 任意数量的类名值（字符串、对象、数组等）
 *                 Any number of class name values (strings, objects, arrays, etc.)
 * @returns 合并后的类名字符串 / Merged class name string
 */
export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

/**
 * 将文本写入剪贴板，自动降级兼容非安全上下文（如 http://0.0.0.0）。
 * Writes text to clipboard with fallback for non-secure contexts (e.g. http://0.0.0.0).
 *
 * @param text - 要复制的文本 / Text to copy
 * @returns 是否成功 / Whether it succeeded
 */
export async function copyToClipboard(text: string): Promise<boolean> {
	// 优先使用现代 Clipboard API（需要安全上下文）
	// Prefer modern Clipboard API (requires secure context)
	if (navigator.clipboard?.writeText) {
		try {
			await navigator.clipboard.writeText(text);
			return true;
		} catch {
			// 权限被拒绝时降级 / Fall through to legacy fallback on permission denial
		}
	}
	// 降级方案：通过临时 textarea + execCommand 实现
	// Legacy fallback: use temporary textarea + execCommand
	try {
		const el = document.createElement("textarea");
		el.value = text;
		el.style.cssText = "position:fixed;top:-9999px;left:-9999px;opacity:0";
		document.body.appendChild(el);
		el.focus();
		el.select();
		const ok = document.execCommand("copy");
		document.body.removeChild(el);
		return ok;
	} catch {
		return false;
	}
}
