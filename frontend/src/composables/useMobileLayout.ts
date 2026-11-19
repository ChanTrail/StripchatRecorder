/**
 * 移动端布局检测 / Mobile layout detection
 *
 * 依据 User-Agent 判定移动端（而非视口宽度）：优先使用 Client Hints 的
 * `navigator.userAgentData.mobile`，不可用时回退 UA 正则，并覆盖 iPadOS
 * 桌面 UA（Macintosh + 多点触控）的情况。
 *
 * Detects mobile devices from the User-Agent (not viewport width): prefers
 * Client Hints `navigator.userAgentData.mobile`, falls back to a UA regex,
 * and covers iPadOS reporting a desktop (Macintosh) UA with multi-touch.
 *
 * UA 在会话内不会变化，因此无需监听 resize。
 * The UA does not change within a session, so no resize listener is needed.
 */
import { ref } from "vue";

/** 判断当前 UA 是否移动端（任一信号命中即为真）/ Whether the current UA is a mobile device (true if any signal matches) */
function check(): boolean {
	// Client Hints：移动端请求"桌面版网站"时 UA 会被伪装成桌面，但
	// userAgentData.mobile 仍为 true —— 不能拿它的 false 覆盖 UA 正则命中。
	// Client Hints: when a mobile browser requests "desktop site" the UA is
	// spoofed as desktop but userAgentData.mobile stays true — so a `false`
	// value must not override a positive UA-regex hit.
	const ch = (
		navigator as Navigator & { userAgentData?: { mobile?: boolean } }
	).userAgentData;

	const ua = navigator.userAgent;
	// iPadOS 13+ 默认桌面 UA，靠 Macintosh + maxTouchPoints 识别 /
	// iPadOS 13+ sends a desktop UA by default; identify via Macintosh + maxTouchPoints
	const isIpadOs = /Macintosh/.test(ua) && navigator.maxTouchPoints > 1;
	return (
		isIpadOs ||
		ch?.mobile === true ||
		/Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini|Mobile/i.test(ua)
	);
}

export function useMobileLayout() {
	const isMobile = ref(check());
	return { isMobile };
}
