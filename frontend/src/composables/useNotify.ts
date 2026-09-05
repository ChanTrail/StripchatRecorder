/**
 * 通知与确认对话框 Composable / Notification and Confirm Dialog Composable
 *
 * 提供三种全局通知机制：
 * 1. toast()   — 仅弹出右下角 Toast，不进面板（操作反馈、其他客户端操作通知等）
 * 2. notify()  — 弹出 Toast 的同时追加到前端内存通知面板（系统/后台事件）
 * 3. confirm() — 模态确认对话框（阻塞式，返回 Promise<boolean>）
 *
 * Provides three global notification mechanisms:
 * 1. toast()   — Toast only, not added to panel (operation feedback, other-client notices etc.)
 * 2. notify()  — Toast + appended to in-memory frontend notification panel (system/background events)
 * 3. confirm() — Modal confirmation dialog (blocking, returns Promise<boolean>)
 *
 * notify() 的面板通知是纯前端内存状态，刷新后清空。
 * 后端持久化通知（notification-created SSE）由 stores/notifications.ts 管理，
 * 两者在 App.vue 通知面板中合并展示。
 *
 * notify() panel entries are pure in-memory frontend state, cleared on page refresh.
 * Backend persistent notifications (notification-created SSE) are managed by
 * stores/notifications.ts; both are merged in the App.vue notification panel.
 */

import { ref, markRaw } from "vue";
import { toast as sonnerToast } from "vue-sonner";

/** Toast / 通知级别类型 / Toast / notification level type */
export type ToastType = "success" | "error" | "info" | "warning";

/** 前端内存通知条目 / Frontend in-memory notification entry */
export interface FrontendNotification {
	id: number;
	message: string;
	type: ToastType;
	timestamp: Date;
}

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

// 前端内存通知列表（模块级单例，刷新后清空）
// Frontend in-memory notification list (module-level singleton, cleared on refresh)
export const frontendNotifications = ref<FrontendNotification[]>([]);
let _nextId = 0;

/**
 * 仅显示 Toast 通知消息，不追加到通知面板。
 * 适用于操作反馈（用户主动触发的结果）和其他客户端操作通知。
 *
 * Show a Toast notification only, without adding to the notification panel.
 * For operation feedback (user-triggered results) and other-client notices.
 *
 * @param message - 消息内容 / Message content
 * @param type    - 消息类型，默认 "info" / Message type, defaults to "info"
 */
export function toast(message: string, type: ToastType = "info") {
	_fireToast(message, type);
}

/**
 * 显示 Toast 通知消息，同时追加到前端内存通知面板。
 * 适用于系统/后台事件（用户未主动触发，需要持久可见）。
 *
 * Show a Toast notification and also append to the frontend in-memory panel.
 * For system/background events (not user-triggered, needs persistent visibility).
 *
 * @param message - 消息内容 / Message content
 * @param type    - 消息类型，默认 "info" / Message type, defaults to "info"
 */
export function notify(message: string, type: ToastType = "info") {
	_fireToast(message, type);
	frontendNotifications.value.push({
		id: _nextId++,
		message,
		type,
		timestamp: new Date(),
	});
}

/**
 * 关闭一条前端内存通知。
 * Dismiss a frontend in-memory notification.
 */
export function dismissFrontendNotification(id: number) {
	frontendNotifications.value = frontendNotifications.value.filter((n) => n.id !== id);
}

/**
 * 关闭所有前端内存通知。
 * Dismiss all frontend in-memory notifications.
 */
export function dismissAllFrontendNotifications() {
	frontendNotifications.value = [];
}

/** 内部：向 sonner 发送 toast / Internal: fire a sonner toast */
function _fireToast(message: string, type: ToastType) {
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
	return {
		toast,
		notify,
		confirm,
		dialog,
		_resolveDialog,
		frontendNotifications,
		dismissFrontendNotification,
		dismissAllFrontendNotifications,
	};
}
