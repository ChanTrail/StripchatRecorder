/**
 * API 通信层 / API Communication Layer
 *
 * 基于 HTTP REST + SSE 实时事件的 Web 通信层。
 * HTTP REST + SSE real-time event communication layer.
 */

type EventCallback = (payload: unknown) => void;

/** SSE 事件监听器映射表：事件名 -> 回调集合 / SSE event listener map: event name -> set of callbacks */
const sseListeners = new Map<string, Set<EventCallback>>();
/** SSE 是否已连接 / Whether SSE is connected */
let sseConnected = false;
/** SSE 连接就绪时的 resolve 函数 / Resolve function called when SSE connection is ready */
let sseReadyResolve: (() => void) | null = null;

/** SSE 连接就绪的 Promise / Promise that resolves when SSE connection is ready */
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
 * 确保 SSE 连接已建立。
 * 连接断开后每 3 秒自动重连，并触发相应回调。
 *
 * Ensures the SSE connection is established.
 * Auto-reconnects every 3 seconds on disconnect and triggers corresponding callbacks.
 */
function ensureSse() {
	if (sseConnected) return;
	sseConnected = true;
	let isFirstConnect = true;
	let isDisconnected = false;

	const connect = () => {
		const es = new EventSource("/api/events");

		es.onopen = () => {
			sseReadyResolve?.();
			if (!isFirstConnect) {
				sseReconnectCallbacks.forEach((cb) => cb());
			}
			isFirstConnect = false;
			isDisconnected = false;
		};

		es.onmessage = (e) => {
			try {
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
			setTimeout(connect, 3000);
		};
	};

	connect();
}

/**
 * 调用后端命令（HTTP REST）。
 * Invoke a backend command via HTTP REST.
 *
 * @param command - 命令名称 / Command name
 * @param args - 命令参数 / Command arguments
 * @returns 命令返回值 / Command return value
 */
export async function call<T = unknown>(
	command: string,
	args?: Record<string, unknown>,
): Promise<T> {
	return httpCall<T>(command, args);
}

/**
 * 订阅后端事件（SSE）。
 * Subscribe to a backend event via SSE.
 *
 * @param event - 事件名称 / Event name
 * @param cb - 事件回调函数 / Event callback function
 * @returns 取消订阅函数 / Unsubscribe function
 */
export async function on(
	event: string,
	cb: EventCallback,
): Promise<() => void> {
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
	sync_mouflon_keys: {
		method: "POST",
		url: () => "/api/mouflon-keys/sync",
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
	cancel_postprocess: {
		method: "POST",
		url: () => "/api/recordings/postprocess-cancel",
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
	list_relay_sessions: {
		method: "GET",
		url: () => "/api/relay/sessions",
	},
	stop_relay: {
		method: "POST",
		url: (a) => `/api/relay/${a.username}/stop`,
	},
};

/**
 * 通过 HTTP 执行命令。
 * Execute a command via HTTP.
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
	if (!text) return undefined as T;
	return JSON.parse(text) as T;
}
