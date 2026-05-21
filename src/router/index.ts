/**
 * 路由配置 / Router Configuration
 *
 * 定义应用的四个主要页面路由：主播列表、录制文件、后处理流水线、设置。
 * Defines the four main page routes: streamer list, recordings, post-processing pipeline, settings.
 */

import { createRouter, createWebHistory } from "vue-router";

export default createRouter({
	history: createWebHistory(),
	routes: [
		{ path: "/", component: () => import("../views/HomeView.vue") },
		{ path: "/recordings", component: () => import("../views/RecordingsView.vue") },
		{ path: "/postprocess", component: () => import("../views/PostprocessView.vue") },
		{ path: "/settings", component: () => import("../views/SettingsView.vue") },
		{ path: "/finder", component: () => import("../views/FinderView.vue") },
		{ path: "/relay", component: () => import("../views/RelayView.vue") },
	],
});
