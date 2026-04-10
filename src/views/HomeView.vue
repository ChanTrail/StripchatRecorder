<!--
    主播列表页面 / Streamer List View

    展示所有被追踪主播的卡片网格，支持添加/移除主播、手动开始/停止录制、切换自动录制。
    页面挂载时初始化事件监听器并从后端加载主播列表。

    Displays a card grid of all tracked streamers, supporting add/remove streamers,
    manual start/stop recording, and toggling auto-record.
    Initializes event listeners and loads the streamer list from backend on mount.
-->
<script setup lang="ts">
	import { onMounted, ref } from "vue";
	import { useStreamersStore } from "../stores/streamers";
	import type { StreamerEntry } from "../stores/streamers";
	import { useNotify } from "../composables/useNotify";
	import StreamerCard from "../components/StreamerCard.vue";
	import AddStreamerDialog from "../components/AddStreamerDialog.vue";
	import { Button } from "@/components/ui/button";
	import { useI18n } from "vue-i18n";

	const store = useStreamersStore();
	const { toast, confirm } = useNotify();
	const { t } = useI18n();
	/** 是否显示添加主播对话框 / Whether to show the add streamer dialog */
	const showAdd = ref(false);

	onMounted(async () => {
		store.initListeners();
		await store.fetchStreamers();
	});

	/**
	 * 处理移除主播操作，先弹出确认对话框。
	 * Handle remove streamer action with confirmation dialog.
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
	<div class="flex flex-col gap-5">
		<header class="flex items-start justify-between">
			<div>
				<h1 class="text-xl font-bold mb-0.5">{{ t("home.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{ t("home.subtitle", { total: store.streamers.length, recording: store.streamers.filter((s) => s.is_recording).length }) }}
				</p>
			</div>
			<Button @click="showAdd = true">{{ t("home.addStreamer") }}</Button>
		</header>

		<div
			v-if="store.loading && store.streamers.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			{{ t("home.loadingStreamers") }}
		</div>

		<div
			v-else-if="store.streamers.length === 0"
			class="text-center text-muted-foreground py-16 flex flex-col items-center gap-3"
		>
			<p>{{ t("home.noStreamers") }}</p>
			<Button @click="showAdd = true">{{ t("home.addFirst") }}</Button>
		</div>

		<div
			v-else
			class="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3.5"
		>
			<StreamerCard
				v-for="s in [...store.streamers].sort((a, b) =>
					a.username.localeCompare(b.username),
				)"
				:key="s.username"
				:streamer="s"
				@remove="handleRemove(s.username)"
				@toggle-auto="handleToggleAuto(s.username, s, $event)"
				@start="handleStart(s.username)"
				@stop="handleStop(s.username)"
			/>
		</div>

		<AddStreamerDialog
			v-if="showAdd"
			@close="showAdd = false"
			@added="showAdd = false"
		/>
	</div>
</template>

