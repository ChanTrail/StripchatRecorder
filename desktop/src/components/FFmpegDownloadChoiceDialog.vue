<!--
	FFmpeg 下载确认对话框 / FFmpeg download confirmation dialog

	设置页选择 tools 作为 FFmpeg 来源但 tools 中组件不齐时弹出：
	在「是否下载」确认的基础上提供构建选择（构建轨 × 形态），确认后由
	父组件发起带构建参数的下载；取消/关闭则回退到 PATH。

	Shown from the settings page when the user picks the tools location
	but its component set is incomplete: on top of the "download?" prompt
	it offers the build selection (platform track × variant); confirming
	lets the parent start a download carrying the build choice, while
	cancelling/closing falls back to PATH.

	Props:
	    open            - 对话框开关（v-model）/ dialog open state (v-model)
	    downloadOptions - 可选构建矩阵（platforms + variants）/ available build matrix

	Emits:
	    confirm - 携带 {platform, variant} 构建选择 / carries the {platform, variant} choice
	    cancel  - 取消按钮或任意方式关闭且未确认时触发 / fired on cancel button or any dismiss without confirm
-->
<script setup lang="ts">
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import FFmpegBuildSelect from "@/components/FFmpegBuildSelect.vue";

const props = defineProps<{
	/** 对话框开关 / Dialog open state */
	open: boolean;
	/** 可选构建矩阵（后端 download_options）/ Available build matrix (backend download_options) */
	downloadOptions: { platforms: string[]; variants: string[] } | null;
	/** 自定义标题（缺省用设置页 tools 缺失文案）/ Custom title (defaults to the settings tools-missing text) */
	title?: string;
	/** 自定义描述（缺省用设置页 tools 缺失文案——"取消回退 PATH"）/ Custom description (defaults to the settings tools-missing text — "cancel reverts to PATH") */
	desc?: string;
}>();

const emit = defineEmits<{
	"update:open": [v: boolean];
	confirm: [choice: { platform: string; variant: string }];
	cancel: [];
}>();

const { t } = useI18n();

/** 选中的平台轨 / Selected platform track */
const platform = ref("");
/** 选中的形态后缀（"" = 静态）/ Selected variant suffix ("" = static) */
const variant = ref("");
/** 本次打开是否已经 confirm（避免 confirm 后的关闭再触发 cancel）/ Whether this open has already been confirmed (prevents a follow-up cancel on close) */
let confirmed = false;

// 打开时按可选矩阵初始化默认选择：默认轨 = 首元素，形态 = 静态 /
// On open, initialize defaults from the matrix: default track = first entry, variant = static
watch(
	() => props.open,
	(v) => {
		if (v) {
			confirmed = false;
			platform.value = props.downloadOptions?.platforms[0] ?? "";
			variant.value = "";
		}
	},
);

/**
 * 关闭统一出口：未确认过的关闭视为取消（X、遮罩、Esc、取消按钮共用）/
 * Single close funnel: a close that was not confirmed counts as a cancel
 * (shared by the X button, overlay, Esc and the cancel button)
 * @param v - Dialog 发出的新开关状态 / new open state from the Dialog
 */
function onOpenChange(v: boolean) {
	emit("update:open", v);
	if (!v && !confirmed) emit("cancel");
}

/** 确认下载：标记已确认，携带构建选择发出，随后关闭 / Confirm download: mark confirmed, emit the build choice, then close */
function confirm() {
	confirmed = true;
	emit("confirm", { platform: platform.value, variant: variant.value });
	emit("update:open", false);
}
</script>

<template>
	<Dialog :open="open" @update:open="onOpenChange">
		<!-- grid-cols-[minmax(0,1fr)]：强制单列宽度等于内容区，
			防止子项固有 min-width（如 Select 触发器长文案）撑大 auto 列
			导致全部内容向右溢出 /
			grid-cols-[minmax(0,1fr)] pins the single column to the content
			box so a child's intrinsic min-width (e.g. long Select trigger
			text) can't inflate the auto column and push all content right -->
		<DialogContent class="sm:max-w-md grid-cols-[minmax(0,1fr)]">
			<DialogHeader>
				<DialogTitle>{{
					title ?? t("settings.ffmpegPreference.toolsMissingTitle")
				}}</DialogTitle>
				<DialogDescription>{{
					desc ?? t("settings.ffmpegPreference.toolsMissingDesc")
				}}</DialogDescription>
			</DialogHeader>

			<!-- 构建选择：仅在主机支持下载时出现；不支持的平台保持纯确认（auto 兜底）/
				Build selection: only when the host supports downloads; unsupported
				platforms degrade to a plain confirm (auto fallback) -->
			<FFmpegBuildSelect
				v-if="downloadOptions"
				v-model:platform="platform"
				v-model:variant="variant"
				:download-options="downloadOptions"
			/>

			<DialogFooter>
				<Button variant="outline" @click="onOpenChange(false)">{{
					t("common.cancel")
				}}</Button>
				<Button @click="confirm">{{ t("settings.ffmpegPreference.download") }}</Button>
			</DialogFooter>
		</DialogContent>
	</Dialog>
</template>
