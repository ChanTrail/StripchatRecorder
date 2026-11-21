<!--
    目录/文件浏览器对话框组件 / Directory & File Browser Dialog Component

    全局单例组件，挂载于 App.vue，通过 useDirectoryBrowser() composable 控制。
    支持进入子目录、返回上级、查看驱动器、手动输入路径跳转、新建文件夹；
    文件模式下额外列出文件条目，点选后"选择"返回文件路径。
    移动端从底部滑入全屏覆盖层；桌面端居中 Dialog 弹窗。
-->
<script setup lang="ts">
	import { computed, nextTick, watch } from "vue";
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
	import { File, Folder, FolderPlus, ChevronUp, RefreshCw, Loader2, HardDrive, ArrowLeft, ChevronRight } from "@lucide/vue";
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
		selectFile,
		confirmSelect,
		cancel,
	} = useDirectoryBrowser();

	/** 对话框标题随模式切换（选目录 / 选文件 / 选程序）/ Dialog title follows the pick mode (dir vs file vs program) */
	const dialogTitle = computed(() =>
		state.mode !== "file"
			? t("dirBrowser.title")
			: state.execOnly
				? t("dirBrowser.titleExec")
				: t("dirBrowser.titleFile"),
	);
	/** 空列表文案随模式切换 / Empty-list text follows the pick mode */
	const emptyText = computed(() =>
		state.mode !== "file"
			? t("dirBrowser.empty")
			: state.execOnly
				? t("dirBrowser.emptyExec")
				: t("dirBrowser.emptyFile"),
	);
	/** "选择"按钮可用条件：目录模式直接可选；文件模式要求已选中文件 /
	 * Select-button enable rule: dir mode selects the current dir outright;
	 * file mode requires a picked file */
	const canConfirm = computed(
		() => !state.loading && !state.error && !state.isDrives && (state.mode !== "file" || !!state.selectedFile),
	);
	/** 确认按钮文案随模式切换（选此目录 / 选此文件 / 选此程序）/
	 * Confirm-button text follows the pick mode (this dir / this file / this program) */
	const confirmText = computed(() =>
		state.mode !== "file"
			? t("dirBrowser.select")
			: state.execOnly
				? t("dirBrowser.selectExec")
				: t("dirBrowser.selectFile"),
	);

	const scrollEl = ref<HTMLElement | null>(null);
	const mobileScrollEl = ref<HTMLElement | null>(null);
	useScrollbar(scrollEl);
	useScrollbar(mobileScrollEl);

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
			<div class="flex items-center gap-3 px-3 pt-4 pb-3 shrink-0 border-b">
				<button
					class="flex items-center justify-center size-10 rounded-full bg-muted text-foreground active:bg-accent transition-colors shrink-0"
					@click="cancel"
				>
					<ArrowLeft class="size-5" />
				</button>
				<h2 class="flex-1 text-base font-semibold truncate">{{ dialogTitle }}</h2>
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

			<div ref="mobileScrollEl" class="flex-1 min-h-0 overflow-y-auto scrollbar-overlay">
				<div v-if="state.loading" class="flex items-center justify-center h-full text-muted-foreground text-sm gap-2">
					<Loader2 class="size-5 animate-spin" />
					{{ t("common.loading") }}
				</div>
				<p v-else-if="state.error" class="text-sm text-destructive px-4 py-4">{{ state.error }}</p>
				<div v-else-if="state.dirs.length === 0 && state.files.length === 0" class="text-center text-muted-foreground text-sm py-12">
					{{ emptyText }}
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
					<li v-for="f in state.files" :key="f.path" class="border-b border-border/40 last:border-0">
						<button
							type="button"
							class="w-full flex items-center gap-3 px-4 py-3.5 text-sm active:bg-accent transition-colors text-left"
							:class="state.selectedFile === f.path && 'bg-accent'"
							@click="selectFile(f)"
						>
							<File class="size-5 text-muted-foreground shrink-0" />
							<span class="flex-1 truncate">{{ f.name }}</span>
						</button>
					</li>
				</ul>
			</div>

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

			<div class="flex items-center gap-3 px-4 py-4 border-t shrink-0 pb-[calc(1rem+env(safe-area-inset-bottom,0px))]">
				<Button
					size="default"
					class="flex-1 h-12 text-base"
					:disabled="!canConfirm"
					@click="confirmSelect"
				>
					{{ confirmText }}
				</Button>
			</div>
		</div>
	</Transition>

	<!-- ── 桌面端：居中 Dialog / Desktop: centered Dialog ── -->
	<Dialog v-else :open="state.visible" @update:open="state.visible = $event">
		<DialogContent class="p-0 flex flex-col w-full max-w-lg" style="height: 32rem;">
			<DialogHeader class="px-4 pt-4 pb-2 shrink-0">
				<DialogTitle class="text-sm font-semibold">{{ dialogTitle }}</DialogTitle>
			</DialogHeader>

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

			<div ref="scrollEl" class="flex-1 min-h-0 overflow-y-auto scrollbar-overlay border-t">
				<div v-if="state.loading" class="flex items-center justify-center h-full text-muted-foreground text-sm gap-2">
					<Loader2 class="size-4 animate-spin" />
					{{ t("common.loading") }}
				</div>
				<p v-else-if="state.error" class="text-sm text-destructive px-4 py-4">{{ state.error }}</p>
				<div v-else-if="state.dirs.length === 0 && state.files.length === 0" class="text-center text-muted-foreground text-sm py-8">
					{{ emptyText }}
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
					<li v-for="f in state.files" :key="f.path">
						<button
							type="button"
							class="w-full flex items-center gap-2 px-4 py-1.5 text-sm hover:bg-accent transition-colors text-left"
							:class="state.selectedFile === f.path && 'bg-accent'"
							@click="selectFile(f)"
							@dblclick="selectFile(f); confirmSelect()"
						>
							<File class="size-4 text-muted-foreground shrink-0" />
							<span class="truncate">{{ f.name }}</span>
						</button>
					</li>
				</ul>
			</div>

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

			<div class="flex items-center justify-between gap-2 px-4 py-3 border-t shrink-0">
				<span class="text-xs text-muted-foreground truncate font-mono">
					{{ state.isDrives ? t("dirBrowser.thisPc") : (state.selectedFile || state.currentPath) }}
				</span>
				<div class="flex items-center gap-2 shrink-0">
					<Button variant="ghost" size="sm" @click="cancel">{{ t("dirBrowser.cancel") }}</Button>
					<Button size="sm" :disabled="!canConfirm" @click="confirmSelect">
						{{ confirmText }}
					</Button>
				</div>
			</div>
		</DialogContent>
	</Dialog>
</template>
