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
import FinderView from "../views/FinderView.vue";
import RelayView from "../views/RelayView.vue";

export default createRouter({
	history: createWebHistory(),
	routes: [
		{ path: "/", component: HomeView },
		{ path: "/recordings", component: RecordingsView },
		{ path: "/postprocess", component: PostprocessView },
		{ path: "/settings", component: SettingsView },
		{ path: "/finder", component: FinderView },
		{ path: "/relay", component: RelayView },
	],
});
