/**
 * 通知状态管理 Store / Notification State Management Store
 *
 * 管理从后端拉取的进程内通知列表。
 * - 首次连接 / SSE 重连后自动拉取
 * - 收到 notification-created SSE 事件时实时追加
 * - 提供已读标记（单条或全部清除）
 *
 * 通知分两条路径：
 * - 用户在线时收到的实时通知（notification-created SSE）：同时追加面板 + 弹 toast
 * - 离线恢复后补拉的历史通知（fetch()）：只追加面板，不弹 toast
 *
 * Manages the in-process notification list pulled from the backend.
 * - Fetched automatically on first connect / SSE reconnect
 * - Appended in real-time when notification-created SSE events arrive
 * - Provides mark-as-read (individual or clear all)
 *
 * Two delivery paths:
 * - Real-time (online): append to panel + show toast
 * - Offline-restore (fetch after reconnect): panel only, no toast
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { call } from "@/lib/api";
import { toast } from "@/composables/useNotify";
import type { ToastType } from "@/composables/useNotify";

export type NotificationLevel = "info" | "warning" | "error";

export interface NotificationAction {
	action_type: string;
	targets: string[];
}

export interface Notification {
	id: number;
	level: NotificationLevel;
	source: string;
	message: string;
	created_at: string;
	action?: NotificationAction;
}

interface NotificationsResponse {
	notifications: Notification[];
	unread_count: number;
}

/** 将通知级别映射到 toast 类型 / Map notification level to toast type */
function levelToToastType(level: NotificationLevel): ToastType {
	if (level === "error") return "error";
	if (level === "warning") return "warning";
	return "info";
}

export const useNotificationsStore = defineStore("notifications", () => {
	const notifications = ref<Notification[]>([]);
	const loading = ref(false);

	const unreadCount = computed(() => notifications.value.length);

	/** 从后端拉取所有未读通知（不弹 toast，仅追加面板）*/
	/** Fetch all unread notifications from backend (panel only, no toast) */
	async function fetch() {
		loading.value = true;
		try {
			const res = await call<NotificationsResponse>("get_notifications");
			notifications.value = res.notifications;
		} catch {
			// 静默失败，不影响主流程
		} finally {
			loading.value = false;
		}
	}

	/**
	 * 追加一条新通知。
	 * Append a new notification.
	 *
	 * @param n         - 通知对象 / Notification object
	 * @param showToast - 是否同时弹出 toast（用户在线时传 true，离线恢复时传 false）
	 *                    Whether to also show a toast (true when user is online, false on offline restore)
	 */
	function append(n: Notification, showToast: boolean) {
		// 防重：相同 id 不重复追加
		if (!notifications.value.some((x) => x.id === n.id)) {
			notifications.value.push(n);
		}
		if (showToast) {
			toast(n.message, levelToToastType(n.level));
		}
	}

	/** 标记指定 id 列表为已读（空数组 = 全部清除）*/
	/** Mark specified ids as read (empty array = clear all) */
	async function markRead(ids: number[] = []) {
		try {
			await call("mark_notifications_read", { ids });
			if (ids.length === 0) {
				notifications.value = [];
			} else {
				notifications.value = notifications.value.filter(
					(n) => !ids.includes(n.id),
				);
			}
		} catch {
			// 静默失败
		}
	}

	/** 标记全部已读 */
	/** Mark all as read */
	async function markAllRead() {
		return markRead([]);
	}

	return {
		notifications,
		loading,
		unreadCount,
		fetch,
		append,
		markRead,
		markAllRead,
	};
});
