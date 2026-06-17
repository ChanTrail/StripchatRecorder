/**
 * API 通信层（Tauri 桌面版）/ API Communication Layer (Tauri Desktop)
 *
 * 将命令调用从 HTTP REST + SSE 替换为 Tauri IPC：
 * - `call()` → `@tauri-apps/api/core invoke()`
 * - `on()`   → `@tauri-apps/api/event listen()`
 *
 * Replaces HTTP REST + SSE with Tauri IPC:
 * - `call()` → `@tauri-apps/api/core invoke()`
 * - `on()`   → `@tauri-apps/api/event listen()`
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ─── 事件系统（基于 Tauri listen）/ Event system (Tauri listen-based) ───────

/**
 * SSE 重连回调集合（Tauri 模式下无需重连，保留接口兼容性）。
 * SSE reconnect callbacks (no reconnect needed in Tauri mode, kept for interface compatibility).
 */
let sseReconnectCallbacks: Set<() => void> = new Set();
/**
 * SSE 断开连接回调集合（同上，Tauri 无此概念）。
 * SSE disconnect callbacks (same as above, no such concept in Tauri).
 */
let sseDisconnectCallbacks: Set<() => void> = new Set();

/**
 * 注册 SSE 重连回调（Tauri 模式下为空操作）。
 * Register SSE reconnect callback (no-op in Tauri mode).
 */
export function onSseReconnect(cb: () => void): () => void {
	sseReconnectCallbacks.add(cb);
	return () => sseReconnectCallbacks.delete(cb);
}

/**
 * 注册 SSE 断开连接回调（Tauri 模式下为空操作）。
 * Register SSE disconnect callback (no-op in Tauri mode).
 */
export function onSseDisconnect(cb: () => void): () => void {
	sseDisconnectCallbacks.add(cb);
	return () => sseDisconnectCallbacks.delete(cb);
}

/**
 * 调用后端 Tauri 命令。
 * Invoke a backend Tauri command.
 *
 * 命令名（command）直接映射到 #[tauri::command] 函数名（snake_case）。
 * Command names map directly to #[tauri::command] function names (snake_case).
 *
 * @param command - Tauri 命令名 / Tauri command name
 * @param args    - 命令参数（对象键对应命令参数名）/ Command arguments (keys match parameter names)
 */
export async function call<T = unknown>(
	command: string,
	args?: Record<string, unknown>,
): Promise<T> {
	return invoke<T>(command, args);
}

/**
 * 订阅 Tauri 后端事件。
 * Subscribe to a Tauri backend event.
 *
 * 事件名与 server 模式的 SSE 事件名完全相同（因为后端 Emitter 发出的名称一致）。
 * Event names are identical to server mode SSE event names
 * (backend Emitter emits the same names).
 *
 * @param event - 事件名称 / Event name
 * @param cb    - 事件回调 / Event callback
 * @returns 取消订阅函数 / Unsubscribe function
 */
export async function on(
	event: string,
	cb: (payload: unknown) => void,
): Promise<() => void> {
	const unlisten: UnlistenFn = await listen<unknown>(event, (e) => {
		cb(e.payload);
	});
	return unlisten;
}
