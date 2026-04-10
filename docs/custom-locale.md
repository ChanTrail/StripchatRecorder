# 自定义语言

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

本文档说明如何为 StripchatRecorder 添加新的界面语言或修改现有翻译。

---

## 文件结构

```
src/
├── i18n.ts               # i18n 初始化，注册语言包
└── locales/
    ├── zh-CN.ts          # 简体中文（默认）
    └── en-US.ts          # 英文
```

---

## 添加新语言

### 1. 创建语言文件

在 `src/locales/` 下新建文件，文件名使用 BCP 47 语言标签，例如 `ja-JP.ts`。

以 `zh-CN.ts` 为模板复制全部内容，将所有字符串翻译为目标语言：

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

### 2. 注册到 i18n

编辑 `src/i18n.ts`，导入新语言包并注册：

```ts
import { createI18n } from "vue-i18n";
import zhCN from "./locales/zh-CN";
import enUS from "./locales/en-US";
import jaJP from "./locales/ja-JP"; // 新增

export type MessageSchema = typeof zhCN;

const savedLocale = localStorage.getItem("locale") ?? "zh-CN";

const i18n = createI18n<[MessageSchema], "zh-CN" | "en-US" | "ja-JP">({
	// 新增类型
	legacy: false,
	locale: savedLocale,
	fallbackLocale: "zh-CN",
	messages: {
		"zh-CN": zhCN,
		"en-US": enUS,
		"ja-JP": jaJP, // 新增
	},
});

export default i18n;
```

### 3. 在设置页添加选项

找到设置页的语言选择组件，添加新选项。通常在 `src/views/SettingsView.vue` 或类似文件中，搜索 `zh-CN` / `en-US` 即可定位。

### 4. 在首次启动 TUI 中添加选项

编辑 `src-tauri/src/lib.rs`，在 `ask_mode_interactive` 函数中找到语言菜单部分，添加新选项：

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

## 翻译键说明

| 顶层键           | 说明                         |
| ---------------- | ---------------------------- |
| `nav`            | 侧边栏导航项                 |
| `common`         | 通用按钮文字（确认、取消等） |
| `notify`         | 系统通知与弹窗               |
| `home`           | 主播列表页                   |
| `streamerCard`   | 主播卡片组件                 |
| `addStreamer`    | 添加主播对话框               |
| `recordings`     | 录制文件页                   |
| `postprocess`    | 后处理流水线页               |
| `finder`         | 主播查找页                   |
| `settings`       | 设置页                       |
| `usePostprocess` | 后处理任务状态提示           |

插值变量使用 `{变量名}` 格式，翻译时保留占位符，例如：

```ts
// 原文
reconnected: "已重新连接到服务器，{n} 秒后刷新页面…";

// 翻译（保留 {n}）
reconnected: "サーバーに再接続しました。{n} 秒後にリロードします…";
```

---

## 修改现有翻译

直接编辑 `src/locales/zh-CN.ts` 或 `src/locales/en-US.ts`，保存后重新构建即可生效。

```bash
npm run dev   # 开发模式，热更新
npm run build # 生产构建
```
