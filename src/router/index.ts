/**
 * 路由配置 / Router Configuration
 *
 * 定义应用的四个主要页面路由：主播列表、录制文件、后处理流水线、设置。
 * Defines the four main page routes: streamer list, recordings, post-processing pipeline, settings.
 */

import { createRouter, createWebHistory } from "vue-router";
import HomeView from "../views/HomeView.vue";
import RecordingsView from "../views/RecordingsView.vue";
import SettingsView from "../views/SettingsView.vue";
import PostprocessView from "../views/PostprocessView.vue";

export default createRouter({
	history: createWebHistory(),
	routes: [
		// 主播列表页 / Streamer list page
		{ path: "/", component: HomeView },
		// 录制文件管理页 / Recording file management page
		{ path: "/recordings", component: RecordingsView },
		// 后处理流水线配置页 / Post-processing pipeline configuration page
		{ path: "/postprocess", component: PostprocessView },
		// 应用设置页 / Application settings page
		{ path: "/settings", component: SettingsView },
	],
});
