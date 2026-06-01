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
