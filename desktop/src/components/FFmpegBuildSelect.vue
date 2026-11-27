<!--
	FFmpeg 下载构建选择器 / FFmpeg download build selector

	展示「构建轨（platform track）× 形态（variant）」两个下拉框，外加
	-legacy 轨的老硬件提示。用于 Setup 向导、FFmpeg 首选项对话框与
	设置页的 tools 下载确认对话框三处。

	Renders the "platform track × variant" dropdowns plus the old-hardware
	hint for -legacy tracks. Shared by the setup wizard, the FFmpeg
	preference dialog and the settings-page tools-download confirm dialog.

	Props:
	    downloadOptions - 后端 FFmpegLocations.download_options（platforms + variants）/
	                      backend FFmpegLocations.download_options
	Models:
	    platform - 选中的平台轨（默认取 options.platforms[0]，由父组件初始化）/
	               selected platform track
	    variant  - 选中的形态后缀（"" = 静态）/ selected variant suffix ("" = static)
-->
<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useMobileLayout } from "@/composables/useMobileLayout";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";

/** 可下载构建选项（对应后端 FFmpegLocations.download_options）/ Downloadable build options (mirrors backend FFmpegLocations.download_options) */
interface FFmpegDownloadOptions {
	/** 可选平台轨（首元素为宿主默认轨）/ Selectable platform tracks (first = host default) */
	platforms: string[];
	/** 可选形态后缀（"" = 静态）/ Selectable variant suffixes ("" = static) */
	variants: string[];
}

defineProps<{
	/** 可选构建矩阵 / Available build matrix */
	downloadOptions: FFmpegDownloadOptions;
}>();

/** 选中的平台轨 / Selected platform track */
const platform = defineModel<string>("platform", { required: true });
/** 选中的形态后缀（"" = 静态）/ Selected variant suffix ("" = static) */
const variant = defineModel<string>("variant", { required: true });

const { t } = useI18n();
const { isMobile } = useMobileLayout();

/** reka-ui 的 SelectItem value 不能是空串：静态变体（""）在 Select 内部用哨兵值表示，
 *  对外模型仍保持 ""（与后端约定一致）。
 *  reka-ui SelectItem values cannot be empty strings: the static variant ("")
 *  uses a sentinel inside the Select while the external model stays "" (matching
 *  the backend contract). */
const STATIC_SENTINEL = "__static__";
const variantSelect = computed<string>({
	get: () => (variant.value === "" ? STATIC_SENTINEL : variant.value),
	set: (v) => {
		variant.value = v === STATIC_SENTINEL ? "" : String(v);
	},
});

/**
 * 平台轨显示名 / Platform track label
 * @param p - 平台轨名 / platform track name
 */
function trackLabel(p: string): string {
	return p.endsWith("-legacy")
		? t("ffmpeg.preferenceWizard.trackLegacy", { platform: p })
		: t("ffmpeg.preferenceWizard.trackDefault", { platform: p });
}

/**
 * 形态显示名（"" → variant.static；"-shared-debug" → variant.shareddebug）/
 * Variant label ("" → variant.static; "-shared-debug" → variant.shareddebug)
 * @param v - 形态后缀 / variant suffix
 */
function variantLabel(v: string): string {
	const key = v === "" ? "static" : v.slice(1).replace("-", "");
	return t(`ffmpeg.preferenceWizard.variant.${key}`);
}
</script>

<template>
	<!-- 移动端纵向排列：标签在上、下拉全宽，避免两组控件挤在一行 /
		Stacked on mobile (label above, full-width selects) so the two groups don't squeeze into one line -->
	<div
		class="rounded-lg border border-dashed border-muted-foreground/30 px-3 py-2.5"
		:class="isMobile ? 'flex flex-col gap-2.5' : 'flex flex-wrap items-center gap-3'"
	>
		<span class="text-xs font-medium text-muted-foreground">{{
			t("ffmpeg.preferenceWizard.buildLabel")
		}}</span>
		<div
			:class="isMobile ? 'flex flex-col gap-1 min-w-0' : 'flex items-center gap-1.5 min-w-0'"
		>
			<span class="text-xs text-muted-foreground">{{
				t("ffmpeg.preferenceWizard.buildTrack")
			}}</span>
			<!-- min-w-0 + truncate：SelectTrigger 内长文案（如「默认轨（linux-x86_64-legacy，兼容老硬件）」）
				不可换行，button 固有 min-width:auto 会把 dialog grid 的 auto 列撑破——
				移动端下全部内容向右溢出、视觉不居中 /
				min-w-0 + truncate: long unbreakable trigger text makes the button's
				intrinsic min-width:auto blow up the dialog grid's auto column —
				on mobile all content overflows right and looks off-center -->
			<Select v-model="platform">
				<SelectTrigger
					:class="
						isMobile
							? 'h-8 w-full min-w-0 text-xs'
							: 'h-7 w-auto min-w-40 max-w-full text-xs'
					"
				>
					<SelectValue class="truncate" />
				</SelectTrigger>
				<SelectContent>
					<SelectItem v-for="p in downloadOptions.platforms" :key="p" :value="p">
						{{ trackLabel(p) }}
					</SelectItem>
				</SelectContent>
			</Select>
		</div>
		<div
			:class="isMobile ? 'flex flex-col gap-1 min-w-0' : 'flex items-center gap-1.5 min-w-0'"
		>
			<span class="text-xs text-muted-foreground">{{
				t("ffmpeg.preferenceWizard.buildVariant")
			}}</span>
			<Select v-model="variantSelect">
				<SelectTrigger
					:class="
						isMobile
							? 'h-8 w-full min-w-0 text-xs'
							: 'h-7 w-auto min-w-32 max-w-full text-xs'
					"
				>
					<SelectValue class="truncate" />
				</SelectTrigger>
				<SelectContent>
					<SelectItem
						v-for="v in downloadOptions.variants"
						:key="v || STATIC_SENTINEL"
						:value="v === '' ? STATIC_SENTINEL : v"
					>
						{{ variantLabel(v) }}
					</SelectItem>
				</SelectContent>
			</Select>
		</div>
		<span
			v-if="platform.endsWith('-legacy')"
			class="text-xs text-amber-600 dark:text-amber-500"
		>
			{{ t("ffmpeg.preferenceWizard.legacyHint") }}
		</span>
	</div>
</template>
