<!--
    服务器端目录浏览器对话框组件 / Server-side Directory Browser Dialog Component

    类似资源管理器的目录选择弹窗：显示当前路径下的子目录列表，支持进入子目录、
    返回上一级、跳转到"此电脑"查看所有驱动器/根、手动输入路径跳转、新建文件夹，
    最终确认选择当前路径。所有目录数据来自后端 list_dir/list_drives/create_dir
    命令（服务器文件系统），与本地文件选择器无关。

    这是一个全局单例组件：只应在 App.vue 中挂载一次，状态由 useDirectoryBrowser()
    composable 统一管理（模块级共享状态），任何组件调用其 open() 方法即可复用同一个
    弹窗实例，弹窗内容随每次调用传入的初始路径和回调而变化。

    Explorer-like directory picker dialog: lists subdirectories of the current path,
    supports drilling into subdirectories, going up one level, jumping to "This PC" to
    view all drives/roots, jumping to a manually entered path, creating a new folder,
    and confirming the current path as the selection. All directory data comes from the
    backend list_dir/list_drives/create_dir commands (server-side filesystem), unrelated
    to any local file picker.

    This is a global singleton component: it should be mounted exactly once in App.vue.
    State is managed centrally by the useDirectoryBrowser() composable (module-level
    shared state); any component can call its open() method to reuse the same dialog
    instance, with content changing per call based on the initial path and callback passed in.
-->
<script setup lang="ts">
	import { nextTick, watch } from "vue";
	import { Button } from "@/components/ui/button";
	import { Input } from "@/components/ui/input";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import { useScrollbar } from "@/composables/useScrollbar";
	import { useDirectoryBrowser } from "@/composables/useDirectoryBrowser";
	import { ref } from "vue";
	import { Folder, FolderPlus, ChevronUp, RefreshCw, Loader2, HardDrive } from "@lucide/vue";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();
	const {
		state,
		enterDir,
		goUp,
		goToInputPath,
		refresh,
		showDrives,
		toggleNewFolder,
		createFolder,
		confirmSelect,
		cancel,
	} = useDirectoryBrowser();

	const scrollEl = ref<HTMLElement | null>(null);
	useScrollbar(scrollEl);

	// 展开新建文件夹表单时自动聚焦输入框 / Auto-focus the input when the new-folder form expands
	watch(() => state.showNewFolder, async (show) => {
		if (!show) return;
		await nextTick();
		(document.getElementById("dir-browser-new-folder-input") as HTMLInputElement | null)?.focus();
	});
</script>

<template>
	<Dialog :open="state.visible" @update:open="state.visible = $event">
		<DialogContent class="p-0 flex flex-col w-full max-w-lg" style="height: 32rem;">
			<DialogHeader class="px-4 pt-4 pb-2 shrink-0">
				<DialogTitle class="text-sm font-semibold">{{ t("dirBrowser.title") }}</DialogTitle>
			</DialogHeader>

			<!-- 路径栏 / Path bar -->
			<div class="flex items-center gap-1.5 px-4 pb-2 shrink-0">
				<Button
					variant="outline"
					size="icon"
					class="size-8 shrink-0"
					:disabled="state.loading"
					:title="t('dirBrowser.thisPc')"
					@click="showDrives"
				>
					<HardDrive class="size-4" />
				</Button>
				<Button
					variant="outline"
					size="icon"
					class="size-8 shrink-0"
					:disabled="state.isDrives || state.parentPath == null || state.loading"
					:title="t('dirBrowser.up')"
					@click="goUp"
				>
					<ChevronUp class="size-4" />
				</Button>
				<Input
					v-model="state.pathInput"
					class="flex-1 h-8 text-xs font-mono"
					:placeholder="t('dirBrowser.pathPlaceholder')"
					autocomplete="off"
					@keyup.enter="goToInputPath"
				/>
				<Button
					variant="outline"
					size="icon"
					class="size-8 shrink-0"
					:disabled="state.loading"
					:title="t('dirBrowser.refresh')"
					@click="refresh"
				>
					<RefreshCw class="size-4" :class="state.loading && 'animate-spin'" />
				</Button>
			</div>

			<!-- 目录/驱动器列表 / Directory or drive listing -->
			<div ref="scrollEl" class="flex-1 min-h-0 overflow-y-auto scrollbar-overlay border-t">
				<div v-if="state.loading" class="flex items-center justify-center h-full text-muted-foreground text-sm gap-2">
					<Loader2 class="size-4 animate-spin" />
					{{ t("common.loading") }}
				</div>
				<p v-else-if="state.error" class="text-sm text-destructive px-4 py-4">{{ state.error }}</p>
				<div v-else-if="state.dirs.length === 0" class="text-center text-muted-foreground text-sm py-8">
					{{ t("dirBrowser.empty") }}
				</div>
				<ul v-else class="py-1">
					<li v-for="d in state.dirs" :key="d.path">
						<button
							type="button"
							class="w-full flex items-center gap-2 px-4 py-1.5 text-sm hover:bg-accent transition-colors text-left"
							@click="enterDir(d)"
						>
							<HardDrive v-if="state.isDrives" class="size-4 text-muted-foreground shrink-0" />
							<Folder v-else class="size-4 text-amber-500 shrink-0" />
							<span class="truncate">{{ d.name }}</span>
						</button>
					</li>
				</ul>
			</div>

			<!-- 新建文件夹（驱动器列表视图下不可用）/ New folder (unavailable in the drive list view) -->
			<div v-if="!state.isDrives" class="px-4 pt-2 shrink-0 border-t">
				<div v-if="state.showNewFolder" class="flex items-center gap-1.5 py-2">
					<Input
						id="dir-browser-new-folder-input"
						v-model="state.newFolderName"
						class="flex-1 h-8 text-xs"
						:placeholder="t('dirBrowser.newFolderPlaceholder')"
						autocomplete="off"
						@keyup.enter="createFolder"
						@keyup.esc="state.showNewFolder = false"
					/>
					<Button size="sm" class="h-8" :disabled="!state.newFolderName.trim() || state.creatingFolder" @click="createFolder">
						{{ t("common.confirm") }}
					</Button>
					<Button variant="ghost" size="sm" class="h-8" @click="state.showNewFolder = false">
						{{ t("common.cancel") }}
					</Button>
				</div>
				<Button v-else variant="ghost" size="sm" class="h-8 -ml-2 text-muted-foreground" @click="toggleNewFolder">
					<FolderPlus class="size-3.5 mr-1" />
					{{ t("dirBrowser.newFolder") }}
				</Button>
			</div>
			<div v-else class="px-4 pt-2 pb-0.5 shrink-0 border-t" />

			<!-- 底部操作栏 / Bottom action bar -->
			<div class="flex items-center justify-between gap-2 px-4 py-3 border-t shrink-0">
				<span class="text-xs text-muted-foreground truncate font-mono">
					{{ state.isDrives ? t("dirBrowser.thisPc") : state.currentPath }}
				</span>
				<div class="flex items-center gap-2 shrink-0">
					<Button variant="ghost" size="sm" @click="cancel">{{ t("dirBrowser.cancel") }}</Button>
					<Button size="sm" :disabled="state.loading || !!state.error || state.isDrives" @click="confirmSelect">
						{{ t("dirBrowser.select") }}
					</Button>
				</div>
			</div>
		</DialogContent>
	</Dialog>
</template>
