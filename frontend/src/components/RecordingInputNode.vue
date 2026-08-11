<!--
    虚拟录制输入节点组件 / Virtual Recording Input Node Component

    节点图编辑器中始终存在、不可删除的虚拟节点，代表录制产生的 TS 分片目录来源。
    只有一个输出端口（ts_session_dir）。该端口既可以作为拖拽连线的起点（按下鼠标），
    也可以作为其他节点从自己输入端口拖出连线时的落点（释放鼠标）。

    Always-present, non-removable virtual node in the node graph editor, representing
    the TS segment directory produced by recording. Has a single output port (ts_session_dir).
    This port can serve as either the drag origin (mouse down) or the drop target when another
    node's input port is dragged out to it (mouse up).

    Props:
        x, y           - 画布位置 / Canvas position
        registerPortEl - 端口 DOM 元素注册函数（来自 usePortWiring）/ Port DOM registration function (from usePortWiring)
        connected      - 该输出端口当前是否已有连线（决定端点是空心还是实心）/ Whether the output port currently has a wire (hollow vs filled)

    Emits:
        header-mousedown - 在头部按下鼠标（开始拖拽节点）/ Mouse down on header (start dragging the node)
        port-mousedown   - 在输出端口按下鼠标，开始拖拽连线 / Mouse down on the output port, starts a wire drag
        port-mouseup     - 在输出端口释放鼠标，可能完成一条连线 / Mouse up on the output port, may complete a wire
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
		/** 该输出端口当前是否已有连线（决定端点是空心还是实心）/ Whether the output port currently has a wire (hollow vs filled) */
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
