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

	const store = useStreamersStore();
	const { toast, confirm } = useNotify();
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
			title: "移除主播",
			message: `确定要移除 ${username} 吗？相关录制文件也会被删除。`,
			confirmText: "移除",
			danger: true,
		});
		if (!ok) return;
		try {
			await store.removeStreamer(username);
			toast(`已移除 ${username}`, "success");
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
			toast(`开始录制 ${username}`, "success");
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
				toast(`已开始录制 ${username}`, "success");
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
			title: "停止录制",
			message: `确定要停止录制 ${username} 吗？`,
			confirmText: "停止",
			danger: true,
		});
		if (!ok) return;
		try {
			await store.stopRecording(username);
			toast(`已停止录制 ${username}`, "info");
		} catch (e) {
			toast(String(e), "error");
		}
	}
</script>

<template>
	<div class="flex flex-col gap-5">
		<header class="flex items-start justify-between">
			<div>
				<h1 class="text-xl font-bold mb-0.5">主播列表</h1>
				<p class="text-sm text-muted-foreground">
					共 {{ store.streamers.length }} 位主播，{{
						store.streamers.filter((s) => s.is_recording).length
					}}
					个录制中
				</p>
			</div>
			<Button @click="showAdd = true">+ 添加主播</Button>
		</header>

		<div
			v-if="store.loading && store.streamers.length === 0"
			class="text-center text-muted-foreground py-16"
		>
			加载中...
		</div>

		<div
			v-else-if="store.streamers.length === 0"
			class="text-center text-muted-foreground py-16 flex flex-col items-center gap-3"
		>
			<p>还没有添加主播</p>
			<Button @click="showAdd = true">添加第一个主播</Button>
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

