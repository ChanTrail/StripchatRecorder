<!--
    主播列表页面 / Streamer List View

    展示所有被追踪主播的卡片网格，支持添加/移除主播、手动开始/停止录制、切换自动录制。
    支持按在线/离线/录制中状态筛选，以及批量选择后批量开始录制/停止录制/移除主播。
    页面挂载时初始化事件监听器并从后端加载主播列表。

    Displays a card grid of all tracked streamers, supporting add/remove streamers,
    manual start/stop recording, and toggling auto-record.
    Supports filtering by online/offline/recording status, and batch operations
    (start/stop recording, remove) after multi-selection.
    Initializes event listeners and loads the streamer list from backend on mount.
-->
<script setup lang="ts">
	import { onMounted, ref, computed } from "vue";
	import { useStreamersStore } from "../stores/streamers";
	import type { StreamerEntry } from "../stores/streamers";
	import { useNotify } from "../composables/useNotify";
	import { useMergingStore } from "../stores/merging";
	import { usePpStatusStore } from "../stores/ppStatus";
	import StreamerCard from "../components/StreamerCard.vue";
	import AddStreamerDialog from "../components/AddStreamerDialog.vue";
	import { Button } from "@/components/ui/button";
	import { useI18n } from "vue-i18n";

	const store = useStreamersStore();
	const mergingStore = useMergingStore();
	const ppStatusStore = usePpStatusStore();
	const { toast, confirm } = useNotify();
	const { t } = useI18n();

	/** 是否显示添加主播对话框 / Whether to show the add streamer dialog */
	const showAdd = ref(false);

	onMounted(async () => {
		store.initListeners();
		await store.fetchStreamers();
	});

	// ─── 状态筛选 / Status filter ────────────────────────────────────────────────

	type FilterKey = "all" | "online" | "offline" | "recording" | "公开秀" | "私密秀" | "群组秀" | "票务秀" | "计时秀" | "虚拟私密" | "P2P";
	const activeFilter = ref<FilterKey>("all");

	const filterTabs: { key: FilterKey; label: string }[] = [
		{ key: "all",       label: "home.filter.all"       },
		{ key: "online",    label: "home.filter.online"    },
		{ key: "offline",   label: "home.filter.offline"   },
		{ key: "recording", label: "home.filter.recording" },
	];

	/** 在线子状态筛选项（只在有对应主播时显示）/ Online sub-status filter tabs (only shown when relevant streamers exist) */
	const onlineStatusTabs: { key: FilterKey; label: string; color: "green" | "amber" }[] = [
		{ key: "公开秀",   label: "home.filter.public",        color: "green"  },
		{ key: "私密秀",   label: "home.filter.private",       color: "amber"  },
		{ key: "群组秀",   label: "home.filter.group",         color: "amber"  },
		{ key: "票务秀",   label: "home.filter.ticket",        color: "amber"  },
		{ key: "计时秀",   label: "home.filter.perMinute",     color: "amber"  },
		{ key: "虚拟私密", label: "home.filter.virtualPrivate", color: "amber" },
		{ key: "P2P",      label: "home.filter.p2p",           color: "amber"  },
	];

	/** 当前实际存在（有主播）的在线子状态筛选项 / Online sub-status tabs that have at least one streamer */
	const activeOnlineStatusTabs = computed(() =>
		onlineStatusTabs.filter(({ key }) =>
			store.streamers.some((s) => s.is_online && s.status === key),
		),
	);

	/** 是否为在线子状态筛选 / Whether the active filter is an online sub-status */
	const isStatusFilter = computed(() =>
		onlineStatusTabs.some((t) => t.key === activeFilter.value),
	);

	/** 按状态过滤后排序的主播列表 / Filtered and sorted streamer list */
	const filteredStreamers = computed<StreamerEntry[]>(() => {
		const all = [...store.streamers].sort((a, b) =>
			a.username.localeCompare(b.username),
		);
		switch (activeFilter.value) {
			case "online":    return all.filter((s) => s.is_online);
			case "offline":   return all.filter((s) => !s.is_online);
			case "recording": return all.filter((s) => s.is_recording);
			default:
				if (isStatusFilter.value)
					return all.filter((s) => s.is_online && s.status === activeFilter.value);
				return all;
		}
	});

	/** 各筛选分类的数量，用于 badge / Count per filter tab for badge display */
	const filterCounts = computed(() => {
		const counts: Record<string, number> = {
			all:       store.streamers.length,
			online:    store.streamers.filter((s) => s.is_online).length,
			offline:   store.streamers.filter((s) => !s.is_online).length,
			recording: store.streamers.filter((s) => s.is_recording).length,
		};
		for (const { key } of onlineStatusTabs) {
			counts[key] = store.streamers.filter((s) => s.is_online && s.status === key).length;
		}
		return counts;
	});

	/**
	 * 切换状态筛选分类，同时退出批量选择模式并清空已选。
	 * Switch the active status filter tab; also exits batch selection mode and clears selection.
	 *
	 * @param key - 筛选分类键 / Filter tab key
	 */
	function setFilter(key: FilterKey) {
		activeFilter.value = key;
		// 切换筛选时清空已选 / Clear selection on filter change
		selectedSet.value = new Set();
	}

	// ─── 批量选择 / Batch selection ──────────────────────────────────────────────

	/** 已选中的主播用户名集合 / Set of selected streamer usernames */
	const selectedSet = ref(new Set<string>());

	/**
	 * 批量选择模式：有任意主播被选中时自动激活，无需手动进入。
	 * Batch selection mode: automatically active when any streamer is selected.
	 */
	const selectMode = computed(() => selectedSet.value.size > 0);

	const selectedCount = computed(() => selectedSet.value.size);

	/** 当前筛选列表中已全部选中 / Whether all visible streamers are selected */
	const allVisibleSelected = computed(
		() =>
			filteredStreamers.value.length > 0 &&
			filteredStreamers.value.every((s) => selectedSet.value.has(s.username)),
	);

	/**
	 * 退出批量选择模式并清空已选集合。
	 * Exit batch selection mode and clear the selection set.
	 */
	function exitSelectMode() {
		selectedSet.value = new Set();
	}

	/**
	 * 切换单个主播的选中状态。
	 * Toggle the selection state of a single streamer.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	function toggleSelect(username: string) {
		const next = new Set(selectedSet.value);
		if (next.has(username)) next.delete(username);
		else next.add(username);
		selectedSet.value = next;
	}

	/**
	 * 切换当前可见列表的全选/取消全选状态。
	 * 若当前可见列表已全选则取消全选，否则全选。
	 *
	 * Toggle select-all / deselect-all for the currently visible streamer list.
	 * Deselects all if all visible streamers are already selected; otherwise selects all.
	 */
	function toggleSelectAll() {
		if (allVisibleSelected.value) {
			// 已全选 → 取消全选当前可见列表 / All selected → deselect all visible
			const next = new Set(selectedSet.value);
			filteredStreamers.value.forEach((s) => next.delete(s.username));
			selectedSet.value = next;
		} else {
			// 未全选 → 全选当前可见列表 / Not all selected → select all visible
			const next = new Set(selectedSet.value);
			filteredStreamers.value.forEach((s) => next.add(s.username));
			selectedSet.value = next;
		}
	}

	/** 当前已选且在可见列表中的主播名列表 / Selected usernames that are in the current visible list */
	const visibleSelectedUsernames = computed(() =>
		filteredStreamers.value
			.filter((s) => selectedSet.value.has(s.username))
			.map((s) => s.username),
	);

	// ─── 批量操作 / Batch operations ─────────────────────────────────────────────

	/**
	 * 批量开始录制当前已选且可见的主播。
	 * Batch start recording for all currently selected visible streamers.
	 */
	async function handleBatchStart() {
		const targets = visibleSelectedUsernames.value;
		if (!targets.length) return;
		try {
			const { failed } = await store.batchStartRecording(targets);
			if (failed > 0) {
				toast(t("home.batch.startFailed", { failed }), "error");
			} else {
				toast(t("home.batch.startDone", { count: targets.length }), "success");
			}
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 批量停止录制当前已选且正在录制的主播，先弹出确认对话框。
	 * Batch stop recording for selected streamers that are currently recording, with confirmation dialog.
	 */
	async function handleBatchStop() {
		const targets = visibleSelectedUsernames.value.filter(
			(u) => store.streamers.find((s) => s.username === u)?.is_recording,
		);
		if (!targets.length) return;
		const ok = await confirm({
			title: t("home.batch.stopConfirmTitle"),
			message: t("home.batch.stopConfirmMessage", { count: targets.length }),
			confirmText: t("home.stop.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			const { failed } = await store.batchStopRecording(targets);
			if (failed > 0) {
				toast(t("home.batch.stopFailed", { failed }), "error");
			} else {
				toast(t("home.batch.stopDone", { count: targets.length }), "info");
			}
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 批量切换自动录制：选中主播全部已开启则关闭，否则统一开启。
	 * Batch toggle auto-record: disable all if all selected are enabled, otherwise enable all.
	 */
	async function handleBatchAutoRecord() {
		const targets = visibleSelectedUsernames.value;
		if (!targets.length) return;
		const allEnabled = targets.every(
			(u) => store.streamers.find((s) => s.username === u)?.auto_record,
		);
		const enabled = !allEnabled;
		try {
			const { failed } = await store.batchSetAutoRecord(targets, enabled);
			const action = enabled ? t("home.batch.autoRecordEnable") : t("home.batch.autoRecordDisable");
			if (failed > 0) {
				toast(t("home.batch.autoRecordFailed", { failed }), "error");
			} else {
				toast(t("home.batch.autoRecordDone", { count: targets.length, action }), "success");
			}
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 批量移除当前已选且可见的主播，先弹出确认对话框。
	 * 移除前取消所有相关后处理任务并清理合并队列状态。
	 *
	 * Batch remove currently selected visible streamers, with confirmation dialog.
	 * Cancels all related post-processing tasks and clears merge queue state before removal.
	 */
	async function handleBatchRemove() {
		const targets = visibleSelectedUsernames.value;
		if (!targets.length) return;
		const ok = await confirm({
			title: t("home.batch.removeConfirmTitle"),
			message: t("home.batch.removeConfirmMessage", { count: targets.length }),
			confirmText: t("home.remove.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			// 取消并清理所有目标主播的后处理任务 / Cancel and clear post-processing tasks for all targets
			await Promise.all(
				targets.map((u) =>
					ppStatusStore.cancelAndClearForUsername(u).catch(() => {}),
				),
			);
			// 清理所有目标主播的合并队列状态 / Clear merge queue state for all targets
			targets.forEach((u) => mergingStore.clearMergingForUsername(u));
			const { failed } = await store.batchRemove(targets);
			exitSelectMode();
			if (failed > 0) {
				toast(t("home.batch.removeFailed", { failed }), "error");
			} else {
				toast(t("home.batch.removeDone", { count: targets.length }), "success");
			}
		} catch (e) {
			toast(String(e), "error");
		}
	}

	// ─── 单卡片操作（同原来逻辑）/ Per-card operations (same as before) ──────────

	/**
	 * 处理移除主播操作，先弹出确认对话框。
	 * 删除前取消该主播所有正在进行的后处理任务，并清理合并队列状态。
	 *
	 * Handle remove streamer action with confirmation dialog.
	 * Cancels all in-progress post-processing tasks and clears merge queue state before removal.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function handleRemove(username: string) {
		const ok = await confirm({
			title: t("home.remove.title"),
			message: t("home.remove.message", { username }),
			confirmText: t("home.remove.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			// 取消并清理该主播的后处理任务 / Cancel and clear post-processing tasks for this streamer
			await ppStatusStore.cancelAndClearForUsername(username);
			// 清理该主播的合并队列状态 / Clear merge queue state for this streamer
			mergingStore.clearMergingForUsername(username);
			await store.removeStreamer(username);
			toast(t("home.remove.done", { username }), "success");
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 处理手动开始录制操作。
	 * Handle manual start recording action.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function handleStart(username: string) {
		try {
			await store.startRecording(username);
			toast(t("home.start.done", { username }), "success");
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 处理自动录制开关切换。
	 * 若开启自动录制且主播当前可录制但未在录制，则立即开始录制。
	 *
	 * Handle auto-record toggle.
	 * If enabled and streamer is currently recordable but not recording, start recording immediately.
	 *
	 * @param username - 主播用户名 / Streamer username
	 * @param streamer - 主播数据对象 / Streamer data object
	 * @param enabled - 是否开启自动录制 / Whether to enable auto-record
	 */
	async function handleToggleAuto(
		username: string,
		streamer: StreamerEntry,
		enabled: boolean,
	) {
		try {
			await store.setAutoRecord(username, enabled);
			if (enabled && streamer.is_recordable && !streamer.is_recording) {
				await store.startRecording(username);
				toast(t("home.start.autoStarted", { username }), "success");
			}
		} catch (e) {
			toast(String(e), "error");
		}
	}

	/**
	 * 处理停止录制操作，先弹出确认对话框。
	 * Handle stop recording action with confirmation dialog.
	 *
	 * @param username - 主播用户名 / Streamer username
	 */
	async function handleStop(username: string) {
		const ok = await confirm({
			title: t("home.stop.title"),
			message: t("home.stop.message", { username }),
			confirmText: t("home.stop.confirm"),
			danger: true,
		});
		if (!ok) return;
		try {
			await store.stopRecording(username);
			toast(t("home.stop.done", { username }), "info");
		} catch (e) {
			toast(String(e), "error");
		}
	}

	
</script>

<template>
	<div class="flex flex-col gap-4 px-6">
		<!-- 置顶区域：页头 + 筛选 Tab 栏 / Sticky zone: header + filter tabs -->
		<div class="bg-background sticky top-0 z-20 -mx-6 px-6 border-b flex flex-col">
		<!-- 页头 / Header -->
		<header class="flex items-start justify-between gap-3 shrink-0 pt-6 pb-3">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("home.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{
						t("home.subtitle", {
							total: store.streamers.length,
							recording: store.streamers.filter((s) => s.is_recording).length,
						})
					}}
				</p>
			</div>
			<div class="flex items-center gap-2 shrink-0">
				<Button @click="showAdd = true">{{ t("home.addStreamer") }}</Button>
			</div>
		</header>

		<!-- 状态筛选 Tab 栏 / Status filter tabs -->
		<div v-if="store.streamers.length > 0" class="flex flex-col">
			<!-- 主 Tab 行 / Primary tab row -->
			<div class="flex items-center gap-1 border-b pb-0">
				<button
					v-for="tab in filterTabs"
					:key="tab.key"
					class="px-3 py-1.5 text-sm rounded-t font-medium transition-colors flex items-center gap-1.5"
					:class="
						activeFilter === tab.key
							? 'text-foreground border-b-2 border-primary -mb-px'
							: 'text-muted-foreground hover:text-foreground'
					"
					@click="setFilter(tab.key)"
				>
					{{ t(tab.label) }}
					<span
						v-if="filterCounts[tab.key] > 0"
						class="text-xs px-1.5 py-0.5 rounded-full"
						:class="
							activeFilter === tab.key
								? 'bg-primary text-primary-foreground'
								: 'bg-muted text-muted-foreground'
						"
					>
						{{ filterCounts[tab.key] }}
					</span>
				</button>
			</div>

			<!-- 在线子状态行（有在线主播且有对应秀类型时显示）/ Online sub-status row (shown when relevant streamers exist) -->
			<div v-if="activeOnlineStatusTabs.length > 0" class="flex items-center gap-1 border-b pb-0">
				<button
					v-for="tab in activeOnlineStatusTabs"
					:key="tab.key"
					class="px-3 py-1.5 text-xs rounded-t font-medium transition-colors flex items-center gap-1.5"
					:class="
						activeFilter === tab.key
							? 'text-foreground border-b-2 border-primary -mb-px'
							: 'text-muted-foreground hover:text-foreground'
					"
					@click="setFilter(tab.key)"
				>
					{{ t(tab.label) }}
					<span
						class="text-xs px-1.5 py-0.5 rounded-full"
						:class="
							tab.color === 'green'
								? activeFilter === tab.key
									? 'bg-green-600 text-green-50'
									: 'bg-green-900/60 text-green-300'
								: activeFilter === tab.key
									? 'bg-amber-600 text-amber-50'
									: 'bg-amber-900/60 text-amber-300'
						"
					>
						{{ filterCounts[tab.key] }}
					</span>
				</button>
			</div>
		</div>

		<!-- 批量操作控制栏 / Batch action toolbar -->
		<div
			v-if="selectMode"
			class="flex items-center gap-2 flex-wrap bg-muted/50 border rounded-lg px-3 py-2 my-2"
		>
			<span class="text-sm text-muted-foreground mr-1">
				{{ t("home.batch.selected", { count: selectedCount }) }}
			</span>

			<Button variant="outline" size="sm" @click="toggleSelectAll">
				{{ allVisibleSelected ? t("home.batch.deselectAll") : t("home.batch.selectAll") }}
			</Button>

			<div class="flex items-center gap-2 ml-auto flex-wrap">
				<Button
					size="sm"
					:disabled="selectedCount === 0 || visibleSelectedUsernames.every(
						(u) => !store.streamers.find((s) => s.username === u)?.is_recordable
					)"
					@click="handleBatchStart"
				>
					{{ t("home.batch.startRecording", { count: selectedCount }) }}
				</Button>

				<Button
					size="sm"
					variant="outline"
					:disabled="
						selectedCount === 0 ||
						!visibleSelectedUsernames.some(
							(u) => store.streamers.find((s) => s.username === u)?.is_recording,
						)
					"
					@click="handleBatchStop"
				>
					{{ t("home.batch.stopRecording", { count: selectedCount }) }}
				</Button>

				<Button
					size="sm"
					variant="outline"
					:disabled="selectedCount === 0"
					@click="handleBatchAutoRecord"
				>
					{{
						visibleSelectedUsernames.every(
							(u) => store.streamers.find((s) => s.username === u)?.auto_record
						)
							? t("home.batch.disableAutoRecord", { count: selectedCount })
							: t("home.batch.enableAutoRecord", { count: selectedCount })
					}}
				</Button>

				<Button
					size="sm"
					variant="destructive"
					:disabled="selectedCount === 0"
					@click="handleBatchRemove"
				>
					{{ t("home.batch.remove", { count: selectedCount }) }}
				</Button>

				<Button variant="ghost" size="sm" @click="exitSelectMode">
					{{ t("home.batch.exitSelect") }}
				</Button>
			</div>
		</div>
		</div><!-- /sticky zone -->

		<!-- 加载状态 / Loading state -->
		<div
			v-if="store.loading && store.streamers.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			{{ t("home.loadingStreamers") }}
		</div>

		<!-- 空状态 / Empty state -->
		<div
			v-else-if="store.streamers.length === 0"
			class="text-center text-muted-foreground py-16 flex flex-col items-center gap-3"
		>
			<p>{{ t("home.noStreamers") }}</p>
			<Button @click="showAdd = true">{{ t("home.addFirst") }}</Button>
		</div>

		<!-- 筛选后无结果 / Filtered empty state -->
		<div
			v-else-if="filteredStreamers.length === 0"
			class="text-center text-muted-foreground py-12"
		>
			{{ t("home.filter." + activeFilter) }} · {{ t("common.noData") }}
		</div>

		<!-- 主播卡片网格 / Streamer card grid -->
		<div
			v-else
			class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3.5"
		>
			<StreamerCard
				v-for="s in filteredStreamers"
				:key="s.username"
				:streamer="s"
				:select-mode="selectMode"
				:selected="selectedSet.has(s.username)"
				@remove="handleRemove(s.username)"
				@toggle-auto="handleToggleAuto(s.username, s, $event)"
				@start="handleStart(s.username)"
				@stop="handleStop(s.username)"
				@toggle-select="toggleSelect(s.username)"
			/>
		</div>

		<AddStreamerDialog
			v-if="showAdd"
			@close="showAdd = false"
			@added="showAdd = false"
		/>
	</div>
</template>
