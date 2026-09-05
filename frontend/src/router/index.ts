/**
 * 路由配置 / Router Configuration
 *
 * 访问控制逻辑：
 * 1. setup_done=false → /setup（setup 阶段后端无需 token）
 * 2. setup_done=true + password_set=false → /login（老版本升级，需先设置密码）
 * 3. setup_done=true + password_set=true + 无有效 token → /login
 * 4. 其他正常放行
 *
 * Access control:
 * 1. setup_done=false → /setup (backend allows all during setup)
 * 2. setup_done=true + password_set=false → /login (upgrade path: must set password)
 * 3. setup_done=true + password_set=true + no valid token → /login
 * 4. Otherwise pass through
 */

import { createRouter, createWebHistory } from "vue-router";

const router = createRouter({
	history: createWebHistory(),
	routes: [
		{ path: "/login", component: () => import("../views/LoginView.vue") },
		{ path: "/setup", component: () => import("../views/SetupView.vue") },
		{ path: "/", component: () => import("../views/HomeView.vue") },
		{ path: "/recordings", component: () => import("../views/RecordingsView.vue") },
		{ path: "/postprocess", component: () => import("../views/PostprocessView.vue") },
		{ path: "/community", component: () => import("../views/CommunityView.vue") },
		{ path: "/settings", component: () => import("../views/SettingsView.vue") },
		{ path: "/finder", component: () => import("../views/FinderView.vue") },
		{ path: "/relay", component: () => import("../views/RelayView.vue") },
		{ path: "/about", component: () => import("../views/AboutView.vue") },
	],
});

router.beforeEach(async (to) => {
	// /setup 和 /login 不拦截，避免死循环
	if (to.path === "/setup" || to.path === "/login") return true;

	const token = localStorage.getItem("admin_token");

	try {
		// setup 阶段后端放行无 token 请求；setup 完成后需要 token
		const res = await fetch("/api/settings", {
			headers: token ? { Authorization: `Bearer ${token}` } : {},
		});

		if (res.status === 401) {
			// token 失效或 setup 已完成但未登录
			localStorage.removeItem("admin_token");
			return { path: "/login", query: { redirect: to.path } };
		}

		if (res.ok) {
			const settings = await res.json() as { setup_done: boolean };
			if (!settings.setup_done) return "/setup";
		}
	} catch {
		// 后端未就绪，放行
		return true;
	}

	// setup 已完成，检查 password_set + token
	// 同时查 auth/status 确认密码是否已设置（兼容老版本升级）
	try {
		const statusRes = await fetch("/api/auth/status");
		if (statusRes.ok) {
			const status = await statusRes.json() as { password_set: boolean };
			if (!status.password_set) {
				// 老版本升级：setup 完成但密码未设置，去登录页设置密码（不带 redirect，这是初次设置）
				return "/login";
			}
		}
	} catch {
		// 放行
	}

	if (!token) return { path: "/login", query: { redirect: to.path } };

	return true;
});

export default router;
