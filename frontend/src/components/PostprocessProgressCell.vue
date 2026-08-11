<!--
    后处理进度单元格组件 / Post-processing Progress Cell Component

    展示单个录制文件在后处理表格列中的状态：等待中/运行中进度条/完成后各模块结果列表。
    纯展示组件，不持有状态，所有数据通过 props 传入。

    Displays a recording file's status in the post-processing table column:
    waiting indicator / running progress bars / completed per-module result list.
    Pure presentational component; holds no state, all data comes from props.

    Props:
        status   - 后处理任务状态 / Post-processing task status
        progress - 后处理进度数据（运行中或已完成时提供）/ Post-processing progress data (present when running or done)
-->
<script setup lang="ts">
	import { Loader2 } from "@lucide/vue";
	import { Progress } from "@/components/ui/progress";
	import type { PpProgress, PpStatus } from "@/composables/usePostprocess";
	import { useI18n } from "vue-i18n";

	defineProps<{
		status?: PpStatus;
		progress?: PpProgress;
	}>();

	const { t } = useI18n();
</script>

<template>
	<div
		v-if="status === 'running' && progress"
		class="flex flex-col gap-1.5"
	>
		<div class="flex items-center justify-between text-xs text-muted-foreground">
			<span>{{
				progress.moduleExecLabel
					? t("recordings.status.overallProgressWithLabel", { label: progress.moduleExecLabel })
					: t("recordings.status.overallProgress")
			}}</span>
			<span class="tabular-nums shrink-0">{{ progress.overallLabel }}</span>
		</div>
		<Progress :model-value="progress.overallPct" :animated="false" class="h-1.5" />
		<div class="flex items-center justify-between text-xs text-muted-foreground">
			<span class="truncate max-w-50">{{
				progress.moduleName === "processing" ? t("usePostprocess.processing") : progress.moduleName
			}}</span>
			<span class="tabular-nums shrink-0">{{
				progress.moduleLabel === "waiting" ? t("usePostprocess.waitingProgress") : progress.moduleLabel
			}}</span>
		</div>
		<Progress :model-value="progress.modulePct" :animated="false" class="h-1.5" />
	</div>
	<div
		v-else-if="status === 'waiting'"
		class="flex items-center gap-1.5 text-xs text-muted-foreground"
	>
		<Loader2 class="size-3 animate-spin shrink-0" />
		<span>{{ t("recordings.status.waiting") }}</span>
	</div>
	<div
		v-else-if="status === 'done' || status === 'error'"
		class="flex flex-col gap-0.5"
	>
		<template v-if="progress?.moduleResults?.length">
			<div
				v-for="r in progress.moduleResults"
				:key="r.moduleId"
				class="flex items-center gap-1.5 text-xs"
				:class="r.success ? 'text-green-500' : 'text-destructive'"
				:title="r.success ? r.moduleId : `${r.moduleId}: ${r.message}`"
			>
				<span class="shrink-0">{{ r.success ? "✓" : "✗" }}</span>
				<span class="truncate max-w-40">{{ r.moduleId }}</span>
			</div>
		</template>
	</div>
	<span v-else class="text-xs text-muted-foreground">—</span>
</template>
