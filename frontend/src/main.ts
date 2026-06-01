/**
 * 应用程序入口文件 / Application Entry Point
 *
 * 初始化 Vue 应用，挂载 Pinia 状态管理、Vue Router 路由，并将应用挂载到 DOM。
 * Initializes the Vue application, mounts Pinia state management and Vue Router,
 * then mounts the app to the DOM.
 */

import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import router from "./router";
import i18n from "./i18n";
import "./style.css";
import "vue-sonner/style.css";

// 修复第三方库（reka-ui）未将 wheel/touchstart 事件标记为 passive 的问题
// Fix third-party libraries (reka-ui) not marking wheel/touchstart listeners as passive
const _addEventListener = EventTarget.prototype.addEventListener;
EventTarget.prototype.addEventListener = function (type, listener, options) {
	if (type === "wheel" || type === "touchstart") {
		const opts = options === undefined || options === null
			? { passive: true }
			: typeof options === "object"
				? { passive: true, ...options }
				: options;
		return _addEventListener.call(this, type, listener, opts);
	}
	return _addEventListener.call(this, type, listener, options);
};

// 创建 Vue 应用实例，注册 Pinia 和路由，挂载到 #app 节点
// Create Vue app instance, register Pinia and router, mount to #app element
createApp(App).use(createPinia()).use(router).use(i18n).mount("#app");
