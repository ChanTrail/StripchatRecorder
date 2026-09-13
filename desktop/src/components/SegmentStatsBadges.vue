<!--
    分片下载统计徽章组件 / Segment Download Stats Badges Component

    展示录制中文件的分片下载/失败计数与成功率百分比。纯展示组件。
-->
<script setup lang="ts">
	import { computed } from "vue";
	import { Badge } from "@/components/ui/badge";

	const props = defineProps<{
		downloaded: number;
		failed: number;
	}>();

	const successPct = computed(() => {
		const total = props.downloaded + props.failed;
		return total > 0 ? Math.round((props.downloaded / total) * 100) : 100;
	});
</script>

<template>
	<div class="flex items-center gap-1 flex-wrap">
		<Badge variant="secondary" class="tabular-nums text-[11px] px-1.5 py-0">
			{{ downloaded }}
		</Badge>
		<Badge
			v-if="failed > 0"
			variant="secondary"
			class="tabular-nums text-[11px] px-1.5 py-0 bg-destructive/15 text-destructive border-0"
		>
			{{ failed }}
		</Badge>
		<Badge
			variant="outline"
			class="tabular-nums text-[11px] px-1.5 py-0"
			:class="failed === 0 ? 'border-green-500 text-green-500' : 'border-destructive text-destructive'"
		>
			{{ successPct }}%
		</Badge>
	</div>
</template>
