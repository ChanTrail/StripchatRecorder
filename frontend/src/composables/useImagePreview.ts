/**
 * 图片预览 Composable / Image Preview Composable
 *
 * 提供带缩放和平移功能的图片预览弹窗逻辑。
 * 支持鼠标滚轮缩放（以光标为锚点）、鼠标拖拽平移，并自动限制平移范围防止图片移出视口。
 *
 * Provides image preview dialog logic with zoom and pan support.
 * Supports mouse wheel zoom (anchored at cursor), mouse drag panning,
 * and automatically clamps translation to prevent the image from leaving the viewport.
 */

import { computed, ref, watch } from "vue";
import { useElementSize } from "@vueuse/core";
import {
	calculateImageZoomPercent,
	calculateSteppedImageZoomPercent,
	calculateTransformScaleForZoomPercent,
	IMAGE_MAX_ZOOM_PERCENT,
	IMAGE_MIN_ZOOM_PERCENT,
} from "@/utils/imageZoom";

/** 当前持有的 blob URL（关闭时需 revoke）/ Current held blob URL (must be revoked on close) */
let _currentBlobUrl: string | null = null;

/** 用 fetch + Authorization 头加载图片，返回 blob URL / Fetch image with Authorization header, returns blob URL */
async function fetchAuthImage(url: string): Promise<string> {
	const token = localStorage.getItem("admin_token");
	const headers: Record<string, string> = {};
	// 仅同源请求附带认证头，避免把后台 token 泄露给第三方域
	// Only attach the token to same-origin requests — never leak it to third-party hosts
	if (token && (url.startsWith("/") || url.startsWith(window.location.origin))) {
		headers["Authorization"] = `Bearer ${token}`;
	}
	const res = await fetch(url, { headers });
	if (!res.ok) throw new Error(`Failed to load image: ${res.status}`);
	const blob = await res.blob();
	return URL.createObjectURL(blob);
}

/**
 * 图片预览状态与交互逻辑。
 * Image preview state and interaction logic.
 */
export function useImagePreview() {
	/** 预览弹窗是否打开 / Whether the preview dialog is open */
	const previewOpen = ref(false);

	// 弹窗关闭时释放 blob URL，避免内存泄漏
	// Revoke blob URL when dialog closes to prevent memory leaks
	watch(previewOpen, (open) => {
		if (!open && _currentBlobUrl) {
			URL.revokeObjectURL(_currentBlobUrl);
			_currentBlobUrl = null;
			previewUrl.value = "";
		}
		if (!open) {
			isLoadingPreview.value = false;
		}
	});
	/** 当前预览图片的 URL / Current preview image URL */
	const previewUrl = ref("");
	/** 图片是否正在通过 fetch 加载中（blob URL 尚未就绪）/ Whether image is being fetched (blob URL not yet ready) */
	const isLoadingPreview = ref(false);
	/** 当前预览图片的标题 / Current preview image title */
	const previewTitle = ref("");
	/** 当前附加在适配结果上的 transform 缩放（1 = 适应视口布局）/ Transform scale applied on top of the viewport-fit layout (1 = fit layout) */
	const previewScale = ref(1);
	/** 当前平移偏移量（像素）/ Current translation offset (pixels) */
	const previewTranslate = ref({ x: 0, y: 0 });
	/** 视口容器元素引用 / Viewport container element ref */
	const previewViewportRef = ref<HTMLElement | null>(null);
	/** 图片元素引用 / Image element ref */
	const previewImageRef = ref<HTMLImageElement | null>(null);
	/** 当前图片的原始尺寸 / Natural size of the current image */
	const previewNaturalSize = ref({ width: 0, height: 0 });
	/** 图片元素未应用 transform 前的布局尺寸 / Layout size of the image element before transform */
	const { width: previewImageLayoutWidth, height: previewImageLayoutHeight } =
		useElementSize(previewImageRef);
	/** 相对图片原始尺寸的当前缩放百分比 / Current zoom percentage relative to the image's natural size */
	const previewZoomPercent = computed(() =>
		calculateImageZoomPercent(
			// ResizeObserver 的首次回调前，先用 clientWidth/clientHeight 兜底，避免短暂显示成旧口径。
			// Before ResizeObserver's first callback, fall back to clientWidth/clientHeight to
			// avoid briefly showing the legacy ratio.
			previewImageLayoutWidth.value || previewImageRef.value?.clientWidth || 0,
			previewImageLayoutHeight.value || previewImageRef.value?.clientHeight || 0,
			previewNaturalSize.value.width,
			previewNaturalSize.value.height,
			previewScale.value,
		),
	);
	/** 是否正在拖拽 / Whether currently dragging */
	const isDragging = ref(false);

	/**
	 * 根据图片自然尺寸计算图片容器的自适应尺寸（弹窗通过 w-fit/h-fit 跟随）。
	 * 以图片宽高比为基准，在 90vw × (90vh - header) 范围内尽量贴合图片尺寸。
	 *
	 * Computed viewport size that adapts to the image's natural aspect ratio,
	 * fitting within 90vw × (90vh - header). The dialog uses w-fit/h-fit to follow.
	 */
	const viewportSize = ref({ width: "min(90vw, 90vh)", height: "min(90vh, 90vw)" });

	// 拖拽起始状态：鼠标位置和平移偏移量快照
	// Drag start state: mouse position and translation offset snapshot
	let dragStart = { x: 0, y: 0, tx: 0, ty: 0 };

	// 双指捏合状态 / Pinch-to-zoom state
	let pinchStartDist = 0;
	let pinchStartScale = 1;
	let pinchStartMidX = 0;
	let pinchStartMidY = 0;
	let pinchStartTx = 0;
	let pinchStartTy = 0;

	/**
	 * 将值限制在 [min, max] 范围内。
	 * Clamp a value to the [min, max] range.
	 *
	 * Params：
	 * - v: number —— 待限制的值 / Value to clamp
	 * - min: number —— 最小值 / Minimum value
	 * - max: number —— 最大值 / Maximum value
	 *
	 * @returns 限制后的值 / Clamped value
	 */
	function clamp(v: number, min: number, max: number) {
		return Math.min(max, Math.max(min, v));
	}

	/**
	 * 重置缩放和平移到初始状态。
	 * Reset zoom and translation to initial state.
	 *
	 * Params：
	 * 无 / None.
	 */
	function resetPreviewTransform() {
		previewScale.value = 1;
		previewTranslate.value = { x: 0, y: 0 };
	}

	/**
	 * 获取视口和图片的尺寸信息，用于计算缩放边界。
	 * Get viewport and image dimension metrics for calculating zoom bounds.
	 *
	 * Params：
	 * 无 / None.
	 *
	 * @returns 尺寸信息对象，或 null（元素未就绪时）/ Metrics object, or null if elements not ready
	 */
	function getPreviewMetrics() {
		const viewportEl = previewViewportRef.value;
		const imageEl = previewImageRef.value;
		if (!viewportEl || !imageEl) return null;
		if (imageEl.naturalWidth <= 0 || imageEl.naturalHeight <= 0) return null;
		const viewportRect = viewportEl.getBoundingClientRect();
		const viewportWidth = viewportRect.width;
		const viewportHeight = viewportRect.height;
		if (viewportWidth <= 0 || viewportHeight <= 0) return null;

		// 图片作为 flex item 在内容盒内居中，因此必须把视口 padding 从有效画布中排除。
		// The image is centered as a flex item inside the content box, so viewport
		// padding must be excluded from the effective canvas.
		const viewportStyle = window.getComputedStyle(viewportEl);
		const paddingLeft = Number.parseFloat(viewportStyle.paddingLeft) || 0;
		const paddingRight = Number.parseFloat(viewportStyle.paddingRight) || 0;
		const paddingTop = Number.parseFloat(viewportStyle.paddingTop) || 0;
		const paddingBottom = Number.parseFloat(viewportStyle.paddingBottom) || 0;
		const contentWidth = Math.max(0, viewportWidth - paddingLeft - paddingRight);
		const contentHeight = Math.max(0, viewportHeight - paddingTop - paddingBottom);
		if (contentWidth <= 0 || contentHeight <= 0) return null;

		// 计算图片适配视口内容盒的基础缩放比。
		// Calculate the base ratio for fitting the image into the viewport content box.
		const fit = Math.min(
			1,
			contentWidth / imageEl.naturalWidth,
			contentHeight / imageEl.naturalHeight,
		);

		// transform 不影响布局尺寸，因此 clientWidth/clientHeight 或 ResizeObserver 测得
		// 的值就是缩放前的实际适配尺寸；优先使用它们，避免把视口 padding 误算进 fit。
		// Transforms do not affect layout size, so clientWidth/clientHeight or the
		// ResizeObserver measurement is the real pre-zoom fitted size. Prefer it so
		// viewport padding is not mistakenly included in the fit ratio.
		const measuredWidth = previewImageLayoutWidth.value || imageEl.clientWidth;
		const measuredHeight = previewImageLayoutHeight.value || imageEl.clientHeight;
		const hasMeasuredSize = measuredWidth > 0 && measuredHeight > 0;
		const baseWidth = hasMeasuredSize ? measuredWidth : imageEl.naturalWidth * fit;
		const baseHeight = hasMeasuredSize ? measuredHeight : imageEl.naturalHeight * fit;
		const fitRatio = Math.min(
			baseWidth / imageEl.naturalWidth,
			baseHeight / imageEl.naturalHeight,
		);

		return {
			viewportRect,
			viewportWidth,
			viewportHeight,
			contentWidth,
			contentHeight,
			centerX: paddingLeft + contentWidth / 2,
			centerY: paddingTop + contentHeight / 2,
			naturalWidth: imageEl.naturalWidth,
			naturalHeight: imageEl.naturalHeight,
			baseWidth,
			baseHeight,
			fitRatio,
		};
	}

	/**
	 * 将平移偏移量限制在合法范围内，防止图片移出视口。
	 * Clamp translation offset to valid range, preventing the image from leaving the viewport.
	 *
	 * Params：
	 * - x: number —— 目标 X 偏移 / Target X offset
	 * - y: number —— 目标 Y 偏移 / Target Y offset
	 * - scale: number —— 当前 transform 缩放比例 / Current transform scale
	 * - metrics: object | null —— 可选的预计算尺寸信息 / Optional pre-computed metrics
	 *
	 * @returns 限制后的平移偏移 / Clamped translation offset
	 */
	function clampPreviewTranslate(
		x: number,
		y: number,
		scale: number,
		metrics?: ReturnType<typeof getPreviewMetrics>,
	) {
		// transform 缩放比 <= 1 时图片不超过适配尺寸，无需也不允许平移。
		// When the transform scale is <= 1, the image does not exceed its fitted size,
		// so panning is unnecessary and disabled.
		if (scale <= 1) return { x: 0, y: 0 };
		const m = metrics ?? getPreviewMetrics();
		if (!m) return { x, y };
		const maxX = Math.max(0, (m.baseWidth * scale - m.contentWidth) / 2);
		const maxY = Math.max(0, (m.baseHeight * scale - m.contentHeight) / 2);
		return { x: clamp(x, -maxX, maxX), y: clamp(y, -maxY, maxY) };
	}

	/**
	 * 图片加载完成时重置变换并计算图片容器自适应尺寸。
	 * Reset transform and compute adaptive viewport size when image finishes loading.
	 *
	 * Params：
	 * 无 / None.
	 */
	function onPreviewImageLoad() {
		resetPreviewTransform();

		const img = previewImageRef.value;
		if (!img || img.naturalWidth <= 0 || img.naturalHeight <= 0) return;
		previewNaturalSize.value = { width: img.naturalWidth, height: img.naturalHeight };

		// header 高度约 52px（px-4 pt-4 pb-2 + DialogTitle 行高）。
		// Approximate header height: 52px (px-4 pt-4 pb-2 + DialogTitle line height).
		const HEADER_H = 52;
		const viewportEl = previewViewportRef.value;
		const viewportStyle = viewportEl ? window.getComputedStyle(viewportEl) : null;
		const paddingX = viewportStyle
			? (Number.parseFloat(viewportStyle.paddingLeft) || 0) +
				(Number.parseFloat(viewportStyle.paddingRight) || 0)
			: 0;
		const paddingY = viewportStyle
			? (Number.parseFloat(viewportStyle.paddingTop) || 0) +
				(Number.parseFloat(viewportStyle.paddingBottom) || 0)
			: 0;
		const maxOuterW = window.innerWidth * 0.9;
		const maxOuterH = window.innerHeight * 0.9 - HEADER_H;
		const maxContentW = Math.max(1, maxOuterW - paddingX);
		const maxContentH = Math.max(1, maxOuterH - paddingY);
		const ratio = img.naturalWidth / img.naturalHeight;

		// 先按内容盒最大宽度计算高度，再检查是否超出内容盒最大高度。视口外层尺寸
		// 之后再加回 padding，因此小图可以真正以 100% 自然尺寸显示。
		// Try fitting the content box by width first, then clamp by content-box height.
		// Padding is added back to the outer viewport afterwards, so small images can
		// truly be displayed at their 100% natural size.
		let contentW = Math.min(img.naturalWidth, maxContentW);
		let contentH = contentW / ratio;
		if (contentH > maxContentH) {
			contentH = maxContentH;
			contentW = contentH * ratio;
		}

		viewportSize.value = {
			width: `${Math.ceil(contentW + paddingX)}px`,
			height: `${Math.ceil(contentH + paddingY)}px`,
		};
	}

	/**
	 * 处理鼠标滚轮缩放，以光标位置为锚点进行缩放。
	 * Handle mouse wheel zoom, anchored at the cursor position.
	 *
	 * Params：
	 * - e: WheelEvent —— 滚轮事件 / Wheel event
	 */
	function onPreviewWheel(e: WheelEvent) {
		e.preventDefault();
		const metrics = getPreviewMetrics();
		if (!metrics) return;
		const prevScale = previewScale.value;
		const currentPercent = calculateImageZoomPercent(
			metrics.baseWidth,
			metrics.baseHeight,
			metrics.naturalWidth,
			metrics.naturalHeight,
			prevScale,
		);
		// 以界面显示的整数百分比为基准，每次固定增减 5 个百分点。
		// Use the integer percentage shown in the UI as the baseline and adjust it
		// by a fixed five percentage points per wheel step.
		const nextPercent = calculateSteppedImageZoomPercent(currentPercent, e.deltaY > 0 ? -1 : 1);
		if (nextPercent === currentPercent) return;
		const nextScale = calculateTransformScaleForZoomPercent(nextPercent, metrics.fitRatio);
		if (nextScale === prevScale) return;

		// 以光标为锚点计算新的平移偏移，保持光标下的图片内容不变。图片初始中心
		// 位于视口内容盒中心，而不是包含 padding 的外盒中心。
		// Calculate the new translation anchored at the cursor to keep the content
		// under the cursor stable. The image's initial center is the viewport content-box
		// center, not the center of the outer box that includes padding.
		const cursorX = e.clientX - metrics.viewportRect.left;
		const cursorY = e.clientY - metrics.viewportRect.top;
		const curCenterX = metrics.centerX + previewTranslate.value.x;
		const curCenterY = metrics.centerY + previewTranslate.value.y;
		const halfW = (metrics.baseWidth * prevScale) / 2;
		const halfH = (metrics.baseHeight * prevScale) / 2;
		const anchorX = clamp(cursorX, curCenterX - halfW, curCenterX + halfW);
		const anchorY = clamp(cursorY, curCenterY - halfH, curCenterY + halfH);
		const localX = (anchorX - curCenterX) / prevScale;
		const localY = (anchorY - curCenterY) / prevScale;
		let nextX = anchorX - metrics.centerX - localX * nextScale;
		let nextY = anchorY - metrics.centerY - localY * nextScale;
		({ x: nextX, y: nextY } = clampPreviewTranslate(nextX, nextY, nextScale, metrics));
		previewScale.value = nextScale;
		previewTranslate.value = { x: nextX, y: nextY };
	}

	/**
	 * 处理鼠标按下事件，开始拖拽（仅在缩放比 > 1 时生效）。
	 * Handle mouse down to start dragging (only when scale > 1).
	 *
	 * Params：
	 * - e: MouseEvent —— 鼠标事件 / Mouse event
	 */
	function onPreviewMousedown(e: MouseEvent) {
		if (e.button !== 0 || previewScale.value <= 1) return;
		isDragging.value = true;
		dragStart = {
			x: e.clientX,
			y: e.clientY,
			tx: previewTranslate.value.x,
			ty: previewTranslate.value.y,
		};
		e.preventDefault();
	}

	/**
	 * 处理文档级鼠标移动事件，更新拖拽平移偏移。
	 * Handle document-level mouse move to update drag translation.
	 *
	 * Params：
	 * - e: MouseEvent —— 鼠标事件 / Mouse event
	 */
	function onDocMousemove(e: MouseEvent) {
		if (!isDragging.value) return;
		previewTranslate.value = clampPreviewTranslate(
			dragStart.tx + (e.clientX - dragStart.x),
			dragStart.ty + (e.clientY - dragStart.y),
			previewScale.value,
		);
	}

	/**
	 * 处理文档级鼠标释放事件，结束拖拽。
	 * Handle document-level mouse up to end dragging.
	 *
	 * Params：
	 * 无 / None.
	 */
	function onDocMouseup() {
		isDragging.value = false;
	}

	/**
	 * 双指触摸开始：记录初始捏合距离、中心点和变换快照。
	 * Touch start with two fingers: record initial pinch distance, midpoint, and transform snapshot.
	 *
	 * Params：
	 * - e: TouchEvent —— 触摸事件 / Touch event
	 */
	function onViewportTouchstart(e: TouchEvent) {
		if (e.touches.length === 2) {
			e.preventDefault();
			const t0 = e.touches[0];
			const t1 = e.touches[1];
			pinchStartDist = Math.hypot(t1.clientX - t0.clientX, t1.clientY - t0.clientY);
			pinchStartScale = previewScale.value;
			pinchStartMidX = (t0.clientX + t1.clientX) / 2;
			pinchStartMidY = (t0.clientY + t1.clientY) / 2;
			pinchStartTx = previewTranslate.value.x;
			pinchStartTy = previewTranslate.value.y;
		} else if (e.touches.length === 1 && previewScale.value > 1) {
			// 单指拖拽（缩放后）/ Single-finger pan (after zooming in)
			dragStart = {
				x: e.touches[0].clientX,
				y: e.touches[0].clientY,
				tx: previewTranslate.value.x,
				ty: previewTranslate.value.y,
			};
			isDragging.value = true;
		}
	}

	/**
	 * 双指触摸移动：计算缩放比例和平移偏移，以双指中心点为锚点缩放。
	 * Touch move with two fingers: compute scale and translation anchored at the pinch midpoint.
	 *
	 * Params：
	 * - e: TouchEvent —— 触摸事件 / Touch event
	 */
	function onViewportTouchmove(e: TouchEvent) {
		if (e.touches.length === 2) {
			e.preventDefault();
			const t0 = e.touches[0];
			const t1 = e.touches[1];
			const dist = Math.hypot(t1.clientX - t0.clientX, t1.clientY - t0.clientY);
			if (pinchStartDist === 0) return;

			// 双指捏合保持连续比例缩放，但上下限仍按相对图片原始尺寸的百分比换算。
			// Pinch zoom remains continuous, while its bounds are still converted from
			// percentages relative to the image's natural size.
			const metrics = getPreviewMetrics();
			if (!metrics) return;
			const rawScale = (dist / pinchStartDist) * pinchStartScale;
			const minScale = calculateTransformScaleForZoomPercent(
				IMAGE_MIN_ZOOM_PERCENT,
				metrics.fitRatio,
			);
			const maxScale = calculateTransformScaleForZoomPercent(
				IMAGE_MAX_ZOOM_PERCENT,
				metrics.fitRatio,
			);
			const nextScale = clamp(rawScale, minScale, maxScale);

			// 以捏合起始中心点为锚点计算平移偏移。
			// Compute translation offset anchored at the initial pinch midpoint.
			const curMidX = pinchStartMidX - metrics.viewportRect.left;
			const curMidY = pinchStartMidY - metrics.viewportRect.top;
			const curCenterX = metrics.centerX + pinchStartTx;
			const curCenterY = metrics.centerY + pinchStartTy;
			const localX = (curMidX - curCenterX) / pinchStartScale;
			const localY = (curMidY - curCenterY) / pinchStartScale;
			let nextX = curMidX - metrics.centerX - localX * nextScale;
			let nextY = curMidY - metrics.centerY - localY * nextScale;
			({ x: nextX, y: nextY } = clampPreviewTranslate(nextX, nextY, nextScale, metrics));

			previewScale.value = nextScale;
			previewTranslate.value = { x: nextX, y: nextY };
		} else if (e.touches.length === 1 && isDragging.value) {
			e.preventDefault();
			previewTranslate.value = clampPreviewTranslate(
				dragStart.tx + (e.touches[0].clientX - dragStart.x),
				dragStart.ty + (e.touches[0].clientY - dragStart.y),
				previewScale.value,
			);
		}
	}

	/**
	 * 触摸结束：重置捏合状态和拖拽状态。
	 * Touch end: reset pinch and drag state.
	 *
	 * Params：
	 * - e: TouchEvent —— 触摸事件 / Touch event
	 */
	function onViewportTouchend(e: TouchEvent) {
		if (e.touches.length < 2) {
			pinchStartDist = 0;
		}
		if (e.touches.length === 0) {
			isDragging.value = false;
		}
	}

	/**
	 * 打开图片预览弹窗。
	 * Open the image preview dialog.
	 *
	 * Params：
	 * - url: string —— 图片 URL（会通过 fetch + Authorization 头加载）/ Image URL (loaded via fetch with Authorization header)
	 * - title: string —— 图片标题 / Image title
	 */
	async function openPreview(url: string, title: string) {
		previewTitle.value = title;
		resetPreviewTransform();
		previewNaturalSize.value = { width: 0, height: 0 };
		// 打开时先用最大尺寸占位，fetch 完成后图片加载时再自适应
		// Use max size as placeholder while fetching; viewport adapts once image loads
		viewportSize.value = {
			width: `${Math.round(window.innerWidth * 0.9)}px`,
			height: `${Math.round(window.innerHeight * 0.9 - 52)}px`,
		};
		previewUrl.value = "";
		isLoadingPreview.value = true;
		previewOpen.value = true;

		// 释放上一次的 blob URL / Revoke previous blob URL if any
		if (_currentBlobUrl) {
			URL.revokeObjectURL(_currentBlobUrl);
			_currentBlobUrl = null;
		}

		try {
			const blobUrl = await fetchAuthImage(url);
			_currentBlobUrl = blobUrl;
			previewUrl.value = blobUrl;
		} catch {
			// fetch 失败（如外部 CDN 无 CORS 头）时退回原始 URL——<img> 直接显示不受 CORS 限制
			// On fetch failure (e.g. external CDN without CORS) fall back to the raw
			// URL — <img> rendering isn't subject to CORS
			previewUrl.value = url;
		} finally {
			isLoadingPreview.value = false;
		}
	}

	return {
		previewOpen,
		previewUrl,
		isLoadingPreview,
		previewTitle,
		previewScale,
		previewZoomPercent,
		previewTranslate,
		previewViewportRef,
		previewImageRef,
		isDragging,
		viewportSize,
		resetPreviewTransform,
		onPreviewImageLoad,
		onPreviewWheel,
		onPreviewMousedown,
		onDocMousemove,
		onDocMouseup,
		onViewportTouchstart,
		onViewportTouchmove,
		onViewportTouchend,
		openPreview,
	};
}
