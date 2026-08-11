<!--
    模块选择弹出菜单组件 / Module Picker Popup Menu Component

    通用的模块选择菜单，在节点图编辑器中被两处复用：
    - 右键画布弹出的"添加节点"菜单
    - 拖拽连线释放到空白画布时弹出的"连接到…"菜单
    菜单标题区域通过默认 slot 自定义（两处的标题内容不同）。

    Generic module-selection popup menu, reused in two places in the node graph editor:
    - The "add node" menu shown on right-click
    - The "connect to…" menu shown when a wire is dropped on empty canvas
    The header area is customized via the default slot (title content differs between the two).

    Props:
        visible      - 是否显示 / Whether the menu is visible
        x, y         - 菜单左上角位置（像素）/ Menu top-left position (pixels)
        modules      - 可选模块列表 / List of selectable modules
        emptyMessage - 无可选模块时显示的文字 / Text shown when no modules are selectable

    Emits:
        select - 用户选择了一个模块 / User selected a module
        close  - 用户点击取消或关闭菜单 / User clicked cancel or closed the menu
-->
<script setup lang="ts">
	import { Badge } from "@/components/ui/badge";
	import type { ModuleInfo } from "@/stores/postprocess";
	import { useI18n } from "vue-i18n";

	defineProps<{
		visible: boolean;
		x: number;
		y: number;
		modules: ModuleInfo[];
		emptyMessage: string;
	}>();

	const emit = defineEmits<{
		select: [moduleId: string];
		close: [];
	}>();

	const { t } = useI18n();
</script>

<template>
	<Transition name="fade">
		<div
			v-if="visible"
			class="absolute z-50 min-w-44 rounded-lg border bg-popover shadow-xl py-1 text-sm"
			:style="{ left: `${x}px`, top: `${y}px` }"
			@mousedown.stop
			@click.stop
		>
			<div class="px-3 py-1 text-xs text-muted-foreground font-medium flex items-center gap-1.5">
				<slot />
			</div>
			<div v-if="modules.length === 0" class="px-3 py-2 text-xs text-muted-foreground">
				{{ emptyMessage }}
			</div>
			<button
				v-for="mod in modules"
				:key="mod.id"
				class="w-full flex items-start gap-2 px-3 py-1.5 hover:bg-accent transition-colors text-left"
				@click="emit('select', mod.id)"
			>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-1.5">
						<span class="text-sm font-medium truncate">{{ mod.name }}</span>
						<span
							v-if="mod.version"
							class="text-[10px] text-muted-foreground font-mono shrink-0"
						>v{{ mod.version }}</span>
						<Badge
							v-if="mod.official"
							class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30 shrink-0"
						>official</Badge>
					</div>
					<p class="text-xs text-muted-foreground truncate">{{ mod.description }}</p>
				</div>
			</button>
			<div class="border-t my-1" />
			<button
				class="w-full px-3 py-1.5 text-left text-xs text-muted-foreground hover:bg-accent transition-colors"
				@click="emit('close')"
			>{{ t("common.cancel") }}</button>
		</div>
	</Transition>
</template>

<style scoped>
	.fade-enter-active,
	.fade-leave-active {
		transition: opacity 0.1s, transform 0.1s;
	}
	.fade-enter-from,
	.fade-leave-to {
		opacity: 0;
		transform: scale(0.97);
	}
</style>
