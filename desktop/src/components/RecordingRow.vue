<!--
    录制文件表格行组件 / Recording File Table Row Component

    展示单个录制文件的完整表格行。
    所有状态通过 props 传入，操作通过 emits 交给父组件处理。

    Desktop 差异：open-contact-sheet 通过 Tauri read_output_file 命令读取图片。
-->
<script setup lang="ts">
	import { computed } from "vue";
	import { Button } from "@/components/ui/button";
	import { Badge } from "@/components/ui/badge";
	import { Checkbox } from "@/components/ui/checkbox";
	import { Tooltip } from "@/components/ui/tooltip";
	import { TableRow, TableCell } from "@/components/ui/table";
	import { Loader2, Image } from "@lucide/vue";
	import SegmentStatsBadges from "@/components/SegmentStatsBadges.vue";
	import PostprocessProgressCell from "@/components/PostprocessProgressCell.vue";
	import type { RecordingFile } from "@/types/recordings";
	import type { PpProgress, PpStatus } from "@/composables/usePostprocess";
	import { formatSize, formatDuration } from "@/utils/format";
	import { useI18n } from "vue-i18n";

	const props = defineProps<{
		file: RecordingFile;
		checked: boolean;
		elapsedSecs: number;
		speedBps: number | null;
		segmentStats: { downloaded: number; failed: number } | null;
		ppStatus?: PpStatus;
		ppProgress?: PpProgress;
		hasContactSheet: boolean;
		hasPipelineNodes: boolean;
	}>();

	defineEmits<{
		"toggle-checked": [];
		open: [];
		"open-contact-sheet": [];
		"run-postprocess": [];
		delete: [];
	}>();

	const { t } = useI18n();

	const segmentDownloaded = computed(
		() => props.segmentStats?.downloaded ?? props.file.segments_downloaded ?? null,
	);
	const segmentFailed = computed(
		() => props.segmentStats?.failed ?? props.file.segments_failed ?? 0,
	);
</script>

<template>
	<TableRow class="relative">
		<TableCell class="w-8">
			<Checkbox
				:model-value="checked"
				:disabled="file.is_recording"
				@update:model-value="$emit('toggle-checked')"
			/>
		</TableCell>
		<TableCell class="font-medium w-px whitespace-nowrap pl-7">
			{{ file.name }}
			<Badge
				v-if="file.is_recording"
				variant="destructive"
				class="ml-1.5 text-[10px]"
			>{{ t("recordings.status.recording") }}</Badge>
		</TableCell>
		<TableCell class="tabular-nums">{{ formatSize(file.size_bytes) }}</TableCell>
		<TableCell class="tabular-nums text-muted-foreground">{{
			new Date(file.started_at).toLocaleString()
		}}</TableCell>
		<TableCell class="tabular-nums">
			<span v-if="file.is_recording" class="text-destructive">{{ formatDuration(elapsedSecs) }}</span>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="tabular-nums">
			<span v-if="file.video_duration_secs != null">{{ formatDuration(file.video_duration_secs) }}</span>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="tabular-nums font-mono text-xs">
			<span v-if="file.video_resolution">{{ file.video_resolution }}</span>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="tabular-nums">
			<span v-if="file.is_recording && speedBps != null" class="text-xs">
				{{ formatSize(speedBps) }}/s
			</span>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="min-w-36">
			<SegmentStatsBadges
				v-if="segmentDownloaded != null"
				:downloaded="segmentDownloaded"
				:failed="segmentFailed"
			/>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="min-w-45">
			<div v-if="!file.is_recording">
				<PostprocessProgressCell :status="ppStatus" :progress="ppProgress" />
			</div>
			<span v-else class="text-xs text-muted-foreground">—</span>
		</TableCell>
		<TableCell>
			<div class="flex gap-1.5">
				<Button
					size="sm"
					variant="outline"
					:disabled="file.is_recording"
					:title="file.is_recording ? t('recordings.actions.playDisabled') : ''"
					@click="$emit('open')"
				>{{ t("recordings.actions.play") }}</Button>
				<Button
					v-if="hasContactSheet"
					size="sm"
					variant="outline"
					title="查看 Contact Sheet 预览图"
					@click="$emit('open-contact-sheet')"
				>
					<Image class="size-3.5" />
				</Button>
				<Tooltip
					:content="
						file.is_recording
							? t('recordings.status.recording')
							: !hasPipelineNodes
								? t('recordings.postprocessEmptyPipeline')
								: undefined
					"
				>
					<Button
						size="sm"
						variant="outline"
						:disabled="
							file.is_recording ||
							ppStatus === 'running' ||
							ppStatus === 'waiting' ||
							!hasPipelineNodes
						"
						@click="$emit('run-postprocess')"
					>
						<Loader2 v-if="ppStatus === 'running'" class="size-3.5 animate-spin" />
						<span v-else>{{ t("recordings.actions.postprocess") }}</span>
					</Button>
				</Tooltip>
				<Button
					size="sm"
					variant="destructive"
					:disabled="file.is_recording"
					:title="file.is_recording ? t('recordings.actions.deleteDisabled') : ''"
					@click="$emit('delete')"
				>{{ t("recordings.actions.delete") }}</Button>
			</div>
		</TableCell>
	</TableRow>
</template>
