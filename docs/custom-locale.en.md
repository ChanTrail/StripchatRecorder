# Custom Locale

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

This document explains how to add a new UI language or modify existing translations in StripchatRecorder.

---

## File Structure

```
src/
├── i18n.ts               # i18n initialization, registers locale bundles
└── locales/
    ├── zh-CN.ts          # Simplified Chinese (default)
    └── en-US.ts          # English
```

---

## Adding a New Language

### 1. Create the locale file

Create a new file under `src/locales/` using a BCP 47 language tag as the filename, e.g. `ja-JP.ts`.

Copy the full contents of `zh-CN.ts` as a template and translate all strings:

```ts
// src/locales/ja-JP.ts
export default {
	nav: {
		streamers: "配信者",
		recordings: "録画",
		// ...
	},
	// ...
};
```

### 2. Register in i18n

Edit `src/i18n.ts` to import and register the new locale:

```ts
import { createI18n } from "vue-i18n";
import zhCN from "./locales/zh-CN";
import enUS from "./locales/en-US";
import jaJP from "./locales/ja-JP"; // add

export type MessageSchema = typeof zhCN;

const savedLocale = localStorage.getItem("locale") ?? "zh-CN";

const i18n = createI18n<[MessageSchema], "zh-CN" | "en-US" | "ja-JP">({
	// add type
	legacy: false,
	locale: savedLocale,
	fallbackLocale: "zh-CN",
	messages: {
		"zh-CN": zhCN,
		"en-US": enUS,
		"ja-JP": jaJP, // add
	},
});

export default i18n;
```

### 3. Add the option in the Settings page

The language selector is in `src/views/SettingsView.vue`, inside the `setLocale` function and the `RadioGroup` component. Search for `lang-zh` / `lang-en` to locate it and add a new entry following the same pattern:

```vue
<div class="flex items-center gap-2">
  <RadioGroupItem id="lang-ja" value="ja-JP" />
  <Label for="lang-ja" class="cursor-pointer">日本語</Label>
</div>
```

Also update the type assertion in the `setLocale` function to include the new locale:

```ts
function setLocale(lang: string) {
  locale.value = lang as "zh-CN" | "en-US" | "ja-JP";
  localStorage.setItem("locale", lang);
}
```

### 4. Add the option in the first-launch TUI

Edit `src-tauri/src/lib.rs`, find the language menu inside `ask_mode_interactive`, and add the new entry:

```rust
let lang_items = ["中文 (zh-CN)", "English (en-US)", "日本語 (ja-JP)"];
// ...
let (lang_code, lang_en) = match lang_idx {
    1 => ("en-US", true),
    2 => ("ja-JP", false),
    _ => ("zh-CN", false),
};
```

---

## Translation Key Reference

| Top-level key    | Description                           |
| ---------------- | ------------------------------------- |
| `nav`            | Sidebar navigation labels             |
| `common`         | Shared button text (confirm, cancel…) |
| `notify`         | System notifications and dialogs      |
| `home`           | Streamers list page                   |
| `streamerCard`   | Streamer card component               |
| `addStreamer`    | Add streamer dialog                   |
| `recordings`     | Recordings page                       |
| `postprocess`    | Post-processing pipeline page         |
| `relay`          | Relay streams page                    |
| `finder`         | Streamer finder page (includes `gender` sub-key) |
| `settings`       | Settings page                         |
| `usePostprocess` | Post-processing task status messages  |

Interpolation variables use the `{variableName}` format. Keep placeholders as-is when translating:

```ts
// Original
reconnected: "Reconnected to server, reloading in {n} second(s)…";

// Translation (keep {n})
reconnected: "サーバーに再接続しました。{n} 秒後にリロードします…";
```

---

## Modifying Existing Translations

Edit `src/locales/zh-CN.ts` or `src/locales/en-US.ts` directly, then rebuild:

```bash
npm run dev   # dev mode with hot reload
npm run build # production build
```

---

## Module Internationalization

Post-processing modules provide their own translations by declaring an `i18n` field in the `--describe` JSON output. No frontend changes are required. See the [Module Development Guide](module-development.en.md#internationalization-i18n) for details.
