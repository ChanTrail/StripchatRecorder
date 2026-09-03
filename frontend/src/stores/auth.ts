/**
 * 认证状态管理 Store / Authentication State Store
 *
 * 管理管理员登录状态，token 持久化到 localStorage。
 * Manages admin login state; token is persisted to localStorage.
 */

import { defineStore } from "pinia";
import { ref } from "vue";
import { startTokenRenew } from "@/lib/api";

const TOKEN_KEY = "admin_token";

export const useAuthStore = defineStore("auth", () => {
	const token = ref<string | null>(localStorage.getItem(TOKEN_KEY));
	const isLoggedIn = ref(token.value !== null);
	const passwordSet = ref<boolean | null>(null);

	async function fetchStatus(): Promise<{ password_set: boolean }> {
		const res = await fetch("/api/auth/status");
		const data = await res.json() as { password_set: boolean; logged_in: boolean };
		passwordSet.value = data.password_set;
		return data;
	}

	async function initPassword(password: string): Promise<void> {
		const res = await fetch("/api/auth/init-password", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ password }),
		});
		if (!res.ok) {
			const text = await res.text().catch(() => res.statusText);
			throw new Error(text);
		}
		passwordSet.value = true;
	}

	async function login(password: string): Promise<void> {
		const res = await fetch("/api/auth/login", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ password }),
		});
		if (!res.ok) {
			const text = await res.text().catch(() => res.statusText);
			throw new Error(text);
		}
		const data = await res.json() as { token: string };
		token.value = data.token;
		isLoggedIn.value = true;
		localStorage.setItem(TOKEN_KEY, data.token);
		// 登录成功后启动自动续期 / Start auto-renew after successful login
		startTokenRenew();
	}

	async function logout(): Promise<void> {
		const t = token.value;
		token.value = null;
		isLoggedIn.value = false;
		localStorage.removeItem(TOKEN_KEY);
		if (t) {
			await fetch("/api/auth/logout", {
				method: "POST",
				headers: { Authorization: `Bearer ${t}` },
			}).catch(() => {});
		}
	}

	return { token, isLoggedIn, passwordSet, fetchStatus, initPassword, login, logout };
});
