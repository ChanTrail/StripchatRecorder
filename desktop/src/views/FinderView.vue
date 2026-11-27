<!--
    主播查找页面 / Streamer Finder View

    支持两种查找方式：
    1. 人脸查找：上传图片，通过 camgirlfinder.net 的人脸识别 API 查找相似主播
    2. 名字查找：输入主播名，通过 camgirlfinder.net 的名字搜索 API 查找主播
-->
<script setup lang="ts">
import { ref, reactive, onMounted } from "vue";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ImageIcon, Loader2, Search, X } from "@lucide/vue";
import ImagePreviewDialog from "@/components/ImagePreviewDialog.vue";
import { useNotify } from "@/composables/useNotify";
import { useStreamersStore } from "@/stores/streamers";
import { call } from "@/lib/api";
import { useI18n } from "vue-i18n";

const { toast } = useNotify();
const streamersStore = useStreamersStore();
const { t } = useI18n();

onMounted(() => {
	streamersStore.initListeners();
	void streamersStore.fetchStreamers();
});

// ─── Types ───────────────────────────────────────────────────────────────────

interface PredictionUrls {
	profile: string;
	externalProfile: string;
	faceImage: string;
	fullImage: string;
}

/** 人脸搜索结果条目 */
interface Prediction {
	platform: string;
	model: string;
	gender: string;
	distance: number;
	probability: "high" | "medium" | "low";
	seen: string;
	accountSeen: string;
	urls: PredictionUrls;
}

interface JobUrls {
	job: string;
	fullImage: string;
	faceImage?: string;
}

interface Job {
	id: string;
	status: "active" | "finished" | "failed" | "noface";
	error?: string | null;
	created: string;
	duration: number;
	urls: JobUrls;
	predictions?: Prediction[];
}

interface PersonUrls {
	faceImage: string;
	fullImage: string;
}

interface Person {
	person: number;
	faces: number;
	seen: string;
	firstSeen: string;
	lastSeen: string;
	urls: PersonUrls;
}

interface ModelUrls {
	profile: string;
	externalProfile: string;
}

/** 名字搜索结果条目 */
interface ModelResult {
	name: string;
	platform: string;
	gender: string;
	distance: number;
	faces: number;
	firstSeen: string;
	lastSeen: string;
	persons: Person[];
	urls: ModelUrls;
}

// ─── Platform / gender maps ───────────────────────────────────────────────────

const PLATFORM_LABELS: Record<string, string> = {
	atv: "AmateurTV",
	bc: "BongaCams",
	c4: "Cam4",
	cb: "Chaturbate",
	cs: "CamSoda",
	ctv: "CherryTV",
	f4f: "Flirt4Free",
	im: "ImLive",
	lj: "LiveJasmin",
	mfc: "MyFreeCams",
	sc: "StripChat",
	sm: "Streamate",
	sr: "StreamRay",
	stv: "ShowUpTV",
	xl: "XloveCam",
};

const PROBABILITY_COLORS: Record<string, string> = {
	high: "text-green-500",
	medium: "text-yellow-500",
	low: "text-muted-foreground",
};

// ─── State ───────────────────────────────────────────────────────────────────

const activeTab = ref<"face" | "name">("face");
const bgMode = ref(false);

// 图片预览弹窗引用 / Image preview dialog ref
const previewDialogRef = ref<InstanceType<typeof ImagePreviewDialog> | null>(null);

/**
 * 卡片图片点击 → 打开图片预览（预览与卡片当前显示的一致：
 * 小卡片=人脸图，背景图模式=完整背景图）。
 * Card image click → open image preview showing the same image the
 * card displays (small card = face image, background mode = full image).
 * @param url - 图片 URL / image URL
 * @param title - 预览标题（主播名）/ preview title (model name)
 */
function openCardImage(url: string | undefined, title: string) {
	if (!url) return;
	previewDialogRef.value?.openPreview(url, title);
}

// Face search
const dragOver = ref(false);
const selectedFile = ref<File | null>(null);
const previewUrl = ref<string | null>(null);
const faceResults = ref<Prediction[]>([]);
const faceJobUrls = ref<JobUrls | null>(null);
const faceLoading = ref(false);
const faceError = ref<string | null>(null);
const faceSearched = ref(false);

// Name search
const nameQuery = ref("");
const nameResults = ref<ModelResult[]>([]);
const nameLoading = ref(false);
const nameError = ref<string | null>(null);
const nameSearched = ref(false);

// 正在添加中的主播集合，防止重复点击
// 使用 reactive Set：Vue 能追踪 .add() / .delete() 的变更
const addingSet = reactive(new Set<string>());

// 验证状态缓存：username -> "checking" | "exists" | "not_found"
// 使用 reactive 对象：Vue 能追踪属性赋值的变更，无需每次 new Map()
const verifyCache = reactive<Record<string, "checking" | "exists" | "not_found">>({});

// ─── Helpers ─────────────────────────────────────────────────────────────────

const CGF_API = "https://api.camgirlfinder.net";

async function cgfFetch(url: string, init?: RequestInit): Promise<Response> {
	const headers = new Headers(init?.headers);
	headers.set("User-Agent", "StripchatRecorder/1.0");
	const res = await fetch(url, { ...init, headers });
	if (!res.ok) {
		const text = await res.text().catch(() => res.statusText);
		let msg = text;
		try {
			msg = (JSON.parse(text) as { error?: string })?.error ?? text;
		} catch {
			/* 不是 JSON，直接用原文 */
		}
		throw new Error(msg || `HTTP ${res.status}`);
	}
	return res;
}

function platformLabel(code: string): string {
	return PLATFORM_LABELS[code] ?? code.toUpperCase();
}

function genderLabel(code: string): string {
	return t(`finder.gender.${code}`) || code;
}

function probabilityLabel(code: string): string {
	if (code === "high") return t("finder.card.probHigh");
	if (code === "medium") return t("finder.card.probMedium");
	if (code === "low") return t("finder.card.probLow");
	return code;
}

function formatDate(iso: string): string {
	return new Date(iso).toLocaleDateString("zh-CN", {
		year: "numeric",
		month: "2-digit",
		day: "2-digit",
	});
}

// ─── Verify / Add ─────────────────────────────────────────────────────────────

async function verifyStreamer(username: string) {
	if (username in verifyCache) return;
	verifyCache[username] = "checking";
	try {
		const res = await call<{ exists: boolean }>("verify_streamer", { username });
		verifyCache[username] = res.exists ? "exists" : "not_found";
	} catch {
		// 网络错误时乐观处理，不阻塞添加
		verifyCache[username] = "exists";
	}
}

// verify 并发限流：每个 verify_streamer 请求后端都要探测 SC 上游 API，
// 网络不佳时单个请求可能挂起至 30s 超时；若不加限制地并发数十个，
// 会占满浏览器的同源连接池（HTTP/1.1 约 6 个，SSE 再占 1 个），
// 导致页面其它 API 请求长时间排队、移动端表现为「所有按钮点击无效」。
// Verify concurrency limiter: each verify_streamer call probes the SC
// upstream API server-side — on a bad network a single request can hang
// until its 30s timeout. Firing dozens unbounded saturates the browser's
// per-origin connection pool (~6 for HTTP/1.1, minus 1 held by SSE),
// queuing every other /api request for minutes — on mobile this looks
// like "every button is dead".
const VERIFY_CONCURRENCY = 3;
const verifyQueue: string[] = [];
let verifyInFlight = 0;

function pumpVerify() {
	while (verifyInFlight < VERIFY_CONCURRENCY && verifyQueue.length > 0) {
		const username = verifyQueue.shift()!;
		// 队列等待期间缓存可能已被其它路径填充，出队时去重
		// May have been populated by another path while queued — dedupe on dequeue
		if (username in verifyCache) continue;
		verifyInFlight++;
		void verifyStreamer(username).finally(() => {
			verifyInFlight--;
			pumpVerify();
		});
	}
}

function verifyAll(usernames: string[]) {
	for (const u of usernames) {
		if (!(u in verifyCache) && !verifyQueue.includes(u)) verifyQueue.push(u);
	}
	pumpVerify();
}

async function addToRecord(username: string) {
	if (addingSet.has(username)) return;
	addingSet.add(username);
	try {
		await streamersStore.addStreamers([username]);
		toast(t("finder.card.addedToast", { username }), "success");
	} catch (e) {
		toast(e instanceof Error ? e.message : String(e), "error");
	} finally {
		addingSet.delete(username);
	}
}

function isAdded(username: string): boolean {
	return streamersStore.streamers.some(
		(s) => s.username.toLowerCase() === username.toLowerCase(),
	);
}

// ─── Job polling ──────────────────────────────────────────────────────────────

async function pollJob(jobId: string): Promise<Job> {
	for (let i = 0; i < 60; i++) {
		await new Promise((r) => setTimeout(r, 2000));
		const res = await cgfFetch(`${CGF_API}/jobs/${jobId}`);
		const job = (await res.json()) as Job;
		if (job.status !== "active") return job;
	}
	throw new Error(t("finder.face.timeout"));
}

/**
 * 发起一次搜索任务并等待完成。
 * @param submit 提交任务的函数，返回初始 Job
 */
async function runJobSearch(submit: () => Promise<Job>): Promise<Job> {
	const job = await submit();
	return job.status === "active" ? pollJob(job.id) : job;
}

// ─── Face search ─────────────────────────────────────────────────────────────

function setFile(file: File) {
	if (!file.type.startsWith("image/")) {
		faceError.value = t("finder.face.invalidFile");
		return;
	}
	selectedFile.value = file;
	faceError.value = null;
	faceResults.value = [];
	faceSearched.value = false;
	faceJobUrls.value = null;
	if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
	previewUrl.value = URL.createObjectURL(file);
}

function onFileInput(e: Event) {
	const f = (e.target as HTMLInputElement).files?.[0];
	if (f) setFile(f);
}

function onDrop(e: DragEvent) {
	dragOver.value = false;
	const f = e.dataTransfer?.files?.[0];
	if (f) setFile(f);
}

function onPaste(e: ClipboardEvent) {
	const f = Array.from(e.clipboardData?.files ?? []).find((file) =>
		file.type.startsWith("image/"),
	);
	if (f) setFile(f);
}

async function doFaceSearch() {
	if (!selectedFile.value) return;
	faceLoading.value = true;
	faceError.value = null;
	faceResults.value = [];
	faceJobUrls.value = null;
	faceSearched.value = false;

	try {
		const finalJob = await runJobSearch(async () => {
			const form = new FormData();
			form.append("image", selectedFile.value!);
			const res = await cgfFetch(`${CGF_API}/search`, { method: "POST", body: form });
			const job = (await res.json()) as Job;
			faceJobUrls.value = job.urls;
			return job;
		});

		finishFaceJob(finalJob);
	} catch (e) {
		faceError.value = e instanceof Error ? e.message : String(e);
	} finally {
		faceLoading.value = false;
		faceSearched.value = true;
	}
}

/**
 * 通过图片 URL 发起人脸搜索（名字查找转发过来的入口）。
 * Run a face search from an image URL (entry point for name-search forwarding).
 * @param faceUrl - 人脸图 URL / face image URL
 * @param fullImageUrl - 背景图 URL / background image URL
 */
async function doFaceUrlSearch(faceUrl: string, fullImageUrl?: string) {
	faceLoading.value = true;
	faceError.value = null;
	faceResults.value = [];
	faceSearched.value = false;
	// 预览区先显示转发来的图片，job 返回后再用 job.urls 覆盖 /
	// Seed the preview with the forwarded image; job.urls replaces it once returned
	faceJobUrls.value = { job: "", fullImage: fullImageUrl ?? faceUrl, faceImage: faceUrl };

	try {
		const finalJob = await runJobSearch(async () => {
			const res = await cgfFetch(`${CGF_API}/search?url=${encodeURIComponent(faceUrl)}`);
			const job = (await res.json()) as Job;
			if (job.urls?.faceImage || job.urls?.fullImage) faceJobUrls.value = job.urls;
			return job;
		});

		finishFaceJob(finalJob);
	} catch (e) {
		faceError.value = e instanceof Error ? e.message : String(e);
	} finally {
		faceLoading.value = false;
		faceSearched.value = true;
	}
}

/** 人脸搜索 job 完成后的统一收尾（上传与 URL 两种入口共用）/ Shared finish step for both upload and URL-based searches */
function finishFaceJob(finalJob: Job) {
	faceJobUrls.value = finalJob.urls?.fullImage ? finalJob.urls : faceJobUrls.value;

	if (finalJob.status === "noface") {
		faceError.value = t("finder.face.noFace");
	} else if (finalJob.status === "failed") {
		faceError.value = finalJob.error ?? t("finder.face.searchFailed");
	} else {
		faceResults.value = finalJob.predictions ?? [];
		verifyAll(faceResults.value.filter((p) => p.platform === "sc").map((p) => p.model));
	}
}

function clearFace() {
	selectedFile.value = null;
	if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
	previewUrl.value = null;
	faceResults.value = [];
	faceError.value = null;
	faceSearched.value = false;
	faceJobUrls.value = null;
}

// ─── Name search ─────────────────────────────────────────────────────────────

async function doNameSearch() {
	const q = nameQuery.value.trim();
	if (!q) return;
	nameLoading.value = true;
	nameError.value = null;
	nameResults.value = [];
	nameSearched.value = false;

	try {
		const res = await cgfFetch(`${CGF_API}/models/search?model=${encodeURIComponent(q)}`);
		nameResults.value = (await res.json()) as ModelResult[];
		verifyAll(nameResults.value.filter((m) => m.platform === "sc").map((m) => m.name));
	} catch (e) {
		nameError.value = e instanceof Error ? e.message : String(e);
	} finally {
		nameLoading.value = false;
		nameSearched.value = true;
	}
}

function onNameKeydown(e: KeyboardEvent) {
	if (e.key === "Enter") doNameSearch();
}

// ─── Cross-tab forwarding（名字查找 ↔ 人脸查找）───────────────────────────────

/**
 * 名字查找结果 → 转发到人脸查找：切换到人脸 tab，用该主播的人脸图发起 URL 搜索。
 * Forward a name-search result into face search: switch to the face tab and
 * run a URL-based face search with the model's face image.
 * @param m - 名字搜索结果条目 / name-search result entry
 */
function forwardToFaceSearch(m: ModelResult) {
	const faceUrl = m.persons[0]?.urls.faceImage;
	if (!faceUrl) return;

	// 清空上传模式的文件态 / clear the upload-mode file state
	selectedFile.value = null;
	if (previewUrl.value) URL.revokeObjectURL(previewUrl.value);
	previewUrl.value = null;

	activeTab.value = "face";
	void doFaceUrlSearch(faceUrl, m.persons[0]?.urls.fullImage);
}

/**
 * 人脸查找结果 → 转发到名字查找：切换到名字 tab 并立即搜索该主播名。
 * Forward a face-search result into name search: switch to the name tab and
 * run a name search for the model.
 * @param model - 主播名 / model name
 */
function forwardToNameSearch(model: string) {
	activeTab.value = "name";
	nameQuery.value = model;
	void doNameSearch();
}
</script>

<template>
	<div class="flex flex-col gap-5 px-6 pt-6 pb-6" @paste.capture="onPaste">
		<!-- 固定页头区：标题+页签随滚动钉在顶部 / Pinned header zone: title + tabs stick while content scrolls -->
		<div class="bg-background sticky top-0 z-20 -mx-6 flex flex-col gap-5 px-6">
			<!-- Header -->
			<header>
				<h1 class="text-xl font-bold mb-0.5">{{ t("finder.title") }}</h1>
				<p class="text-sm text-muted-foreground">
					{{ t("finder.description") }}
				</p>
			</header>

			<!-- Tabs -->
			<div class="flex gap-1 border-b border-border">
				<Button
					v-for="tab in [
						{ key: 'face', label: t('finder.tabs.face') },
						{ key: 'name', label: t('finder.tabs.name') },
					]"
					:key="tab.key"
					variant="ghost"
					class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px rounded-none"
					:class="
						activeTab === tab.key
							? 'border-primary text-foreground'
							: 'border-transparent text-muted-foreground hover:text-foreground'
					"
					@click="activeTab = tab.key as 'face' | 'name'"
				>
					{{ tab.label }}
				</Button>
			</div>
		</div>
		<!-- /固定页头区 sticky header zone -->

		<!-- ── Face Search Panel ── -->
		<div v-if="activeTab === 'face'" class="flex flex-col gap-4">
			<!-- Drop zone -->
			<div
				class="relative border-2 border-dashed rounded-lg transition-colors cursor-pointer"
				:class="
					dragOver
						? 'border-primary bg-primary/5'
						: 'border-border hover:border-primary/50'
				"
				@dragover.prevent="dragOver = true"
				@dragleave="dragOver = false"
				@drop.prevent="onDrop"
				@click="($refs.fileInput as HTMLInputElement).click()"
			>
				<input
					ref="fileInput"
					type="file"
					accept="image/*"
					class="hidden"
					@change="onFileInput"
				/>

				<div
					v-if="!previewUrl"
					class="flex flex-col items-center gap-2 py-12 text-muted-foreground"
				>
					<ImageIcon class="w-10 h-10 opacity-40" />
					<p class="text-sm">{{ t("finder.face.dropzone") }}</p>
					<p class="text-xs opacity-60">{{ t("finder.face.dropzoneHint") }}</p>
				</div>

				<div v-else class="relative flex items-center justify-center p-4 min-h-[160px]">
					<img
						:src="previewUrl"
						class="max-h-48 max-w-full rounded object-contain"
						alt="预览"
					/>
					<Button
						variant="outline"
						size="icon"
						class="absolute top-2 right-2 w-6 h-6 rounded-full bg-background/80 text-xs hover:bg-destructive hover:text-destructive-foreground"
						@click.stop="clearFace"
						><X class="size-3"
					/></Button>
				</div>
			</div>

			<!-- Extracted face preview (after search) -->
			<div
				v-if="faceJobUrls?.faceImage"
				class="flex items-center gap-3 text-sm text-muted-foreground"
			>
				<img
					:src="faceJobUrls.faceImage"
					class="w-12 h-12 rounded object-cover border border-border"
					:alt="t('finder.face.detectedFace')"
				/>
				<span>{{ t("finder.face.detectedFace") }}</span>
			</div>

			<div class="flex gap-2">
				<Button
					:disabled="!selectedFile || faceLoading"
					class="flex-1"
					@click="doFaceSearch"
				>
					<span v-if="faceLoading" class="flex items-center gap-2">
						<Loader2 class="w-4 h-4 animate-spin" />
						{{ t("finder.face.searching") }}
					</span>
					<span v-else>{{ t("finder.face.search") }}</span>
				</Button>
			</div>

			<p v-if="faceError" class="text-sm text-destructive">{{ faceError }}</p>

			<!-- Face results -->
			<template v-if="faceResults.length > 0">
				<div class="flex items-center justify-between gap-2">
					<p class="text-sm text-muted-foreground">
						{{ t("finder.face.results", { count: faceResults.length }) }}
					</p>
					<!-- 背景图模式：卡片纵向排列、显示完整背景图（同原相似对话框的切换）/
               Background mode: vertical cards showing full background images
               (same toggle the old similar-faces dialog had) -->
					<Button
						variant="outline"
						class="flex items-center gap-1.5 text-xs px-2 py-1 h-auto shrink-0"
						:class="bgMode ? 'border-primary bg-primary/10 text-primary' : ''"
						@click="bgMode = !bgMode"
					>
						<ImageIcon class="w-3 h-3" />
						{{ t("finder.face.bgMode") }}
					</Button>
				</div>
				<div
					class="grid gap-3"
					:class="
						bgMode
							? 'grid-cols-[repeat(auto-fill,minmax(280px,1fr))]'
							: 'grid-cols-[repeat(auto-fill,minmax(240px,1fr))]'
					"
				>
					<div
						v-for="(p, i) in faceResults"
						:key="i"
						class="flex gap-3 p-3 rounded-lg border border-border hover:border-primary/50 hover:bg-accent/30 transition-colors"
						:class="{ 'flex-col': bgMode }"
					>
						<!-- 图片点击 → 图片预览（self-start 防止 flex 拉伸超出图片区域）/
                 Image click → image preview (self-start prevents the flex item
                 from stretching beyond the image bounds) -->
						<button
							type="button"
							class="shrink-0 self-start cursor-zoom-in"
							:class="{ 'w-full': bgMode }"
							@click="
								openCardImage(
									bgMode
										? (p.urls.fullImage ?? p.urls.faceImage)
										: p.urls.faceImage,
									p.model,
								)
							"
						>
							<img
								:src="bgMode ? p.urls.fullImage : p.urls.faceImage"
								:class="
									bgMode
										? 'w-full aspect-video object-cover rounded border border-border'
										: 'w-14 h-14 rounded object-cover border border-border'
								"
								:alt="p.model"
							/>
						</button>
						<div class="flex flex-col min-w-0 flex-1 relative">
							<!-- 平台名固定右上角 -->
							<span
								class="absolute top-0 right-0 text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
							>
								{{ platformLabel(p.platform) }}
							</span>
							<!-- 主播名（点击跳 camgirlfinder 页面）/ Model name (links to camgirlfinder page) -->
							<a
								:href="p.urls.profile"
								target="_blank"
								rel="noopener noreferrer"
								class="font-semibold text-sm truncate hover:underline pr-16"
								>{{ p.model }}</a
							>
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ genderLabel(p.gender) }}
							</div>
							<div class="text-xs mt-0.5">
								{{ t("finder.card.match")
								}}<span :class="PROBABILITY_COLORS[p.probability] ?? ''">
									{{ probabilityLabel(p.probability) }}
								</span>
								<span class="text-muted-foreground ml-1"
									>({{ p.distance.toFixed(3) }})</span
								>
							</div>
							<!-- seen = 该人脸记录首次被抓拍的时间 / seen = first capture time of this face record -->
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ t("finder.card.firstSeen") }}{{ formatDate(p.seen) }}
							</div>
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ t("finder.card.lastSeen") }}{{ formatDate(p.accountSeen) }}
							</div>
							<!-- 底部动作：按名查找（所有平台可用）+ 录制（仅 SC）/
                   Bottom actions: name-search forward (all platforms) + record (SC only) -->
							<div class="flex justify-end items-center gap-2 mt-auto pt-1">
								<Button
									size="sm"
									variant="ghost"
									class="text-xs h-7 px-2"
									:title="t('finder.card.searchName')"
									@click="forwardToNameSearch(p.model)"
								>
									<Search class="w-3.5 h-3.5" />
									{{ t("finder.card.searchName") }}
								</Button>
								<template v-if="p.platform !== 'sc'">
									<span
										class="text-xs text-muted-foreground self-center whitespace-nowrap"
										>{{ t("finder.card.scOnly") }}</span
									>
								</template>
								<template v-else-if="verifyCache[p.model] === 'not_found'">
									<span
										class="text-xs text-destructive self-center whitespace-nowrap"
										>{{ t("finder.card.notFound") }}</span
									>
								</template>
								<template v-else-if="isAdded(p.model)">
									<span
										class="text-xs text-muted-foreground self-center whitespace-nowrap"
										>{{ t("finder.card.added") }}</span
									>
								</template>
								<template v-else>
									<Button
										size="sm"
										variant="outline"
										:disabled="
											addingSet.has(p.model) ||
											verifyCache[p.model] === 'checking'
										"
										class="text-xs h-7 px-2"
										@click="addToRecord(p.model)"
									>
										{{
											addingSet.has(p.model)
												? t("finder.card.adding")
												: verifyCache[p.model] === "checking"
													? t("finder.card.verifying")
													: t("finder.card.addRecord")
										}}
									</Button>
								</template>
							</div>
						</div>
					</div>
				</div>
			</template>

			<div
				v-else-if="faceSearched && !faceLoading && !faceError"
				class="text-center text-muted-foreground py-8 text-sm"
			>
				{{ t("finder.face.noResults") }}
			</div>
		</div>

		<!-- ── Name Search Panel ── -->
		<div v-if="activeTab === 'name'" class="flex flex-col gap-4">
			<div class="flex gap-2">
				<Input
					v-model="nameQuery"
					:placeholder="t('finder.name.placeholder')"
					class="flex-1"
					@keydown="onNameKeydown"
				/>
				<Button
					:disabled="nameQuery.trim().length < 3 || nameLoading"
					@click="doNameSearch"
				>
					<span v-if="nameLoading" class="flex items-center gap-2">
						<Loader2 class="w-4 h-4 animate-spin" />
						{{ t("finder.name.searching") }}
					</span>
					<span v-else><Search class="size-4" /></span>
				</Button>
			</div>

			<p v-if="nameError" class="text-sm text-destructive">{{ nameError }}</p>

			<!-- Name results（卡片结构与人脸查找一致，仅无匹配度行 /
           Card structure mirrors face results, minus the match row） -->
			<template v-if="nameResults.length > 0">
				<div class="flex items-center justify-between gap-2">
					<p class="text-sm text-muted-foreground">
						{{ t("finder.name.results", { count: nameResults.length }) }}
					</p>
					<Button
						variant="outline"
						class="flex items-center gap-1.5 text-xs px-2 py-1 h-auto shrink-0"
						:class="bgMode ? 'border-primary bg-primary/10 text-primary' : ''"
						@click="bgMode = !bgMode"
					>
						<ImageIcon class="w-3 h-3" />
						{{ t("finder.face.bgMode") }}
					</Button>
				</div>
				<div
					class="grid gap-3"
					:class="
						bgMode
							? 'grid-cols-[repeat(auto-fill,minmax(280px,1fr))]'
							: 'grid-cols-[repeat(auto-fill,minmax(240px,1fr))]'
					"
				>
					<div
						v-for="(m, i) in nameResults"
						:key="i"
						class="flex gap-3 p-3 rounded-lg border border-border hover:border-primary/50 hover:bg-accent/30 transition-colors"
						:class="{ 'flex-col': bgMode }"
					>
						<!-- 图片点击 → 图片预览（self-start 防止 flex 拉伸超出图片区域）/
                 Image click → image preview (self-start prevents the flex item
                 from stretching beyond the image bounds) -->
						<button
							v-if="m.persons[0]?.urls.faceImage"
							type="button"
							class="shrink-0 self-start cursor-zoom-in"
							:class="{ 'w-full': bgMode }"
							@click="
								openCardImage(
									bgMode
										? (m.persons[0]?.urls.fullImage ??
												m.persons[0]?.urls.faceImage)
										: m.persons[0]?.urls.faceImage,
									m.name,
								)
							"
						>
							<img
								:src="
									bgMode
										? (m.persons[0]?.urls.fullImage ??
											m.persons[0]?.urls.faceImage)
										: m.persons[0]?.urls.faceImage
								"
								:class="
									bgMode
										? 'w-full aspect-video object-cover rounded border border-border'
										: 'w-14 h-14 rounded object-cover border border-border'
								"
								:alt="m.name"
							/>
						</button>
						<div
							v-else
							class="rounded shrink-0 border border-border bg-muted flex items-center justify-center text-muted-foreground text-xs self-start"
							:class="bgMode ? 'w-full aspect-video' : 'w-14 h-14'"
						>
							{{ t("finder.card.noImage") }}
						</div>
						<div class="flex flex-col min-w-0 flex-1 relative">
							<!-- 平台名固定右上角 -->
							<span
								class="absolute top-0 right-0 text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground"
							>
								{{ platformLabel(m.platform) }}
							</span>
							<!-- 主播名（点击跳 camgirlfinder 页面）/ Model name (links to camgirlfinder page) -->
							<a
								:href="m.urls.profile"
								target="_blank"
								rel="noopener noreferrer"
								class="font-semibold text-sm truncate hover:underline pr-16"
								>{{ m.name }}</a
							>
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ genderLabel(m.gender) }} ·
								{{ t("finder.card.imageCount", { count: m.faces }) }}
							</div>
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ t("finder.card.firstSeen") }}{{ formatDate(m.firstSeen) }}
							</div>
							<div class="text-xs text-muted-foreground mt-0.5">
								{{ t("finder.card.lastSeen") }}{{ formatDate(m.lastSeen) }}
							</div>
							<!-- 底部动作：相似人脸（人脸图可用时）+ 录制（仅 SC）/
                   Bottom actions: similar-faces forward (when a face image exists) + record (SC only) -->
							<div class="flex justify-end items-center gap-2 mt-auto pt-1">
								<Button
									size="sm"
									variant="ghost"
									class="text-xs h-7 px-2"
									:title="t('finder.card.findSimilar')"
									:disabled="!m.persons[0]?.urls.faceImage"
									@click="forwardToFaceSearch(m)"
								>
									<Search class="w-3.5 h-3.5" />
									{{ t("finder.card.findSimilar") }}
								</Button>
								<template v-if="m.platform !== 'sc'">
									<span
										class="text-xs text-muted-foreground self-center whitespace-nowrap"
										>{{ t("finder.card.scOnly") }}</span
									>
								</template>
								<template v-else-if="verifyCache[m.name] === 'not_found'">
									<span
										class="text-xs text-destructive self-center whitespace-nowrap"
										>{{ t("finder.card.notFound") }}</span
									>
								</template>
								<template v-else-if="isAdded(m.name)">
									<span
										class="text-xs text-muted-foreground self-center whitespace-nowrap"
										>{{ t("finder.card.added") }}</span
									>
								</template>
								<template v-else>
									<Button
										size="sm"
										variant="outline"
										:disabled="
											addingSet.has(m.name) ||
											verifyCache[m.name] === 'checking'
										"
										class="text-xs h-7 px-2"
										@click="addToRecord(m.name)"
									>
										{{
											addingSet.has(m.name)
												? t("finder.card.adding")
												: verifyCache[m.name] === "checking"
													? t("finder.card.verifying")
													: t("finder.card.addRecord")
										}}
									</Button>
								</template>
							</div>
						</div>
					</div>
				</div>
			</template>

			<div
				v-else-if="nameSearched && !nameLoading && !nameError"
				class="text-center text-muted-foreground py-8 text-sm"
			>
				{{ t("finder.name.noResults") }}
			</div>
		</div>

		<!-- 图片预览弹窗（两种卡片共用）/ Shared image preview dialog for both card types -->
		<ImagePreviewDialog ref="previewDialogRef" />
	</div>
</template>
