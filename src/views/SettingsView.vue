<!--
    应用设置页面 / Application Settings View

    提供录制器的全局配置界面，包括：
    - 录制输出目录（支持系统目录选择器）
    - 最大并发录制数、轮询间隔、合并格式
    - 网络代理：API 代理、Stripchat 镜像站、CDN 代理
    - Mouflon HLS 解密密钥管理（pkey/pdkey 对）

    大多数设置在失焦或按回车时自动保存；
    部分设置（轮询间隔、并发数、合并格式）通过 watch 自动保存。
    支持多客户端实时同步：其他客户端修改设置时自动更新表单。

    Provides global recorder configuration UI including:
    - Recording output directory (with system directory picker)
    - Max concurrent recordings, poll interval, merge format
    - Network proxies: API proxy, Stripchat mirror, CDN proxy
    - Mouflon HLS decryption key management (pkey/pdkey pairs)

    Most settings auto-save on blur or Enter key;
    some settings (poll interval, concurrency, merge format) auto-save via watch.
    Supports real-time multi-client sync: form updates when another client changes settings.
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, reactive, ref, watch, nextTick } from "vue";
	import { call, on } from "@/lib/api";
	import { useSettingsStore, type Settings } from "../stores/settings";
	import { useNotify } from "../composables/useNotify";
	import { Button } from "@/components/ui/button";
	import { Input } from "@/components/ui/input";
	import { Label } from "@/components/ui/label";
	import {
		NumberField,
		NumberFieldContent,
		NumberFieldDecrement,
		NumberFieldIncrement,
		NumberFieldInput,
	} from "@/components/ui/number-field";
	import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";

	const store = useSettingsStore();
	const { toast, confirm } = useNotify();

	/** 表单响应式数据（与 store.settings 保持同步）/ Reactive form data (synced with store.settings) */
	const form = reactive<Settings>({
		output_dir: "",
		poll_interval_secs: 30,
		auto_record: true,
		api_proxy_url: null,
		cdn_proxy_url: null,
		sc_mirror_url: null,
		max_concurrent: 0,
		merge_format: "mp4",
	});

	// 保存各代理字段的原始值，用于检测是否有实际变更
	// Store original values for proxy fields to detect actual changes
	const originalOutputDir = ref("");
	const originalApiProxy = ref<string | null>(null);
	const originalCdnProxy = ref<string | null>(null);
	const originalScMirror = ref<string | null>(null);
	/** 是否已完成初始化（防止初始化时触发自动保存）/ Whether initialization is complete (prevents auto-save during init) */
	let initialized = false;

	const unlisteners: (() => void)[] = [];

	onMounted(async () => {
		await store.initListeners();
		await store.fetchSettings();
		Object.assign(form, store.settings);
		originalOutputDir.value = form.output_dir;
		originalApiProxy.value = form.api_proxy_url;
		originalCdnProxy.value = form.cdn_proxy_url;
		originalScMirror.value = form.sc_mirror_url;
		await nextTick();
		initialized = true;
		await loadKeys();

		// 监听其他客户端的 Mouflon 密钥更新 / Listen for Mouflon key updates from other clients
		unlisteners.push(
			await on("mouflon-keys-updated", (payload) => {
				mouflonKeys.value = payload as Record<string, string>;
				toast("Mouflon Keys 已由其他客户端更新", "info");
			}),
		);
	});

	onUnmounted(() => {
		unlisteners.forEach((fn) => fn());
	});

	// 监听不需要确认的设置字段，变更时自动保存
	// Watch settings fields that don't require confirmation, auto-save on change
	watch(
		() => ({
			poll_interval_secs: form.poll_interval_secs,
			auto_record: form.auto_record,
			max_concurrent: form.max_concurrent,
			merge_format: form.merge_format,
		}),
		async () => {
			if (!initialized) return;
			await store.saveSettings({ ...form });
			toast("设置已保存", "success");
		},
		{ deep: true },
	);

	// 监听 store.settings 变化（来自其他客户端），同步到表单
	// Watch store.settings changes (from other clients) and sync to form
	watch(
		() => store.settings,
		(newSettings) => {
			if (!initialized || store.isSavingLocally) return;
			initialized = false;
			Object.assign(form, newSettings);
			originalOutputDir.value = newSettings.output_dir;
			originalApiProxy.value = newSettings.api_proxy_url;
			originalCdnProxy.value = newSettings.cdn_proxy_url;
			originalScMirror.value = newSettings.sc_mirror_url;
			nextTick(() => {
				initialized = true;
			});
			toast("设置已由其他客户端更新", "info");
		},
		{ deep: true },
	);

	/**
	 * 保存代理相关字段（仅在值有实际变更时保存）。
	 * Save a proxy-related field (only if the value actually changed).
	 *
	 * @param field - 要保存的设置字段名 / Settings field name to save
	 */
	async function saveProxy(
		field: "api_proxy_url" | "cdn_proxy_url" | "sc_mirror_url",
	) {
		if (!initialized) return;
		const original =
			field === "api_proxy_url"
				? originalApiProxy
				: field === "cdn_proxy_url"
					? originalCdnProxy
					: originalScMirror;
		if (form[field] === original.value) return;
		await store.saveSettings({ ...form });
		original.value = form[field];
		toast("设置已保存", "success");
	}

	/**
	 * 保存输出目录（需要用户确认，因为会影响正在进行的录制）。
	 * Save the output directory (requires user confirmation as it affects ongoing recordings).
	 */
	async function saveOutputDir() {
		if (!initialized) return;
		if (form.output_dir === originalOutputDir.value) return;
		const ok = await confirm({
			title: "修改输出目录",
			message: `将输出目录改为：\n${form.output_dir}\n\n此更改将在下次录制时生效。`,
			confirmText: "确认",
		});
		if (ok) {
			await store.saveSettings({ ...form });
			originalOutputDir.value = form.output_dir;
			toast("输出目录已更新，将在下次录制时生效", "info");
		} else {
			// 用户取消时恢复原始值 / Restore original value if user cancels
			form.output_dir = originalOutputDir.value;
		}
	}

	/**
	 * 打开系统目录选择器并保存选择的目录。
	 * Open the system directory picker and save the selected directory.
	 */
	async function pickDir() {
		const dir = await store.pickOutputDir();
		if (dir) {
			form.output_dir = dir;
			await saveOutputDir();
		}
	}

	/** Mouflon 解密密钥列表（pkey -> pdkey）/ Mouflon decryption key list (pkey -> pdkey) */
	const mouflonKeys = ref<Record<string, string>>({});
	/** 新密钥表单：pkey 输入值 / New key form: pkey input value */
	const newPkey = ref("");
	/** 新密钥表单：pdkey 输入值 / New key form: pdkey input value */
	const newPdkey = ref("");
	/** 密钥添加错误信息 / Key addition error message */
	const keyError = ref("");

	/**
	 * 从后端加载 Mouflon 密钥列表。
	 * Load the Mouflon key list from the backend.
	 */
	async function loadKeys() {
		mouflonKeys.value = await call<Record<string, string>>("list_mouflon_keys");
	}

	/**
	 * 添加新的 Mouflon 密钥对。
	 * Add a new Mouflon key pair.
	 */
	async function addKey() {
		keyError.value = "";
		const pkey = newPkey.value.trim();
		const pdkey = newPdkey.value.trim();
		if (!pkey || !pdkey) {
			keyError.value = "pkey 和 pdkey 均不能为空";
			return;
		}
		try {
			await call("add_mouflon_key", { pkey, pdkey });
			newPkey.value = "";
			newPdkey.value = "";
			await loadKeys();
		} catch (e: any) {
			keyError.value = String(e);
		}
	}

	/**
	 * 删除指定的 Mouflon 密钥。
	 * Remove a specific Mouflon key.
	 *
	 * @param pkey - 要删除的密钥标识符 / Key identifier to remove
	 */
	async function removeKey(pkey: string) {
		await call("remove_mouflon_key", { pkey });
		await loadKeys();
	}
</script>

<template>
	<div class="flex flex-col gap-5 max-w-160">
		<h1 class="text-xl font-bold">设置</h1>

		<div v-if="store.loading" class="text-muted-foreground">加载中...</div>

		<form v-else class="flex flex-col gap-7">
			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					录制
				</h2>

				<div class="flex flex-col gap-1.5">
					<Label>输出目录</Label>
					<div class="flex gap-2">
						<Input
							v-model="form.output_dir"
							placeholder="/path/to/recordings"
							@keyup.enter="saveOutputDir"
							@blur="saveOutputDir"
						/>
						<Button
							type="button"
							variant="outline"
							class="shrink-0"
							@click="pickDir"
						>
							选择
						</Button>
					</div>
					<p class="text-xs text-muted-foreground">
						修改后按回车或点击其他区域确认
					</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>最大并发录制数</Label>
					<NumberField
						:model-value="form.max_concurrent"
						:min="0"
						:max="50"
						class="w-32"
						@update:model-value="
							(v) => v !== undefined && (form.max_concurrent = v)
						"
					>
						<NumberFieldContent>
							<NumberFieldDecrement />
							<NumberFieldInput />
							<NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">0 表示不限制</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>轮询间隔（秒）</Label>
					<NumberField
						:model-value="form.poll_interval_secs"
						:min="10"
						:max="300"
						class="w-32"
						@update:model-value="
							(v) => v !== undefined && (form.poll_interval_secs = v)
						"
					>
						<NumberFieldContent>
							<NumberFieldDecrement />
							<NumberFieldInput />
							<NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>合并格式</Label>
					<RadioGroup
						:model-value="form.merge_format"
						class="flex flex-row gap-4"
						@update:model-value="(v) => v && (form.merge_format = v as string)"
					>
						<div
							v-for="fmt in ['mp4', 'mkv', 'ts']"
							:key="fmt"
							class="flex items-center gap-2"
						>
							<RadioGroupItem :id="`fmt-${fmt}`" :value="fmt" />
							<Label :for="`fmt-${fmt}`" class="font-mono cursor-pointer">{{
								fmt
							}}</Label>
						</div>
					</RadioGroup>
					<p class="text-xs text-muted-foreground">
						录制结束后自动合并分片为单一文件的格式
					</p>
				</div>
			</section>

			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					网络
				</h2>
				<div class="flex flex-col gap-1.5">
					<Label>API 代理（访问 stripchat.com，留空不使用）</Label>
					<Input
						:model-value="form.api_proxy_url ?? ''"
						placeholder="socks5://127.0.0.1:10808"
						@update:model-value="
							form.api_proxy_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('api_proxy_url')"
						@blur="saveProxy('api_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">
						修改后按回车或点击其他区域确认
					</p>
				</div>
				<div class="flex flex-col gap-1.5">
					<Label
						>Stripchat 镜像站（替换 API 中的 stripchat.com
						域名，留空不使用）</Label
					>
					<Input
						:model-value="form.sc_mirror_url ?? ''"
						placeholder="stripchat.example.com"
						@update:model-value="
							form.sc_mirror_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('sc_mirror_url')"
						@blur="saveProxy('sc_mirror_url')"
					/>
					<p class="text-xs text-muted-foreground">
						同时填写 API 代理与镜像站时，将通过 API 代理访问镜像站
					</p>
				</div>
				<div class="flex flex-col gap-1.5">
					<Label>CDN 代理（下载 HLS 分片，留空不使用）</Label>
					<Input
						:model-value="form.cdn_proxy_url ?? ''"
						placeholder="socks5://127.0.0.1:10808"
						@update:model-value="
							form.cdn_proxy_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('cdn_proxy_url')"
						@blur="saveProxy('cdn_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">
						修改后按回车或点击其他区域确认
					</p>
				</div>
			</section>

			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					Mouflon 解密密钥
				</h2>
				<p class="text-xs text-muted-foreground leading-relaxed">
					Stripchat 对 HLS 分片文件名进行了加密（Mouflon
					系统）。录制前需在此填入对应的
					<code class="bg-muted px-1 py-0.5 rounded text-xs font-mono"
						>pkey → pdkey</code
					>
					密钥对，密钥可从社区渠道获取。
				</p>

				<table
					v-if="Object.keys(mouflonKeys).length"
					class="w-full text-xs border-collapse"
				>
					<thead>
						<tr>
							<th
								class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold"
							>
								pkey（密钥标识符）
							</th>
							<th
								class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold"
							>
								pdkey（解密密钥）
							</th>
							<th class="border-b"></th>
						</tr>
					</thead>
					<tbody>
						<tr v-for="(pdkey, pkey) in mouflonKeys" :key="pkey">
							<td class="px-2 py-1.5 border-b font-mono">{{ pkey }}</td>
							<td class="px-2 py-1.5 border-b font-mono max-w-60 truncate">
								{{ pdkey }}
							</td>
							<td class="px-2 py-1.5 border-b">
								<Button
									type="button"
									variant="destructive"
									size="sm"
									class="h-6 text-xs px-2"
									@click="removeKey(pkey)"
								>
									删除
								</Button>
							</td>
						</tr>
					</tbody>
				</table>
				<p v-else class="text-xs text-muted-foreground">暂无密钥</p>

				<div class="flex gap-2 items-center">
					<Input
						v-model="newPkey"
						placeholder="pkey"
						class="flex-1 font-mono text-xs"
					/>
					<Input
						v-model="newPdkey"
						placeholder="pdkey"
						class="flex-2 font-mono text-xs"
					/>
					<Button type="button" variant="outline" @click="addKey">添加</Button>
				</div>
				<p v-if="keyError" class="text-xs text-destructive">{{ keyError }}</p>
			</section>
		</form>
	</div>
</template>

