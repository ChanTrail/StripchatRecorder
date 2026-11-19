/**
 * 快速缩略图加载 Composable / Fast Thumbnail Loading Composable
 *
 * 通过并行竞速多个 CDN 域名来找到响应最快的缩略图源，
 * 从而减少主播卡片的图片加载延迟。
 *
 * Races multiple CDN TLDs in parallel to find the fastest thumbnail source,
 * reducing image load latency on streamer cards.
 */

import { ref, watch, type Ref } from "vue";

/** 缩略图竞速用的 CDN 顶级域名列表（与后端 CDN_TLDS 不同：无 .media，
 * 且顺序即生产竞速顺序）/ CDN TLDs raced for thumbnails (differs from
 * the backend CDN_TLDS: no .media; order mirrors production racing) */
export const THUMB_CDN_TLDS = [
	"doppiocdn.com",
	"doppiocdn.org",
	"doppiocdn.live",
	"doppiocdn.net",
];

/**
 * 对给定的缩略图 URL 进行多 CDN 竞速，返回最快加载成功的 URL。
 * Races the given thumbnail URL across multiple CDNs and returns the fastest one.
 *
 * @param thumbnailUrl - 原始缩略图 URL 的响应式引用 / Reactive ref of the original thumbnail URL
 * @returns 解析后的最优 URL 响应式引用 / Reactive ref of the resolved optimal URL
 */
export function useFastThumbnail(thumbnailUrl: Ref<string | null | undefined>) {
	const resolvedUrl = ref<string | null>(null);

	/**
	 * 并行尝试所有 CDN 域名，取最先加载成功的 URL。
	 * Try all CDN TLDs in parallel, use the first one that loads successfully.
	 *
	 * @param url - 原始图片 URL / Original image URL
	 */
	async function race(url: string) {
		// 检查 URL 是否包含已知 CDN 域名，否则直接使用原始 URL
		// Check if URL contains a known CDN TLD, otherwise use the original URL directly
		const matchedTld = THUMB_CDN_TLDS.find((tld) => url.includes(tld));
		if (!matchedTld) {
			resolvedUrl.value = url;
			return;
		}

		// 创建所有候选 img 元素，胜出后立即清空其余 img 的 src 以中止请求
		// Create all candidate img elements; clear the src of losers immediately after a winner is found
		const imgs = THUMB_CDN_TLDS.map((tld) => {
			const img = new Image();
			img.src = url.replace(matchedTld, tld);
			return { tld, img };
		});

		const winner = await Promise.any(
			imgs.map(
				({ tld, img }) =>
					new Promise<string>((resolve, reject) => {
						img.onload = () => resolve(img.src);
						img.onerror = () => reject();
						// src 已在上面赋值，此处无需重复赋值
						// src is already set above, no need to reassign here
						void tld;
					}),
			),
		).catch(() => url); // 全部失败时回退到原始 URL / Fall back to original URL if all fail

		// 中止未完成的请求：将非胜出者的 src 清空
		// Abort unfinished requests by clearing the src of non-winners
		for (const { img } of imgs) {
			if (img.src !== winner) {
				img.onload = null;
				img.onerror = null;
				img.src = "";
			}
		}

		resolvedUrl.value = winner;
	}

	// 监听 thumbnailUrl 变化，重新竞速 / Watch thumbnailUrl changes and re-race
	watch(
		thumbnailUrl,
		(url) => {
			resolvedUrl.value = null;
			if (url) race(url);
		},
		{ immediate: true },
	);

	return resolvedUrl;
}
