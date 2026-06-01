<!--
    流水线节点盒子组件 / Pipeline Node Box Component

    在后处理流水线编辑器中展示单个节点（模块实例）的标签和描述。
    固定节点（如输入/输出端点）使用不同的视觉样式加以区分。

    Displays a single node (module instance) label and description in the
    post-processing pipeline editor. Fixed nodes (e.g. input/output endpoints)
    use a distinct visual style.

    Props:
        label       - 节点显示名称 / Node display name
        description - 节点描述（可选）/ Node description (optional)
        fixed       - 是否为固定节点（不可移除）/ Whether this is a fixed node (non-removable)
-->
<script setup lang="ts">
	defineProps<{
		label: string;
		description?: string;
		fixed?: boolean;
	}>();
</script>

<template>
	<div
		class="border rounded-xl p-4 bg-card flex items-center gap-3"
		:class="fixed ? 'border-primary/40 bg-primary/5' : ''"
	>
		<div v-if="fixed" class="w-2 h-2 rounded-full bg-primary shrink-0" />
		<div>
			<div class="text-sm font-medium">{{ label }}</div>
			<div v-if="description" class="text-xs text-muted-foreground">
				{{ description }}
			</div>
		</div>
	</div>
</template>
