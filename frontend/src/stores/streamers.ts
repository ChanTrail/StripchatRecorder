/**
 * 主播状态管理 Store / Streamer State Management Store
 *
 * 管理所有被追踪主播的状态，包括在线状态、录制状态、观看人数和缩略图。
 * 通过 SSE/Tauri 事件实时同步多客户端之间的状态变更。
 *
 * Manages the state of all tracked streamers, including online status, recording state,
 * viewer count, and thumbnails. Synchronizes state changes across multiple clients
 * in real-time via SSE/Tauri events.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { call, on } from "@/lib/api";
import { toast as sonnerToast } from "vue-sonner";

/** 主播条目数据结构 / Streamer entry data structure */
export interface StreamerEntry {
	username: string;
	/** 是否开启自动录制 / Whether auto-record is enabled */
	auto_record: boolean;
	/** 添加时间（ISO 字符串）/ Time added (ISO string) */
	added_at: string;
	is_online: boolean;
	is_recording: boolean;
	/** 是否可录制（直播间是否公开可访问）/ Whether the stream is recordable (publicly accessible) */
	is_recordable: boolean;
	/** 直播间状态文字（如"公开秀"）/ Stream status text (e.g. "公开秀") */
	status: string;
	thumbnail_url: string | null;
	/** 是否正在转发流 / Whether the stream is being relayed */
	is_relaying?: boolean;
}

/** 状态更新事件载荷 / Status update event payload */
export interface StatusUpdatePayload {
	username: string;
	is_online: boolean;
	is_recording: boolean;
	is_recordable: boolean;
	status: string;
	thumbnail_url: string | null;
}

export const useStreamersStore = defineStore("streamers", () => {
	/** 主播列表 / Streamer list */
	const streamers = ref<StreamerEntry[]>([]);
	/** 是否正在加载 / Whether loading */
	const loading = ref(false);
	/** 最近一次错误信息 / Most recent error message */
	const error = ref<string | null>(null);
	/** 正在停止录制的主播用户名集合（用于防止状态闪烁）/ Set of usernames with stop-recording in progress (prevents status flicker) */
	const stoppingSet = ref(new Set<string>());
	/** 本地操作标记集合（用于过滤自身触发的事件通知）/ Local action markers (to filter self-triggered event notifications) */
	const localActions = new Set<string>();
	/** 事件监听器是否已初始化（防止重复注册）/ Whether event listeners are initialized (prevents duplicate registration) */
	let listenersReady = false;

	/**
	 * 标记一个操作为本地发起，在 TTL 内忽略对应的远程事件通知。
	 * Mark an action as locally initiated; ignore corresponding remote event notifications within TTL.
	 *
	 * @param key - 操作标识键（如 "add:username"）/ Action key (e.g. "add:username")
	 * @param ttl - 标记有效期（毫秒），默认 3000ms / Marker TTL (ms), defaults to 3000ms
	 */
	function markLocal(key: string, ttl = 3000) {
		localActions.add(key);
		setTimeout(() => localActions.delete(key), ttl);
	}

	/**
	 * 从后端获取主播列表。
	 * Fetch the streamer list from the backend.
	 */
	async function fetchStreamers() {
		loading.value = true;
		try {
			streamers.value = await call<StreamerEntry[]>("list_streamers");
		} catch (e) {
			error.value = String(e);
		} finally {
			loading.value = false;
		}
	}

	/**
	 * 添加新主播到追踪列表，支持批量。
	 * 向后端发送用户名列表，后端并发验证后逐个添加。
	 * 通过订阅 streamer-batch-progress 事件实时调用进度回调。
	 *
	 * Add streamers to the tracking list, supporting batch input.
	 * Sends a list of usernames to the backend for concurrent verification and sequential addition.
	 * Invokes the optional progress callback in real time via streamer-batch-progress events.
	 *
	 * @param usernames - 主播用户名列表 / List of streamer usernames
	 * @param onProgress - 可选进度回调 (done, currentUsername) / Optional progress callback (done, currentUsername)
	 */
	async function addStreamers(
		usernames: string[],
		onProgress?: (done: number, current: string) => void,
	): Promise<{ total: number; success: number; skipped: number; failed: number }> {
		for (const u of usernames) markLocal(`add:${u}`);

		// 若提供进度回调，订阅进度事件并在完成后取消订阅
		// If a progress callback is provided, subscribe to progress events and unsubscribe when done
		let unlisten: (() => void) | null = null;
		if (onProgress) {
			unlisten = await on("streamer-batch-progress", (payload) => {
				const p = payload as { done: number; username: string };
				onProgress(p.done, p.username);
			});
		}

		try {
			const result = await call<{ total: number; success: number; skipped: number; failed: number }>(
				"add_streamer",
				{ usernames },
			);
			await fetchStreamers();
			return result;
		} finally {
			unlisten?.();
		}
	}

	/**
	 * 从追踪列表中移除主播。
	 * Remove a streamer from the tracking list.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function removeStreamer(username: string) {
		markLocal(`remove:${username}`);
		await call("remove_streamer", { username });
		streamers.value = streamers.value.filter((s) => s.username !== username);
	}

	/**
	 * 设置主播的自动录制开关。
	 * Set the auto-record toggle for a streamer.
	 *
	 * @param username - 主播用户名 / Streamer username
	 * @param enabled - 是否开启自动录制 / Whether to enable auto-record
	 */
	async function setAutoRecord(username: string, enabled: boolean) {
		markLocal(`auto:${username}`);
		await call("set_auto_record", { username, enabled });
		const s = streamers.value.find((s) => s.username === username);
		if (s) s.auto_record = enabled;
	}

	/**
	 * 手动开始录制指定主播。
	 * Manually start recording a specific streamer.
	 *
	 * @param username - 主播用户名 / Streamer username
	 * @returns 录制文件路径 / Recording file path
	 */
	async function startRecording(username: string): Promise<string> {
		return call<string>("start_recording", { username });
	}

	/**
	 * 手动停止录制指定主播。
	 * 立即在本地将录制状态设为 false，防止 UI 闪烁。
	 *
	 * Manually stop recording a specific streamer.
	 * Immediately sets recording state to false locally to prevent UI flicker.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function stopRecording(username: string) {
		stoppingSet.value.add(username);
		const s = streamers.value.find((s) => s.username === username);
		if (s) s.is_recording = false;
		await call("stop_recording", { username });
	}

	/**
	 * 停止指定主播的流转发。
	 * Stop stream relay for the given streamer.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function stopRelay(username: string) {
		await call("stop_relay", { username });
		const s = streamers.value.find((s) => s.username === username);
		if (s) s.is_relaying = false;
	}

	/**
	 * 批量开始录制，并发发起，返回失败数量。
	 * Batch start recording concurrently, returns failure count.
	 *
	 * @param usernames - 主播用户名列表 / List of streamer usernames
	 */
	async function batchStartRecording(
		usernames: string[],
	): Promise<{ failed: number }> {
		const results = await Promise.allSettled(
			usernames.map((u) => call("start_recording", { username: u })),
		);
		const failed = results.filter((r) => r.status === "rejected").length;
		return { failed };
	}

	/**
	 * 批量停止录制，并发发起，返回失败数量。
	 * 立即在本地将录制状态设为 false，防止 UI 闪烁。
	 *
	 * Batch stop recording concurrently, returns failure count.
	 * Immediately sets local recording state to false to prevent UI flicker.
	 *
	 * @param usernames - 主播用户名列表 / List of streamer usernames
	 */
	async function batchStopRecording(
		usernames: string[],
	): Promise<{ failed: number }> {
		for (const u of usernames) {
			stoppingSet.value.add(u);
			const s = streamers.value.find((st) => st.username === u);
			if (s) s.is_recording = false;
		}
		const results = await Promise.allSettled(
			usernames.map((u) => call("stop_recording", { username: u })),
		);
		const failed = results.filter((r) => r.status === "rejected").length;
		return { failed };
	}

	/**
	 * 批量设置自动录制开关，并发执行，返回失败数量。
	 * Batch set auto-record toggle concurrently, returns failure count.
	 *
	 * @param usernames - 主播用户名列表 / List of streamer usernames
	 * @param enabled   - 是否开启自动录制 / Whether to enable auto-record
	 */
	async function batchSetAutoRecord(
		usernames: string[],
		enabled: boolean,
	): Promise<{ failed: number }> {
		const results = await Promise.allSettled(
			usernames.map(async (u) => {
				markLocal(`auto:${u}`);
				await call("set_auto_record", { username: u, enabled });
				const s = streamers.value.find((st) => st.username === u);
				if (s) s.auto_record = enabled;
			}),
		);
		const failed = results.filter((r) => r.status === "rejected").length;
		return { failed };
	}

	/**
	 * 批量移除主播，串行执行以保证顺序一致，返回失败数量。
	 * Batch remove streamers serially to maintain consistency, returns failure count.
	 *
	 * @param usernames - 主播用户名列表 / List of streamer usernames
	 */
	async function batchRemove(
		usernames: string[],
	): Promise<{ failed: number }> {
		let failed = 0;
		for (const u of usernames) {
			try {
				markLocal(`remove:${u}`);
				await call("remove_streamer", { username: u });
				streamers.value = streamers.value.filter((s) => s.username !== u);
			} catch {
				failed++;
			}
		}
		return { failed };
	}
	/**
	 * 初始化后端事件监听器（只执行一次）。
	 * 监听主播添加/移除、状态更新、录制开始/停止、自动录制变更等事件。
	 *
	 * Initialize backend event listeners (executed only once).
	 * Listens for streamer add/remove, status updates, recording start/stop, auto-record changes, etc.
	 */
	async function initListeners() {
		if (listenersReady) return;
		listenersReady = true;
		await Promise.all([
			on("streamer-added", (payload) => {
				const p = payload as { username: string };
				// 非本地操作时显示提示 / Show notification for non-local actions
				if (!localActions.has(`add:${p.username}`)) {
					sonnerToast.info(`其他客户端添加了主播：${p.username}`);
				}
				void fetchStreamers();
			}),
			on("streamer-removed", (payload) => {
				const p = payload as { username: string };
				if (!localActions.has(`remove:${p.username}`)) {
					sonnerToast.info(`其他客户端移除了主播：${p.username}`);
				}
				streamers.value = streamers.value.filter(
					(s) => s.username !== p.username,
				);
			}),
			on("status-update", (payload) => {
				const p = payload as StatusUpdatePayload;
				const s = streamers.value.find((s) => s.username === p.username);
				if (s) {
					// 若正在停止录制，忽略后端的录制状态更新，防止状态闪烁
					// If stop is in progress, ignore backend recording state to prevent flicker
					const isStopping = stoppingSet.value.has(p.username);
					Object.assign(s, {
						is_online: p.is_online,
						is_recording: isStopping ? false : p.is_recording,
						is_recordable: isStopping ? s.is_recordable : p.is_recordable,
						status: p.status,
						// 仅在有新缩略图时更新，避免清空已有缩略图
						// Only update thumbnail if a new one is provided
						...(p.thumbnail_url ? { thumbnail_url: p.thumbnail_url } : {}),
					});
				}
			}),
			on("recording-started", (payload) => {
				const p = payload as { username: string; file_path: string };
				stoppingSet.value.delete(p.username);
				const s = streamers.value.find((s) => s.username === p.username);
				if (s) s.is_recording = true;
			}),
			on("recording-stopped", (payload) => {
				const p = payload as { username: string };
				stoppingSet.value.delete(p.username);
				const s = streamers.value.find((s) => s.username === p.username);
				if (s) s.is_recording = false;
			}),
			on("auto-record-changed", (payload) => {
				const p = payload as { username: string; enabled: boolean };
				if (!localActions.has(`auto:${p.username}`)) {
					sonnerToast.info(
						`其他客户端${p.enabled ? "开启" : "关闭"}了 ${p.username} 的自动录制`,
					);
				}
				const s = streamers.value.find((s) => s.username === p.username);
				if (s) s.auto_record = p.enabled;
			}),
			// 主播改名：后端通过缓存的 model_id 反查确认改名后，直接原地更新用户名，
			// 无需重新拉取整个列表。
			// Streamer renamed: once the backend confirms a rename via the cached
			// model_id lookup, update the username in place without refetching the
			// whole list.
			on("streamer-renamed", (payload) => {
				const p = payload as { old_username: string; new_username: string };
				const s = streamers.value.find((s) => s.username === p.old_username);
				if (s) s.username = p.new_username;
				sonnerToast.info(`主播 ${p.old_username} 已改名为 ${p.new_username}`);
			}),
			on("api-error", (payload) => {
				const p = payload as { message: string };
				sonnerToast.error(`Stripchat API连接错误: ${p.message}`);
			}),
		]);
	}

	return {
		streamers,
		loading,
		error,
		fetchStreamers,
		addStreamers,
		removeStreamer,
		setAutoRecord,
		startRecording,
		stopRecording,
		batchStartRecording,
		batchStopRecording,
		batchSetAutoRecord,
		batchRemove,
		stopRelay,
		initListeners,
	};
});
