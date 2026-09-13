<!--
    应用设置页面 / Application Settings View

    与 frontend 版对齐，新增：
    - 目录浏览器按钮（useDirectoryBrowser）
    - sc_mirror_scheme 协议选择
    - preferred_resolution / resolution_preference
    - max_recording_duration_secs
    - max_pp_concurrent（SystemStore 动态上限）
    - max_concurrent（SystemStore 动态上限）
    - community_proxy_url / community_mirror_url
    - showToken 眼睛图标
    - sticky header + section observer
-->
<script setup lang="ts">
	import { onMounted, onUnmounted, reactive, ref, watch, nextTick } from "vue";
	import { call, on } from "@/lib/api";
	import { useSettingsStore, type Settings, type MouflonKeysStore } from "../stores/settings";
	import { useSystemStore } from "../stores/system";
	import { useNotify } from "../composables/useNotify";
	import { useDirectoryBrowser } from "@/composables/useDirectoryBrowser";
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
	import { useI18n } from "vue-i18n";
	import { loadLocaleFromServer } from "@/i18n";
	import { useModuleLocaleStore } from "@/stores/moduleLocale";
	import { useLocalesStore } from "@/stores/locales";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
	} from "@/components/ui/select";
	import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
	import { FolderOpen, Eye, EyeOff } from "@lucide/vue";

	const store = useSettingsStore();
	const systemStore = useSystemStore();
	const { toast, confirm } = useNotify();
	const { t, locale } = useI18n();
	const moduleLocaleStore = useModuleLocaleStore();
	const localesStore = useLocalesStore();

	const resolutionDirections = ["lower", "higher"] as const;

	async function setLocale(lang: string) {
		const { modules: moduleLocales, warning } = await loadLocaleFromServer(lang);
		locale.value = lang;
		localStorage.setItem("locale", lang);
		form.language = lang;
		await store.saveSettings({ ...form, language: lang });
		toast(t("settings.saved"), "success");
		moduleLocaleStore.setLocales(lang, moduleLocales);
		if (warning) {
			toast(t("settings.localeFileInvalid", { file: `${lang}.json` }) + ": " + warning, "warning");
		}
	}

	const form = reactive<Settings>({
		output_dir: "",
		poll_interval_secs: 30,
		auto_record: true,
		api_proxy_url: null,
		cdn_proxy_url: null,
		sc_mirror_url: null,
		sc_mirror_scheme: "https",
		max_concurrent: 0,
		max_recording_duration_secs: 0,
		preferred_resolution: 0,
		resolution_preference: "lower" as "lower" | "higher",
		max_tmp_dir_gb: 50,
		language: "zh-CN",
		mouflon_sync_url: null,
		mouflon_sync_token: null,
		community_proxy_url: null,
		community_mirror_url: null,
		community_terms_accepted: false,
		setup_done: true,
		max_pp_concurrent: 0,
	});

	const originalOutputDir = ref("");
	const originalApiProxy = ref<string | null>(null);
	const originalCdnProxy = ref<string | null>(null);
	const originalScMirror = ref<string | null>(null);
	const originalMouflonSyncUrl = ref<string | null>(null);
	const originalMouflonSyncToken = ref<string | null>(null);
	const originalCommunityProxy = ref<string | null>(null);
	const originalCommunityMirror = ref<string | null>(null);
	let initialized = false;

	const unlisteners: (() => void)[] = [];

	onMounted(async () => {
		await store.initListeners();
		await Promise.all([store.fetchSettings(), systemStore.fetchSystemInfo()]);
		Object.assign(form, store.settings);
		originalOutputDir.value = form.output_dir;
		originalApiProxy.value = form.api_proxy_url;
		originalCdnProxy.value = form.cdn_proxy_url;
		originalScMirror.value = form.sc_mirror_url;
		originalMouflonSyncUrl.value = form.mouflon_sync_url;
		originalMouflonSyncToken.value = form.mouflon_sync_token;
		originalCommunityProxy.value = form.community_proxy_url;
		originalCommunityMirror.value = form.community_mirror_url;
		await nextTick();
		initialized = true;
		await loadKeys();

		unlisteners.push(
			await on("mouflon-keys-updated", (payload) => {
				const s = payload as MouflonKeysStore;
				mouflonStore.value = s;
				toast(t("settings.mouflonUpdatedByOther"), "info");
			}),
		);
	});

	onUnmounted(() => {
		unlisteners.forEach((fn) => fn());
		sectionObserver?.disconnect();
	});

	watch(
		() => ({
			poll_interval_secs: form.poll_interval_secs,
			auto_record: form.auto_record,
			max_concurrent: form.max_concurrent,
			max_recording_duration_secs: form.max_recording_duration_secs,
			preferred_resolution: form.preferred_resolution,
			resolution_preference: form.resolution_preference,
			max_tmp_dir_gb: form.max_tmp_dir_gb,
			sc_mirror_scheme: form.sc_mirror_scheme,
			max_pp_concurrent: form.max_pp_concurrent,
		}),
		async () => {
			if (!initialized) return;
			await store.saveSettings({ ...form });
			toast(t("settings.saved"), "success");
		},
		{ deep: true },
	);

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
			originalCommunityProxy.value = newSettings.community_proxy_url;
			originalCommunityMirror.value = newSettings.community_mirror_url;
			nextTick(() => { initialized = true; });
			toast(t("settings.updatedByOther"), "info");
		},
		{ deep: true },
	);

	async function saveProxy(
		field: "api_proxy_url" | "cdn_proxy_url" | "sc_mirror_url" | "mouflon_sync_url" | "mouflon_sync_token" | "community_proxy_url" | "community_mirror_url",
	) {
		if (!initialized) return;
		const originalMap = {
			api_proxy_url: originalApiProxy,
			cdn_proxy_url: originalCdnProxy,
			sc_mirror_url: originalScMirror,
			mouflon_sync_url: originalMouflonSyncUrl,
			mouflon_sync_token: originalMouflonSyncToken,
			community_proxy_url: originalCommunityProxy,
			community_mirror_url: originalCommunityMirror,
		};
		const original = originalMap[field];
		if (form[field] === original.value) return;
		await store.saveSettings({ ...form });
		original.value = form[field];
		toast(t("settings.saved"), "success");
	}

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
			form.output_dir = originalOutputDir.value;
		}
	}

	const { open: openDirectoryBrowser } = useDirectoryBrowser();

	function browseOutputDir() {
		openDirectoryBrowser(form.output_dir, (picked) => {
			form.output_dir = picked;
			saveOutputDir();
		});
	}

	const mouflonStore = ref<MouflonKeysStore>({ keys: {}, auto_synced_at: null, manual_updated_at: null });
	const newPkey = ref("");
	const newPdkey = ref("");
	const keyError = ref("");
	const syncing = ref(false);
	const showToken = ref(false);

	async function loadKeys() {
		mouflonStore.value = await call<MouflonKeysStore>("list_mouflon_keys");
	}

	async function addKey() {
		keyError.value = "";
		const pkey = newPkey.value.trim();
		const pdkey = newPdkey.value.trim();
		if (!pkey || !pdkey) { keyError.value = t("settings.keyError.empty"); return; }
		try {
			await call("add_mouflon_key", { pkey, pdkey });
			newPkey.value = "";
			newPdkey.value = "";
			await loadKeys();
		} catch (e: unknown) {
			keyError.value = String(e);
		}
	}

	async function removeKey(pkey: string) {
		await call("remove_mouflon_key", { pkey });
		await loadKeys();
	}

	async function syncKeys() {
		syncing.value = true;
		try {
			const updated = await call<boolean>("sync_mouflon_keys");
			await loadKeys();
			toast(updated ? t("settings.mouflonSyncDone") : t("settings.mouflonSyncUpToDate"), "success");
		} catch (e: unknown) {
			toast(t("settings.mouflonSyncFailed", { error: String(e) }), "error");
		} finally {
			syncing.value = false;
		}
	}

	function formatTs(ts: string | null): string {
		if (!ts) return t("settings.mouflonNever");
		return new Date(ts).toLocaleString();
	}

	// ── section 追踪 / Section tracking ──────────────────────────────────────
	const SECTION_KEYS = ["language", "recording", "network", "mouflonKeys"] as const;
	type SectionKey = (typeof SECTION_KEYS)[number];
	const activeSection = ref<SectionKey | null>(null);
	const sectionRefs = ref<Partial<Record<SectionKey, HTMLElement>>>({});
	let sectionObserver: IntersectionObserver | null = null;

	function initSectionObserver() {
		if (sectionObserver) sectionObserver.disconnect();
		sectionObserver = new IntersectionObserver(
			(entries) => {
				for (const entry of entries) {
					const key = entry.target.getAttribute("data-section") as SectionKey;
					if (entry.isIntersecting) activeSection.value = key;
				}
			},
			{ rootMargin: "-60px 0px -80% 0px", threshold: 0 },
		);
		for (const key of SECTION_KEYS) {
			const el = sectionRefs.value[key];
			if (el) sectionObserver.observe(el);
		}
	}

	watch(() => store.loading, (loading) => {
		if (!loading) nextTick(initSectionObserver);
	}, { immediate: true });
</script>

<template>
	<div class="flex flex-col">
		<!-- sticky 标题区 / Sticky header -->
		<header class="bg-background sticky top-0 z-20 px-6 border-b shrink-0 pt-6 pb-3">
			<h1 class="text-xl font-bold mb-0.5">{{ t("settings.title") }}</h1>
			<p class="text-sm text-muted-foreground h-5 relative overflow-hidden">
				<Transition name="section-label">
					<span v-if="activeSection" :key="activeSection" class="absolute inset-0 truncate">
						{{ t(`settings.sections.${activeSection}`) }}
					</span>
				</Transition>
			</p>
		</header>

		<div class="flex flex-col gap-5 max-w-160 px-6 pt-5 pb-6">
		<div v-if="store.loading" class="text-muted-foreground">{{ t("settings.loading") }}</div>

		<section v-else class="flex flex-col gap-7">

			<!-- 语言 / Language -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.language = (el as HTMLElement) }"
					data-section="language"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>{{ t("settings.sections.language") }}</h2>
				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.language.label") }}</Label>
					<Select :model-value="String(locale)" @update:model-value="(v) => v && setLocale(String(v))">
						<SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
						<SelectContent>
							<SelectItem v-for="loc in localesStore.locales" :key="loc.code" :value="loc.code">
								{{ loc.name }}
							</SelectItem>
						</SelectContent>
					</Select>
				</div>
			</section>

			<!-- 录制设置 / Recording -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.recording = (el as HTMLElement) }"
					data-section="recording"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>{{ t("settings.sections.recording") }}</h2>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.outputDir.label") }}</Label>
					<div class="flex items-center gap-1.5">
						<Input
							v-model="form.output_dir"
							:placeholder="t('settings.outputDir.placeholder')"
							autocomplete="off"
							class="flex-1"
							@keyup.enter="saveOutputDir"
							@blur="saveOutputDir"
						/>
						<Button type="button" variant="outline" size="icon" :title="t('settings.outputDir.pick')" @click="browseOutputDir">
							<FolderOpen class="size-4" />
						</Button>
					</div>
					<p class="text-xs text-muted-foreground">{{ t("settings.outputDir.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.preferredResolution.label") }}</Label>
					<Select
						:model-value="String(form.preferred_resolution)"
						@update:model-value="form.preferred_resolution = Number($event ?? 0)"
					>
						<SelectTrigger class="w-48"><SelectValue /></SelectTrigger>
						<SelectContent>
							<SelectItem value="0">{{ t("settings.preferredResolution.original") }}</SelectItem>
							<SelectItem v-for="res in [2160, 1440, 1080, 720, 540, 480, 360, 240]" :key="res" :value="String(res)">
								{{ res }}p
							</SelectItem>
						</SelectContent>
					</Select>
					<p class="text-xs text-muted-foreground">{{ t("settings.preferredResolution.hint") }}</p>
				</div>

				<div v-if="form.preferred_resolution > 0" class="flex flex-col gap-1.5">
					<Label>{{ t("settings.resolutionPreference.label") }}</Label>
					<RadioGroup
						:model-value="form.resolution_preference"
						class="flex flex-row gap-4"
						@update:model-value="(v) => v && (form.resolution_preference = v as 'lower' | 'higher')"
					>
						<div v-for="dir in resolutionDirections" :key="dir" class="flex items-center gap-2">
							<RadioGroupItem :id="`resolution-${dir}`" :value="dir" />
							<Label :for="`resolution-${dir}`" class="cursor-pointer">
								{{ t(`settings.resolutionPreference.${dir}`) }}
							</Label>
						</div>
					</RadioGroup>
					<p class="text-xs text-muted-foreground">{{ t("settings.resolutionPreference.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.recordingDuration.label") }}</Label>
					<NumberField
						:model-value="form.max_recording_duration_secs"
						:min="0"
						:max="Number.MAX_SAFE_INTEGER"
						:step="1"
						class="w-40"
						@update:model-value="(v) => v !== undefined && Number.isSafeInteger(v) && (form.max_recording_duration_secs = Math.max(0, v))"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">{{ t("settings.recordingDuration.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.maxConcurrent.label") }}</Label>
					<NumberField
						:model-value="form.max_concurrent"
						:min="0"
						:max="systemStore.maxConcurrentCap > 0 ? systemStore.maxConcurrentCap : undefined"
						class="w-32"
						@update:model-value="(v) => v !== undefined && (form.max_concurrent = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">{{ t("settings.maxConcurrent.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.maxPpConcurrent.label") }}</Label>
					<NumberField
						:model-value="form.max_pp_concurrent"
						:min="0"
						:max="systemStore.maxPpConcurrentCap > 0 ? systemStore.maxPpConcurrentCap : undefined"
						class="w-32"
						@update:model-value="(v) => v !== undefined && (form.max_pp_concurrent = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">{{ t("settings.maxPpConcurrent.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.pollInterval.label") }}</Label>
					<NumberField
						:model-value="form.poll_interval_secs"
						:min="10" :max="300"
						class="w-32"
						@update:model-value="(v) => v !== undefined && (form.poll_interval_secs = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.maxTmpDirGb.label") }}</Label>
					<NumberField
						:model-value="form.max_tmp_dir_gb"
						:min="0" :step="0.5"
						class="w-36"
						@update:model-value="(v) => v !== undefined && (form.max_tmp_dir_gb = v)"
					>
						<NumberFieldContent>
							<NumberFieldDecrement /><NumberFieldInput /><NumberFieldIncrement />
						</NumberFieldContent>
					</NumberField>
					<p class="text-xs text-muted-foreground">{{ t("settings.maxTmpDirGb.hint") }}</p>
				</div>
			</section>

			<!-- 网络 / Network -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.network = (el as HTMLElement) }"
					data-section="network"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>{{ t("settings.sections.network") }}</h2>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.apiProxy.label") }}</Label>
					<Input
						:model-value="form.api_proxy_url ?? ''"
						:placeholder="t('settings.apiProxy.placeholder')"
						autocomplete="url"
						@update:model-value="form.api_proxy_url = ($event as string) || null"
						@keyup.enter="saveProxy('api_proxy_url')"
						@blur="saveProxy('api_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">{{ t("settings.apiProxy.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.scMirror.label") }}</Label>
					<div class="flex items-center gap-1.5">
						<Select
							:model-value="form.sc_mirror_scheme"
							class="w-28 shrink-0"
							@update:model-value="(v) => v && (form.sc_mirror_scheme = String(v))"
						>
							<SelectTrigger class="w-28"><SelectValue /></SelectTrigger>
							<SelectContent>
								<SelectItem value="https">https://</SelectItem>
								<SelectItem value="http">http://</SelectItem>
							</SelectContent>
						</Select>
						<Input
							:model-value="form.sc_mirror_url ?? ''"
							:placeholder="t('settings.scMirror.placeholder')"
							autocomplete="off"
							class="flex-1"
							@update:model-value="form.sc_mirror_url = ($event as string) || null"
							@keyup.enter="saveProxy('sc_mirror_url')"
							@blur="saveProxy('sc_mirror_url')"
						/>
					</div>
					<p class="text-xs text-muted-foreground">{{ t("settings.scMirror.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.cdnProxy.label") }}</Label>
					<Input
						:model-value="form.cdn_proxy_url ?? ''"
						:placeholder="t('settings.cdnProxy.placeholder')"
						autocomplete="url"
						@update:model-value="form.cdn_proxy_url = ($event as string) || null"
						@keyup.enter="saveProxy('cdn_proxy_url')"
						@blur="saveProxy('cdn_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">{{ t("settings.cdnProxy.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.communityProxy.label") }}</Label>
					<Input
						:model-value="form.community_proxy_url ?? ''"
						:placeholder="t('settings.communityProxy.placeholder')"
						autocomplete="url"
						@update:model-value="form.community_proxy_url = ($event as string) || null"
						@keyup.enter="saveProxy('community_proxy_url')"
						@blur="saveProxy('community_proxy_url')"
					/>
					<p class="text-xs text-muted-foreground">{{ t("settings.communityProxy.hint") }}</p>
				</div>

				<div class="flex flex-col gap-1.5">
					<Label>{{ t("settings.communityMirror.label") }}</Label>
					<Input
						:model-value="form.community_mirror_url ?? ''"
						:placeholder="t('settings.communityMirror.placeholder')"
						autocomplete="url"
						@update:model-value="form.community_mirror_url = ($event as string) || null"
						@keyup.enter="saveProxy('community_mirror_url')"
						@blur="saveProxy('community_mirror_url')"
					/>
					<p class="text-xs text-muted-foreground">{{ t("settings.communityMirror.hint") }}</p>
				</div>
			</section>

			<!-- Mouflon Keys -->
			<section class="flex flex-col gap-3.5">
				<h2
					:ref="(el) => { if (el) sectionRefs.mouflonKeys = (el as HTMLElement) }"
					data-section="mouflonKeys"
					class="text-xs font-bold uppercase tracking-widest text-muted-foreground pb-2 border-b"
				>{{ t("settings.sections.mouflonKeys") }}</h2>
				<p class="text-xs text-muted-foreground leading-relaxed">
					{{ t("settings.mouflonKeysDesc") }}
					<code class="bg-muted px-1 py-0.5 rounded text-xs font-mono">pkey → pdkey</code>
				</p>

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
					<div class="flex items-center gap-1.5">
						<Input
							:model-value="form.mouflon_sync_token ?? ''"
							:placeholder="t('settings.mouflonSyncToken.placeholder')"
							:type="showToken ? 'text' : 'password'"
							autocomplete="off"
							class="flex-1"
							@update:model-value="form.mouflon_sync_token = ($event as string) || null"
							@keyup.enter="saveProxy('mouflon_sync_token')"
							@blur="saveProxy('mouflon_sync_token')"
						/>
						<Button
							type="button"
							variant="outline"
							size="icon"
							:title="showToken ? t('common.hide') : t('common.show')"
							@click="showToken = !showToken"
						>
							<EyeOff v-if="showToken" class="size-4" />
							<Eye v-else class="size-4" />
						</Button>
					</div>
				</div>

				<div class="flex items-center justify-between gap-4 text-xs text-muted-foreground">
					<div class="flex flex-col gap-0.5">
						<span>{{ t("settings.mouflonAutoSyncedAt") }}{{ formatTs(mouflonStore.auto_synced_at) }}</span>
						<span>{{ t("settings.mouflonManualUpdatedAt") }}{{ formatTs(mouflonStore.manual_updated_at) }}</span>
					</div>
					<Button type="button" variant="outline" size="sm" :disabled="syncing || !form.mouflon_sync_url" @click="syncKeys">
						{{ syncing ? t("settings.mouflonSyncing") : t("settings.mouflonSync") }}
					</Button>
				</div>

				<table v-if="Object.keys(mouflonStore.keys).length" class="w-full text-xs border-collapse">
					<thead>
						<tr>
							<th class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold">{{ t("settings.mouflonTable.pkey") }}</th>
							<th class="text-left px-2 py-1.5 border-b text-muted-foreground font-semibold">{{ t("settings.mouflonTable.pdkey") }}</th>
							<th class="border-b"></th>
						</tr>
					</thead>
					<tbody>
						<tr v-for="(pdkey, pkey) in mouflonStore.keys" :key="pkey">
							<td class="px-2 py-1.5 border-b font-mono">{{ pkey }}</td>
							<td class="px-2 py-1.5 border-b font-mono max-w-60 truncate">{{ pdkey }}</td>
							<td class="px-2 py-1.5 border-b">
								<Button type="button" variant="destructive" size="sm" class="h-6 text-xs px-2" @click="removeKey(pkey)">
									{{ t("common.delete") }}
								</Button>
							</td>
						</tr>
					</tbody>
				</table>
				<p v-else class="text-xs text-muted-foreground">{{ t("settings.noKeys") }}</p>

				<div class="flex gap-2 items-center">
					<Input v-model="newPkey" placeholder="pkey" autocomplete="off" class="flex-1 font-mono text-xs" />
					<Input v-model="newPdkey" placeholder="pdkey" autocomplete="off" class="flex-2 font-mono text-xs" />
					<Button type="button" variant="outline" @click="addKey">{{ t("settings.addKey") }}</Button>
				</div>
				<p v-if="keyError" class="text-xs text-destructive">{{ keyError }}</p>
			</section>
		</section>
		</div>
	</div>
</template>

<style scoped>
.section-label-enter-active,
.section-label-leave-active { transition: opacity 0.15s, transform 0.15s; }
.section-label-enter-from { opacity: 0; transform: translateY(4px); }
.section-label-leave-to   { opacity: 0; transform: translateY(-4px); }
</style>
