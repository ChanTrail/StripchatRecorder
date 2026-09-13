<!--
    后处理进度单元格组件 / Post-processing Progress Cell Component

    展示单个录制文件在后处理表格列中的状态。
    移动端：done/error 状态改为点击展开内联详情。
    桌面端：hover tooltip。
-->
<script setup lang="ts">
	import { computed, ref, onMounted, onUnmounted } from "vue";
	import { Loader2, Check, X, CheckCheck, AlertCircle } from "@lucide/vue";
	import { Progress } from "@/components/ui/progress";
	import {
		TooltipProvider,
		TooltipRoot,
		TooltipTrigger,
		TooltipContent,
		TooltipArrow,
	} from "reka-ui";
	import { useMobileLayout } from "@/composables/useMobileLayout";
	import type { PpProgress, PpStatus } from "@/composables/usePostprocess";
	import { useI18n } from "vue-i18n";

	const props = defineProps<{
		status?: PpStatus;
		progress?: PpProgress;
	}>();

	const { t } = useI18n();
	const { isMobile } = useMobileLayout();

	const mobileDetailOpen = ref(false);
	const triggerRef = ref<HTMLElement | null>(null);

	function onDocClick(e: MouseEvent) {
		if (!mobileDetailOpen.value) return;
		if (triggerRef.value && triggerRef.value.contains(e.target as Node)) return;
		mobileDetailOpen.value = false;
	}

	onMounted(() => document.addEventListener("click", onDocClick, true));
	onUnmounted(() => document.removeEventListener("click", onDocClick, true));

	const allSuccess = computed(
		() =>
			props.progress?.moduleResults?.length &&
			props.progress.moduleResults.every((r) => r.success),
	);

	const failCount = computed(
		() => props.progress?.moduleResults?.filter((r) => !r.success).length ?? 0,
	);
	void failCount;
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

	<!-- done / error -->
	<div v-else-if="status === 'done' || status === 'error'">
		<template v-if="progress?.moduleResults?.length">

			<!-- ── 移动端：点击展开内联详情 / Mobile: tap to expand inline detail ── -->
			<div v-if="isMobile" ref="triggerRef" class="relative">
				<button
					class="inline-flex items-center gap-1.5 text-xs"
					:class="allSuccess ? 'text-green-500' : 'text-destructive'"
					@click.stop="mobileDetailOpen = !mobileDetailOpen"
				>
					<CheckCheck v-if="allSuccess" class="size-3.5 shrink-0" />
					<AlertCircle v-else class="size-3.5 shrink-0" />
					<span>
						{{ progress.moduleResults.filter((r) => r.success).length }}/{{ progress.moduleResults.length }}
					</span>
				</button>

				<Transition
					enter-active-class="transition-all duration-150 ease-out"
					enter-from-class="opacity-0 -translate-y-1"
					enter-to-class="opacity-100 translate-y-0"
					leave-active-class="transition-all duration-100 ease-in"
					leave-from-class="opacity-100 translate-y-0"
					leave-to-class="opacity-0 -translate-y-1"
				>
					<div
						v-if="mobileDetailOpen"
						class="absolute left-0 top-full mt-1 z-20 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-lg min-w-40"
					>
						<div class="flex flex-col gap-1">
							<div
								v-for="r in progress.moduleResults"
								:key="r.moduleId"
								class="flex items-start gap-1.5"
								:class="r.success ? 'text-green-500' : 'text-destructive'"
							>
								<Check v-if="r.success" class="size-3 shrink-0 mt-0.5" />
								<X v-else class="size-3 shrink-0 mt-0.5" />
								<span class="break-all">
									{{ r.moduleId }}
									<span v-if="!r.success && r.message" class="text-muted-foreground ml-1">{{ r.message }}</span>
								</span>
							</div>
						</div>
					</div>
				</Transition>
			</div>

			<!-- ── 桌面端：hover tooltip / Desktop: hover tooltip ── -->
			<TooltipProvider v-else :delay-duration="200">
				<TooltipRoot>
					<TooltipTrigger as-child>
						<span
							class="inline-flex items-center gap-1.5 text-xs cursor-default select-none"
							:class="allSuccess ? 'text-green-500' : 'text-destructive'"
						>
							<CheckCheck v-if="allSuccess" class="size-3.5 shrink-0" />
							<AlertCircle v-else class="size-3.5 shrink-0" />
							<span>
								{{ progress.moduleResults.filter((r) => r.success).length }}/{{ progress.moduleResults.length }}
							</span>
						</span>
					</TooltipTrigger>
					<TooltipContent
						side="left"
						:side-offset="8"
						class="z-50 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-lg"
					>
						<div class="flex flex-col gap-1 min-w-32">
							<div
								v-for="r in progress.moduleResults"
								:key="r.moduleId"
								class="flex items-start gap-1.5"
								:class="r.success ? 'text-green-500' : 'text-destructive'"
							>
								<Check v-if="r.success" class="size-3 shrink-0 mt-0.5" />
								<X v-else class="size-3 shrink-0 mt-0.5" />
								<span class="break-all">
									{{ r.moduleId }}
									<span v-if="!r.success && r.message" class="text-muted-foreground ml-1">{{ r.message }}</span>
								</span>
							</div>
						</div>
						<TooltipArrow class="fill-popover" />
					</TooltipContent>
				</TooltipRoot>
			</TooltipProvider>
		</template>

		<span
			v-else
			class="inline-flex items-center gap-1.5 text-xs"
			:class="status === 'done' ? 'text-green-500' : 'text-destructive'"
		>
			<CheckCheck v-if="status === 'done'" class="size-3.5 shrink-0" />
			<AlertCircle v-else class="size-3.5 shrink-0" />
		</span>
	</div>

	<span v-else class="text-xs text-muted-foreground">—</span>
</template>
