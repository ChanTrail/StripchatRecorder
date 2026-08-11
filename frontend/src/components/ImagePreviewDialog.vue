<!--
    图片预览对话框组件 / Image Preview Dialog Component

    带缩放（滚轮，以光标为锚点）和平移（拖拽）功能的图片预览弹窗。
    内部持有全部预览状态，通过 openPreview(url, title) 暴露给父组件调用。

    Image preview dialog with zoom (mouse wheel, cursor-anchored) and pan (drag) support.
    Holds all preview state internally; exposes openPreview(url, title) for the parent to call.

    Exposes:
        openPreview(url, title) - 打开预览并加载图片 / Open the preview and load an image
-->
<script setup lang="ts">
	import { onMounted, onUnmounted } from "vue";
	import { useImagePreview } from "@/composables/useImagePreview";
	import { Button } from "@/components/ui/button";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();

	const {
		previewOpen,
		previewUrl,
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
		openPreview,
	} = useImagePreview();

	onMounted(() => {
		document.addEventListener("mousemove", onDocMousemove);
		document.addEventListener("mouseup", onDocMouseup);
	});
	onUnmounted(() => {
		document.removeEventListener("mousemove", onDocMousemove);
		document.removeEventListener("mouseup", onDocMouseup);
	});

	defineExpose({ openPreview });
</script>

<template>
	<Dialog :open="previewOpen" @update:open="previewOpen = $event">
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
				<img
					ref="previewImageRef"
					:src="previewUrl"
					:alt="previewTitle"
					class="rounded select-none pointer-events-none"
					@load="onPreviewImageLoad"
					:style="{
						maxWidth: '100%',
						maxHeight: '100%',
						transform: `translate(${previewTranslate.x}px, ${previewTranslate.y}px) scale(${previewScale})`,
						transformOrigin: 'center center',
						transition: isDragging ? 'none' : 'transform 0.1s',
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
</style>
