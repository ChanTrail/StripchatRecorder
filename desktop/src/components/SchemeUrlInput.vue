<!--
    协议 + 地址输入框 / Scheme + Address Input

    左侧下拉选择协议，右侧只填写地址（主机[:端口]，代理可带 user:pass@ 前缀）。
    v-model 仍是完整 URL 字符串（如 socks5://127.0.0.1:10808），与后端保存的格式一致，
    已保存的配置无需迁移；地址为空时 v-model 为 null。

    - kind="proxy"：代理，支持 http / socks5
    - kind="mirror"：镜像站，支持 https / http
    - 已保存的值若使用了列表之外的协议（如 socks5h），会临时多出对应选项，不会被悄悄改成别的协议
    - 向地址框粘贴带协议的完整 URL（如 socks5://127.0.0.1:10808）时，自动拆出协议并切换下拉框

    事件：
    - update:modelValue：协议或地址变化时触发（输入过程中持续触发）
    - commit：地址框失焦 / 按回车 / 切换协议时触发，用于"确认后保存"的场景

    Left: a dropdown to pick the scheme. Right: the address only (host[:port]; proxies may
    carry a user:pass@ prefix). The v-model stays a full URL string (e.g.
    socks5://127.0.0.1:10808), the same format the backend stores, so saved settings need no
    migration; it is null when the address is empty.

    - kind="proxy": proxy, supports http / socks5
    - kind="mirror": mirror site, supports https / http
    - A saved value with a scheme outside the list (e.g. socks5h) temporarily adds that option
      so it is never silently rewritten to another scheme
    - Pasting a full URL with a scheme (e.g. socks5://127.0.0.1:10808) into the address box
      splits the scheme out and switches the dropdown

    Events:
    - update:modelValue: fired whenever the scheme or address changes (continuously while typing)
    - commit: fired when the address box blurs / Enter is pressed / the scheme is switched,
      for "save on confirm" flows
-->
<script setup lang="ts">
	import { computed, nextTick, ref, watch } from "vue";
	import { Input } from "@/components/ui/input";
	import {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
	} from "@/components/ui/select";

	const props = defineProps<{
		/** 完整 URL（scheme://地址）；为空或 null 表示未设置 / Full URL (scheme://address); empty or null = unset */
		modelValue: string | null;
		/** 用途：代理（http / socks5）或镜像站（https / http）/ Purpose: proxy (http / socks5) or mirror (https / http) */
		kind: "proxy" | "mirror";
		/** 地址框占位文本（不含协议）/ Address box placeholder (without scheme) */
		placeholder?: string;
	}>();

	const emit = defineEmits<{
		(e: "update:modelValue", value: string | null): void;
		(e: "commit"): void;
	}>();

	/** 各用途可选的协议，第一项为默认值 / Schemes available per purpose; the first is the default */
	const SCHEMES = {
		proxy: ["socks5", "http"],
		mirror: ["https", "http"],
	} as const;

	/** 匹配开头的 `协议://` / Matches a leading `scheme://` */
	const SCHEME_RE = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/i;

	const defaultScheme = (): string => SCHEMES[props.kind][0];

	const scheme = ref(defaultScheme());
	const address = ref("");

	/** 下拉框选项：固定列表 + 当前值若在列表之外则追加 / Dropdown options: fixed list, plus the current scheme if it is outside the list */
	const schemeOptions = computed<string[]>(() => {
		const fixed: string[] = [...SCHEMES[props.kind]];
		return fixed.includes(scheme.value) ? fixed : [...fixed, scheme.value];
	});

	/** 由协议和地址拼出完整 URL，地址为空返回 null / Build the full URL; null when the address is empty */
	function compose(): string | null {
		const addr = address.value.trim();
		return addr ? `${scheme.value}://${addr}` : null;
	}

	function emitValue() {
		emit("update:modelValue", compose());
	}

	/**
	 * 外部值变化（初始化、其他客户端修改）时同步到内部状态。
	 * 与当前内部状态拼出的值一致时不动，保留用户已选的协议（例如地址为空时选好的协议）。
	 *
	 * Sync external value changes (init, edits from other clients) into the inner state.
	 * Does nothing when it already matches what the inner state composes, so a scheme the
	 * user picked is kept (e.g. one chosen while the address is still empty).
	 */
	function syncFromModel(value: string | null) {
		const next = value || null;
		if (compose() === next) return;
		if (next === null) {
			address.value = "";
			return;
		}
		const m = SCHEME_RE.exec(next);
		if (m) {
			scheme.value = m[1].toLowerCase();
			address.value = m[2];
		} else {
			// 旧配置可能没写协议：代理按 reqwest 的规则视为 http，镜像站用默认的 https
			// Older settings may carry no scheme: proxies are treated as http (reqwest's rule), mirrors use https
			scheme.value = defaultScheme();
			address.value = next;
		}
	}

	watch(() => props.modelValue, syncFromModel, { immediate: true });

	async function onAddressInput(v: string | number) {
		const text = String(v);
		const m = SCHEME_RE.exec(text);
		if (!m) {
			address.value = text;
			emitValue();
			return;
		}
		// 粘贴了带协议的完整 URL：拆出协议。先让输入框拿到原文再换成去掉协议的地址，
		// 否则地址本身没变时（只是多了协议前缀），输入框会残留前缀
		// A full URL with a scheme was pasted: split the scheme out. Hand the input the raw text
		// first, then the stripped address; otherwise the input keeps the prefix when the address
		// itself did not change
		scheme.value = m[1].toLowerCase();
		address.value = text;
		await nextTick();
		address.value = m[2];
		emitValue();
	}

	function onSchemeChange(v: unknown) {
		if (typeof v !== "string" || !v) return;
		scheme.value = v;
		emitValue();
		emit("commit");
	}
</script>

<template>
	<div class="flex items-center gap-1.5">
		<Select :model-value="scheme" @update:model-value="onSchemeChange">
			<SelectTrigger class="w-28 shrink-0"><SelectValue /></SelectTrigger>
			<SelectContent>
				<SelectItem v-for="s in schemeOptions" :key="s" :value="s">{{
					s
				}}</SelectItem>
			</SelectContent>
		</Select>
		<Input
			:model-value="address"
			:placeholder="placeholder"
			autocomplete="off"
			class="flex-1"
			@update:model-value="onAddressInput"
			@keyup.enter="emit('commit')"
			@blur="emit('commit')"
		/>
	</div>
</template>

