<!--
    录制文件表格行组件 / Recording File Table Row Component

    展示单个录制文件的完整表格行：勾选框、文件名、大小、时间、时长、分辨率、
    实时速度、分片统计、后处理进度、操作按钮。
    所有状态均通过 props 传入，操作通过 emits 交给父组件处理。

    Displays a single recording file's full table row: checkbox, filename, size,
    timestamps, duration, resolution, live speed, segment stats, post-processing
    progress, and action buttons.
    All state comes from props; actions are delegated to the parent via emits.

    Props:
        file          - 录制文件数据 / Recording file data
        checked       - 是否被选中 / Whether the row is selected
        elapsedSecs   - 已录制时长（秒，仅录制中时有意义）/ Elapsed recording duration (seconds, meaningful only while recording)
        speedBps      - 实时录制速度（字节/秒）/ Live recording speed (bytes/second)
        segmentStats  - 分片下载统计 / Segment download stats
        ppStatus      - 后处理任务状态 / Post-processing task status
        ppProgress    - 后处理进度数据 / Post-processing progress data
        hasContactSheet - 是否有 contact sheet 预览图可查看 / Whether a contact sheet preview is available
        hasPipelineNodes - 流水线是否有已连接的启用节点 / Whether the pipeline has any enabled connected node

    Emits:
        toggle-checked   - 切换勾选状态 / Toggle checked state
        open             - 打开文件 / Open the file
        open-contact-sheet - 打开 contact sheet 预览图 / Open contact sheet preview
        run-postprocess  - 触发后处理 / Trigger post-processing
        delete           - 删除文件 / Delete the file
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

	/**
	 * 分片统计的已下载/失败数：录制中优先用实时内存态（segmentStats prop，来自
	 * recording-file-update 事件，更新频率高于 meta 落盘），否则回退到 meta 的
	 * 持久化字段（file.segments_downloaded/segments_failed）——该字段在录制结束后
	 * 依然保留最终值，使分片统计列不再仅限于"录制中"才显示。
	 *
	 * Segment stats downloaded/failed counts: prefer the real-time in-memory value
	 * while recording (segmentStats prop, from recording-file-update events, updated
	 * more frequently than meta is persisted to disk), otherwise fall back to the
	 * meta-persisted fields (file.segments_downloaded/segments_failed) — which retain
	 * their final values after recording ends, so the segment stats column is no
	 * longer limited to "while recording" only.
	 */
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
				>{{ t("recordings.status.recording") }}</Badge
			>
		</TableCell>
		<TableCell class="tabular-nums">{{ formatSize(file.size_bytes) }}</TableCell>
		<TableCell class="tabular-nums text-muted-foreground">{{
			new Date(file.started_at).toLocaleString()
		}}</TableCell>
		<TableCell class="tabular-nums">
			<span v-if="file.is_recording" class="text-destructive">{{
				formatDuration(elapsedSecs)
			}}</span>
			<span v-else class="text-muted-foreground">—</span>
		</TableCell>
		<TableCell class="tabular-nums">
			<span v-if="file.video_duration_secs != null">{{
				formatDuration(file.video_duration_secs)
			}}</span>
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
					>{{ t("recordings.actions.play") }}</Button
				>
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
					>{{ t("recordings.actions.delete") }}</Button
				>
			</div>
		</TableCell>
	</TableRow>
</template>
