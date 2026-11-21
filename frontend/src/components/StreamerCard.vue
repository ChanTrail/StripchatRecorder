<!--
    主播卡片组件 / Streamer Card Component

    展示单个主播的缩略图、在线状态、观看人数和录制控制按钮。
    通过 useFastThumbnail 对多个 CDN 域名进行竞速，加快缩略图加载速度。
    缩略图左上角常驻 checkbox 覆盖层：未选中时悬停可见，选中后常驻高亮。
    有任意主播被选中时（selectMode），卡片点击区域变为切换选中状态，操作按钮被隐藏。

    Displays a single streamer's thumbnail, online status, viewer count, and recording controls.
    Uses useFastThumbnail to race multiple CDN domains for faster thumbnail loading.
    A checkbox overlay on the thumbnail is always present: visible on hover when unselected,
    always visible when selected. When any streamer is selected (selectMode), clicking the card
    toggles selection and normal action buttons are hidden.

    Props:
        streamer    - 主播数据对象 / Streamer data object
        selectMode  - 是否处于批量选择模式（外部有任意主播被选中）/ Whether batch selection mode is active
        selected    - 是否被选中 / Whether this card is selected

    Emits:
        remove        - 用户点击移除按钮 / User clicks remove button
        start         - 用户点击开始录制 / User clicks start recording
        stop          - 用户点击停止录制 / User clicks stop recording
        toggle-auto   - 用户切换自动录制开关 / User toggles auto-record switch
        toggle-select - 用户切换选中状态 / User toggles selection state
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
import { copyToClipboard } from "@/lib/utils";
import { X, Circle, Check, Copy, Eye, EyeOff } from "@lucide/vue";
import { useI18n } from "vue-i18n";

const props = defineProps<{
	streamer: StreamerEntry;
	selectMode?: boolean;
	selected?: boolean;
	/**
	 * 隐私模式：off 不模糊；live 仅模糊在线（直播）预览图；all 在线与离线图都模糊。
	 * Privacy mode: off = no blur; live = blur online (live) previews only;
	 * all = blur both online previews and offline images.
	 */
	privacyMode?: "off" | "live" | "all";
}>();
const emit = defineEmits<{
	remove: [];
	start: [];
	stop: [];
	"toggle-auto": [enabled: boolean];
	"toggle-select": [];
}>();
const { t } = useI18n();

const autoRecord = ref(props.streamer.auto_record);
watch(
	() => props.streamer.auto_record,
	(val) => {
		autoRecord.value = val;
	},
);

const thumbnailSrc = computed(() => props.streamer.thumbnail_url ?? null);
const fastThumbnail = useFastThumbnail(thumbnailSrc);

/**
 * 本张卡片被用户手动解除模糊的会话级标记（不持久化，组件销毁即复位）。
 * Per-card session flag set when the user manually unblurs this thumbnail
 * (not persisted; resets when the component is destroyed).
 */
const unblurred = ref(false);

/**
 * 隐私模式是否作用于本卡片：all 对所有卡片生效；live 仅作用于在线卡片。
 * Whether the privacy mode applies to this card: `all` affects every card;
 * `live` only affects online (live-preview) cards.
 */
const privacyApplies = computed(
	() => props.privacyMode === "all" || (props.privacyMode === "live" && props.streamer.is_online),
);

/** 当前是否处于模糊状态（模式适用且未被手动解除）/ Whether the thumbnail is currently blurred (mode applies and not manually unblurred) */
const shouldBlur = computed(() => privacyApplies.value && !unblurred.value);

const copied = ref(false);

// 转发流地址 / Stream relay URL
const streamUrl = computed(() => `${window.location.origin}/stream/${props.streamer.username}`);

async function copyStreamUrl() {
	try {
		await copyToClipboard(streamUrl.value);
		copied.value = true;
		setTimeout(() => {
			copied.value = false;
		}, 2000);
	} catch {}
}

function onAutoChange(val: boolean) {
	autoRecord.value = val;
	emit("toggle-auto", val);
}

// 将后端返回的中文 status 值映射为 i18n key（home.filter.*）
// Map backend Chinese status value to i18n key (home.filter.*)
const STATUS_KEY_MAP: Record<string, string> = {
	公开秀: "home.filter.public",
	私密秀: "home.filter.private",
	群组秀: "home.filter.group",
	票务秀: "home.filter.ticket",
	计时秀: "home.filter.perMinute",
	P2P: "home.filter.p2p",
	虚拟私密: "home.filter.virtualPrivate",
	等待: "home.filter.waiting",
};

function statusLabel(s: StreamerEntry): string {
	if (!s.is_online) return t("streamerCard.offline");
	return t(STATUS_KEY_MAP[s.status] ?? "home.filter.online");
}

function statusClass(s: StreamerEntry): string {
	if (!s.is_online) return "bg-zinc-800 text-zinc-400 border-transparent";
	if (STATUS_KEY_MAP[s.status] === "home.filter.public")
		return "bg-green-900 text-green-300 border-transparent";
	return "bg-amber-900 text-amber-300 border-transparent";
}

function onCardClick() {
	if (props.selectMode) emit("toggle-select");
}
</script>

<template>
	<Card
		class="overflow-hidden transition-colors py-0 group"
		:class="{
			'border-green-900/50': streamer.is_online && !streamer.is_recording && !selected,
			'border-red-900/50': streamer.is_recording && !selected,
			'border-primary ring-2 ring-primary/40': selected,
			'cursor-pointer': selectMode,
		}"
		@click="onCardClick"
	>
		<!-- 缩略图区：contextmenu 仅在模糊时拦截（防止保存被模糊的原图），
			未模糊时保留正常的右键/长按菜单 / Thumbnail area: context menu is
			suppressed only while blurred (keeps the original from being saved);
			the normal right-click/long-press menu still works when visible -->
		<div
			class="relative aspect-video bg-muted overflow-hidden select-none"
			@contextmenu="shouldBlur && $event.preventDefault()"
		>
			<!-- 模糊容器：blur-2xl + scale-110 掩盖羽化边缘；transition 平滑切换 / Blur container: blur-2xl + scale-110 hides feathered edges; transition smooths toggling -->
			<div
				class="absolute inset-0 transition-[filter,transform] duration-300"
				:class="shouldBlur ? 'blur-2xl scale-110' : ''"
			>
				<img
					v-if="fastThumbnail"
					:src="fastThumbnail"
					loading="lazy"
					class="w-full h-full object-cover [-webkit-touch-callout:none]"
				/>
				<div
					v-else
					class="w-full h-full flex items-center justify-center text-4xl font-bold text-muted-foreground/20"
				>
					{{ streamer.username[0].toUpperCase() }}
				</div>
			</div>

			<!-- 隐私模式眼睛按钮：模糊适用时常驻于缩略图右下角，点击在
				模糊/清晰之间切换（z-10 盖过全图 checkbox 覆盖层）/
				Privacy eye button: pinned to the thumbnail's bottom-right while
				privacy mode applies; click toggles blur on/off (z-10 keeps it
				above the full-image checkbox overlay) -->
			<button
				v-if="privacyApplies"
				type="button"
				class="absolute bottom-2 right-2 z-10 size-7 rounded-md bg-black/55 text-white/90 flex items-center justify-center opacity-100 transition-opacity hover:bg-black/75"
				:title="shouldBlur ? t('streamerCard.revealImage') : t('streamerCard.blurImage')"
				@click.stop.prevent="unblurred = !unblurred"
			>
				<Eye v-if="shouldBlur" class="size-4" />
				<EyeOff v-else class="size-4" />
			</button>

			<!-- 录制状态指示器（非批量模式显示）/ Recording indicator (shown outside batch mode) -->
			<Circle
				v-if="streamer.is_recording && !selectMode"
				class="absolute top-1.5 right-2 size-2.5 fill-red-500 text-red-500 animate-pulse"
			/>

			<!-- checkbox 覆盖层：选中时常驻，未选中时悬停显示 / Checkbox overlay: always visible when selected, shown on hover when not -->
			<div
				class="absolute inset-0 flex items-start justify-start p-2 transition-opacity cursor-pointer"
				:class="
					selected
						? 'opacity-100 bg-primary/15'
						: 'opacity-0 group-hover:opacity-100 group-hover:bg-black/25'
				"
				@click.stop="emit('toggle-select')"
			>
				<!-- 自绘受控 checkbox，避免 reka-ui CheckboxRoot 内部状态与父组件状态冲突 / -->
				<!-- Custom controlled checkbox to avoid reka-ui CheckboxRoot internal state conflicts -->
				<div
					class="size-5 rounded border-2 shadow-md bg-background/80 flex items-center justify-center transition-colors"
					:class="selected ? 'bg-primary border-primary' : 'border-input'"
					@click.stop="emit('toggle-select')"
				>
					<Check v-if="selected" class="size-3.5 text-primary-foreground" />
				</div>
			</div>
		</div>

		<CardContent class="p-3 flex flex-col gap-2">
			<div class="flex items-center justify-between">
				<span class="font-semibold text-sm truncate">{{ streamer.username }}</span>
				<Button
					variant="ghost"
					size="icon"
					class="h-6 w-6 shrink-0 text-muted-foreground hover:text-destructive"
					:title="t('streamerCard.removeTitle')"
					@click.stop="emit('remove')"
				>
					<X class="size-3.5" />
				</Button>
			</div>

			<div class="flex items-center gap-1.5 flex-wrap">
				<Badge :class="statusClass(streamer)">
					{{ statusLabel(streamer) }}
				</Badge>
				<Badge v-if="streamer.is_recording" variant="destructive">{{
					t("streamerCard.recording")
				}}</Badge>
			</div>

			<div class="flex items-center gap-2 mt-0.5">
				<Button
					v-if="!streamer.is_recording"
					size="sm"
					class="flex-1"
					:disabled="!streamer.is_recordable"
					:title="!streamer.is_recordable ? streamer.status : ''"
					@click.stop="emit('start')"
				>
					{{ t("streamerCard.startRecording") }}
				</Button>
				<Button
					v-else
					size="sm"
					variant="destructive"
					class="flex-1"
					@click.stop="emit('stop')"
				>
					{{ t("streamerCard.stopRecording") }}
				</Button>

				<div
					class="flex items-center gap-1.5 shrink-0"
					:title="t('streamerCard.autoRecordTitle')"
				>
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

			<!-- 转发流地址（始终显示）/ Stream URL (always shown) -->
			<div class="flex items-center gap-2">
				<div
					class="flex-1 text-xs font-mono text-blue-400/60 bg-blue-950/10 rounded px-2 py-1 truncate select-all"
					:title="t('streamerCard.streamUrlHint')"
				>
					{{ streamUrl }}
				</div>
				<Button
					size="sm"
					variant="ghost"
					class="shrink-0 px-2 h-6 text-muted-foreground hover:text-blue-300"
					:title="t('streamerCard.copyStreamUrl')"
					@click.stop="copyStreamUrl"
				>
					<Check v-if="copied" class="size-3 text-green-400" />
					<Copy v-else class="size-3" />
				</Button>
			</div>
		</CardContent>
	</Card>
</template>
