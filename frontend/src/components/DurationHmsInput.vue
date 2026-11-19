<!--
	DurationHmsInput.vue - 时/分/秒时长输入控件（单控件 + 三列滚轮选择器）
	DurationHmsInput.vue - H/M/S duration input (single control + three-column wheel picker)

	对外只暴露总秒数的 v-model；控件本体显示 HH:MM:SS，
	点击弹出与原生 time picker 类似的三列滚动选择器（时/分/秒），
	确认后一次性回写总秒数。0 表示"不限制"语义由调用方解释。
	弹层配色/动画/滚动条全部沿用项目 shadcn 规范（bg-popover、SelectContent 动画、
	scrollbar-overlay 隐藏式滚动条）。

	Exposes a single v-model of total seconds; the control displays HH:MM:SS
	and opens a three-column scroll picker (hours/minutes/seconds) similar to
	the native time picker. Confirmed once → writes back total seconds.
	0 means "unlimited" — interpretation left to the caller.
	Popover colors/animations/scrollbars follow the project's shadcn
	conventions (bg-popover tokens, SelectContent animations,
	scrollbar-overlay auto-hide scrollbars).
-->
<script setup lang="ts">
	import { computed, nextTick, ref, watch } from "vue";
	import type { HTMLAttributes } from "vue";
	import { PopoverContent, PopoverPortal, PopoverRoot, PopoverTrigger } from "reka-ui";
	import { Clock, X } from "@lucide/vue";
	import { useI18n } from "vue-i18n";
	import { cn } from "@/lib/utils";
	import { Button } from "@/components/ui/button";
	import { useScrollbar } from "@/composables/useScrollbar";

	const props = withDefaults(
		defineProps<{
			/** 总秒数（0 = 不限制）/ Total seconds (0 = unlimited) */
			modelValue: number;
			class?: HTMLAttributes["class"];
		}>(),
		{ modelValue: 0 },
	);

	const emit = defineEmits<{
		"update:modelValue": [seconds: number];
	}>();

	const { t } = useI18n();

	/** 弹层开关 / Picker open state */
	const open = ref(false);

	// ── 显示文本 / Display text ──────────────────────────────────────────────
	/** 时部分：不足两位补零，>=100 自然扩展 / Hours: zero-padded to 2, grows naturally >=100 */
	const display = computed(() => {
		const total = Math.max(0, Math.floor(props.modelValue || 0));
		const h = Math.floor(total / 3600);
		const m = Math.floor((total % 3600) / 60);
		const s = total % 60;
		const pad = (n: number) => String(n).padStart(2, "0");
		return `${pad(h)}:${pad(m)}:${pad(s)}`;
	});

	// ── 弹层草稿值（确认前不改真值）/ Picker draft values (untouched until confirm) ──
	const draftH = ref(0);
	const draftM = ref(0);
	const draftS = ref(0);

	/** 时列取值范围：默认 0-99，若当前值更大则扩展列表 / Hour list range: 0-99 default, grows if the current value exceeds it */
	const hourList = computed(() => {
		const n = Math.max(99, draftH.value);
		return Array.from({ length: n + 1 }, (_, i) => i);
	});
	/** 分/秒列固定 0-59 / Minute/second columns are fixed 0-59 */
	const msList = Array.from({ length: 60 }, (_, i) => i);

	/** 三列定义：取值列表 + 草稿读写 + aria 标签键 / Column defs: value list + draft accessors + aria-label key */
	const columns = [
		{ key: "h", aria: "settings.recordingDuration.unitH", list: () => hourList.value, get: () => draftH.value, set: (n: number) => (draftH.value = n) },
		{ key: "m", aria: "settings.recordingDuration.unitM", list: () => msList, get: () => draftM.value, set: (n: number) => (draftM.value = n) },
		{ key: "s", aria: "settings.recordingDuration.unitS", list: () => msList, get: () => draftS.value, set: (n: number) => (draftS.value = n) },
	] as const;

	// 三列容器引用：打开时滚动定位 + 隐藏式滚动条 / Column container refs:
	// scroll-to-selection on open + overlay scrollbar wiring
	const colRefs = [ref<HTMLElement | null>(null), ref<HTMLElement | null>(null), ref<HTMLElement | null>(null)];
	colRefs.forEach((r) => useScrollbar(r));

	/** 把列中 data-active 的项滚动到视口中央 / Scroll the data-active item to the column's center */
	function scrollActiveIntoView(el: HTMLElement | null) {
		el?.querySelector('[data-active="true"]')?.scrollIntoView({ block: "center" });
	}

	// 打开弹层：用真值播种草稿，并把三列的选中项滚到中间 / On open: seed drafts
	// from the real value and center the selected item in each column
	watch(open, async (v) => {
		if (!v) return;
		const total = Math.max(0, Math.floor(props.modelValue || 0));
		draftH.value = Math.floor(total / 3600);
		draftM.value = Math.floor((total % 3600) / 60);
		draftS.value = total % 60;
		await nextTick();
		colRefs.forEach((r) => scrollActiveIntoView(r.value));
	});

	/** 确认：草稿合成总秒数回写并关闭 / Confirm: combine drafts into total seconds, emit, close */
	function confirm() {
		emit(
			"update:modelValue",
			Math.max(0, draftH.value) * 3600 + Math.max(0, draftM.value) * 60 + Math.max(0, draftS.value),
		);
		open.value = false;
	}

	/** 清零（= 不限制）/ Clear to 0 (= unlimited) */
	function clear() {
		emit("update:modelValue", 0);
	}
</script>

<template>
	<PopoverRoot v-model:open="open">
		<PopoverTrigger as-child>
			<!-- 单控件外观：时钟图标 + HH:MM:SS + 清除钮（复用 input 基类保持与表单一致）/
				Single-control look: clock icon + HH:MM:SS + clear button (reuses the
				input base classes to stay consistent with the form) -->
			<button
				type="button"
				:class="
					cn(
						'border-input dark:bg-input/30 flex h-9 items-center gap-2 rounded-md border bg-transparent px-3 py-1 text-sm shadow-xs transition-[color,box-shadow] outline-none',
						'focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]',
						props.class,
					)
				"
			>
				<Clock class="size-4 shrink-0 text-muted-foreground" />
				<span class="tabular-nums">{{ display }}</span>
				<X
					v-if="modelValue > 0"
					class="ml-auto size-3.5 shrink-0 cursor-pointer text-muted-foreground transition-colors hover:text-foreground"
					:aria-label="t('common.cancel')"
					@click.stop="clear"
				/>
			</button>
		</PopoverTrigger>
		<PopoverPortal>
			<!-- 弹层样式完全对齐 SelectContent 的 shadcn 规范（含进出动画）/
				Popover styling mirrors SelectContent's shadcn spec including
				the enter/exit animations -->
			<PopoverContent
				align="start"
				:side-offset="4"
				class="z-50 w-auto overflow-hidden rounded-md border bg-popover p-0 text-popover-foreground shadow-md outline-none data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2"
			>
				<!-- 三列滚动选择器：snap 滚动 + 隐藏式滚动条 / Three scrollable
					picker columns: snap scrolling + auto-hide overlay scrollbar -->
				<div class="flex">
					<div
						v-for="(col, ci) in columns"
						:key="col.key"
						:ref="
							(el) => {
								colRefs[ci].value = el as HTMLElement | null;
							}
						"
						class="scrollbar-overlay h-40 w-16 snap-y snap-proximity overflow-y-auto overscroll-contain px-1 py-1"
						:class="ci > 0 ? 'border-l' : ''"
						:aria-label="t(col.aria)"
					>
						<button
							v-for="n in col.list()"
							:key="n"
							type="button"
							:data-active="n === col.get()"
							class="flex h-8 w-full snap-center items-center justify-center rounded-md text-sm tabular-nums transition-colors"
							:class="
								n === col.get()
									? 'bg-primary font-medium text-primary-foreground'
									: 'hover:bg-accent hover:text-accent-foreground'
							"
							@click="col.set(n)"
						>
							{{ String(n).padStart(2, "0") }}
						</button>
					</div>
				</div>
				<!-- 底部：取消/确认（草稿确认前不落真值）/ Footer: cancel/confirm
					(draft only lands on confirm) -->
				<div class="flex justify-end gap-2 border-t px-2.5 py-2">
					<Button variant="ghost" size="sm" class="h-7 px-2.5 text-xs" @click="open = false">
						{{ t("common.cancel") }}
					</Button>
					<Button size="sm" class="h-7 px-2.5 text-xs" @click="confirm">
						{{ t("common.confirm") }}
					</Button>
				</div>
			</PopoverContent>
		</PopoverPortal>
	</PopoverRoot>
</template>
