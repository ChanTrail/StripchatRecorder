<!--
    图片预览对话框组件 / Image Preview Dialog Component

    带缩放（滚轮/双指捏合）和平移（拖拽/单指）功能的图片预览弹窗。
    移动端全屏覆盖；桌面端居中 Dialog。
    通过 openPreview(url, title) 暴露给父组件调用。
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
			<div class="flex items-center gap-3 px-3 pt-4 pb-2 shrink-0">
				<button
					class="flex items-center justify-center w-9 h-9 rounded-full bg-white/10 text-white active:bg-white/20 transition-colors shrink-0"
					@click="previewOpen = false"
				>
					<ArrowLeft class="size-5" />
				</button>
				<span class="text-sm font-mono text-white/80 truncate">{{ previewTitle }}</span>
			</div>

			<div
				ref="previewViewportRef"
				class="relative flex-1 overflow-hidden flex items-center justify-center"
				:style="{ cursor: isDragging ? 'grabbing' : previewScale > 1 ? 'grab' : 'default' }"
				@wheel.prevent="onPreviewWheel"
				@mousedown="onPreviewMousedown"
			>
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
				<DialogTitle class="text-sm font-mono truncate">{{ previewTitle }}</DialogTitle>
			</DialogHeader>
			<div
				ref="previewViewportRef"
				class="relative overflow-hidden flex items-center justify-center bg-black/5 px-4 pb-4"
				:style="{
					width: viewportSize.width,
					height: viewportSize.height,
					cursor: isDragging ? 'grabbing' : previewScale > 1 ? 'grab' : 'default',
				}"
				@wheel.prevent="onPreviewWheel"
				@mousedown="onPreviewMousedown"
			>
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
						{{ t("recordings.resetZoom", { pct: Math.round(previewScale * 100) }) }}
					</Button>
				</Transition>
			</div>
		</DialogContent>
	</Dialog>
</template>

<style scoped>
	.fade-enter-active,
	.fade-leave-active { transition: opacity 0.15s; }
	.fade-enter-from,
	.fade-leave-to { opacity: 0; }

	@keyframes indeterminate-bar {
		0% { transform: translateX(-100%); width: 40%; }
		50% { width: 60%; }
		100% { transform: translateX(260%); width: 40%; }
	}
	.animate-indeterminate-bar {
		animation: indeterminate-bar 1.4s ease-in-out infinite;
	}
</style>
