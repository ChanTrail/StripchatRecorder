<!--
    管理员登录 / 设置密码页面 / Admin Login & Set-Password Page

    - password_set=false → 显示"设置密码"表单（老版本升级兼容）
    - password_set=true  → 显示登录表单

    - password_set=false → show "Set password" form (upgrade compatibility for existing users)
    - password_set=true  → show login form
-->
<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { useRouter, useRoute } from "vue-router";
import { useI18n } from "vue-i18n";
import { useAuthStore } from "@/stores/auth";
import { useNotificationsStore } from "@/stores/notifications";
import { useSettingsStore } from "@/stores/settings";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const router = useRouter();
const route = useRoute();
const { t } = useI18n();
const auth = useAuthStore();
const notificationsStore = useNotificationsStore();
const settingsStore = useSettingsStore();

/** null = 正在加载 / null = loading */
const passwordSet = ref<boolean | null>(null);
const password = ref("");
const error = ref("");
const loading = ref(false);

const isInit = computed(() => passwordSet.value === false);

onMounted(async () => {
	try {
		const status = await auth.fetchStatus();
		passwordSet.value = status.password_set;
	} catch {
		error.value = t("login.statusFetchFailed");
	}
});

async function submit() {
	error.value = "";
	const pwd = password.value.trim();
	if (!pwd) {
		error.value = t("login.passwordRequired");
		return;
	}
	if (
		isInit.value &&
		(pwd.length < 6 || !/[a-zA-Z]/.test(pwd) || !/[0-9]/.test(pwd) || !/[^a-zA-Z0-9]/.test(pwd))
	) {
		error.value = t("login.passwordStrength");
		return;
	}

	loading.value = true;
	try {
		if (isInit.value) {
			// 老版本升级：先设置密码，再登录
			// Upgrade path: set password first, then login
			await auth.initPassword(pwd);
		}
		await auth.login(pwd);
		// 登录成功后立即拉取通知，补回登录前因 401 拉取失败的启动期通知
		// Re-fetch notifications after login to recover any missed during pre-login 401 phase
		await notificationsStore.fetch();
		const redirect = route.query.redirect;
		const target = typeof redirect === "string" && redirect.startsWith("/") ? redirect : "/";
		await router.replace(target);
	} catch (e: unknown) {
		error.value = isInit.value ? String(e) : t("login.wrongPassword");
	} finally {
		loading.value = false;
	}
}

/**
 * 不设置密码，直接进入：settings 标记 admin_password_disabled 后跳首页；
 * 无密码时鉴权中间件整体放行，此后访问不再被路由守卫拦到登录页。
 *
 * Continue without a password: flags admin_password_disabled in settings
 * then goes home; with no password the auth middleware passes everything,
 * and the router guard no longer bounces to /login.
 */
async function continueWithoutPassword() {
	error.value = "";
	loading.value = true;
	try {
		await settingsStore.fetchSettings();
		await settingsStore.saveSettings({
			...settingsStore.settings,
			admin_password_disabled: true,
		});
		// 同步 store 与本页状态（store 上 ref 已解包，直接赋值）/
		// Sync store and local state (store refs are unwrapped — assign directly)
		passwordSet.value = false;
		auth.passwordSet = false;
		auth.passwordDisabled = true;
		const redirect = route.query.redirect;
		const target = typeof redirect === "string" && redirect.startsWith("/") ? redirect : "/";
		await router.replace(target);
	} catch (e: unknown) {
		error.value = String(e);
	} finally {
		loading.value = false;
	}
}
</script>

<template>
	<!-- min-h-dvh：同 App.vue 根布局的说明，避免移动端 vh/小视口错位 /
		min-h-dvh: same rationale as the App.vue root — avoids the mobile
		vh vs. small-viewport mismatch -->
	<div class="min-h-dvh flex items-center justify-center bg-background p-6">
		<div class="w-full max-w-sm flex flex-col gap-6">
			<div class="flex flex-col gap-1.5">
				<div class="flex items-center gap-2.5">
					<img src="/icon.png" alt="icon" class="w-6 h-6 shrink-0" />
					<span class="text-lg font-bold">StripchatRecorder</span>
				</div>
				<h1 class="text-2xl font-bold mt-1">
					{{ isInit ? t("login.initTitle") : t("login.title") }}
				</h1>
				<p class="text-sm text-muted-foreground">
					{{ isInit ? t("login.initSubtitle") : t("login.subtitle") }}
				</p>
			</div>

			<!-- 加载中 / Loading -->
			<p v-if="passwordSet === null && !error" class="text-sm text-muted-foreground">
				{{ t("common.loading") }}
			</p>

			<!-- 表单 / Form -->
			<form
				v-else-if="passwordSet !== null"
				class="flex flex-col gap-4"
				@submit.prevent="submit"
			>
				<!-- 无障碍：密码表单需要 username 字段 / Accessibility: password forms require a username field -->
				<input
					type="text"
					name="username"
					value="admin"
					autocomplete="username"
					class="sr-only"
					aria-hidden="true"
					tabindex="-1"
				/>
				<div class="flex flex-col gap-1.5">
					<Label for="password">{{ t("login.passwordLabel") }}</Label>
					<Input
						id="password"
						v-model="password"
						type="password"
						:autocomplete="isInit ? 'new-password' : 'current-password'"
						:placeholder="
							isInit ? t('login.initPlaceholder') : t('login.passwordPlaceholder')
						"
						autofocus
					/>
					<p v-if="isInit" class="text-xs text-muted-foreground">
						{{ t("login.passwordStrengthHint") }}
					</p>
				</div>

				<!-- 错误行常驻占位：登录失败时不挤压提交按钮位置 / Error line always rendered: a failed login doesn't shift the submit button -->
				<p class="min-h-5 text-sm text-destructive">{{ error }}</p>

				<Button type="submit" :disabled="loading" class="w-full">
					{{
						loading
							? t("common.loading")
							: isInit
								? t("login.initButton")
								: t("login.loginButton")
					}}
				</Button>
				<!-- 初次设密可跳过：进入无密码模式（标记 admin_password_disabled）/
					First-time setup can be skipped: enter no-password mode (flags admin_password_disabled) -->
				<Button
					v-if="isInit"
					type="button"
					variant="ghost"
					:disabled="loading"
					class="w-full"
					@click="continueWithoutPassword"
				>
					{{ t("login.continueWithoutPassword") }}
				</Button>
			</form>

			<!-- 连接失败 / Connection failed -->
			<p v-if="error && passwordSet === null" class="text-sm text-destructive">{{ error }}</p>
		</div>
	</div>
</template>
