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
import { useScrollbar } from "@/composables/useScrollbar";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from "@/components/ui/dialog";
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
    try { msg = (JSON.parse(text) as { error?: string })?.error ?? text; } catch { /* 不是 JSON，直接用原文 */ }
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
    year: "numeric", month: "2-digit", day: "2-digit",
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

function verifyAll(usernames: string[]) {
  for (const u of usernames) verifyStreamer(u);
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
    (s) => s.username.toLowerCase() === username.toLowerCase()
  );
}

// ─── Job polling ──────────────────────────────────────────────────────────────

async function pollJob(jobId: string): Promise<Job> {
  for (let i = 0; i < 60; i++) {
    await new Promise((r) => setTimeout(r, 2000));
    const res = await cgfFetch(`${CGF_API}/jobs/${jobId}`);
    const job = await res.json() as Job;
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
    file.type.startsWith("image/")
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
      const job = await res.json() as Job;
      faceJobUrls.value = job.urls;
      return job;
    });

    faceJobUrls.value = finalJob.urls;

    if (finalJob.status === "noface") {
      faceError.value = t("finder.face.noFace");
    } else if (finalJob.status === "failed") {
      faceError.value = finalJob.error ?? t("finder.face.searchFailed");
    } else {
      faceResults.value = finalJob.predictions ?? [];
      verifyAll(faceResults.value.filter((p) => p.platform === "sc").map((p) => p.model));
    }
  } catch (e) {
    faceError.value = e instanceof Error ? e.message : String(e);
  } finally {
    faceLoading.value = false;
    faceSearched.value = true;
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
    nameResults.value = await res.json() as ModelResult[];
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

// ─── Similar-face dialog (triggered from name results) ────────────────────────

const dialogOpen = ref(false);
const dialogModel = ref<ModelResult | null>(null);
const dialogResults = ref<Prediction[]>([]);
const dialogLoading = ref(false);
const dialogError = ref<string | null>(null);
const dialogFaceImage = ref<string | null>(null);

const dialogScrollEl = ref<HTMLElement | null>(null);
useScrollbar(dialogScrollEl);

async function openSimilarDialog(m: ModelResult) {
  const faceUrl = m.persons[0]?.urls.faceImage;
  if (!faceUrl) return;

  dialogModel.value = m;
  dialogResults.value = [];
  dialogError.value = null;
  dialogFaceImage.value = faceUrl;
  dialogLoading.value = true;
  dialogOpen.value = true;
  bgMode.value = false;

  try {
    const finalJob = await runJobSearch(async () => {
      const res = await cgfFetch(`${CGF_API}/search?url=${encodeURIComponent(faceUrl)}`);
      return await res.json() as Job;
    });

    if (finalJob.status === "noface") {
      dialogError.value = t("finder.dialog.noFace");
    } else if (finalJob.status === "failed") {
      dialogError.value = finalJob.error ?? t("finder.face.searchFailed");
    } else {
      // 过滤掉来源主播本身
      dialogResults.value = (finalJob.predictions ?? []).filter(
        (p) => !(p.platform === m.platform && p.model === m.name)
      );
      verifyAll(dialogResults.value.filter((p) => p.platform === "sc").map((p) => p.model));
    }
  } catch (e) {
    dialogError.value = e instanceof Error ? e.message : String(e);
  } finally {
    dialogLoading.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col gap-5 px-6 pt-6 pb-6" @paste.capture="onPaste">
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
        v-for="tab in [{ key: 'face', label: t('finder.tabs.face') }, { key: 'name', label: t('finder.tabs.name') }]"
        :key="tab.key"
        variant="ghost"
        class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px rounded-none"
        :class="activeTab === tab.key
          ? 'border-primary text-foreground'
          : 'border-transparent text-muted-foreground hover:text-foreground'"
        @click="activeTab = tab.key as 'face' | 'name'"
      >
        {{ tab.label }}
      </Button>
    </div>

    <!-- ── Face Search Panel ── -->
    <div v-if="activeTab === 'face'" class="flex flex-col gap-4">

      <!-- Drop zone -->
      <div
        class="relative border-2 border-dashed rounded-lg transition-colors cursor-pointer"
        :class="dragOver ? 'border-primary bg-primary/5' : 'border-border hover:border-primary/50'"
        @dragover.prevent="dragOver = true"
        @dragleave="dragOver = false"
        @drop.prevent="onDrop"
        @click="($refs.fileInput as HTMLInputElement).click()"
      >
        <input ref="fileInput" type="file" accept="image/*" class="hidden" @change="onFileInput" />

        <div v-if="!previewUrl" class="flex flex-col items-center gap-2 py-12 text-muted-foreground">
          <ImageIcon class="w-10 h-10 opacity-40" />
          <p class="text-sm">{{ t("finder.face.dropzone") }}</p>
          <p class="text-xs opacity-60">{{ t("finder.face.dropzoneHint") }}</p>
        </div>

        <div v-else class="relative flex items-center justify-center p-4 min-h-[160px]">
          <img :src="previewUrl" class="max-h-48 max-w-full rounded object-contain" alt="预览" />
          <Button
            variant="outline"
            size="icon"
            class="absolute top-2 right-2 w-6 h-6 rounded-full bg-background/80 text-xs hover:bg-destructive hover:text-destructive-foreground"
            @click.stop="clearFace"
          ><X class="size-3" /></Button>
        </div>
      </div>

      <!-- Extracted face preview (after search) -->
      <div v-if="faceJobUrls?.faceImage" class="flex items-center gap-3 text-sm text-muted-foreground">
        <img :src="faceJobUrls.faceImage" class="w-12 h-12 rounded object-cover border border-border" :alt="t('finder.face.detectedFace')" />
        <span>{{ t("finder.face.detectedFace") }}</span>
      </div>

      <div class="flex gap-2">
        <Button :disabled="!selectedFile || faceLoading" class="flex-1" @click="doFaceSearch">
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
        <p class="text-sm text-muted-foreground">{{ t("finder.face.results", { count: faceResults.length }) }}</p>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
          <div
            v-for="(p, i) in faceResults"
            :key="i"
            class="flex gap-3 p-3 rounded-lg border border-border hover:border-primary/50 hover:bg-accent/30 transition-colors"
          >
            <a :href="p.urls.externalProfile" target="_blank" rel="noopener noreferrer" class="shrink-0">
              <img
                :src="p.urls.faceImage"
                class="w-14 h-14 rounded object-cover border border-border"
                :alt="p.model"
              />
            </a>
            <div class="flex flex-col min-w-0 flex-1 relative">
              <!-- 平台名固定右上角 -->
              <span class="absolute top-0 right-0 text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground">
                {{ platformLabel(p.platform) }}
              </span>
              <!-- 主播名 -->
              <a :href="p.urls.externalProfile" target="_blank" rel="noopener noreferrer" class="font-semibold text-sm truncate hover:underline pr-16">{{ p.model }}</a>
              <div class="text-xs text-muted-foreground mt-0.5">{{ genderLabel(p.gender) }}</div>
              <div class="text-xs mt-0.5">
                {{ t("finder.card.match") }}<span :class="PROBABILITY_COLORS[p.probability] ?? ''">
                  {{ probabilityLabel(p.probability) }}
                </span>
                <span class="text-muted-foreground ml-1">({{ p.distance.toFixed(3) }})</span>
              </div>
              <div class="text-xs text-muted-foreground mt-0.5">{{ t("finder.card.lastSeen") }}{{ formatDate(p.accountSeen) }}</div>
              <!-- 录制按钮固定在右下角 -->
              <div class="flex justify-end mt-auto pt-1">
                <template v-if="p.platform !== 'sc'">
                  <span class="text-xs text-muted-foreground self-end">{{ t("finder.card.scOnly") }}</span>
                </template>
                <template v-else-if="verifyCache[p.model] === 'not_found'">
                  <span class="text-xs text-destructive self-end">{{ t("finder.card.notFound") }}</span>
                </template>
                <template v-else-if="isAdded(p.model)">
                  <span class="text-xs text-muted-foreground self-end">{{ t("finder.card.added") }}</span>
                </template>
                <template v-else>
                  <Button
                    size="sm"
                    variant="outline"
                    :disabled="addingSet.has(p.model) || verifyCache[p.model] === 'checking'"
                    class="text-xs h-7 px-2"
                    @click="addToRecord(p.model)"
                  >
                    {{ addingSet.has(p.model) ? t("finder.card.adding") : verifyCache[p.model] === 'checking' ? t("finder.card.verifying") : t("finder.card.addRecord") }}
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
        <Button :disabled="nameQuery.trim().length < 3 || nameLoading" @click="doNameSearch">
          <span v-if="nameLoading" class="flex items-center gap-2">
            <Loader2 class="w-4 h-4 animate-spin" />
            {{ t("finder.name.searching") }}
          </span>
          <span v-else><Search class="size-4" /></span>
        </Button>
      </div>

      <p v-if="nameError" class="text-sm text-destructive">{{ nameError }}</p>

      <!-- Name results -->
      <template v-if="nameResults.length > 0">
        <p class="text-sm text-muted-foreground">{{ t("finder.name.results", { count: nameResults.length }) }}</p>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3">
          <Button
            v-for="(m, i) in nameResults"
            :key="i"
            variant="outline"
            class="flex gap-3 p-3 rounded-lg border border-border hover:border-primary/50 hover:bg-accent/30 transition-colors text-left w-full h-auto justify-start"
            :disabled="!m.persons[0]?.urls.faceImage"
            @click="openSimilarDialog(m)"
          >
            <!-- Person face thumbnail (first person) -->
            <img
              v-if="m.persons[0]?.urls.faceImage"
              :src="m.persons[0].urls.faceImage"
              class="w-14 h-14 rounded object-cover shrink-0 border border-border"
              :alt="m.name"
            />
            <div v-else class="w-14 h-14 rounded shrink-0 border border-border bg-muted flex items-center justify-center text-muted-foreground text-xs">
              {{ t("finder.card.noImage") }}
            </div>
            <div class="flex flex-col min-w-0 flex-1 relative">
              <!-- 平台名固定右上角 -->
              <span class="absolute top-0 right-0 text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground">
                {{ platformLabel(m.platform) }}
              </span>
              <span class="font-semibold text-sm truncate pr-16">{{ m.name }}</span>
              <div class="text-xs text-muted-foreground mt-0.5">{{ genderLabel(m.gender) }} · {{ t("finder.card.imageCount", { count: m.faces }) }}</div>
              <div class="text-xs text-muted-foreground">{{ t("finder.card.firstSeen") }}{{ formatDate(m.firstSeen) }}</div>
              <div class="text-xs text-muted-foreground">{{ t("finder.card.lastSeenShort") }}{{ formatDate(m.lastSeen) }}</div>
            </div>
          </Button>
        </div>
      </template>

      <div
        v-else-if="nameSearched && !nameLoading && !nameError"
        class="text-center text-muted-foreground py-8 text-sm"
      >
        {{ t("finder.name.noResults") }}
      </div>
    </div>

    <!-- ── Similar Face Dialog ── -->
    <Dialog v-model:open="dialogOpen">
      <DialogContent
        class="p-0 flex flex-col overflow-hidden"
        style="height: 90vh; width: calc(90vh * 16 / 9); max-width: 96vw;"
      >
        <!-- Header -->
        <div class="flex items-center gap-3 px-5 py-3 border-b border-border shrink-0">
          <img
            v-if="dialogFaceImage"
            :src="dialogFaceImage"
            class="w-8 h-8 rounded object-cover border border-border shrink-0"
            alt=""
          />
          <DialogTitle class="font-semibold text-sm">{{ t("finder.dialog.title", { name: dialogModel?.name }) }}</DialogTitle>
          <DialogDescription class="sr-only">{{ t("finder.dialog.description") }}</DialogDescription>
        </div>

        <!-- Body -->
        <div class="flex flex-1 min-h-0">
          <!-- Left: source model info -->
          <div class="w-52 shrink-0 border-r border-border flex flex-col gap-3 p-4">
            <img
              v-if="dialogFaceImage"
              :src="dialogFaceImage"
              class="w-full aspect-square rounded object-cover border border-border"
              alt=""
            />
            <div v-if="dialogModel" class="flex flex-col gap-1">
              <span class="font-semibold text-sm">{{ dialogModel.name }}</span>
              <span class="text-xs px-1.5 py-0.5 rounded bg-muted text-muted-foreground w-fit">
                {{ platformLabel(dialogModel.platform) }}
              </span>
              <span class="text-xs text-muted-foreground">{{ genderLabel(dialogModel.gender) }}</span>
              <span class="text-xs text-muted-foreground">{{ t("finder.card.imageCount", { count: dialogModel.faces }) }}</span>
              <a
                :href="dialogModel.urls.externalProfile"
                target="_blank"
                rel="noopener noreferrer"
                class="text-xs text-primary hover:underline mt-1"
              >{{ t("finder.dialog.profile") }}</a>
            </div>
            <Button
              variant="outline"
              class="mt-auto flex items-center gap-1.5 text-xs px-2 py-1 w-fit h-auto"
              :class="bgMode ? 'border-primary bg-primary/10 text-primary' : ''"
              @click="bgMode = !bgMode"
            >
              <ImageIcon class="w-3 h-3" />
              {{ t("finder.dialog.bgMode") }}
            </Button>
          </div>

          <!-- Right: results -->
          <div class="flex-1 min-w-0 flex flex-col">
            <!-- Loading -->
            <div v-if="dialogLoading" class="flex items-center justify-center gap-2 h-full text-muted-foreground text-sm">
              <Loader2 class="w-4 h-4 animate-spin" />
              {{ t("finder.dialog.searching") }}
            </div>

            <!-- Error -->
            <div v-else-if="dialogError" class="flex items-center justify-center h-full">
              <p class="text-sm text-destructive">{{ dialogError }}</p>
            </div>

            <!-- Results grid -->
            <template v-else-if="dialogResults.length > 0">
              <div class="px-4 pt-3 pb-2 text-xs text-muted-foreground shrink-0">
                {{ t("finder.dialog.results", { count: dialogResults.length }) }}
              </div>
              <div ref="dialogScrollEl" class="overflow-y-auto flex-1 px-4 pb-4 scrollbar-overlay">
                <div class="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-2">
                  <div
                    v-for="(p, i) in dialogResults"
                    :key="i"
                    class="flex flex-col rounded-lg border border-border hover:border-primary/50 hover:bg-accent/30 transition-colors overflow-hidden"
                  >
                    <a :href="p.urls.externalProfile" target="_blank" rel="noopener noreferrer">
                      <img
                        :src="bgMode ? p.urls.fullImage : p.urls.faceImage"
                        :class="['w-full object-cover', bgMode ? 'aspect-video' : 'aspect-square']"
                        :alt="p.model"
                      />
                    </a>
                    <div class="flex flex-col gap-1 p-2">
                      <div class="flex items-center gap-1 flex-wrap">
                        <a :href="p.urls.externalProfile" target="_blank" rel="noopener noreferrer" class="font-semibold text-xs truncate hover:underline">{{ p.model }}</a>
                        <span class="text-[10px] px-1 py-0.5 rounded bg-muted text-muted-foreground shrink-0">
                          {{ platformLabel(p.platform) }}
                        </span>
                      </div>
                      <div class="text-[10px]">
                        {{ t("finder.card.matchShort") }}<span :class="PROBABILITY_COLORS[p.probability] ?? ''">{{ probabilityLabel(p.probability) }}</span>
                        <span class="text-muted-foreground ml-1">{{ p.distance.toFixed(3) }}</span>
                      </div>
                      <template v-if="p.platform !== 'sc'">
                        <span class="text-[10px] text-muted-foreground text-center mt-0.5">{{ t("finder.card.scOnly") }}</span>
                      </template>
                      <template v-else-if="verifyCache[p.model] === 'not_found'">
                        <span class="text-[10px] text-destructive text-center mt-0.5">{{ t("finder.card.notFound") }}</span>
                      </template>
                      <template v-else-if="isAdded(p.model)">
                        <span class="text-[10px] text-muted-foreground text-center mt-0.5">{{ t("finder.card.added") }}</span>
                      </template>
                      <template v-else>
                        <Button
                          size="sm"
                          variant="outline"
                          :disabled="addingSet.has(p.model) || verifyCache[p.model] === 'checking'"
                          class="text-[10px] h-6 px-2 mt-0.5 w-full"
                          @click="addToRecord(p.model)"
                        >
                          {{ addingSet.has(p.model) ? t("finder.card.adding") : verifyCache[p.model] === 'checking' ? t("finder.card.verifying") : t("finder.card.addRecordLong") }}
                        </Button>
                      </template>
                    </div>
                  </div>
                </div>
              </div>
            </template>

            <div v-else class="flex items-center justify-center h-full text-muted-foreground text-sm">
              {{ t("finder.dialog.noResults") }}
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  </div>
</template>
