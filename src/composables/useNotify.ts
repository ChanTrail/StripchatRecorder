/**
 * 通知与确认对话框 Composable / Notification and Confirm Dialog Composable
 *
 * 提供全局 Toast 消息通知和模态确认对话框功能。
 * Toast 通知通过 vue-sonner 实现；确认对话框通过共享的响应式状态实现单例模式。
 *
 * Provides global Toast message notifications and modal confirmation dialogs.
 * Toast notifications are powered by vue-sonner; confirm dialogs use shared reactive
 * state in a singleton pattern.
 */

import { ref, markRaw } from "vue";
import { toast as sonnerToast } from "vue-sonner";

/** Toast 消息类型 / Toast message type */
export type ToastType = "success" | "error" | "info" | "warning";

/** 确认对话框配置选项 / Confirm dialog configuration options */
export interface DialogOptions {
	title: string;
	message: string;
	/** 确认按钮文字，默认"确认" / Confirm button text, defaults to "确认" */
	confirmText?: string;
	/** 取消按钮文字，默认"取消" / Cancel button text, defaults to "取消" */
	cancelText?: string;
	/** 是否为危险操作（按钮显示为红色）/ Whether this is a destructive action (red button) */
	danger?: boolean;
	/** 是否隐藏取消按钮 / Whether to hide the cancel button */
	hideCancelButton?: boolean;
}

// 当前对话框的 Promise resolve 函数（单例）
// Promise resolve function for the current dialog (singleton)
let _dialogResolve: ((confirmed: boolean) => void) | null = null;

// 当前显示的对话框配置，null 表示无对话框
// Current dialog config, null means no dialog is shown
const dialog = ref<DialogOptions | null>(null);

/**
 * 显示 Toast 通知消息。
 * Show a Toast notification message.
 *
 * @param message - 消息内容 / Message content
 * @param type - 消息类型，默认 "info" / Message type, defaults to "info"
 */
function toast(message: string, type: ToastType = "info") {
	switch (type) {
		case "success":
			sonnerToast.success(message);
			break;
		case "error":
			sonnerToast.error(message);
			break;
		case "warning":
			sonnerToast.warning(message);
			break;
		default:
			sonnerToast.info(message);
	}
}

/**
 * 显示模态确认对话框，返回用户是否确认的 Promise。
 * Show a modal confirmation dialog, returns a Promise of whether the user confirmed.
 *
 * @param options - 对话框配置 / Dialog configuration
 * @returns 用户点击确认返回 true，取消返回 false / true if confirmed, false if cancelled
 */
function confirm(options: DialogOptions): Promise<boolean> {
	// 使用 markRaw 避免 Vue 对 options 对象进行深度响应式代理
	// Use markRaw to prevent Vue from deeply proxying the options object
	dialog.value = markRaw(options) as DialogOptions;
	return new Promise((resolve) => {
		_dialogResolve = resolve;
	});
}

/**
 * 内部函数：解析当前对话框的 Promise 并关闭对话框。
 * Internal function: resolves the current dialog's Promise and closes the dialog.
 *
 * @param result - 用户操作结果（true=确认，false=取消）/ User action result (true=confirm, false=cancel)
 */
function _resolveDialog(result: boolean) {
	dialog.value = null;
	_dialogResolve?.(result);
	_dialogResolve = null;
}

/**
 * 返回通知相关的工具函数和状态。
 * Returns notification-related utility functions and state.
 */
export function useNotify() {
	return { toast, confirm, dialog, _resolveDialog };
}
