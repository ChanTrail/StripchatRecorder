<!--
    虚拟录制输入节点组件 / Virtual Recording Input Node Component

    节点图编辑器中始终存在、不可删除的虚拟节点，代表录制产生的 TS 分片目录来源。
    只有一个输出端口（ts_session_dir）。
-->
<script setup lang="ts">
	import { PORT_TYPE_COLORS } from "@/stores/postprocess";
	import { INPUT_NODE_ID } from "@/composables/useNodeDragging";
	import { Badge } from "@/components/ui/badge";
	import { useI18n } from "vue-i18n";

	defineProps<{
		x: number;
		y: number;
		registerPortEl: (el: HTMLElement | null, nodeId: string, isOutput: boolean, portIndex: number) => void;
		/** 该输出端口当前是否已有连线 / Whether the output port currently has a wire */
		connected?: boolean;
	}>();

	defineEmits<{
		"header-mousedown": [e: MouseEvent];
		"port-mousedown": [e: MouseEvent];
		"port-mouseup": [e: MouseEvent];
	}>();

	const { t } = useI18n();
</script>

<template>
	<div
		class="pipeline-node absolute"
		:style="{ left: `${x}px`, top: `${y}px`, zIndex: 1 }"
		@mousedown.stop
	>
		<div class="rounded-xl border border-amber-500/40 bg-card shadow-xl min-w-44">
			<div class="px-3 py-2 cursor-move" @mousedown.stop="$emit('header-mousedown', $event)">
				<div class="flex items-center gap-1.5">
					<span class="text-xs font-semibold text-amber-400">{{ t("postprocess.input.label") }}</span>
					<Badge class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30">source</Badge>
				</div>
				<p class="text-[10px] text-muted-foreground mt-0.5">{{ t("postprocess.input.description") }}</p>
			</div>
			<div class="flex justify-end px-3 pb-2">
				<div class="flex items-center gap-1.5">
					<span class="text-[9px] text-muted-foreground">ts session dir</span>
					<div
						:ref="(el) => registerPortEl(el as HTMLElement | null, INPUT_NODE_ID, true, 0)"
						class="w-3 h-3 rounded-full border-2 cursor-crosshair -mr-4.5 shrink-0 transition-transform hover:scale-125"
						:style="{
							borderColor: PORT_TYPE_COLORS['ts_session_dir'],
							backgroundColor: connected ? PORT_TYPE_COLORS['ts_session_dir'] : PORT_TYPE_COLORS['ts_session_dir'] + '40',
						}"
						title="ts_session_dir"
						@mousedown.stop="$emit('port-mousedown', $event)"
						@mouseup.stop="$emit('port-mouseup', $event)"
					/>
				</div>
			</div>
		</div>
	</div>
</template>
