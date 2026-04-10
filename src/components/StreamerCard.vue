<!--
    主播卡片组件 / Streamer Card Component

    展示单个主播的缩略图、在线状态、观看人数和录制控制按钮。
    通过 useFastThumbnail 对多个 CDN 域名进行竞速，加快缩略图加载速度。

    Displays a single streamer's thumbnail, online status, viewer count, and recording controls.
    Uses useFastThumbnail to race multiple CDN domains for faster thumbnail loading.

    Props:
        streamer - 主播数据对象 / Streamer data object

    Emits:
        remove       - 用户点击移除按钮 / User clicks remove button
        start        - 用户点击开始录制 / User clicks start recording
        stop         - 用户点击停止录制 / User clicks stop recording
        toggle-auto  - 用户切换自动录制开关 / User toggles auto-record switch
-->
<script setup lang="ts">
	import type { StreamerEntry } from "../stores/streamers";
	import { Card, CardContent } from "@/components/ui/card";
	import { Badge } from "@/components/ui/badge";
	import { Button } from "@/components/ui/button";
	import { Switch } from "@/components/ui/switch";
	import { Label } from "@/components/ui/label";
	import { ref, watch, computed } from "vue";
	import { useFastThumbnail } from "@/composables/useFastThumbnail";
	import { X, Circle, Eye } from "lucide-vue-next";
	import { useI18n } from "vue-i18n";

	const props = defineProps<{ streamer: StreamerEntry }>();
	void props;
	const emit = defineEmits<{
		remove: [];
		start: [];
		stop: [];
		"toggle-auto": [enabled: boolean];
	}>();
	const { t } = useI18n();

	// 本地维护 autoRecord 状态，避免直接修改 props
	// Maintain local autoRecord state to avoid directly mutating props
	const autoRecord = ref(props.streamer.auto_record);
	watch(
		() => props.streamer.auto_record,
		(val) => {
			autoRecord.value = val;
		},
	);

	// 将缩略图 URL 转为响应式引用，传入 useFastThumbnail 进行 CDN 竞速
	// Convert thumbnail URL to reactive ref for CDN racing via useFastThumbnail
	const thumbnailSrc = computed(() => props.streamer.thumbnail_url ?? null);
	const fastThumbnail = useFastThumbnail(thumbnailSrc);

	/**
	 * 处理自动录制开关变更，同步本地状态并向父组件发出事件。
	 * Handle auto-record toggle change, sync local state and emit to parent.
	 */
	function onAutoChange(val: boolean) {
		autoRecord.value = val;
		emit("toggle-auto", val);
	}

	/**
	 * 根据主播在线状态和直播间类型返回对应的 Badge 样式类。
	 * Return Badge style classes based on streamer online status and stream type.
	 *
	 * @param s - 主播数据 / Streamer data
	 */
	function statusClass(s: StreamerEntry): string {
		if (!s.is_online) return "bg-zinc-800 text-zinc-400 border-transparent";
		if (s.status === "公开秀")
			return "bg-green-900 text-green-300 border-transparent";
		return "bg-amber-900 text-amber-300 border-transparent";
	}
</script>

<template>
	<Card
		class="overflow-hidden transition-colors py-0"
		:class="{
			'border-green-900/50': streamer.is_online && !streamer.is_recording,
			'border-red-900/50': streamer.is_recording,
		}"
	>
		<div class="relative aspect-video bg-muted overflow-hidden">
			<img
				v-if="fastThumbnail"
				:src="fastThumbnail"
				loading="lazy"
				class="w-full h-full object-cover"
			/>
			<div
				v-else
				class="w-full h-full flex items-center justify-center text-4xl font-bold text-muted-foreground/20"
			>
				{{ streamer.username[0].toUpperCase() }}
			</div>
			<Circle
				v-if="streamer.is_recording"
				class="absolute top-1.5 right-2 size-2.5 fill-red-500 text-red-500 animate-pulse"
			/>
		</div>

		<CardContent class="p-3 flex flex-col gap-2">
			<div class="flex items-center justify-between">
				<span class="font-semibold text-sm truncate">{{
					streamer.username
				}}</span>
				<Button
					variant="ghost"
					size="icon"
					class="h-6 w-6 shrink-0 text-muted-foreground hover:text-destructive"
					:title="t('streamerCard.removeTitle')"
					@click="emit('remove')"
				>
					<X class="size-3.5" />
				</Button>
			</div>

			<div class="flex items-center gap-1.5 flex-wrap">
				<Badge :class="statusClass(streamer)">
					{{ streamer.is_online ? streamer.status : t("streamerCard.offline") }}
				</Badge>
				<Badge v-if="streamer.is_recording" variant="destructive">{{ t("streamerCard.recording") }}</Badge>
				<span
					v-if="streamer.is_online && streamer.viewers > 0"
					class="text-xs text-muted-foreground flex items-center gap-1"
				>
					<Eye class="size-3" /> {{ streamer.viewers.toLocaleString() }}
				</span>
			</div>

			<div class="flex items-center gap-2 mt-0.5">
				<Button
					v-if="!streamer.is_recording"
					size="sm"
					class="flex-1"
					:disabled="!streamer.is_recordable"
					:title="!streamer.is_recordable ? streamer.status : ''"
					@click="emit('start')"
				>
					{{ t("streamerCard.startRecording") }}
				</Button>
				<Button
					v-else
					size="sm"
					variant="destructive"
					class="flex-1"
					@click="emit('stop')"
				>
					{{ t("streamerCard.stopRecording") }}
				</Button>

				<div class="flex items-center gap-1.5 shrink-0" :title="t('streamerCard.autoRecordTitle')">
					<Switch
						:id="`auto-${streamer.username}`"
						:model-value="autoRecord"
						@update:model-value="onAutoChange"
					/>
					<Label
						:for="`auto-${streamer.username}`"
						class="text-xs text-muted-foreground select-none"
					>
						{{ t("streamerCard.autoRecord") }}
					</Label>
				</div>
			</div>
		</CardContent>
	</Card>
</template>

