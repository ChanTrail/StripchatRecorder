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
	import { useSettingsStore, type Settings, type MouflonKeysStore } from "../stores/settings";
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
	import { useI18n } from "vue-i18n";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";

	const store = useSettingsStore();
	const { toast, confirm } = useNotify();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();

	/** 可用语言列表（从共享 store 读取，由 App.vue 统一维护）
	 * Available locales (from shared store, maintained by App.vue) */
	// 直接使用 store 的 ref，保持响应式；不能赋给普通变量（会丢失响应式）
	// Use the store's ref directly to keep reactivity; assigning to a plain variable breaks it

	async function setLocale(lang: string) {
		// 先加载消息，再切换 locale，避免 vue-i18n 在消息就绪前以 fallback 语言渲染
		// Load messages first, then switch locale to avoid vue-i18n rendering with fallback
		const { modules: moduleLocales, warning } = await loadLocaleFromServer(lang);
		locale.value = lang;
		// 将语言写入 settings 持久化 / Persist language to settings
		form.language = lang;
		await store.saveSettings({ ...form, language: lang });
		toast(t("settings.saved"), "success");
		moduleLocaleStore.setLocales(lang, moduleLocales);
		if (warning) {
			toast(t("settings.localeFileInvalid", { file: `${lang}.json` }) + ": " + warning, "warning");
		}
	}

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
		max_tmp_dir_gb: 50,
		language: "zh-CN",
		mouflon_sync_url: null,
		mouflon_sync_token: null,
		setup_done: true,
	});

	// 保存各代理字段的原始值，用于检测是否有实际变更
	// Store original values for proxy fields to detect actual changes
	const originalOutputDir = ref("");
	const originalApiProxy = ref<string | null>(null);
	const originalCdnProxy = ref<string | null>(null);
	const originalScMirror = ref<string | null>(null);
	const originalMouflonSyncUrl = ref<string | null>(null);
	const originalMouflonSyncToken = ref<string | null>(null);
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
		originalMouflonSyncUrl.value = form.mouflon_sync_url;
		originalMouflonSyncToken.value = form.mouflon_sync_token;
		await nextTick();
		initialized = true;
		await loadKeys();

		// 监听其他客户端的 Mouflon 密钥更新 / Listen for Mouflon key updates from other clients
		unlisteners.push(
			await on("mouflon-keys-updated", (payload) => {
				const store = payload as MouflonKeysStore;
				mouflonStore.value = store;
				toast(t("settings.mouflonUpdatedByOther"), "info");
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
			max_tmp_dir_gb: form.max_tmp_dir_gb,
		}),
		async () => {
			if (!initialized) return;
			await store.saveSettings({ ...form });
			toast(t("settings.saved"), "success");
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
			originalMouflonSyncUrl.value = newSettings.mouflon_sync_url;
			originalMouflonSyncToken.value = newSettings.mouflon_sync_token;
			nextTick(() => {
				initialized = true;
			});
			toast(t("settings.updatedByOther"), "info");
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
		field: "api_proxy_url" | "cdn_proxy_url" | "sc_mirror_url" | "mouflon_sync_url" | "mouflon_sync_token",
	) {
		if (!initialized) return;
		const originalMap = {
			api_proxy_url: originalApiProxy,
			cdn_proxy_url: originalCdnProxy,
			sc_mirror_url: originalScMirror,
			mouflon_sync_url: originalMouflonSyncUrl,
			mouflon_sync_token: originalMouflonSyncToken,
		};
		const original = originalMap[field];
		if (form[field] === original.value) return;
		await store.saveSettings({ ...form });
		original.value = form[field];
		toast(t("settings.saved"), "success");
	}

	/**
	 * 保存输出目录（需要用户确认，因为会影响正在进行的录制）。
	 * Save the output directory (requires user confirmation as it affects ongoing recordings).
	 */
	async function saveOutputDir() {
		if (!initialized) return;
		if (form.output_dir === originalOutputDir.value) return;
		const ok = await confirm({
			title: t("settings.outputDir.changeTitle"),
			message: t("settings.outputDir.changeMessage", { dir: form.output_dir }),
			confirmText: t("settings.outputDir.changeConfirm"),
		});
		if (ok) {
			await store.saveSettings({ ...form });
			originalOutputDir.value = form.output_dir;
			toast(t("settings.outputDir.changeDone"), "info");
		} else {
			// 用户取消时恢复原始值 / Restore original value if user cancels
			form.output_dir = originalOutputDir.value;
		}
	}

	/** Mouflon 密钥存储（含时间戳）/ Mouflon key store (with timestamps) */
	const mouflonStore = ref<MouflonKeysStore>({ keys: {}, auto_synced_at: null, manual_updated_at: null });
	/** 新密钥表单：pkey 输入值 / New key form: pkey input value */
	const newPkey = ref("");
	/** 新密钥表单：pdkey 输入值 / New key form: pdkey input value */
	const newPdkey = ref("");
	/** 密钥添加错误信息 / Key addition error message */
	const keyError = ref("");
	/** 是否正在手动同步 / Whether manual sync is in progress */
	const syncing = ref(false);

	/**
	 * 从后端加载 Mouflon 密钥列表。
	 * Load the Mouflon key list from the backend.
	 */
	async function loadKeys() {
		mouflonStore.value = await call<MouflonKeysStore>("list_mouflon_keys");
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
			keyError.value = t("settings.keyError.empty");
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

	/**
	 * 手动触发一次从同步 URL 拉取密钥。
	 * Manually trigger a key sync from the configured URL.
	 */
	async function syncKeys() {
		syncing.value = true;
		try {
			const updated = await call<boolean>("sync_mouflon_keys");
			await loadKeys();
			toast(updated ? t("settings.mouflonSyncDone") : t("settings.mouflonSyncUpToDate"), "success");
		} catch (e: any) {
			toast(t("settings.mouflonSyncFailed", { error: String(e) }), "error");
		} finally {
			syncing.value = false;
		}
	}

	/** 格式化 RFC 3339 时间戳为本地时间字符串 / Format RFC 3339 timestamp to local time string */
	function formatTs(ts: string | null): string {
		if (!ts) return t("settings.mouflonNever");
		return new Date(ts).toLocaleString();
	}
</script>

<template>
	<div class="flex flex-col gap-5 max-w-160">
		<h1 class="text-xl font-bold">{{ t("settings.title") }}</h1>

		<div v-if="store.loading" class="text-muted-foreground">{{ t("settings.loading") }}</div>

		<form v-else class="flex flex-col gap-7">
			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					{{ t("settings.sections.language") }}
				</h2>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.language.label") }}</Label>
					<RadioGroup
						:model-value="String(locale)"
						class="flex flex-row gap-4"
						@update:model-value="(v) => v && setLocale(String(v))"
					>
						<div
							v-for="loc in localesStore.locales"
							:key="loc.code"
							class="flex items-center gap-2"
						>
							<RadioGroupItem :id="`lang-${loc.code}`" :value="loc.code" />
							<Label :for="`lang-${loc.code}`" class="cursor-pointer">{{ loc.name }}</Label>
						</div>
					</RadioGroup>
				</div>
			</section>

			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					{{ t("settings.sections.recording") }}
				</h2>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.outputDir.label") }}</Label>
					<Input
						v-model="form.output_dir"
						:placeholder="t('settings.outputDir.placeholder')"
						autocomplete="off"
						@keyup.enter="saveOutputDir"
						@blur="saveOutputDir"
					/>
					<p class="text-xs text-muted-foreground">
						{{ t("settings.outputDir.hint") }}
					</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.maxConcurrent.label") }}</Label>
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
					<p class="text-xs text-muted-foreground">{{ t("settings.maxConcurrent.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.pollInterval.label") }}</Label>
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
					<Label>{{ t("settings.mergeFormat.label") }}</Label>
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
						{{ t("settings.mergeFormat.hint") }}
					</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.maxTmpDirGb.label") }}</Label>
					<NumberField
						:model-value="form.max_tmp_dir_gb"
						:min="0"
						:step="0.5"
						class="w-36"
						@update:model-value="
							(v) => v !== undefined && (form.max_tmp_dir_gb = v)
						"
					>
						<NumberFieldContent>
							<NumberFieldDecrement />
							<NumberFieldInput />
							<NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">{{ t("settings.maxTmpDirGb.hint") }}</p>
				</div>
			</section>

			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					{{ t("settings.sections.network") }}
				</h2>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.apiProxy.label") }}</Label>
					<Input
						:model-value="form.api_proxy_url ?? ''"
						:placeholder="t('settings.apiProxy.placeholder')"
						autocomplete="url"
						@update:model-value="
							form.api_proxy_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('api_proxy_url')"
						@blur="saveProxy('api_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">
						{{ t("settings.apiProxy.hint") }}
					</p>
				</div>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.scMirror.label") }}</Label>
					<Input
						:model-value="form.sc_mirror_url ?? ''"
						:placeholder="t('settings.scMirror.placeholder')"
						autocomplete="url"
						@update:model-value="
							form.sc_mirror_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('sc_mirror_url')"
						@blur="saveProxy('sc_mirror_url')"
					/>
					<p class="text-xs text-muted-foreground">
						{{ t("settings.scMirror.hint") }}
					</p>
				</div>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.cdnProxy.label") }}</Label>
					<Input
						:model-value="form.cdn_proxy_url ?? ''"
						:placeholder="t('settings.cdnProxy.placeholder')"
						autocomplete="url"
						@update:model-value="
							form.cdn_proxy_url = ($event as string) || null
						"
						@keyup.enter="saveProxy('cdn_proxy_url')"
						@blur="saveProxy('cdn_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">
						{{ t("settings.cdnProxy.hint") }}
					</p>
				</div>
			</section>

			<section class="flex flex-col gap-3.5">
				<h2
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>
					{{ t("settings.sections.mouflonKeys") }}
				</h2>
				<p class="text-xs text-muted-foreground leading-relaxed">
					{{ t("settings.mouflonKeysDesc") }}
					<code class="bg-muted px-1 py-0.5 rounded text-xs font-mono"
						>pkey → pdkey</code
					>
				</p>

				<!-- 同步配置 / Sync configuration -->
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.mouflonSyncUrl.label") }}</Label>
					<Input
						:model-value="form.mouflon_sync_url ?? ''"
						:placeholder="t('settings.mouflonSyncUrl.placeholder')"
						autocomplete="url"
						@update:model-value="form.mouflon_sync_url = ($event as string) || null"
						@keyup.enter="saveProxy('mouflon_sync_url')"
						@blur="saveProxy('mouflon_sync_url')"
					/>
				</div>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.mouflonSyncToken.label") }}</Label>
					<Input
						:model-value="form.mouflon_sync_token ?? ''"
						:placeholder="t('settings.mouflonSyncToken.placeholder')"
						type="password"
						autocomplete="current-password"
						@update:model-value="form.mouflon_sync_token = ($event as string) || null"
						@keyup.enter="saveProxy('mouflon_sync_token')"
						@blur="saveProxy('mouflon_sync_token')"
					/>
				</div>

				<!-- 同步状态 + 手动同步按钮 / Sync status + manual sync button -->
				<div class="flex items-center justify-between gap-4 text-xs text-muted-foreground">
					<div class="flex flex-col gap-0.5">
						<span>{{ t("settings.mouflonAutoSyncedAt") }}{{ formatTs(mouflonStore.auto_synced_at) }}</span>
						<span>{{ t("settings.mouflonManualUpdatedAt") }}{{ formatTs(mouflonStore.manual_updated_at) }}</span>
					</div>
					<Button
						type="button"
						variant="outline"
						size="sm"
						:disabled="syncing || !form.mouflon_sync_url"
						@click="syncKeys"
					>
						{{ syncing ? t("settings.mouflonSyncing") : t("settings.mouflonSync") }}
					</Button>
				</div>

				<table
					v-if="Object.keys(mouflonStore.keys).length"
					class="w-full text-xs border-collapse"
				>
					<thead>
						<tr>
							<th
								class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold"
							>
								{{ t("settings.mouflonTable.pkey") }}
							</th>
							<th
								class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold"
							>
								{{ t("settings.mouflonTable.pdkey") }}
							</th>
							<th class="border-b"></th>
						</tr>
					</thead>
					<tbody>
						<tr v-for="(pdkey, pkey) in mouflonStore.keys" :key="pkey">
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
									{{ t("common.delete") }}
								</Button>
							</td>
						</tr>
					</tbody>
				</table>
				<p v-else class="text-xs text-muted-foreground">{{ t("settings.noKeys") }}</p>

				<div class="flex gap-2 items-center">
					<Input
						v-model="newPkey"
						placeholder="pkey"
						autocomplete="off"
						class="flex-1 font-mono text-xs"
					/>
					<Input
						v-model="newPdkey"
						placeholder="pdkey"
						autocomplete="off"
						class="flex-2 font-mono text-xs"
					/>
					<Button type="button" variant="outline" @click="addKey">{{ t("settings.addKey") }}</Button>
				</div>
				<p v-if="keyError" class="text-xs text-destructive">{{ keyError }}</p>
			</section>
		</form>
	</div>
</template>

