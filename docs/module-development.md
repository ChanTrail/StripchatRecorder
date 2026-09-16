# 后处理模块开发文档

[简体中文](module-development.md) | [English](module-development.en.md)

本文档描述如何为 StripchatRecorder 编写自定义后处理模块。

---

## 目录

1. [概述](#概述)
2. [模块协议](#模块协议)
3. [元数据描述](#元数据描述)
4. [参数类型](#参数类型)
5. [端口类型系统](#端口类型系统)
6. [多语言支持](#多语言支持)
7. [流水线机制](#流水线机制)
8. [进度上报](#进度上报)
9. [pp_utils 工具库](#pp_utils-工具库)
10. [完整示例](#完整示例)
11. [部署模块](#部署模块)
12. [发布到社区](#发布到社区)
13. [内置模块](#内置模块)
14. [注意事项](#注意事项)

---

## 概述

后处理模块是**独立的可执行文件**，由主程序在录制完成后按流水线顺序依次调用。每个模块通过 **stdin JSON** 接收输入，通过 **stdout** 与主程序通信。模块可以用任何语言编写，只要遵守本文档定义的协议即可。

---

## 模块协议

### 调用约定

模块必须支持两种调用模式：

| 模式     | 命令                    | 说明                                |
| -------- | ----------------------- | ----------------------------------- |
| 描述模式 | `./<module> --describe` | 输出模块元数据 JSON，不执行任何处理 |
| 执行模式 | `./<module>`            | 从 stdin 读取 JSON，执行处理逻辑   |

> **文件名格式要求：** 主程序只会发现并加载符合 `{name}-{platform}-{version}` 格式的可执行文件（Windows 含 `.exe` 后缀），例如 `my_module-linux-x86_64-1.0.0`。不符合此格式的文件将被忽略。

### stdin 输入 JSON

执行模式下，主程序通过 stdin 向模块传入一个 JSON 对象：

```json
{
  "inputs": ["/recordings/alice_20240101_120000.ts"],
  "params": {
    "param_key": "param_value"
  },
  "exe_dir": "/app/stripchat-recorder",
  "max_tmp_mb": 51200,
  "recording": {
    "video_path": "/recordings/alice_20240101_120000.ts",
    "started_at": "2024-01-01T12:00:00Z",
    "username": "alice"
  }
}
```

| 字段         | 说明                                                                          |
| ------------ | ----------------------------------------------------------------------------- |
| `inputs`     | 输入路径列表，按端口索引顺序排列；单输入模块取 `inputs[0]`                   |
| `params`     | 模块参数键值对（与 `--describe` 的 `params[].key` 对应）                     |
| `exe_dir`    | 后端可执行文件所在目录（`pp_utils::tmp_dir()` 据此定位临时目录）             |
| `max_tmp_mb` | 临时目录最大占用（MB），`pp_utils::tmp_dir()` 会据此自动清理旧文件           |
| `recording`  | 录制上下文（`video_path`、`started_at`、`username`，供通知类模块使用）       |

> **兼容旧协议：** 若 stdin 为空或解析失败，`pp_utils::ModuleInput::read()` 会自动回退到旧的环境变量协议（`PP_INPUT`、`PP_PARAM_<KEY>`、`PP_EXE_DIR`），以兼容老版本调用方式。

### stdout 协议

模块通过向 stdout 写入内容与主程序通信。有两种类型的输出行：

**进度行**（随时可输出，多条）：

| 输出行                    | 说明                                                                       |
| ------------------------- | -------------------------------------------------------------------------- |
| `PROGRESS:<done>/<total>` | 进度上报，`done` 和 `total` 均为整数，`total` 固定为 `10000`（50% = 5000）|
| `STATUS:<text>`           | 可选的状态文字（如上传速度），显示在 UI 的进度条旁                        |

**最终结果行**（最后一行，必须输出一次）：

```json
{"code": "ok", "message": "处理完成", "outputs": ["/recordings/alice_20240101_120000.ts"]}
```

| `code` 值  | 说明                                                                              |
| ---------- | --------------------------------------------------------------------------------- |
| `"ok"`     | 成功，`outputs` 中的路径将作为下游节点的输入                                     |
| `"done"`   | 成功且流水线在此终止（如 `cleanup` 模块删除文件后不需要下游继续），`outputs` 为空 |
| `"skipped"`| 已跳过处理（如输出文件已存在），`outputs` 中照常传递路径                          |
| `"error"`  | 失败，流水线中止，后续节点不再执行                                                |

> `pp_utils` 提供了 `output_ok()`、`output_done()`、`output_skipped()` 函数直接输出对应格式。

### stderr

所有诊断信息、警告和错误消息应写入 stderr。主程序会捕获 stderr 内容并在模块失败时展示给用户。

### 退出码

| 退出码 | 含义                               |
| ------ | ---------------------------------- |
| `0`    | 成功（结合最终 JSON 的 `code` 判断）|
| 非零   | 失败，流水线中止，后续模块不再执行 |

---

## 元数据描述

当以 `--describe` 参数调用时，模块必须向 stdout 打印一个 JSON 对象，然后以退出码 `0` 退出。

### JSON 结构

```jsonc
{
  "id": "my_module",            // 模块唯一标识符，仅含小写字母、数字和下划线
  "name": "我的模块",           // UI 中显示的名称
  "description": "模块功能描述", // UI 中显示的简短描述
  "version": "1.0.0",           // 版本号（由 pp_utils::describe_with_version 注入，无需手写）
  "inputTypes": ["video_file"], // 输入端口类型列表，见端口类型系统
  "outputTypes": ["video_file"],// 输出端口类型列表，留空表示流水线在此终止
  "official": false,            // 是否为官方模块（官方模块建议置于 ts_merge 之后）
  "params": [
    {
      "key": "param_key",       // 参数键名，对应 JSON 中 params.param_key
      "label": "参数标签",       // UI 中显示的标签
      "type": "string",         // 参数类型，见下文
      "default": ""             // 默认值
    }
  ]
}
```

### 字段说明

| 字段          | 类型    | 必填 | 说明                                                         |
| ------------- | ------- | ---- | ------------------------------------------------------------ |
| `id`          | string  | 是   | 模块唯一 ID，同一实例中不可重复                              |
| `name`        | string  | 是   | UI 显示名称                                                  |
| `description` | string  | 是   | UI 显示描述                                                  |
| `inputTypes`  | array   | 是   | 输入端口类型列表（顺序对应 `inputs[]` 索引），见[端口类型系统](#端口类型系统) |
| `outputTypes` | array   | 是   | 输出端口类型列表（顺序对应 `outputs[]` 索引），为空表示流水线在此终止  |
| `official`    | boolean | 否   | 是否为官方模块（UI 中会提示应置于 `ts_merge` 之后），默认 `false` |
| `params`      | array   | 是   | 参数定义列表，无参数时传空数组                               |

> **版本号：** 不要在 `DESCRIBE` 常量中手写 `"version"` 字段。使用 `pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION"))` 在运行时自动从 `Cargo.toml` 注入，只需维护一处。

---

## 参数类型

`params` 数组中每个对象的 `type` 字段决定 UI 渲染方式：

| 类型      | UI 控件      | 值格式                       |
| --------- | ------------ | ---------------------------- |
| `string`  | 文本输入框   | 任意字符串                   |
| `number`  | 数字输入框   | 十进制整数或浮点数字符串     |
| `boolean` | 开关         | `true` 或 `false`（JSON）    |
| `select`  | 下拉选择框   | 选项值字符串                 |
| `dir`     | 目录选择器   | 目录绝对路径字符串           |

`select` 类型需额外提供 `options` 字段：

```jsonc
{
  "key": "format",
  "label": "输出格式",
  "type": "select",
  "default": "mp4",
  "options": ["mp4", "mkv", "ts"]
}
```

`number` 类型可选 `min` / `max` 约束：

```jsonc
{
  "key": "quality",
  "label": "图片质量",
  "type": "number",
  "default": 85,
  "min": 1,
  "max": 100
}
```

---

## 端口类型系统

每个模块通过 `inputTypes` 和 `outputTypes` 声明其输入/输出端口的数据类型。在可视化流水线编辑器中，只有类型兼容的端口才能相互连接。

| 类型标识符      | 说明                                                        |
| --------------- | ----------------------------------------------------------- |
| `ts_session_dir`| TS 分片录制目录（由录制系统产生，作为 `ts_merge` 的输入）  |
| `video_file`    | 单个视频文件（mp4 / mkv / ts 等）                          |
| `image_file`    | 单个图片文件（webp / jpg / png 等）                        |
| `media_bundle`  | 媒体包（视频路径 + 图片路径，以 `\n` 分隔的单一路径字符串）|
| `any_file`      | 任意文件（可接受上游任何文件类型，包括 `media_bundle`）    |
| `any_dir`       | 任意目录（可接受上游任何目录类型）                         |

**兼容规则：**
- 完全相同的类型可以连接
- 任何文件类型（含 `media_bundle`）都可以连接到 `any_file`
- 任何目录类型都可以连接到 `any_dir`

**多输入/多输出端口：** `inputTypes` / `outputTypes` 按索引顺序对应 `inputs[]` / `outputs[]`，例如 `inputTypes: ["video_file", "image_file"]` 表示模块有两个输入端口，`inputs[0]` 为视频，`inputs[1]` 为图片。

若模块不声明 `inputTypes`/`outputTypes`（旧版兼容），主程序默认为 `["any_file"]`。

---

## 多语言支持

模块可以在 `--describe` JSON 中声明可选的 `i18n` 字段，为 `name`、`description` 和参数 `label` 提供多语言翻译。主程序会根据用户当前的界面语言自动选择对应翻译，找不到时回退到原始字段值。

### JSON 结构

```jsonc
{
  "id": "my_module",
  "name": "我的模块",          // 默认语言（中文）
  "description": "模块功能描述",
  "params": [
    {
      "key": "dest_dir",
      "label": "目标目录路径",
      "type": "dir",
      "default": ""
    }
  ],
  "i18n": {
    "en-US": {
      "name": "My Module",
      "description": "Module description",
      "params": {
        "dest_dir": { "label": "Destination Directory" }
      }
    }
    // 可继续添加其他语言，如 "ja-JP": { ... }
  }
}
```

优先级：**服务器端 `locale/modules/<id>/` 目录下的 JSON 文件** > `--describe` 的 `i18n` 字段 > 原始默认值。

---

## 流水线机制

流水线是一个 **DAG（有向无环图）**，由节点和连线组成，支持分叉与合并。主程序按拓扑顺序执行每个启用的节点：

1. 上游节点 `outputs[]` 中的路径，按连线定义的端口索引传入下游节点的 `inputs[]`。
2. 流水线的起始节点为 `recording_input` 内置节点，输出类型为 `ts_session_dir`，接收当次录制产生的 TS 分片目录。
3. 若模块返回 `{"code": "error"}` 或退出码非零，流水线立即中止。
4. 若模块返回 `{"code": "done"}` 或 `outputs` 为空，该分支在此终止，不继续向下游传递。

```
recording_input（TS 分片目录）
    │
    ▼  inputs[0] = /ts_fragment/alice_20240101_120000/
┌──────────┐
│ ts_merge │── outputs[0] = /recordings/alice/alice_20240101_120000.mp4
└──────────┘
    │
    ▼  inputs[0] = /recordings/alice/alice_20240101_120000.mp4
┌──────────────┐
│ filter_short │── 时长不足 → code: "done"（流水线终止）
│              │── 时长满足 → outputs[0] = 同路径
└──────────────┘
    │
    ▼  inputs[0] = /recordings/alice/alice_20240101_120000.mp4
┌───────────────┐
│ contact_sheet │── outputs[0] = media_bundle（视频路径\n图片路径）
└───────────────┘
    │
    ▼  inputs[0] = media_bundle
┌──────────────────┐
│ __builtin__unpack│── outputs[0] = video_file, outputs[1] = image_file（分叉）
└──────────────────┘
   │                └──────────────────────────────────┐
   ▼ video_file                                        ▼ image_file
┌────────────────┐                         ┌─────────────────────┐
│ notify_discord │                         │    test_suffix      │
└────────────────┘                         └─────────────────────┘
```

---

## 进度上报

进度通过向 stdout 输出 `PROGRESS:<done>/<total>` 行来上报，其中 `total` 固定为 `10000`。

```
PROGRESS:0/10000      # 0%
PROGRESS:5000/10000   # 50%
PROGRESS:10000/10000  # 100%
```

**注意事项：**
- 主程序取所有上报值中的最大值，进度不会倒退。
- 建议在处理开始前输出 `PROGRESS:0/10000`，结束后输出 `PROGRESS:10000/10000`。
- 对于固定步骤的任务，可按步骤均分进度（如 3 步：0、3333、6666、10000）。

---

## pp_utils 工具库

`pp_utils` 是项目内置的 Rust 工具库，封装了所有模块常用的功能。使用 Rust 编写模块时强烈建议依赖此库。

在 `Cargo.toml` 中引入：

```toml
[dependencies]
pp_utils = { path = "../pp_utils" }
```

### 读取输入（新协议）

```rust
use pp_utils::ModuleInput;

// 从 stdin 读取并解析 JSON，自动回退到旧环境变量协议
let input = ModuleInput::read();

// 获取第一个输入路径（单输入模块）
let path = input.first_input().ok_or("inputs[0] required")?;

// 读取参数
let dest_dir: String  = input.param_str("dest_dir", "");
let interval: u32     = input.param_u32("interval", 30);
let min_dur: f64      = input.param_f64("min_duration", 60.0);
let dry_run: bool     = input.param_bool("dry_run", false);

// 获取录制上下文（通知类模块使用）
if let Some(rec) = &input.recording {
    let username = rec.get("username").and_then(|v| v.as_str()).unwrap_or("");
}
```

### 输出最终结果（新协议）

```rust
use pp_utils::{output_ok, output_done, output_skipped};

// 成功，传递输出路径给下游
output_ok(&[&output_path.to_string_lossy()], "处理完成");

// 流水线终止（如 cleanup 模块删除文件后）
output_done("已清理相关文件");

// 跳过处理，透传输入
output_skipped(&input_path.to_string_lossy(), "输出已存在，跳过");
```

### 视频工具

```rust
use pp_utils::{video_duration, video_meta};
use std::path::Path;

// 通过 ffprobe 获取视频时长（秒）
let duration: Option<f64> = video_duration(Path::new("/path/to/video.ts"));

// 一次获取时长、宽度和高度
let meta: Option<(f64, i32, i32)> = video_meta(Path::new("/path/to/video.ts"));
// Some((duration_secs, width, height))
```

### 格式化工具

```rust
use pp_utils::{format_duration, format_bytes, format_speed};

format_duration(3661.0);   // "01:01:01"
format_bytes(1_500_000);   // "1.43 MB"
format_speed(1_048_576.0); // "↑ 1.0 MB/s"
```

### 文件名解析

录制文件名格式为 `{model_name}_{YYYYMMDD}_{HHmmss}`。

```rust
use pp_utils::parse_stem;

let (model, timestamp) = parse_stem("alice_20240101_120000");
// model = "alice", timestamp = "2024-01-01 12:00:00"

// 含下划线的主播名同样支持
let (model, timestamp) = parse_stem("my_streamer_20240101_120000");
// model = "my_streamer", timestamp = "2024-01-01 12:00:00"
```

### 封面图查找

在视频同目录下查找同名封面图，支持 `jpg`、`jpeg`、`webp`、`png`。

```rust
use pp_utils::find_cover;
use std::path::Path;

let cover: Option<PathBuf> = find_cover(Path::new("/recordings/alice_20240101_120000.ts"));
```

### 图片元数据

```rust
use pp_utils::image_dimensions;
use std::path::Path;

let dims: Option<(u32, u32)> = image_dimensions(Path::new("/recordings/cover.webp"));
// Some((1280, 720))
```

### 临时目录

返回 `{后端可执行文件目录}/tmp/`，目录自动创建，若设置了 `max_tmp_mb` 会自动清理超出部分。

```rust
use pp_utils::tmp_dir;

let tmp: PathBuf = tmp_dir();
// 例如 /app/stripchat-recorder/tmp/
```

### 进度上报

```rust
use pp_utils::{emit_progress, emit_progress_step, PROGRESS_SCALE};

// 按已完成量/总量上报（自动缩放到 10000）
emit_progress(0, 100);   // PROGRESS:0/10000
emit_progress(50, 100);  // PROGRESS:5000/10000
emit_progress(100, 100); // PROGRESS:10000/10000

// 按固定步骤上报
emit_progress_step(0, 3); // PROGRESS:0/10000
emit_progress_step(1, 3); // PROGRESS:3333/10000
emit_progress_step(2, 3); // PROGRESS:6667/10000
emit_progress_step(3, 3); // PROGRESS:10000/10000
```

### 注入版本号

```rust
// 在 --describe 模式下使用，自动把 Cargo.toml 中的 version 注入到 JSON
print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
```

---

## 完整示例

以下是一个完整的 Rust 模块示例，功能为将视频文件复制到指定目录。

### `Cargo.toml`

```toml
[package]
name = "copy_to_dir"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "copy_to_dir"
path = "src/main.rs"

[dependencies]
pp_utils = { path = "../pp_utils" }
```

### `src/main.rs`

```rust
use pp_utils::{emit_progress_step, output_ok, ModuleInput};

const DESCRIBE: &str = r#"{
  "id": "copy_to_dir",
  "name": "复制到目录",
  "description": "将录制文件复制到指定目录",
  "inputTypes": ["video_file"],
  "outputTypes": ["video_file"],
  "official": false,
  "params": [
    {
      "key": "dest_dir",
      "label": "目标目录路径",
      "type": "dir",
      "default": ""
    }
  ],
  "i18n": {
    "en-US": {
      "name": "Copy to Directory",
      "description": "Copies the recording to a specified directory",
      "params": {
        "dest_dir": { "label": "Destination Directory" }
      }
    }
  }
}"#;

fn run() -> Result<(), String> {
    let input = ModuleInput::read();
    let input_path = input.first_input()
        .ok_or("inputs[0] is required")?;

    if !input_path.exists() {
        return Err(format!("Input file not found: {}", input_path.display()));
    }

    let dest_dir = input.param_str("dest_dir", "");
    if dest_dir.is_empty() {
        return Err("dest_dir is required".to_string());
    }

    emit_progress_step(0, 2);

    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create dest_dir: {}", e))?;

    let file_name = input_path.file_name().ok_or("Invalid input filename")?;
    let dest_path = std::path::PathBuf::from(&dest_dir).join(file_name);

    std::fs::copy(&input_path, &dest_path)
        .map_err(|e| format!("Copy failed: {}", e))?;

    emit_progress_step(2, 2);

    // 将原始文件路径传递给下一个节点
    output_ok(&[&input_path.to_string_lossy()], "复制完成");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
        return;
    }
    if let Err(e) = run() {
        let json = serde_json::json!({
            "code": "error",
            "message": e,
            "outputs": []
        });
        println!("{}", json);
        std::process::exit(1);
    }
}
```

### 构建

```bash
cargo build --release --bins
# 产物位于 target/release/copy_to_dir
```

---

## 部署模块

将编译好的模块二进制文件放入 `modules` 目录即可被自动发现。

> **文件名格式：** 主程序只加载符合 `{name}-{platform}-{version}` 格式的文件，直接命名的裸二进制（如 `copy_to_dir`）会被忽略。建议通过社区模块 CI 模板（内置自动命名逻辑）或手动按格式重命名。

### Docker 部署

```bash
cp ./copy_to_dir-linux-x86_64-0.1.0 ./data/modules/
chmod +x ./data/modules/copy_to_dir-linux-x86_64-0.1.0
```

完成后在 Web UI 的「后处理流水线」页面点击「添加模块」即可找到并添加。

> **注意：** 每次容器启动时，`modules.default` 中在 `modules` 里尚不存在（按文件名判断）的官方模块会被自动补充复制进来，已存在的文件不会被覆盖。

### Desktop 模块安装路径

Desktop 端的模块安装到每用户数据目录的 `modules` 子目录：

| 平台    | 路径                                                                              |
| ------- | --------------------------------------------------------------------------------- |
| Windows | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                             |
| macOS   | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`         |
| Linux   | `~/.local/share/com.chantrail.stripchat-recorder/modules/`（或 `$XDG_DATA_HOME`）|

---

## 发布到社区

模块开发完成后，可以通过社区模块市场分享给其他用户。社区采用**两级 registry** 设计——维护者仓库由 CI 自动维护元数据，向中央索引提交 PR **只需一次**，后续版本升级完全自动化。

详细的发布流程、CI 模板配置和中央索引注册步骤见[社区模块发布指南](community-registry.md)。

---

## 内置模块

项目自带以下官方模块：

| 模块 ID           | 输入类型         | 输出类型          | 说明                                                                              |
| ----------------- | ---------------- | ----------------- | --------------------------------------------------------------------------------- |
| `ts_merge`        | `ts_session_dir` | `video_file`      | 将 TS 分片目录合并为单一视频文件，**官方流水线的首节点**，其他模块应置于其后     |
| `filter_short`    | `any_file`       | `any_file`        | 请求主程序删除时长低于阈值的视频，支持 `dry_run` 预览模式                        |
| `contact_sheet`   | `video_file`     | `media_bundle`    | 按指定间隔截帧，拼合成带时间戳预览图，输出 media_bundle（视频路径 + 图片路径）    |
| `notify_discord`  | `any_file`       | `any_file`        | 将录制信息和封面图通过 Webhook 发送到 Discord，支持 HTTP/SOCKS5 代理              |
| `notify_telegram` | `any_file`       | `any_file`        | 通过 MTProto 向 Telegram 发送录制信息、封面图和视频，支持 >2GB 文件自动分割      |
| `cleanup`         | `any_file`       | —（流水线终止）   | 清理本次录制相关文件：可选删除视频/预览图/meta 记录，以及其他模块产生的临时缓存  |

> **内置 DAG 节点**（不是可执行文件，由后端直接处理）：
> - `recording_input`：虚拟录制输入节点，始终存在，不可删除，输出 `ts_session_dir`
> - `unpack`：将 `media_bundle` 拆分为 `video_file`（端口 0）和 `image_file`（端口 1）两个独立输出

### ts_merge 参数

| 参数               | 类型    | 默认值  | 说明                                                              |
| ------------------ | ------- | ------- | ----------------------------------------------------------------- |
| `format`           | select  | `mp4`   | 输出格式：`mp4`、`mkv`、`ts`                                     |
| `output_dir`       | dir     | `""`    | 合并后视频的输出目录，留空则与 TS 分片目录的父目录相同            |
| `split_by_streamer`| boolean | `true`  | 在输出目录下按主播用户名创建子目录（仅当设置了 `output_dir` 时有意义）|

### filter_short 参数

| 参数           | 类型    | 默认值  | 说明                         |
| -------------- | ------- | ------- | ---------------------------- |
| `min_duration` | number  | `60`    | 最短时长（秒），低于此值删除 |
| `dry_run`      | boolean | `false` | 仅预览，不实际删除           |

### contact_sheet 参数

| 参数          | 类型   | 默认值  | 说明                                   |
| ------------- | ------ | ------- | -------------------------------------- |
| `interval`    | number | `30`    | 截帧间隔（秒）                         |
| `thumb_width` | number | `320`   | 单帧宽度（px）                         |
| `format`      | select | `webp`  | 图片格式：`webp`、`jpg`、`png`         |
| `quality`     | number | `100`   | 图片质量（1–100，jpg/webp 有效）       |
| `cols`        | number | `0`     | 列数，`0` 为自动                       |
| `rows`        | number | `0`     | 行数，`0` 为自动                       |
| `fontfile`    | string | `""`    | 字体文件路径，留空自动检测             |
| `fontsize`    | number | `18`    | 时间戳字号                             |

### notify_discord 参数

| 参数          | 类型   | 默认值          | 说明                                   |
| ------------- | ------ | --------------- | -------------------------------------- |
| `webhook_url` | string | `""`            | Discord Webhook URL（必填）            |
| `proxy`       | string | `""`            | 代理地址，支持 `http://`、`socks5://`  |
| `username`    | string | `Recorder Bot`  | Bot 显示名称                           |

### notify_telegram 参数

| 参数         | 类型    | 默认值  | 说明                                              |
| ------------ | ------- | ------- | ------------------------------------------------- |
| `api_id`     | string  | `""`    | Telegram API ID（从 my.telegram.org 获取，必填）  |
| `api_hash`   | string  | `""`    | Telegram API Hash（必填）                         |
| `bot_token`  | string  | `""`    | Bot Token（从 @BotFather 获取，必填）             |
| `chat_id`    | string  | `""`    | Chat ID，超级群组格式为 `-100xxxxxxxxxx`（必填）  |
| `username`   | string  | `""`    | 群组 Username，超级群组必填（不含 `@`）           |
| `proxy`      | string  | `""`    | 代理地址，支持 `http://`、`socks5://`             |
| `send_video` | boolean | `true`  | 是否同时发送视频文件                              |

### cleanup 参数

| 参数                     | 类型    | 默认值  | 说明                                                                         |
| ------------------------ | ------- | ------- | ---------------------------------------------------------------------------- |
| `delete_recording_data`  | boolean | `false` | 删除录制数据（视频文件 + 预览图 + meta 记录），三者作为整体同时删除或都不删  |
| `delete_tmp_files`       | boolean | `true`  | 删除其他模块在共享临时目录中产生的同名残留缓存文件                           |
| `dry_run`                | boolean | `false` | 仅预览，不实际删除                                                           |

---

## 注意事项

- 模块应为**无状态**的，不依赖上次执行的任何残留状态。
- 模块不应修改或删除 `inputs[0]` 以外的文件，除非这是其明确功能（如 `cleanup`）。
- 若模块需要临时文件，使用 `pp_utils::tmp_dir()` 并在退出前清理。
- 模块必须在合理时间内完成，长时间无进度输出可能导致 UI 显示异常。
- 可执行文件名必须符合 `{name}-{platform}-{version}[.exe]` 格式，才能被主程序自动发现。
