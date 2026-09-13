/**
 * 路由配置（桌面版）/ Router Configuration (Desktop)
 *
 * 与服务器版相比：
 * - 无 /login、/relay（桌面版不支持流转发、无独立认证）
 * - 新增 /community、/about（与服务器版同步）
 *
 * Compared to the server version:
 * - No /login or /relay (desktop has no relay and no separate auth)
 * - Added /community and /about (synced with server version)
 */

import { createRouter, createWebHistory } from "vue-router";
import { call } from "@/lib/api";
import type { Settings } from "@/stores/settings";

const router = createRouter({
	history: createWebHistory(),
	routes: [
		{ path: "/setup",       component: () => import("../views/SetupView.vue") },
		{ path: "/",            component: () => import("../views/HomeView.vue") },
		{ path: "/recordings",  component: () => import("../views/RecordingsView.vue") },
		{ path: "/postprocess", component: () => import("../views/PostprocessView.vue") },
		{ path: "/community",   component: () => import("../views/CommunityView.vue") },
		{ path: "/settings",    component: () => import("../views/SettingsView.vue") },
		{ path: "/finder",      component: () => import("../views/FinderView.vue") },
		{ path: "/about",       component: () => import("../views/AboutView.vue") },
	],
});

// 首次启动检测：setup_done 为 false 时强制跳转到 /setup
// First-launch detection: redirect to /setup when setup_done is false
let setupChecked = false;

router.beforeEach(async (to) => {
	if (setupChecked) return true;

	try {
		const settings = await call<Settings>("get_settings");
		setupChecked = true;

		if (!settings.setup_done) {
			if (to.path !== "/setup") return "/setup";
		} else {
			if (to.path === "/setup") return "/";
		}
	} catch {
		setupChecked = true;
	}

	return true;
});

export default router;
