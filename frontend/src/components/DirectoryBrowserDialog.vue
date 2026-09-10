<!--
    服务器端目录浏览器对话框组件 / Server-side Directory Browser Dialog Component

    类似资源管理器的目录选择弹窗：显示当前路径下的子目录列表，支持进入子目录、
    返回上一级、跳转到"此电脑"查看所有驱动器/根、手动输入路径跳转、新建文件夹，
    最终确认选择当前路径。所有目录数据来自后端 list_dir/list_drives/create_dir
    命令（服务器文件系统），与本地文件选择器无关。

    这是一个全局单例组件：只应在 App.vue 中挂载一次，状态由 useDirectoryBrowser()
    composable 统一管理（模块级共享状态），任何组件调用其 open() 方法即可复用同一个
    弹窗实例，弹窗内容随每次调用传入的初始路径和回调而变化。

    移动端（isMobile 为 true）：从底部滑入的全屏覆盖层，操作元素更大更易点击。
    桌面端：保持原有居中 Dialog 弹窗。

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

    Mobile (isMobile === true): full-screen overlay that slides in from the bottom,
    with larger touch targets. Desktop: original centered Dialog.
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
	import { useMobileLayout } from "@/composables/useMobileLayout";
	import { ref } from "vue";
	import { Folder, FolderPlus, ChevronUp, RefreshCw, Loader2, HardDrive, ArrowLeft, ChevronRight } from "@lucide/vue";
	import { useI18n } from "vue-i18n";

	const { t } = useI18n();
	const { isMobile } = useMobileLayout();
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
	const mobileScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(scrollEl);
	useScrollbar(mobileScrollEl);

	// 展开新建文件夹表单时自动聚焦输入框 / Auto-focus the input when the new-folder form expands
	watch(() => state.showNewFolder, async (show) => {
		if (!show) return;
		await nextTick();
		(document.getElementById("dir-browser-new-folder-input") as HTMLInputElement | null)?.focus();
	});
</script>

<template>
	<!-- ── 移动端：底部全屏抽屉 / Mobile: bottom full-screen drawer ── -->
	<Transition
		v-if="isMobile"
		enter-active-class="transition-transform duration-300 ease-out"
		enter-from-class="translate-y-full"
		enter-to-class="translate-y-0"
		leave-active-class="transition-transform duration-250 ease-in"
		leave-from-class="translate-y-0"
		leave-to-class="translate-y-full"
	>
		<div
			v-if="state.visible"
			class="fixed inset-0 z-50 flex flex-col bg-background"
		>
			<!-- 顶部栏 / Top bar -->
			<div class="flex items-center gap-3 px-3 pt-4 pb-3 shrink-0 border-b">
				<button
					class="flex items-center justify-center size-10 rounded-full bg-muted text-foreground active:bg-accent transition-colors shrink-0"
					@click="cancel"
				>
					<ArrowLeft class="size-5" />
				</button>
				<h2 class="flex-1 text-base font-semibold truncate">{{ t("dirBrowser.title") }}</h2>
				<button
					class="flex items-center justify-center size-10 rounded-full bg-muted text-foreground active:bg-accent transition-colors shrink-0"
					:class="(state.isDrives || state.loading) && 'opacity-40 pointer-events-none'"
					@click="showDrives"
				>
					<HardDrive class="size-5" />
				</button>
				<button
					class="flex items-center justify-center size-10 rounded-full bg-muted text-foreground active:bg-accent transition-colors shrink-0"
					:class="(state.isDrives || state.parentPath == null || state.loading) && 'opacity-40 pointer-events-none'"
					@click="goUp"
				>
					<ChevronUp class="size-5" />
				</button>
				<button
					class="flex items-center justify-center size-10 rounded-full bg-muted text-foreground active:bg-accent transition-colors shrink-0"
					:class="state.loading && 'opacity-40 pointer-events-none'"
					@click="refresh"
				>
					<RefreshCw class="size-5" :class="state.loading && 'animate-spin'" />
				</button>
			</div>

			<!-- 当前路径显示 + 路径跳转输入框 / Current path + path jump input -->
			<div class="px-4 py-3 shrink-0 border-b bg-muted/40">
				<p class="text-xs text-muted-foreground mb-2 truncate font-mono">
					{{ state.isDrives ? t("dirBrowser.thisPc") : (state.currentPath || "—") }}
				</p>
				<div class="flex items-center gap-2">
					<Input
						v-model="state.pathInput"
						class="flex-1 h-10 text-sm font-mono"
						:placeholder="t('dirBrowser.pathPlaceholder')"
						autocomplete="off"
						@keyup.enter="goToInputPath"
					/>
					<Button size="default" class="h-10 px-4 shrink-0" variant="secondary" @click="goToInputPath">
						{{ t("dirBrowser.go") }}
					</Button>
				</div>
			</div>

			<!-- 目录/驱动器列表 / Directory or drive listing -->
			<div ref="mobileScrollEl" class="flex-1 min-h-0 overflow-y-auto scrollbar-overlay">
				<div v-if="state.loading" class="flex items-center justify-center h-full text-muted-foreground text-sm gap-2">
					<Loader2 class="size-5 animate-spin" />
					{{ t("common.loading") }}
				</div>
				<p v-else-if="state.error" class="text-sm text-destructive px-4 py-4">{{ state.error }}</p>
				<div v-else-if="state.dirs.length === 0" class="text-center text-muted-foreground text-sm py-12">
					{{ t("dirBrowser.empty") }}
				</div>
				<ul v-else class="py-1">
					<li v-for="d in state.dirs" :key="d.path" class="border-b border-border/40 last:border-0">
						<button
							type="button"
							class="w-full flex items-center gap-3 px-4 py-3.5 text-sm active:bg-accent transition-colors text-left"
							@click="enterDir(d)"
						>
							<HardDrive v-if="state.isDrives" class="size-5 text-muted-foreground shrink-0" />
							<Folder v-else class="size-5 text-amber-500 shrink-0" />
							<span class="flex-1 truncate">{{ d.name }}</span>
							<ChevronRight class="size-4 text-muted-foreground shrink-0" />
						</button>
					</li>
				</ul>
			</div>

			<!-- 新建文件夹（驱动器列表视图下不可用）/ New folder (unavailable in drive list view) -->
			<div v-if="!state.isDrives" class="px-4 py-3 shrink-0 border-t">
				<div v-if="state.showNewFolder" class="flex items-center gap-2">
					<Input
						id="dir-browser-new-folder-input"
						v-model="state.newFolderName"
						class="flex-1 h-11 text-sm"
						:placeholder="t('dirBrowser.newFolderPlaceholder')"
						autocomplete="off"
						@keyup.enter="createFolder"
						@keyup.esc="state.showNewFolder = false"
					/>
					<Button size="default" class="h-11 px-4 shrink-0" :disabled="!state.newFolderName.trim() || state.creatingFolder" @click="createFolder">
						{{ t("common.confirm") }}
					</Button>
					<Button variant="ghost" size="default" class="h-11 px-3 shrink-0" @click="state.showNewFolder = false">
						{{ t("common.cancel") }}
					</Button>
				</div>
				<Button v-else variant="ghost" size="default" class="h-11 w-full justify-start text-muted-foreground" @click="toggleNewFolder">
					<FolderPlus class="size-4 mr-2" />
					{{ t("dirBrowser.newFolder") }}
				</Button>
			</div>

			<!-- 底部确认栏 / Bottom confirm bar -->
			<div class="flex items-center gap-3 px-4 py-4 border-t shrink-0 pb-[calc(1rem+env(safe-area-inset-bottom,0px))]">
				<Button
					size="default"
					class="flex-1 h-12 text-base"
					:disabled="state.loading || !!state.error || state.isDrives"
					@click="confirmSelect"
				>
					{{ t("dirBrowser.select") }}
				</Button>
			</div>
		</div>
	</Transition>

	<!-- ── 桌面端：居中 Dialog / Desktop: centered Dialog ── -->
	<Dialog v-else :open="state.visible" @update:open="state.visible = $event">
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
