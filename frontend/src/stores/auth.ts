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
	/** 用户显式选择"无密码"模式 / Whether the user opted into no-password mode */
	const passwordDisabled = ref<boolean>(false);

	async function fetchStatus(): Promise<{ password_set: boolean; password_disabled: boolean }> {
		const res = await fetch("/api/auth/status");
		const data = (await res.json()) as {
			password_set: boolean;
			logged_in: boolean;
			password_disabled?: boolean;
		};
		passwordSet.value = data.password_set;
		passwordDisabled.value = data.password_disabled === true;
		return { password_set: data.password_set, password_disabled: passwordDisabled.value };
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
		passwordDisabled.value = false;
	}

	/**
	 * 移除管理员密码进入无密码模式：验当前密码，成功后后端清除哈希并
	 * 标记 admin_password_disabled——此后免登录直进首页。
	 * Remove the admin password and enter no-password mode: verifies the
	 * current password; on success the backend clears the hash and flags
	 * admin_password_disabled — the app then opens without login.
	 *
	 * @param password - 当前管理员密码 / current admin password
	 */
	async function removePassword(password: string): Promise<void> {
		const res = await fetch("/api/auth/remove-password", {
			method: "POST",
			headers: {
				"Content-Type": "application/json",
				Authorization: `Bearer ${token.value}`,
			},
			body: JSON.stringify({ password }),
		});
		if (!res.ok) {
			const text = await res.text().catch(() => res.statusText);
			throw new Error(text);
		}
		passwordSet.value = false;
		passwordDisabled.value = true;
		token.value = null;
		isLoggedIn.value = false;
		localStorage.removeItem(TOKEN_KEY);
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
		const data = (await res.json()) as { token: string };
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

	return {
		token,
		isLoggedIn,
		passwordSet,
		passwordDisabled,
		fetchStatus,
		initPassword,
		removePassword,
		login,
		logout,
	};
});
