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

/** 支持的 CDN 顶级域名列表 / Supported CDN top-level domains */
const CDN_TLDS = [
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
		const matchedTld = CDN_TLDS.find((tld) => url.includes(tld));
		if (!matchedTld) {
			resolvedUrl.value = url;
			return;
		}

		// 对每个 CDN 域名创建一个图片加载 Promise，取最先成功的
		// Create an image load Promise for each CDN TLD, take the first to succeed
		const winner = await Promise.any(
			CDN_TLDS.map(
				(tld) =>
					new Promise<string>((resolve, reject) => {
						const candidate = url.replace(matchedTld, tld);
						const img = new Image();
						img.onload = () => resolve(candidate);
						img.onerror = () => reject();
						img.src = candidate;
					}),
			),
		).catch(() => url); // 全部失败时回退到原始 URL / Fall back to original URL if all fail

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
