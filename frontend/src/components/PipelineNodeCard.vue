<!--
    流水线节点卡片组件 / Pipeline Node Card Component

    在节点图编辑器画布中展示单个常规节点（模块实例）：节点头部（名称/徽章/启用开关）、
    输入端口列表、参数内联编辑区、输出端口列表。不涉及拖拽/连线的底层逻辑，
    仅通过 emits 将端口鼠标事件转发给父组件（父组件持有 usePortWiring/useNodeDragging 状态）。

    Displays a single regular node (module instance) in the node graph editor canvas:
    node header (name/badges/enable switch), input port list, inline parameter editor,
    output port list. Does not implement drag/wiring logic itself; forwards port mouse
    events to the parent via emits (parent owns usePortWiring/useNodeDragging state).

    Props:
        node            - 流水线节点数据 / Pipeline node data
        moduleInfo      - 对应的模块信息（未找到时为 undefined）/ Corresponding module info (undefined if not found)
        inputTypes      - 输入端口类型列表 / Input port types
        outputTypes     - 输出端口类型列表 / Output port types
        selected        - 是否被选中 / Whether the node is selected
        registerPortEl  - 端口 DOM 元素注册函数（来自 usePortWiring）/ Port DOM registration function (from usePortWiring)

    Emits:
        select            - 点击节点主体（选中）/ Click on node body (select)
        header-mousedown  - 在头部按下鼠标（开始拖拽）/ Mouse down on header (start drag)
        toggle-enabled    - 切换启用开关 / Toggle enable switch
        update-param      - 更新某个参数值 / Update a parameter value
        port-mousedown    - 在端口按下鼠标 / Mouse down on a port
        port-mouseup      - 在端口释放鼠标 / Mouse up on a port
-->
<script setup lang="ts">
	import type { PipelineNode, ModuleInfo, PortType } from "@/stores/postprocess";
	import { nodeEffectiveId, usePostprocessStore } from "@/stores/postprocess";
	import { PORT_TYPE_COLORS } from "@/stores/postprocess";
	import { Switch } from "@/components/ui/switch";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import { Badge } from "@/components/ui/badge";
	import { Button } from "@/components/ui/button";
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
	import { useDirectoryBrowser } from "@/composables/useDirectoryBrowser";
	import { FolderOpen } from "@lucide/vue";
	import { useI18n } from "vue-i18n";

	const props = defineProps<{
		node: PipelineNode;
		moduleInfo: ModuleInfo | undefined;
		inputTypes: PortType[];
		outputTypes: PortType[];
		selected: boolean;
		registerPortEl: (el: HTMLElement | null, nodeId: string, isOutput: boolean, portIndex: number) => void;
	}>();

	const emit = defineEmits<{
		select: [];
		"header-mousedown": [e: MouseEvent];
		"toggle-enabled": [enabled: boolean];
		"update-param": [key: string, value: string | number | boolean];
		"port-mousedown": [e: MouseEvent, portIndex: number, type: PortType, isOutput: boolean];
		"port-mouseup": [e: MouseEvent, portIndex: number, type: PortType, isOutput: boolean];
	}>();

	const { t } = useI18n();
	const store = usePostprocessStore();

	function eid() {
		return nodeEffectiveId(props.node);
	}

	/** 输入端口 i 是否已有连线（来自本节点的 inputs 映射）/ Whether input port i has a wire (from this node's inputs map) */
	function isInputConnected(portIndex: number): boolean {
		return props.node.inputs?.[String(portIndex)] !== undefined;
	}

	/**
	 * 输出端口 i 是否已有连线：需要遍历流水线所有节点的 inputs，
	 * 查找是否有节点的某个输入端口指向"本节点 + 该输出端口"。
	 *
	 * Whether output port i has a wire: requires scanning every node's inputs in the
	 * pipeline to find whether any input port points at "this node + this output port".
	 */
	function isOutputConnected(portIndex: number): boolean {
		const selfId = eid();
		return store.pipeline.nodes.some((n) =>
			Object.values(n.inputs ?? {}).some(
				(ref) => ref.nodeId === selfId && ref.port === portIndex,
			),
		);
	}

	const { open: openDirectoryBrowser } = useDirectoryBrowser();

	/**
	 * 打开全局目录浏览器，选择结果写回指定参数。
	 * Open the global directory browser; the chosen path is written back to the given parameter.
	 */
	function browseDirParam(key: string) {
		const current = String(props.node.params[key] ?? "");
		openDirectoryBrowser(current, (picked) => {
			emit("update-param", key, picked);
		});
	}
</script>

<template>
	<div
		class="pipeline-node absolute"
		:style="{
			left: `${node.position?.x ?? 0}px`,
			top: `${node.position?.y ?? 0}px`,
			zIndex: selected ? 10 : 1,
		}"
		@mousedown.stop
		@click.stop="emit('select')"
	>
		<div
			class="rounded-xl border bg-card shadow-xl w-96 transition-colors"
			:class="[
				!node.enabled && 'opacity-50',
				selected ? 'border-primary' : 'border-white/10',
			]"
		>
			<!-- 节点头部 / Node header -->
			<div
				class="flex items-center gap-2 px-3 py-2 border-b border-white/5 cursor-move"
				@mousedown.stop="emit('header-mousedown', $event)"
			>
				<div class="flex-1 min-w-0">
					<div class="flex items-center gap-1.5 flex-wrap">
						<span class="text-xs font-semibold leading-none truncate">
							{{ moduleInfo?.name ?? node.moduleId }}
						</span>
						<span
							v-if="moduleInfo?.version"
							class="text-[9px] text-muted-foreground font-mono leading-none shrink-0"
						>v{{ moduleInfo.version }}</span>
						<Badge
							v-if="moduleInfo?.official"
							class="text-[9px] px-1 py-0 h-4 bg-amber-500/20 text-amber-400 border-amber-500/30"
						>official</Badge>
						<Badge
							v-if="!moduleInfo"
							variant="destructive"
							class="text-[9px] px-1 py-0 h-4"
						>{{ t("postprocess.node.missing") }}</Badge>
						<Badge
							v-else-if="!node.enabled"
							variant="secondary"
							class="text-[9px] px-1 py-0 h-4"
						>{{ t("postprocess.node.skipped") }}</Badge>
					</div>
					<p
						class="text-[10px] text-muted-foreground leading-none mt-0.5 truncate"
						:title="moduleInfo?.description"
					>
						{{ moduleInfo?.description }}
					</p>
				</div>
				<div class="flex items-center gap-1 shrink-0">
					<Switch
						:id="`enable-${eid()}`"
						:model-value="node.enabled"
						class="scale-75"
						@update:model-value="emit('toggle-enabled', !!$event)"
						@click.stop
					/>
				</div>
			</div>

			<!-- 端口区域 / Ports area -->
			<div class="flex gap-2 px-3 py-2">
				<!-- 输入端口 / Input ports -->
				<div class="flex flex-col gap-2 items-start shrink-0">
					<div
						v-for="(type, i) in inputTypes"
						:key="`in-${i}`"
						class="flex items-center gap-1.5"
					>
						<div
							:ref="(el) => registerPortEl(el as HTMLElement | null, eid(), false, i)"
							class="w-3 h-3 rounded-full border-2 cursor-crosshair -ml-4.5 shrink-0 transition-transform hover:scale-125"
							:style="{
								borderColor: PORT_TYPE_COLORS[type],
								backgroundColor: isInputConnected(i) ? PORT_TYPE_COLORS[type] : PORT_TYPE_COLORS[type] + '40',
							}"
							:title="type"
							@mousedown.stop="emit('port-mousedown', $event, i, type, false)"
							@mouseup.stop="emit('port-mouseup', $event, i, type, false)"
						/>
						<span class="text-[9px] text-muted-foreground whitespace-nowrap">{{ type.replace(/_/g, ' ') }}</span>
					</div>
				</div>

				<!-- 参数区域 / Parameters area -->
				<div class="flex-1 min-w-0">
					<div v-if="moduleInfo?.params.length" class="flex flex-col gap-2.5">
						<div
							v-for="param in moduleInfo.params"
							:key="`${eid()}__${param.key}`"
							class="flex flex-col gap-1"
						>
							<Label class="text-[10px] text-muted-foreground leading-tight">{{ param.label }}</Label>
							<Switch
								v-if="param.type === 'boolean'"
								:model-value="node.params[param.key] === true || node.params[param.key] === 'true'"
								class="scale-75 origin-left"
								@update:model-value="emit('update-param', param.key, !!$event)"
								@click.stop
							/>
							<Select
								v-else-if="param.type === 'select'"
								:model-value="String(node.params[param.key] ?? param.default)"
								@update:model-value="emit('update-param', param.key, String($event ?? param.default))"
							>
								<SelectTrigger size="sm" class="h-7 text-xs w-full" @click.stop>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									<SelectItem v-for="opt in param.options" :key="opt" :value="opt" class="text-xs">
										{{ opt }}
									</SelectItem>
								</SelectContent>
							</Select>
							<NumberField
								v-else-if="param.type === 'number'"
								:model-value="Number(node.params[param.key] ?? param.default)"
								@update:model-value="emit('update-param', param.key, $event ?? 0)"
							>
								<NumberFieldContent>
									<NumberFieldDecrement class="h-7" />
									<NumberFieldInput class="h-7 text-xs" @click.stop />
									<NumberFieldIncrement class="h-7" />
								</NumberFieldContent>
							</NumberField>
							<!-- 目录类参数：输入框 + 浏览按钮，点击按钮打开服务器端目录浏览器
							     Directory-type param: input + browse button, opens the server-side directory browser -->
							<div v-else-if="param.type === 'dir'" class="flex items-center gap-1">
								<Input
									:model-value="String(node.params[param.key] ?? param.default)"
									class="h-7 text-xs flex-1 min-w-0"
									@update:model-value="emit('update-param', param.key, String($event))"
									@click.stop
								/>
								<Button
									variant="outline"
									size="icon"
									class="h-7 w-7 shrink-0"
									:title="t('settings.outputDir.pick')"
									@click.stop="browseDirParam(param.key)"
								>
									<FolderOpen class="size-3.5" />
								</Button>
							</div>
							<Input
								v-else
								:model-value="String(node.params[param.key] ?? param.default)"
								class="h-7 text-xs w-full"
								@update:model-value="emit('update-param', param.key, String($event))"
								@click.stop
							/>
						</div>
					</div>
				</div>

				<!-- 输出端口 / Output ports -->
				<div class="flex flex-col gap-2 items-end shrink-0">
					<div
						v-for="(type, i) in outputTypes"
						:key="`out-${i}`"
						class="flex items-center gap-1.5"
					>
						<span class="text-[9px] text-muted-foreground whitespace-nowrap">{{ type.replace(/_/g, ' ') }}</span>
						<div
							:ref="(el) => registerPortEl(el as HTMLElement | null, eid(), true, i)"
							class="w-3 h-3 rounded-full border-2 cursor-crosshair -mr-4.5 shrink-0 transition-transform hover:scale-125"
							:style="{
								borderColor: PORT_TYPE_COLORS[type],
								backgroundColor: isOutputConnected(i) ? PORT_TYPE_COLORS[type] : PORT_TYPE_COLORS[type] + '40',
							}"
							:title="type"
							@mousedown.stop="emit('port-mousedown', $event, i, type, true)"
							@mouseup.stop="emit('port-mouseup', $event, i, type, true)"
						/>
					</div>
				</div>
			</div>

			<!-- official 提示已移至底部信息栏 / Official hint moved to bottom info bar -->
		</div>
	</div>
</template>
