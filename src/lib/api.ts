/**
 * 统一 API 通信层 / Unified API Communication Layer
 *
 * 同时支持 Tauri 桌面端（IPC invoke）和 Web 端（HTTP REST + SSE 实时事件）两种运行模式。
 * 自动检测运行环境并选择对应的通信方式。
 *
 * Supports both Tauri desktop (IPC invoke) and Web (HTTP REST + SSE real-time events) modes.
 * Automatically detects the runtime environment and selects the appropriate communication method.
 */

/** 是否运行在 Tauri 桌面环境中 / Whether running in Tauri desktop environment */
const isTauri =
	typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

type EventCallback = (payload: unknown) => void;

/** SSE 事件监听器映射表：事件名 -> 回调集合 / SSE event listener map: event name -> set of callbacks */
const sseListeners = new Map<string, Set<EventCallback>>();
/** SSE 是否已连接 / Whether SSE is connected */
let sseConnected = false;
/** SSE 连接就绪时的 resolve 函数 / Resolve function called when SSE connection is ready */
let sseReadyResolve: (() => void) | null = null;

/** SSE 连接就绪的 Promise，用于在订阅事件前等待连接建立 / Promise that resolves when SSE connection is ready */
const sseReady: Promise<void> = new Promise((resolve) => {
	sseReadyResolve = resolve;
});

/** SSE 重连事件的回调集合 / Callback set for SSE reconnect events */
let sseReconnectCallbacks: Set<() => void> = new Set();
/** SSE 断开连接事件的回调集合 / Callback set for SSE disconnect events */
let sseDisconnectCallbacks: Set<() => void> = new Set();

/**
 * 注册 SSE 重连回调，返回取消注册函数。
 * Register an SSE reconnect callback, returns an unregister function.
 */
export function onSseReconnect(cb: () => void): () => void {
	sseReconnectCallbacks.add(cb);
	return () => sseReconnectCallbacks.delete(cb);
}

/**
 * 注册 SSE 断开连接回调，返回取消注册函数。
 * Register an SSE disconnect callback, returns an unregister function.
 */
export function onSseDisconnect(cb: () => void): () => void {
	sseDisconnectCallbacks.add(cb);
	return () => sseDisconnectCallbacks.delete(cb);
}

/**
 * 确保 SSE 连接已建立（仅 Web 模式）。
 * 连接断开后每 3 秒自动重连，并触发相应回调。
 *
 * Ensures the SSE connection is established (Web mode only).
 * Auto-reconnects every 3 seconds on disconnect and triggers corresponding callbacks.
 */
function ensureSse() {
	if (sseConnected || isTauri) return;
	sseConnected = true;
	let isFirstConnect = true;
	let isDisconnected = false;

	const connect = () => {
		const es = new EventSource("/api/events");

		es.onopen = () => {
			sseReadyResolve?.();
			// 非首次连接时触发重连回调 / Trigger reconnect callbacks on non-first connect
			if (!isFirstConnect) {
				sseReconnectCallbacks.forEach((cb) => cb());
			}
			isFirstConnect = false;
			isDisconnected = false;
		};

		es.onmessage = (e) => {
			try {
				// 解析 JSON 格式的事件数据并分发给对应监听器
				// Parse JSON event data and dispatch to corresponding listeners
				const { event, payload } = JSON.parse(e.data) as {
					event: string;
					payload: unknown;
				};
				sseListeners.get(event)?.forEach((cb) => cb(payload));
			} catch {}
		};

		es.onerror = () => {
			es.close();
			if (!isFirstConnect && !isDisconnected) {
				isDisconnected = true;
				sseDisconnectCallbacks.forEach((cb) => cb());
			}
			// 3 秒后重连 / Reconnect after 3 seconds
			setTimeout(connect, 3000);
		};
	};

	connect();
}

/**
 * 调用后端命令（Tauri IPC 或 HTTP）。
 * Invoke a backend command (Tauri IPC or HTTP).
 *
 * @param command - 命令名称 / Command name
 * @param args - 命令参数 / Command arguments
 * @returns 命令返回值 / Command return value
 */
export async function call<T = unknown>(
	command: string,
	args?: Record<string, unknown>,
): Promise<T> {
	if (isTauri) {
		const { invoke } = await import("@tauri-apps/api/core");
		return invoke<T>(command, args);
	}
	return httpCall<T>(command, args);
}

/**
 * 订阅后端事件（Tauri 事件或 SSE）。
 * Subscribe to a backend event (Tauri event or SSE).
 *
 * @param event - 事件名称 / Event name
 * @param cb - 事件回调函数 / Event callback function
 * @returns 取消订阅函数 / Unsubscribe function
 */
export async function on(
	event: string,
	cb: EventCallback,
): Promise<() => void> {
	if (isTauri) {
		const { listen } = await import("@tauri-apps/api/event");
		return listen(event, ({ payload }) => cb(payload));
	}
	ensureSse();
	await sseReady;
	if (!sseListeners.has(event)) sseListeners.set(event, new Set());
	sseListeners.get(event)!.add(cb);
	return () => sseListeners.get(event)?.delete(cb);
}

/**
 * HTTP 命令路由表：将命令名映射到对应的 HTTP 方法、URL 和请求体构造函数。
 * HTTP command routing table: maps command names to HTTP method, URL, and body builder.
 */
const COMMAND_MAP: Record<
	string,
	{
		method: string;
		url: (args: Record<string, unknown>) => string;
		body?: (args: Record<string, unknown>) => unknown;
	}
> = {
	list_streamers: { method: "GET", url: () => "/api/streamers" },
	add_streamer: {
		method: "POST",
		url: () => "/api/streamers",
		body: (a) => ({ username: a.username }),
	},
	remove_streamer: {
		method: "DELETE",
		url: (a) => `/api/streamers/${a.username}`,
	},
	set_auto_record: {
		method: "POST",
		url: (a) => `/api/streamers/${a.username}/auto-record`,
		body: (a) => ({ enabled: a.enabled }),
	},
	start_recording: {
		method: "POST",
		url: (a) => `/api/streamers/${a.username}/start`,
	},
	stop_recording: {
		method: "POST",
		url: (a) => `/api/streamers/${a.username}/stop`,
	},
	verify_streamer: {
		method: "GET",
		url: (a) => `/api/streamers/${a.username}/verify`,
	},
	get_settings: { method: "GET", url: () => "/api/settings" },
	save_settings_cmd: {
		method: "POST",
		url: () => "/api/settings",
		body: (a) => a.newSettings,
	},
	list_mouflon_keys: { method: "GET", url: () => "/api/mouflon-keys" },
	add_mouflon_key: {
		method: "POST",
		url: () => "/api/mouflon-keys",
		body: (a) => ({ pkey: a.pkey, pdkey: a.pdkey }),
	},
	remove_mouflon_key: {
		method: "DELETE",
		url: (a) => `/api/mouflon-keys/${a.pkey}`,
	},
	remove_missing_pp_results: {
		method: "POST",
		url: () => "/api/startup-warnings/pp-results",
		body: (a) => ({ paths: a.paths }),
	},
	get_disk_space: { method: "GET", url: () => "/api/disk-space" },
	list_recordings: { method: "GET", url: () => "/api/recordings" },
	get_merging_dirs: { method: "GET", url: () => "/api/recordings/merging" },
	delete_recording: {
		method: "POST",
		url: () => "/api/recordings/delete",
		body: (a) => ({ path: a.path }),
	},
	run_postprocess_cmd: {
		method: "POST",
		url: () => "/api/recordings/postprocess",
		body: (a) => ({ path: a.path }),
	},
	open_recording: {
		method: "POST",
		url: () => "/api/recordings/open",
		body: (a) => ({ path: a.path }),
	},
	open_output_dir: { method: "POST", url: () => "/api/recordings/open-dir" },
	get_pipeline: { method: "GET", url: () => "/api/pipeline" },
	save_pipeline: {
		method: "POST",
		url: () => "/api/pipeline",
		body: (a) => a.pipeline,
	},
	list_modules: { method: "GET", url: () => "/api/modules" },
	get_postprocess_tasks: { method: "GET", url: () => "/api/postprocess-tasks" },
	get_module_outputs: {
		method: "POST",
		url: () => "/api/recordings/module-outputs",
		body: (a) => ({ path: a.path }),
	},
	pick_output_dir: {
		method: "GET",
		url: () => "/api/settings/pick-output-dir",
	},
};

/**
 * 通过 HTTP 执行命令（Web 模式专用）。
 * Execute a command via HTTP (Web mode only).
 *
 * @param command - 命令名称，必须在 COMMAND_MAP 中定义 / Command name, must be defined in COMMAND_MAP
 * @param args - 命令参数 / Command arguments
 * @returns 解析后的响应数据 / Parsed response data
 * @throws 命令未知或 HTTP 请求失败时抛出错误 / Throws on unknown command or HTTP failure
 */
async function httpCall<T>(
	command: string,
	args: Record<string, unknown> = {},
): Promise<T> {
	const def = COMMAND_MAP[command];
	if (!def) throw new Error(`Unknown command: ${command}`);

	const url = def.url(args);
	const hasBody = def.body !== undefined;
	const res = await fetch(url, {
		method: def.method,
		headers: hasBody ? { "Content-Type": "application/json" } : undefined,
		body: hasBody ? JSON.stringify(def.body!(args)) : undefined,
	});

	if (!res.ok) {
		const text = await res.text().catch(() => res.statusText);
		throw new Error(text);
	}

	const text = await res.text();
	// 空响应体返回 undefined（如 204 No Content）
	// Return undefined for empty response body (e.g. 204 No Content)
	if (!text) return undefined as T;
	return JSON.parse(text) as T;
}
