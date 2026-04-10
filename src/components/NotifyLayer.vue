<!--
    全局通知层组件 / Global Notification Layer Component

    挂载在应用根部，提供两种全局通知机制：
    1. Sonner Toast 通知（右下角弹出，带颜色区分）
    2. 模态确认对话框（通过 useNotify 的共享状态驱动）

    Mounted at the application root, provides two global notification mechanisms:
    1. Sonner Toast notifications (bottom-right popup with color coding)
    2. Modal confirmation dialogs (driven by shared state from useNotify)
-->
<script setup lang="ts">
	import { useNotify } from "../composables/useNotify";
	import { Toaster } from "@/components/ui/sonner";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
		DialogDescription,
		DialogFooter,
	} from "@/components/ui/dialog";
	import { Button } from "@/components/ui/button";
	import { useI18n } from "vue-i18n";

	// 从 useNotify 获取对话框状态和解析函数
	// Get dialog state and resolve function from useNotify
	const { dialog, _resolveDialog } = useNotify();
	const { t } = useI18n();
</script>

<template>
	<Toaster position="bottom-right" rich-colors />

	<Dialog :open="!!dialog" @update:open="(v) => !v && _resolveDialog(false)">
		<DialogContent class="sm:max-w-95">
			<DialogHeader>
				<DialogTitle>{{ dialog?.title }}</DialogTitle>
				<DialogDescription class="whitespace-pre-line">
					{{ dialog?.message }}
				</DialogDescription>
			</DialogHeader>
			<DialogFooter>
				<Button
					v-if="!dialog?.hideCancelButton"
					variant="outline"
					@click="_resolveDialog(false)"
				>
					{{ dialog?.cancelText ?? t("common.cancel") }}
				</Button>
				<Button
					:variant="dialog?.danger ? 'destructive' : 'default'"
					@click="_resolveDialog(true)"
				>
					{{ dialog?.confirmText ?? t("common.confirm") }}
				</Button>
			</DialogFooter>
		</DialogContent>
	</Dialog>
</template>
