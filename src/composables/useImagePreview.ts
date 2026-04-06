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

import { ref } from "vue";

/**
 * 图片预览状态与交互逻辑。
 * Image preview state and interaction logic.
 */
export function useImagePreview() {
	/** 预览弹窗是否打开 / Whether the preview dialog is open */
	const previewOpen = ref(false);
	/** 当前预览图片的 URL / Current preview image URL */
	const previewUrl = ref("");
	/** 当前预览图片的标题 / Current preview image title */
	const previewTitle = ref("");
	/** 当前缩放比例（1 = 原始适配尺寸）/ Current zoom scale (1 = fit size) */
	const previewScale = ref(1);
	/** 当前平移偏移量（像素）/ Current translation offset (pixels) */
	const previewTranslate = ref({ x: 0, y: 0 });
	/** 视口容器元素引用 / Viewport container element ref */
	const previewViewportRef = ref<HTMLElement | null>(null);
	/** 图片元素引用 / Image element ref */
	const previewImageRef = ref<HTMLImageElement | null>(null);
	/** 是否正在拖拽 / Whether currently dragging */
	const isDragging = ref(false);

	// 拖拽起始状态：鼠标位置和平移偏移量快照
	// Drag start state: mouse position and translation offset snapshot
	let dragStart = { x: 0, y: 0, tx: 0, ty: 0 };

	/**
	 * 将值限制在 [min, max] 范围内。
	 * Clamp a value to the [min, max] range.
	 */
	function clamp(v: number, min: number, max: number) {
		return Math.min(max, Math.max(min, v));
	}

	/**
	 * 重置缩放和平移到初始状态。
	 * Reset zoom and translation to initial state.
	 */
	function resetPreviewTransform() {
		previewScale.value = 1;
		previewTranslate.value = { x: 0, y: 0 };
	}

	/**
	 * 获取视口和图片的尺寸信息，用于计算缩放边界。
	 * Get viewport and image dimension metrics for calculating zoom bounds.
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
		// 计算图片适配视口的基础缩放比 / Calculate base scale to fit image in viewport
		const fit = Math.min(
			1,
			viewportWidth / imageEl.naturalWidth,
			viewportHeight / imageEl.naturalHeight,
		);
		return {
			viewportRect,
			viewportWidth,
			viewportHeight,
			baseWidth: imageEl.naturalWidth * fit,
			baseHeight: imageEl.naturalHeight * fit,
		};
	}

	/**
	 * 将平移偏移量限制在合法范围内，防止图片移出视口。
	 * Clamp translation offset to valid range, preventing the image from leaving the viewport.
	 *
	 * @param x - 目标 X 偏移 / Target X offset
	 * @param y - 目标 Y 偏移 / Target Y offset
	 * @param scale - 当前缩放比例 / Current scale
	 * @param metrics - 可选的预计算尺寸信息 / Optional pre-computed metrics
	 */
	function clampPreviewTranslate(
		x: number,
		y: number,
		scale: number,
		metrics?: ReturnType<typeof getPreviewMetrics>,
	) {
		// 缩放比 <= 1 时不允许平移 / No panning allowed when scale <= 1
		if (scale <= 1) return { x: 0, y: 0 };
		const m = metrics ?? getPreviewMetrics();
		if (!m) return { x, y };
		const maxX = Math.max(0, (m.baseWidth * scale - m.viewportWidth) / 2);
		const maxY = Math.max(0, (m.baseHeight * scale - m.viewportHeight) / 2);
		return { x: clamp(x, -maxX, maxX), y: clamp(y, -maxY, maxY) };
	}

	/**
	 * 图片加载完成时重置变换。
	 * Reset transform when image finishes loading.
	 */
	function onPreviewImageLoad() {
		resetPreviewTransform();
	}

	/**
	 * 处理鼠标滚轮缩放，以光标位置为锚点进行缩放。
	 * Handle mouse wheel zoom, anchored at the cursor position.
	 *
	 * @param e - 滚轮事件 / Wheel event
	 */
	function onPreviewWheel(e: WheelEvent) {
		e.preventDefault();
		const metrics = getPreviewMetrics();
		if (!metrics) return;
		const prevScale = previewScale.value;
		const delta = e.deltaY > 0 ? -0.1 : 0.1;
		// 缩放范围限制在 [1, 10] / Clamp scale to [1, 10]
		const nextScale = Math.min(
			10,
			Math.max(1, Math.round((prevScale + delta) * 100) / 100),
		);
		if (nextScale === prevScale) return;

		// 以光标为锚点计算新的平移偏移，保持光标下的图片内容不变
		// Calculate new translation offset anchored at cursor to keep content under cursor stable
		const cursorX = e.clientX - metrics.viewportRect.left;
		const cursorY = e.clientY - metrics.viewportRect.top;
		const curCenterX = metrics.viewportWidth / 2 + previewTranslate.value.x;
		const curCenterY = metrics.viewportHeight / 2 + previewTranslate.value.y;
		const halfW = (metrics.baseWidth * prevScale) / 2;
		const halfH = (metrics.baseHeight * prevScale) / 2;
		const anchorX = clamp(cursorX, curCenterX - halfW, curCenterX + halfW);
		const anchorY = clamp(cursorY, curCenterY - halfH, curCenterY + halfH);
		const localX = (anchorX - curCenterX) / prevScale;
		const localY = (anchorY - curCenterY) / prevScale;
		let nextX = anchorX - metrics.viewportWidth / 2 - localX * nextScale;
		let nextY = anchorY - metrics.viewportHeight / 2 - localY * nextScale;
		({ x: nextX, y: nextY } = clampPreviewTranslate(
			nextX,
			nextY,
			nextScale,
			metrics,
		));
		previewScale.value = nextScale;
		previewTranslate.value = { x: nextX, y: nextY };
	}

	/**
	 * 处理鼠标按下事件，开始拖拽（仅在缩放比 > 1 时生效）。
	 * Handle mouse down to start dragging (only when scale > 1).
	 *
	 * @param e - 鼠标事件 / Mouse event
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
	 * @param e - 鼠标事件 / Mouse event
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
	 */
	function onDocMouseup() {
		isDragging.value = false;
	}

	/**
	 * 打开图片预览弹窗。
	 * Open the image preview dialog.
	 *
	 * @param url - 图片 URL / Image URL
	 * @param title - 图片标题 / Image title
	 */
	function openPreview(url: string, title: string) {
		previewUrl.value = url;
		previewTitle.value = title;
		resetPreviewTransform();
		previewOpen.value = true;
	}

	return {
		previewOpen,
		previewUrl,
		previewTitle,
		previewScale,
		previewTranslate,
		previewViewportRef,
		previewImageRef,
		isDragging,
		resetPreviewTransform,
		onPreviewImageLoad,
		onPreviewWheel,
		onPreviewMousedown,
		onDocMousemove,
		onDocMouseup,
		openPreview,
	};
}
