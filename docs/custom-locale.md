# 自定义语言

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

本文档说明如何在 StripchatRecorder 中自定义界面翻译或添加新语言。

翻译现在从**磁盘上的 JSON 文件**读取，不再编译进程序。程序首次运行时会自动创建默认语言文件，可以随意编辑——更新程序不会覆盖你的修改。

---

## 目录结构

```
<exe_dir>/
└── locale/
    ├── app/                        # 主程序翻译
    │   ├── zh-CN.json              # 简体中文（默认）
    │   └── en-US.json              # 英文
    └── modules/                    # 模块翻译
        ├── filter_short/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── contact_sheet/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── notify_discord/
        │   ├── zh-CN.json
        │   └── en-US.json
        └── notify_telegram/
            ├── zh-CN.json
            └── en-US.json
```

> **注意：** 文件在首次运行时自动创建。若文件已存在则**永远不会被覆盖**，重启或更新程序后自定义内容均会保留。

---

## 修改现有翻译

直接编辑 `locale/app/` 或 `locale/modules/<module_id>/` 下对应语言的 JSON 文件即可。

前端在启动和每次切换语言时都会请求 `/api/locale/{code}`，因此修改后**刷新页面**立即生效，无需重新编译。

---

## 添加新语言

### 1. 创建语言文件

在 `locale/app/` 下以 BCP 47 语言标签为文件名新建 JSON 文件，例如 `ja-JP.json`。

以 `zh-CN.json` 为模板，翻译所有字符串值（保持所有 key 和 `{变量}` 占位符不变），并设置 `languageName` 字段为该语言的自身显示名称：

```json
{
  "languageName": "日本語",
  "nav": {
    "streamers": "配信者",
    ...
  },
  ...
}
```

**完成，无需修改任何代码。** 程序重启后，`/api/locales` 会自动包含新语言，设置页和初始配置向导的语言列表也会自动更新。

---

## 模块翻译

每个模块在 `locale/modules/<module_id>/` 下有独立文件夹。在其中放置 `{语言代码}.json` 文件，格式如下：

```json
{
  "name": "我的模块",
  "description": "模块功能描述",
  "params": {
    "param_key": { "label": "参数标签" }
  }
}
```

所有字段均为可选——未填写的字段会回退到模块 `--describe` JSON 中的对应值。

> **优先级：** 服务器端 locale JSON > `--describe` 中的 i18n 字段 > 原始默认值

对于第三方模块（不在内置列表中），只需在 `locale/modules/` 下新建以模块 `id` 命名的文件夹并放入 JSON 文件，系统会自动发现。

---

## 翻译键参考

| 顶层 key         | 说明                          |
| ---------------- | ----------------------------- |
| `nav`            | 侧边栏导航标签                |
| `common`         | 通用按钮文字（确认、取消…）  |
| `notify`         | 系统通知和对话框              |
| `home`           | 主播列表页                    |
| `streamerCard`   | 主播卡片组件                  |
| `addStreamer`    | 添加主播对话框                |
| `recordings`     | 录制文件页                    |
| `postprocess`    | 后处理流水线页                |
| `relay`          | 转发流页                      |
| `finder`         | 主播查找页                    |
| `settings`       | 设置页                        |
| `usePostprocess` | 后处理任务状态消息            |
| `setup`          | 首次启动配置向导              |

插值变量使用 `{变量名}` 格式，翻译时保持占位符不变：

```json
"reconnected": "已重新连接到服务器，{n} 秒后刷新页面…"
```
