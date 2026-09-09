/**
 * 移动端布局检测 / Mobile layout detection
 *
 * 当视口宽度 < 1600px 或高度 < 600px 时，启用移动端 UI 布局。
 * 通过监听 resize 事件响应窗口大小变化。
 *
 * Activates mobile UI layout when viewport width < 1600px or height < 600px.
 * Responds to window resize events reactively.
 */

import { ref, onMounted, onUnmounted } from "vue";

const MOBILE_WIDTH = 1600;
const MOBILE_HEIGHT = 600;

function check(): boolean {
	return window.innerWidth < MOBILE_WIDTH || window.innerHeight < MOBILE_HEIGHT;
}

export function useMobileLayout() {
	const isMobile = ref(false);

	function update() {
		isMobile.value = check();
	}

	onMounted(() => {
		update();
		window.addEventListener("resize", update);
	});

	onUnmounted(() => {
		window.removeEventListener("resize", update);
	});

	return { isMobile };
}
