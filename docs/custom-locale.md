# 自定义语言

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

本文档说明如何在 StripchatRecorder 中自定义界面翻译或添加新语言。

翻译从**磁盘上的 JSON 文件**读取，不编译进程序。程序首次运行时会自动创建默认语言文件，可以随意编辑——更新程序不会覆盖你的修改。

---

## 目录结构

```
<exe_dir>/
└── locale/
    ├── .builtin-state.json         # 内置文件托管状态（内容指纹，程序自动维护）
    ├── app/                        # 主程序翻译
    │   ├── zh-CN.json              # 简体中文（默认）
    │   └── en-US.json              # 英文
    ├── log/                        # 后端日志翻译（影响日志输出语言）
    │   ├── zh-CN.json
    │   └── en-US.json
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
        ├── notify_telegram/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── cleanup/
        │   ├── zh-CN.json
        │   └── en-US.json
        └── __builtin__/            # 内置 DAG 节点翻译（recording_input、unpack）
            ├── zh-CN.json
            └── en-US.json
```

> **注意：** 文件在首次运行时自动创建。**内置语言文件**（`app/`、`log/`、`modules/<内置模块id>/`）带指纹版本化管理：程序在 `locale/.builtin-state.json` 中记录每次写出的内容指纹——你**未修改过**的内置文件会随程序升级自动刷新为新文案；**修改过**的文件会被识别为自定义（指纹不匹配，标记 `"custom"`）并**永远不会被覆盖**。你自己创建的文件（第三方模块目录、自定义语言代码）不受托管，同样不会被覆盖。
>
> 例外：若内置语言文件存在但 JSON 校验失败，程序会先备份（`xxx.json.bak`，已被占用时带时间戳）再重建为默认内容。自定义语言文件校验失败时仅记录警告，不重建。
>
> 从旧版本升级后首次运行该机制时：没有指纹记录的存量文件若与默认内容一致会被直接认领；不一致则无法区分「旧默认」与「用户自定义」，会先备份为 `.bak` 再刷新为新默认——如有手动修改请从备份文件找回。

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

> **回退语言：** 简体中文。语言文件里没有翻译的条目（包括升级后新增的文案）会显示简体中文，不会显示 key 名；界面、后端日志和模块翻译都按这个规则。英文缺少的条目同样显示简体中文。

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

对于第三方模块（不在内置列表中），只需在 `locale/modules/` 下新建以模块 `id` 命名的文件夹并放入 JSON 文件，系统会自动发现。当前语言没有对应文件时，使用该文件夹下的 `zh-CN.json`。

### 内置 DAG 节点翻译（`__builtin__`）

`__builtin__` 模块翻译文件采用**按节点分组的嵌套结构**，与普通模块的扁平结构不同：

```json
{
  "recording_input": {
    "name": "录制输入",
    "description": "虚拟录制输入节点，始终作为流水线的起点"
  },
  "unpack": {
    "name": "解组媒体包",
    "description": "将媒体包拆分为视频文件（端口 0）和图片文件（端口 1）"
  }
}
```

---

## 翻译键参考

| 顶层 key         | 说明                                      |
| ---------------- | ----------------------------------------- |
| `nav`            | 侧边栏导航标签                            |
| `common`         | 通用按钮文字（确认、取消…）              |
| `notify`         | 系统通知和对话框（断线提示、缺失文件等）  |
| `home`           | 主播列表页                                |
| `streamerCard`   | 主播卡片组件                              |
| `addStreamer`    | 添加主播对话框                            |
| `recordings`     | 录制文件页                                |
| `postprocess`    | 后处理流水线页                            |
| `finder`         | 主播查找页                                |
| `statusPage`     | 服务状态页                                |
| `usagePage`      | 资源占用页                                |
| `settings`       | 设置页                                    |
| `usePostprocess` | 后处理任务状态消息                        |
| `relay`          | 转发流页                                  |
| `setup`          | 首次启动配置向导                          |
| `login`          | 登录页（密码输入、初次设置密码）          |
| `notifications`  | 应用内通知面板                            |
| `dirBrowser`     | 目录选择器对话框                          |
| `streamers`      | 主播相关事件提示（改名通知、API 错误等）  |
| `stripchat`      | StripChat 浏览页                          |
| `community`      | 社区模块市场页                            |
| `about`          | 关于页面（版本信息、更新检查）            |
| `ffmpeg`         | FFmpeg 下载进度与首选项向导               |

插值变量使用 `{变量名}` 格式，翻译时保持占位符不变：

```json
"reconnected": "已重新连接到服务器，{n} 秒后刷新页面…"
```
