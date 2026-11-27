/**
 * 图片缩放倍率计算工具 / Image zoom-ratio calculation helpers
 *
 * 仅包含与 DOM 无关的纯计算逻辑，便于在 composable 与回归测试中复用。
 * Contains only DOM-independent pure calculations so the composable and regression
 * tests can share the same semantics.
 */

/** 滚轮缩放步进（百分点）/ Mouse-wheel zoom step in percentage points */
export const IMAGE_ZOOM_STEP_PERCENT = 5;
/** 最小缩放百分比 / Minimum zoom percentage */
export const IMAGE_MIN_ZOOM_PERCENT = 5;
/** 最大缩放百分比 / Maximum zoom percentage */
export const IMAGE_MAX_ZOOM_PERCENT = 1000;

/**
 * 计算图片当前相对原始像素尺寸的缩放百分比。
 *
 * 100% 表示图片按自然尺寸显示（一个图片像素对应一个 CSS 像素）。当图片为了适配
 * 视口而缩小时，即使内部 transform 缩放仍为 1，显示倍率也应小于 100%，与
 * BandiView 等图片查看器的口径一致。
 *
 * Calculate the current zoom percentage relative to the image's natural pixel size.
 *
 * 100% means the image is shown at natural size (one image pixel per CSS pixel). When
 * the image is shrunk to fit the viewport, the displayed ratio must be below 100% even
 * if the internal transform scale is still 1, matching the semantics used by image
 * viewers such as BandiView.
 *
 * Params：
 * - layoutWidth: number —— 图片元素未应用 transform 前的布局宽度（CSS px）/ Layout width before transform (CSS px)
 * - layoutHeight: number —— 图片元素未应用 transform 前的布局高度（CSS px）/ Layout height before transform (CSS px)
 * - naturalWidth: number —— 图片原始宽度（像素）/ Natural image width in pixels
 * - naturalHeight: number —— 图片原始高度（像素）/ Natural image height in pixels
 * - transformScale: number —— 当前附加在适配结果上的 transform 缩放 / Transform scale applied on top of the fitted layout
 *
 * @returns 四舍五入后的缩放百分比 / Rounded zoom percentage
 */
export function calculateImageZoomPercent(
	layoutWidth: number,
	layoutHeight: number,
	naturalWidth: number,
	naturalHeight: number,
	transformScale: number,
): number {
	const fallbackPercent = Math.round(transformScale * 100);
	if (
		!Number.isFinite(naturalWidth) ||
		!Number.isFinite(naturalHeight) ||
		naturalWidth <= 0 ||
		naturalHeight <= 0 ||
		!Number.isFinite(transformScale) ||
		transformScale <= 0
	) {
		return fallbackPercent;
	}

	const ratios = [layoutWidth / naturalWidth, layoutHeight / naturalHeight].filter(
		(ratio) => Number.isFinite(ratio) && ratio > 0,
	);
	// object-fit: contain 以宽度与高度中更受限的一侧决定实际倍率；若某侧尚未测得，
	// 则退而使用另一侧。
	// object-fit: contain is limited by the more constrained of width and height; if one
	// side has not been measured yet, fall back to the other side.
	const baseRatio = ratios.length > 0 ? Math.min(...ratios) : Number.NaN;

	if (!Number.isFinite(baseRatio) || baseRatio <= 0) return fallbackPercent;
	return Math.round(baseRatio * transformScale * 100);
}

/**
 * 按固定百分点计算下一次滚轮缩放百分比。
 *
 * 以当前界面显示的整数百分比为基准增加或减少固定百分点，而不是对内部 transform
 * 做相对乘法，因此从 40% 放大一步会变为 45%，而不是 44%。
 *
 * Calculate the next wheel zoom percentage using a fixed percentage-point step.
 *
 * The integer percentage shown in the UI is used as the baseline instead of applying
 * a relative multiplication to the internal transform, so zooming in once from 40%
 * produces 45%, not 44%.
 *
 * Params：
 * - currentPercent: number —— 当前缩放百分比 / Current zoom percentage
 * - direction: number —— 缩放方向，正值放大、负值缩小 / Zoom direction; positive zooms in and negative zooms out
 *
 * @returns 限制在允许范围内的下一次缩放百分比 / Next zoom percentage clamped to the supported range
 */
export function calculateSteppedImageZoomPercent(
	currentPercent: number,
	direction: number,
): number {
	const safeCurrent = Number.isFinite(currentPercent) ? Math.round(currentPercent) : 100;
	const step = direction >= 0 ? IMAGE_ZOOM_STEP_PERCENT : -IMAGE_ZOOM_STEP_PERCENT;
	return Math.min(IMAGE_MAX_ZOOM_PERCENT, Math.max(IMAGE_MIN_ZOOM_PERCENT, safeCurrent + step));
}

/**
 * 将绝对缩放百分比换算为作用在适配布局上的 transform 倍率。
 *
 * Convert an absolute zoom percentage into the transform scale applied on top of the
 * viewport-fitted layout.
 *
 * Params：
 * - zoomPercent: number —— 相对图片原始尺寸的缩放百分比 / Zoom percentage relative to natural image size
 * - fitRatio: number —— 未应用 transform 时的适配倍率 / Fit ratio before transform
 *
 * @returns 应应用的 transform 倍率；输入无效时返回 1 / Transform scale to apply, or 1 for invalid input
 */
export function calculateTransformScaleForZoomPercent(
	zoomPercent: number,
	fitRatio: number,
): number {
	if (
		!Number.isFinite(zoomPercent) ||
		!Number.isFinite(fitRatio) ||
		zoomPercent <= 0 ||
		fitRatio <= 0
	) {
		return 1;
	}
	// 限制到小数点后 6 位，避免 0.8749999999999999 这类浮点噪声进入 CSS 与测试。
	// Round to six decimal places so floating-point noise such as
	// 0.8749999999999999 does not leak into CSS or tests.
	return Math.round((zoomPercent / 100 / fitRatio) * 1_000_000) / 1_000_000;
}
