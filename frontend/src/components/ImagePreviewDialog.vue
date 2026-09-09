<!--
    图片预览对话框组件 / Image Preview Dialog Component

    带缩放（滚轮/双指捏合，以光标为锚点）和平移（拖拽/单指）功能的图片预览弹窗。
    移动端全屏覆盖，左上角显示返回按钮。
    内部持有全部预览状态，通过 openPreview(url, title) 暴露给父组件调用。

    Image preview dialog with zoom (mouse wheel / pinch) and pan (drag / single-finger) support.
    On mobile: full-screen overlay with a back button in the top-left corner.
    Holds all preview state internally; exposes openPreview(url, title) for the parent to call.

    Exposes:
        openPreview(url, title) - 打开预览并加载图片 / Open the preview and load an image
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, watchEffect } from "vue";
	import { useImagePreview } from "@/composables/useImagePreview";
	import { useMobileLayout } from "@/composables/useMobileLayout";
	import { Button } from "@/components/ui/button";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import { ArrowLeft } from "@lucide/vue";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();
	const { isMobile } = useMobileLayout();

	const {
		previewOpen,
		previewUrl,
		isLoadingPreview,
		previewTitle,
		previewScale,
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
	} = useImagePreview();

	onMounted(() => {
		document.addEventListener("mousemove", onDocMousemove);
		document.addEventListener("mouseup", onDocMouseup);
	});

	// viewport ref 在 dialog 打开后才挂载，用 watchEffect 等它就绪后以 passive:false 绑定 touch 事件
	// viewport ref is mounted only after the dialog opens; use watchEffect to bind
	// touch listeners with passive:false once the element is available
	let _touchEl: HTMLElement | null = null;
	watchEffect(() => {
		const el = previewViewportRef.value;
		if (el && el !== _touchEl) {
			if (_touchEl) {
				_touchEl.removeEventListener("touchstart", onViewportTouchstart);
				_touchEl.removeEventListener("touchmove", onViewportTouchmove);
				_touchEl.removeEventListener("touchend", onViewportTouchend);
			}
			el.addEventListener("touchstart", onViewportTouchstart, { passive: false });
			el.addEventListener("touchmove", onViewportTouchmove, { passive: false });
			el.addEventListener("touchend", onViewportTouchend);
			_touchEl = el;
		}
	});

	onUnmounted(() => {
		document.removeEventListener("mousemove", onDocMousemove);
		document.removeEventListener("mouseup", onDocMouseup);
		if (_touchEl) {
			_touchEl.removeEventListener("touchstart", onViewportTouchstart);
			_touchEl.removeEventListener("touchmove", onViewportTouchmove);
			_touchEl.removeEventListener("touchend", onViewportTouchend);
		}
	});

	defineExpose({ openPreview });
</script>

<template>
	<!-- ── 移动端：全屏覆盖层 / Mobile: full-screen overlay ── -->
	<Transition
		v-if="isMobile"
		enter-active-class="transition-opacity duration-200 ease-out"
		enter-from-class="opacity-0"
		enter-to-class="opacity-100"
		leave-active-class="transition-opacity duration-200 ease-in"
		leave-from-class="opacity-100"
		leave-to-class="opacity-0"
	>
		<div
			v-if="previewOpen"
			class="fixed inset-0 z-50 bg-black flex flex-col"
		>
			<!-- 顶部栏：返回按钮 + 标题 / Top bar: back button + title -->
			<div class="flex items-center gap-3 px-3 pt-4 pb-2 shrink-0">
				<button
					class="flex items-center justify-center w-9 h-9 rounded-full bg-white/10 text-white active:bg-white/20 transition-colors shrink-0"
					@click="previewOpen = false"
				>
					<ArrowLeft class="size-5" />
				</button>
				<span class="text-sm font-mono text-white/80 truncate">{{ previewTitle }}</span>
			</div>

			<!-- 图片视口 / Image viewport -->
			<div
				ref="previewViewportRef"
				class="relative flex-1 overflow-hidden flex items-center justify-center"
				:style="{
					cursor: isDragging ? 'grabbing' : previewScale > 1 ? 'grab' : 'default',
				}"
				@wheel.prevent="onPreviewWheel"
				@mousedown="onPreviewMousedown"
			>
				<!-- 加载进度条 / Loading bar -->
				<Transition name="fade">
					<div
						v-if="isLoadingPreview"
						class="absolute inset-x-0 top-0 h-0.5 overflow-hidden z-20"
					>
						<div class="h-full bg-white/60 animate-indeterminate-bar" />
					</div>
				</Transition>

				<img
					ref="previewImageRef"
					:src="previewUrl"
					:alt="previewTitle"
					class="select-none pointer-events-none transition-opacity duration-200"
					:class="isLoadingPreview ? 'opacity-0' : 'opacity-100'"
					@load="onPreviewImageLoad"
					:style="{
						maxWidth: '100%',
						maxHeight: '100%',
						objectFit: 'contain',
						transform: `translate(${previewTranslate.x}px, ${previewTranslate.y}px) scale(${previewScale})`,
						transformOrigin: 'center center',
						transition: isDragging ? 'opacity 0.2s' : 'opacity 0.2s, transform 0.1s',
					}"
				/>

				<!-- 重置缩放按钮 / Reset zoom button -->
				<Transition name="fade">
					<Button
						v-if="previewScale !== 1"
						variant="secondary"
						size="sm"
						class="absolute bottom-6 left-1/2 -translate-x-1/2 z-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xs px-3 py-1.5 backdrop-blur-sm"
						@click="resetPreviewTransform"
					>
						{{ t("recordings.resetZoom", { pct: Math.round(previewScale * 100) }) }}
					</Button>
				</Transition>
			</div>
		</div>
	</Transition>

	<!-- ── 桌面端：Dialog 弹窗 / Desktop: Dialog popup ── -->
	<Dialog v-else :open="previewOpen" @update:open="previewOpen = $event">
		<DialogContent
			class="p-0 overflow-hidden flex flex-col w-fit"
			style="max-width: 90vw; max-height: 90vh"
		>
			<DialogHeader class="px-4 pt-4 pb-2 shrink-0">
				<DialogTitle class="text-sm font-mono truncate">{{
					previewTitle
				}}</DialogTitle>
			</DialogHeader>
			<div
				ref="previewViewportRef"
				class="relative overflow-hidden flex items-center justify-center bg-black/5 px-4 pb-4"
				:style="{
					width: viewportSize.width,
					height: viewportSize.height,
					cursor: isDragging
						? 'grabbing'
						: previewScale > 1
							? 'grab'
							: 'default',
				}"
				@wheel.prevent="onPreviewWheel"
				@mousedown="onPreviewMousedown"
			>
				<!-- 加载进度条：fetch blob URL 期间显示 / Loading bar shown while fetching blob URL -->
				<Transition name="fade">
					<div
						v-if="isLoadingPreview"
						class="absolute inset-x-0 top-0 h-0.5 overflow-hidden z-20"
					>
						<div class="h-full bg-primary/60 animate-indeterminate-bar" />
					</div>
				</Transition>
				<img
					ref="previewImageRef"
					:src="previewUrl"
					:alt="previewTitle"
					class="rounded select-none pointer-events-none transition-opacity duration-200"
					:class="isLoadingPreview ? 'opacity-0' : 'opacity-100'"
					@load="onPreviewImageLoad"
					:style="{
						maxWidth: '100%',
						maxHeight: '100%',
						transform: `translate(${previewTranslate.x}px, ${previewTranslate.y}px) scale(${previewScale})`,
						transformOrigin: 'center center',
						transition: isDragging ? 'opacity 0.2s' : 'opacity 0.2s, transform 0.1s',
					}"
				/>
				<Transition name="fade">
					<Button
						v-if="previewScale !== 1"
						variant="secondary"
						size="sm"
						class="absolute bottom-5 left-1/2 -translate-x-1/2 z-10 rounded-full bg-black/60 hover:bg-black/80 text-white text-xs px-3 py-1.5 backdrop-blur-sm"
						@click="resetPreviewTransform"
					>
						{{
							t("recordings.resetZoom", {
								pct: Math.round(previewScale * 100),
							})
						}}
					</Button>
				</Transition>
			</div>
		</DialogContent>
	</Dialog>
</template>

<style scoped>
	.fade-enter-active,
	.fade-leave-active {
		transition: opacity 0.15s;
	}
	.fade-enter-from,
	.fade-leave-to {
		opacity: 0;
	}

	/* 不定长进度条动画 / Indeterminate progress bar animation */
	@keyframes indeterminate-bar {
		0% {
			transform: translateX(-100%);
			width: 40%;
		}
		50% {
			width: 60%;
		}
		100% {
			transform: translateX(260%);
			width: 40%;
		}
	}
	.animate-indeterminate-bar {
		animation: indeterminate-bar 1.4s ease-in-out infinite;
	}
</style>
