<!--
    添加主播对话框组件 / Add Streamer Dialog Component

    提供一个模态对话框，允许用户通过输入 Stripchat 用户名或链接来添加主播到追踪列表。
    支持批量添加：文本区域中每行为一个用户名或链接，提交时一次性发给后端，
    后端并发验证后逐个添加，通过 streamer-batch-progress 事件实时更新前端进度。

    Provides a modal dialog for users to add streamers to the tracking list
    by entering Stripchat usernames or URLs. Supports batch add: all entries are
    sent to the backend in one request, verified concurrently, and added one by one.
    Real-time progress is shown via streamer-batch-progress events.

    Emits:
        close - 对话框关闭时触发 / Emitted when dialog is closed
        added - 至少一个主播成功添加后触发 / Emitted after at least one streamer is successfully added
-->
<script setup lang="ts">
import { ref, computed } from "vue";
import { useStreamersStore } from "../stores/streamers";
import { useNotify } from "../composables/useNotify";
import { useScrollbar } from "@/composables/useScrollbar";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
	DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { useI18n } from "vue-i18n";

const emit = defineEmits<{ close: []; added: [] }>();
const store = useStreamersStore();
const { toast } = useNotify();
const { t } = useI18n();
const raw = ref("");
const loading = ref(false);
const textareaEl = ref<HTMLTextAreaElement | null>(null);
useScrollbar(textareaEl);

/** 批量进度状态（本地计数驱动，不依赖 SSE 事件）/ Batch progress state (locally driven, no SSE dependency) */
const progress = ref<{ done: number; total: number; current: string } | null>(null);

/** 进度百分比（0-100）/ Progress percentage (0-100) */
const progressPct = computed(() =>
	progress.value ? Math.round((progress.value.done / progress.value.total) * 100) : 0,
);

/**
 * 从单行输入中提取用户名：支持直接输入用户名、stripchat.com 链接或 mirror 链接。
 * Extract username from a single line: supports plain username, stripchat.com URL, or mirror URL.
 */
function extractUsername(input: string): string {
	const trimmed = input.trim();
	try {
		const url = new URL(trimmed.startsWith("http") ? trimmed : `https://${trimmed}`);
		// 匹配 stripchat.com 或任意 mirror 域名下的 /<username> 路径
		// Match /<username> path under stripchat.com or any mirror domain
		const parts = url.pathname.split("/").filter(Boolean);
		if (parts.length > 0) return parts[0];
	} catch {
		// 不是 URL，直接当用户名 / Not a URL, treat as plain username
	}
	return trimmed;
}

/**
 * 将文本区域内容解析为去重后的用户名列表（跳过空行）。
 * Parse textarea content into a deduplicated list of usernames, skipping blank lines.
 */
const parsedUsernames = computed<string[]>(() => {
	const seen = new Set<string>();
	const result: string[] = [];
	for (const line of raw.value.split("\n")) {
		const name = extractUsername(line).toLowerCase();
		if (name && !seen.has(name)) {
			seen.add(name);
			result.push(name);
		}
	}
	return result;
});

/**
 * 提交表单：将所有用户名一次性发给后端，通过进度回调实时更新 UI，汇总结果后反馈。
 * 单个成功时使用 done 提示；批量时按成功/失败数量使用不同提示。
 *
 * Submit the form: send all usernames to the backend in one request,
 * update UI via progress callback, then summarize results.
 * Uses single-item done message for one entry; batch messages for multiple.
 */
async function submit() {
	const targets = parsedUsernames.value;
	if (!targets.length) return;
	loading.value = true;
	// 多条时立即初始化进度，让进度条立刻出现 / Init progress immediately for multi-entry so bar appears right away
	progress.value =
		targets.length > 1 ? { done: 0, total: targets.length, current: targets[0] } : null;
	try {
		const { total, success, skipped, failed } = await store.addStreamers(
			targets,
			targets.length > 1
				? (done, current) => {
						progress.value = { done, total: targets.length, current };
					}
				: undefined,
		);
		if (success > 0 || skipped > 0) {
			if (total === 1 && success === 1) {
				toast(t("addStreamer.done", { name: targets[0] }), "success");
			} else if (failed === 0 && skipped === 0) {
				toast(t("addStreamer.batchDone", { count: success }), "success");
			} else if (failed === 0 && skipped > 0) {
				toast(t("addStreamer.batchSkipped", { success, skipped }), "success");
			} else {
				toast(t("addStreamer.batchPartialFailed", { success, failed }), "error");
			}
			emit("added");
		} else {
			// 全部失败时不触发 added，让用户留在对话框里修正
			// All failed: stay in dialog so the user can correct the input
			toast(t("addStreamer.batchPartialFailed", { success: 0, failed }), "error");
		}
	} catch (e) {
		toast(String(e), "error");
	} finally {
		loading.value = false;
		progress.value = null;
	}
}
</script>

<template>
	<Dialog :open="true" @update:open="(v) => !v && emit('close')">
		<DialogContent class="sm:max-w-lg">
			<DialogHeader>
				<DialogTitle>{{ t("addStreamer.title") }}</DialogTitle>
				<DialogDescription class="sr-only">{{
					t("addStreamer.description")
				}}</DialogDescription>
			</DialogHeader>

			<form @submit.prevent="submit" class="flex flex-col gap-4 py-2">
				<div class="flex flex-col gap-2">
					<Label for="usernames">{{ t("addStreamer.label") }}</Label>
					<textarea
						id="usernames"
						ref="textareaEl"
						v-model="raw"
						:placeholder="t('addStreamer.placeholder')"
						:disabled="loading"
						autofocus
						rows="6"
						class="flex w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50 resize-none scrollbar-overlay"
					/>
					<p
						v-if="!loading && parsedUsernames.length > 0"
						class="text-xs text-muted-foreground"
					>
						{{ t("addStreamer.inputCount", { count: parsedUsernames.length }) }}
					</p>
				</div>

				<!-- 进度显示区（提交后出现）/ Progress display (appears after submit) -->
				<div v-if="loading && progress" class="flex flex-col gap-1.5">
					<!-- 进度条 / Progress bar -->
					<div class="w-full h-1.5 bg-muted rounded-full overflow-hidden">
						<div
							class="h-full bg-primary rounded-full transition-all duration-300"
							:style="{ width: `${progressPct}%` }"
						/>
					</div>
					<div class="flex items-center justify-between text-xs text-muted-foreground">
						<!-- 当前处理的用户名 / Currently processing username -->
						<span class="truncate max-w-[70%]">{{ progress.current }}</span>
						<!-- 计数 / Count -->
						<span class="shrink-0">{{ progress.done }} / {{ progress.total }}</span>
					</div>
				</div>
				<!-- 单条时的简单加载提示 / Simple loading hint for single entry -->
				<div v-else-if="loading" class="text-xs text-muted-foreground">
					{{ t("addStreamer.submitting") }}
				</div>

				<DialogFooter>
					<Button
						type="button"
						variant="outline"
						:disabled="loading"
						@click="emit('close')"
					>
						{{ t("addStreamer.cancel") }}
					</Button>
					<Button type="submit" :disabled="loading || parsedUsernames.length === 0">
						{{ loading ? t("addStreamer.submitting") : t("addStreamer.submit") }}
					</Button>
				</DialogFooter>
			</form>
		</DialogContent>
	</Dialog>
</template>
