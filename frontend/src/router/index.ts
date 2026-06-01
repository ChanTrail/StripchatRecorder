/**
 * 路由配置 / Router Configuration
 */

import { createRouter, createWebHistory } from "vue-router";
import { call } from "@/lib/api";
import type { Settings } from "@/stores/settings";

const router = createRouter({
	history: createWebHistory(),
	routes: [
		{ path: "/setup", component: () => import("../views/SetupView.vue") },
		{ path: "/", component: () => import("../views/HomeView.vue") },
		{ path: "/recordings", component: () => import("../views/RecordingsView.vue") },
		{ path: "/postprocess", component: () => import("../views/PostprocessView.vue") },
		{ path: "/settings", component: () => import("../views/SettingsView.vue") },
		{ path: "/finder", component: () => import("../views/FinderView.vue") },
		{ path: "/relay", component: () => import("../views/RelayView.vue") },
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
		// 后端未就绪时放行，页面自身会处理错误
		setupChecked = true;
	}

	return true;
});

export default router;
