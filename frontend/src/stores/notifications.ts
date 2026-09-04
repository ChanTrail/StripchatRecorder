/**
 * 通知状态管理 Store / Notification State Management Store
 *
 * 管理从后端拉取的进程内通知列表。
 * - 首次连接 / SSE 重连后自动拉取
 * - 收到 notification-created SSE 事件时实时追加
 * - 提供已读标记（单条或全部清除）
 *
 * Manages the in-process notification list pulled from the backend.
 * - Fetched automatically on first connect / SSE reconnect
 * - Appended in real-time when notification-created SSE events arrive
 * - Provides mark-as-read (individual or clear all)
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { call } from "@/lib/api";

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

export const useNotificationsStore = defineStore("notifications", () => {
	const notifications = ref<Notification[]>([]);
	const loading = ref(false);

	const unreadCount = computed(() => notifications.value.length);

	/** 从后端拉取所有未读通知 */
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

	/** 实时追加一条新通知（SSE notification-created 事件回调）*/
	function append(n: Notification) {
		// 防重：相同 id 不重复追加
		if (!notifications.value.some((x) => x.id === n.id)) {
			notifications.value.push(n);
		}
	}

	/** 标记指定 id 列表为已读（空数组 = 全部清除）*/
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
