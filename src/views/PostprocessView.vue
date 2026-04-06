<!--
    后处理流水线配置页面 / Post-processing Pipeline Configuration View

    提供可视化的流水线编辑器，支持：
    - 拖拽排序节点
    - 启用/禁用节点
    - 配置每个节点的参数（字符串、数字、布尔、下拉选择）
    - 从模块选择器添加新节点
    - 自动保存（防抖 600ms）

    Provides a visual pipeline editor supporting:
    - Drag-and-drop node reordering
    - Enable/disable nodes
    - Configure per-node parameters (string, number, boolean, select)
    - Add new nodes from a module picker
    - Auto-save with 600ms debounce
-->
<script setup lang="ts">
	import { onMounted, computed, ref } from "vue";
	import { usePostprocessStore, type PipelineNode } from "@/stores/postprocess";
	import { useNotify } from "@/composables/useNotify";
	import { Button } from "@/components/ui/button";
	import { Switch } from "@/components/ui/switch";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import { Badge } from "@/components/ui/badge";
	import {
		Dialog,
		DialogContent,
		DialogHeader,
		DialogTitle,
	} from "@/components/ui/dialog";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
	} from "@/components/ui/select";
	import {
		NumberField,
		NumberFieldContent,
		NumberFieldDecrement,
		NumberFieldIncrement,
		NumberFieldInput,
	} from "@/components/ui/number-field";
	import PipelineNodeBox from "@/components/PipelineNodeBox.vue";

	const store = usePostprocessStore();
	const { toast } = useNotify();

	onMounted(async () => {
		await Promise.all([store.fetchModules(), store.fetchPipeline()]);
		// 初始化模块监听器，当其他客户端更新流水线时显示提示
		// Initialize module watcher; show notification when another client updates the pipeline
		store.initModuleWatcher(() =>
			toast("后处理流水线已由其他客户端更新", "info"),
		);
	});

	/** 当前可用模块 ID 集合 / Set of currently available module IDs */
	const availableModuleIds = computed(
		() => new Set(store.modules.map((m) => m.id)),
	);

	/**
	 * 判断节点对应的模块是否已不存在（模块文件被删除）。
	 * Check if a node's module is missing (module file was deleted).
	 */
	function isModuleMissing(node: PipelineNode) {
		return !availableModuleIds.value.has(node.moduleId);
	}

	// 拖拽排序状态 / Drag-and-drop sorting state
	/** 正在拖拽的节点索引 / Index of the node being dragged */
	const dragIndex = ref<number | null>(null);
	/** 拖拽悬停的目标节点索引 / Index of the node being dragged over */
	const dragOverIndex = ref<number | null>(null);
	/** 是否正在拖拽手柄（防止整个卡片触发拖拽）/ Whether dragging via handle (prevents card from triggering drag) */
	const isDraggingHandle = ref(false);

	/**
	 * 拖拽开始：记录源节点索引。
	 * Drag start: record source node index.
	 */
	function onDragStart(e: DragEvent, idx: number) {
		e.dataTransfer?.setData("text/plain", String(idx));
		dragIndex.value = idx;
	}

	/**
	 * 拖拽经过：更新悬停目标索引。
	 * Drag over: update hover target index.
	 */
	function onDragOver(e: DragEvent, idx: number) {
		e.preventDefault();
		dragOverIndex.value = idx;
	}

	/**
	 * 放置：执行节点位置交换。
	 * Drop: perform node position swap.
	 */
	function onDrop(e: DragEvent, idx: number) {
		e.preventDefault();
		if (dragIndex.value === null || dragIndex.value === idx) {
			dragIndex.value = null;
			dragOverIndex.value = null;
			return;
		}
		const nodes = [...store.pipeline.nodes];
		const [moved] = nodes.splice(dragIndex.value, 1);
		nodes.splice(idx, 0, moved);
		store.pipeline.nodes = nodes;
		dragIndex.value = null;
		dragOverIndex.value = null;
	}
	/**
	 * 拖拽结束：清理拖拽状态。
	 * Drag end: clean up drag state.
	 */
	function onDragEnd() {
		dragIndex.value = null;
		dragOverIndex.value = null;
		isDraggingHandle.value = false;
	}

	/** 是否显示模块选择器对话框 / Whether to show the module picker dialog */
	const showPicker = ref(false);

	/**
	 * 打开模块选择器（先刷新模块列表）。
	 * Open the module picker (refresh module list first).
	 */
	async function openPicker() {
		await store.fetchModules();
		showPicker.value = true;
	}

	/** 已在流水线中使用的模块 ID 集合（每个模块只能添加一次）/ Set of module IDs already in the pipeline (each module can only be added once) */
	const usedModuleIds = computed(
		() => new Set(store.pipeline.nodes.map((n) => n.moduleId)),
	);

	/** 尚未添加到流水线的可用模块列表 / Available modules not yet added to the pipeline */
	const availableModules = computed(() =>
		store.modules.filter((m) => !usedModuleIds.value.has(m.id)),
	);

	/**
	 * 将指定模块添加到流水线并关闭选择器。
	 * Add the specified module to the pipeline and close the picker.
	 *
	 * @param id - 模块 ID / Module ID
	 */
	function addModule(id: string) {
		store.addNode(id);
		showPicker.value = false;
	}

	/** 已启用的节点列表（用于底部统计显示）/ List of enabled nodes (for bottom stats display) */
	const enabledNodes = computed(() =>
		store.pipeline.nodes.filter((n) => n.enabled),
	);

	/** 节点列表附带对应模块信息（用于模板渲染）/ Node list with corresponding module info (for template rendering) */
	const nodesWithInfo = computed(() =>
		store.pipeline.nodes.map((node) => ({
			node,
			info: store.modules.find((m) => m.id === node.moduleId),
		})),
	);
</script>

<template>
	<div class="flex flex-col gap-6 max-w-2xl mx-auto">
		<div class="flex items-center justify-between">
			<div>
				<h1 class="text-lg font-semibold">后处理流水线</h1>
				<p class="text-sm text-muted-foreground mt-0.5">
					录制合并完成后，视频文件将依次经过以下模块处理
				</p>
			</div>
			<span class="text-xs text-muted-foreground">
				{{ store.saving ? "保存中…" : "已自动保存" }}
			</span>
		</div>

		<div class="relative">
			<PipelineNodeBox label="输入" description="合并后的单视频文件" fixed />
			<div
				v-if="store.pipeline.nodes.length > 0"
				class="absolute left-1/2 -translate-x-1/2 w-0.5 h-6 bg-border"
				style="bottom: -24px"
			/>
		</div>

		<template v-if="store.pipeline.nodes.length > 0">
			<div
				v-for="({ node, info }, idx) in nodesWithInfo"
				:key="node.nodeId"
				class="relative"
				:draggable="isDraggingHandle"
				@dragstart="onDragStart($event, idx)"
				@dragover="onDragOver($event, idx)"
				@drop="onDrop($event, idx)"
				@dragend="onDragEnd"
			>
				<div
					class="border-2 rounded-xl p-4 bg-card transition-opacity"
					:class="[
						!node.enabled && 'opacity-50',
						dragIndex === idx ? 'opacity-30' : '',
						isModuleMissing(node)
							? 'border-destructive/70 bg-destructive/5'
							: dragOverIndex === idx && dragIndex !== idx
								? 'border-primary'
								: 'border-transparent',
					]"
				>
					<div class="flex items-center gap-3">
						<span
							class="cursor-grab text-muted-foreground select-none text-lg leading-none"
							@mousedown="isDraggingHandle = true"
							@mouseup="isDraggingHandle = false"
							>⠿</span
						>

						<div class="flex-1 min-w-0">
							<div class="flex items-center gap-2">
								<span
									class="font-medium text-sm"
									:class="isModuleMissing(node) && 'text-destructive'"
									>{{ info?.name ?? node.moduleId }}</span
								>
								<Badge
									v-if="isModuleMissing(node)"
									variant="destructive"
									class="text-xs"
									>模块缺失</Badge
								>
								<Badge
									v-else-if="!node.enabled"
									variant="secondary"
									class="text-xs"
									>已跳过</Badge
								>
							</div>
							<p class="text-xs text-muted-foreground truncate">
								{{ info?.description ?? "" }}
							</p>
						</div>

						<div class="flex items-center gap-2 shrink-0">
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								:disabled="idx === 0"
								@click="store.moveNode(node.nodeId, 'up')"
								>↑</Button
							>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								:disabled="idx === store.pipeline.nodes.length - 1"
								@click="store.moveNode(node.nodeId, 'down')"
								>↓</Button
							>

							<span class="text-xs text-muted-foreground select-none"
								>跳过</span
							>
							<Switch
								:model-value="!node.enabled"
								@update:model-value="node.enabled = !$event"
							/>

							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-destructive hover:text-destructive"
								@click="store.removeNode(node.nodeId)"
								>✕</Button
							>
						</div>
					</div>

					<div
						v-if="info?.params.length"
						class="mt-3 grid grid-cols-2 gap-x-4 gap-y-3"
					>
						<template
							v-for="param in info.params"
							:key="`${node.nodeId}__${param.key}`"
						>
							<div class="flex flex-col gap-1.5">
								<Label class="text-xs">{{ param.label }}</Label>

								<Switch
									v-if="param.type === 'boolean'"
									:model-value="
										node.params[param.key] === true ||
										node.params[param.key] === 'true'
									"
									@update:model-value="node.params[param.key] = $event"
								/>

								<Select
									v-else-if="param.type === 'select'"
									:model-value="String(node.params[param.key] ?? param.default)"
									@update:model-value="
										node.params[param.key] = String($event ?? param.default)
									"
								>
									<SelectTrigger size="sm" class="w-full">
										<SelectValue />
									</SelectTrigger>
									<SelectContent>
										<SelectItem
											v-for="opt in param.options"
											:key="opt"
											:value="opt"
										>
											{{ opt }}
										</SelectItem>
									</SelectContent>
								</Select>

								<NumberField
									v-else-if="param.type === 'number'"
									:model-value="Number(node.params[param.key] ?? param.default)"
									@update:model-value="node.params[param.key] = $event ?? 0"
								>
									<NumberFieldContent>
										<NumberFieldDecrement />
										<NumberFieldInput />
										<NumberFieldIncrement />
									</NumberFieldContent>
								</NumberField>

								<Input
									v-else
									:model-value="String(node.params[param.key] ?? param.default)"
									class="h-8 text-sm"
									@update:model-value="node.params[param.key] = String($event)"
								/>
							</div>
						</template>
					</div>
				</div>

				<div
					v-if="idx < store.pipeline.nodes.length - 1"
					class="absolute left-1/2 -translate-x-1/2 w-0.5 h-6 bg-border"
					style="bottom: -24px"
				/>
			</div>
		</template>

		<div class="flex flex-col items-center gap-2">
			<div
				v-if="store.pipeline.nodes.length === 0"
				class="text-sm text-muted-foreground"
			>
				暂无后处理节点，点击下方添加
			</div>

			<Button variant="outline" size="sm" @click="openPicker">
				+ 添加模块
			</Button>
		</div>

		<Dialog :open="showPicker" @update:open="showPicker = $event">
			<DialogContent class="max-w-sm">
				<DialogHeader>
					<DialogTitle>选择模块</DialogTitle>
				</DialogHeader>
				<div class="flex flex-col gap-1 mt-1">
					<div
						v-if="availableModules.length === 0"
						class="text-sm text-muted-foreground px-2 py-4 text-center"
					>
						<template v-if="store.modules.length === 0">
							未找到模块。<br />
							<span class="text-xs"
								>请将模块可执行文件放入程序目录下的
								<code>modules/</code> 文件夹</span
							>
						</template>
						<template v-else> 所有模块已添加 </template>
					</div>
					<button
						v-for="mod in availableModules"
						:key="mod.id"
						class="flex flex-col items-start px-3 py-2.5 rounded-lg hover:bg-accent text-left transition-colors"
						@click="addModule(mod.id)"
					>
						<span class="text-sm font-medium">{{ mod.name }}</span>
						<span class="text-xs text-muted-foreground">{{
							mod.description
						}}</span>
					</button>
				</div>
			</DialogContent>
		</Dialog>

		<div
			v-if="store.pipeline.nodes.length > 0"
			class="text-xs text-muted-foreground text-center"
		>
			{{ enabledNodes.length }} 个执行节点 · 共
			{{ store.pipeline.nodes.length }} 个节点
		</div>
	</div>
</template>
