# 后处理模块开发文档

> 🌐 [English](module-development.en.md)

本文档描述如何为 StripchatRecorder 编写自定义后处理模块。

---

## 目录

1. [概述](#概述)
2. [模块协议](#模块协议)
3. [元数据描述](#元数据描述)
4. [参数类型](#参数类型)
5. [流水线机制](#流水线机制)
6. [进度上报](#进度上报)
7. [pp_utils 工具库](#pp_utils-工具库)
8. [完整示例](#完整示例)
9. [部署模块](#部署模块)
10. [注意事项](#注意事项)

---

## 概述

后处理模块是**独立的可执行文件**，由主程序在录制完成后按流水线顺序依次调用。每个模块通过环境变量接收输入，通过标准输出与主程序通信。模块可以用任何语言编写，只要遵守本文档定义的协议即可。

---

## 模块协议

### 调用约定

主程序通过以下方式调用模块：

```
PP_INPUT=<path>  PP_PARAM_<KEY>=<value> ...  ./<module_binary>
```

模块必须支持两种调用模式：

| 模式 | 命令 | 说明 |
|------|------|------|
| 描述模式 | `./<module> --describe` | 输出模块元数据 JSON，不执行任何处理 |
| 执行模式 | `./<module>` | 读取环境变量并执行处理逻辑 |

### 环境变量

| 变量 | 必填 | 说明 |
|------|------|------|
| `PP_INPUT` | 是 | 输入视频文件的绝对路径 |
| `PP_PARAM_<KEY>` | 否 | 模块参数，`<KEY>` 为参数键名的大写形式 |
| `PP_EXE_DIR` | 否 | 模块二进制文件所在目录，可用于存放临时文件 |

### 标准输出协议

模块通过向标准输出写入特定前缀的行与主程序通信：

| 输出行 | 说明 |
|--------|------|
| `OUTPUT:<path>` | **必须输出一次**。将 `<path>` 作为下一个模块的输入传递。若模块删除了文件（如 `filter_short`），则不输出此行，流水线后续模块将被跳过。 |
| `PROGRESS:<done>/<total>` | 进度上报，`done` 和 `total` 均为整数，`total` 固定为 `10000`（即 `done=5000` 表示 50%）。 |
| `STATUS:<text>` | 可选的状态文字（如上传速度），显示在 UI 的进度条旁。 |
| `SKIP:<reason>` | 可选。通知主程序本模块已跳过处理（如输出文件已存在），仍需输出 `OUTPUT:` 行。 |

非协议行（不以上述前缀开头的行）会被主程序忽略。

### 标准错误

所有诊断信息、警告和错误消息应写入标准错误（stderr）。主程序会捕获 stderr 内容并在模块失败时展示给用户。

### 退出码

| 退出码 | 含义 |
|--------|------|
| `0` | 成功 |
| 非零 | 失败，流水线中止，后续模块不再执行 |

---

## 元数据描述

当以 `--describe` 参数调用时，模块必须向标准输出打印一个 JSON 对象，然后以退出码 `0` 退出。

### JSON 结构

```jsonc
{
  "id": "my_module",           // 模块唯一标识符，仅含小写字母、数字和下划线
  "name": "我的模块",           // UI 中显示的名称
  "description": "模块功能描述", // UI 中显示的简短描述
  "params": [                  // 参数列表，可为空数组
    {
      "key": "param_key",      // 参数键名，对应环境变量 PP_PARAM_PARAM_KEY
      "label": "参数标签",      // UI 中显示的标签
      "type": "string",        // 参数类型，见下文
      "default": ""            // 默认值
    }
  ]
}
```

### 字段说明

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `id` | string | 是 | 模块唯一 ID，在同一实例中不可重复 |
| `name` | string | 是 | UI 显示名称 |
| `description` | string | 是 | UI 显示描述 |
| `params` | array | 是 | 参数定义列表，无参数时传空数组 |

---

## 参数类型

`params` 数组中每个对象的 `type` 字段决定 UI 渲染方式和环境变量的值格式。

| 类型 | UI 控件 | 环境变量值格式 |
|------|---------|--------------|
| `string` | 文本输入框 | 任意字符串 |
| `number` | 数字输入框 | 十进制整数或浮点数字符串 |
| `boolean` | 开关 | `"true"` 或 `"false"` |
| `select` | 下拉选择框 | 选项值字符串 |

`select` 类型需额外提供 `options` 字段：

```jsonc
{
  "key": "format",
  "label": "输出格式",
  "type": "select",
  "default": "webp",
  "options": ["webp", "jpg", "png"]
}
```

---

## 流水线机制

流水线由一组有序的模块节点组成。主程序按顺序执行每个已启用的节点：

1. 将上一个模块输出的 `OUTPUT:` 路径作为下一个模块的 `PP_INPUT`。第一个模块的 `PP_INPUT` 为录制完成的原始视频路径。
2. 若某个模块退出码非零，流水线立即中止，后续模块不再执行。
3. 若某个模块未输出 `OUTPUT:` 行（例如文件已被删除），流水线同样中止。

```
录制完成
    │
    ▼  PP_INPUT=/recordings/alice_20240101_120000.ts
┌─────────────┐
│ filter_short│ ── 时长不足，删除文件，不输出 OUTPUT → 流水线终止
│             │ ── 时长满足 → OUTPUT:/recordings/alice_20240101_120000.ts
└─────────────┘
    │
    ▼  PP_INPUT=/recordings/alice_20240101_120000.ts
┌──────────────┐
│contact_sheet │ ── OUTPUT:/recordings/alice_20240101_120000.ts
└──────────────┘
    │
    ▼  PP_INPUT=/recordings/alice_20240101_120000.ts
┌────────────────┐
│ notify_discord │ ── OUTPUT:/recordings/alice_20240101_120000.ts
└────────────────┘
    │
   完成
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

- 主程序会取所有上报值中的最大值，进度不会倒退。
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

### 读取参数

```rust
use pp_utils::{param, param_u32, param_f64, param_bool};

let url: String  = param("webhook_url", "");       // 字符串参数
let interval: u32 = param_u32("interval", 30);     // u32 参数
let min_dur: f64  = param_f64("min_duration", 60.0); // f64 参数
let dry_run: bool = param_bool("dry_run", false);  // 布尔参数（"true"/"1"/"yes" 均视为 true）
```

参数键名不区分大小写，`param("interval", ...)` 对应环境变量 `PP_PARAM_INTERVAL`。

### 视频工具

```rust
use pp_utils::video_duration;
use std::path::Path;

// 通过 ffprobe 获取视频时长（秒），ffprobe 不可用时返回 None
let duration: Option<f64> = video_duration(Path::new("/path/to/video.ts"));
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
// 若存在则返回 /recordings/alice_20240101_120000.webp 等
```

### 进度上报

```rust
use pp_utils::{emit_progress, emit_progress_step};

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

---

## 完整示例

以下是一个完整的 Rust 模块示例，功能为将视频文件复制到指定目录。

### `Cargo.toml`

```toml
[package]
name = "copy_to_dir"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "copy_to_dir"
path = "src/main.rs"

[dependencies]
pp_utils = { path = "../pp_utils" }
```

### `src/main.rs`

```rust
use pp_utils::{emit_progress_step, param};
use std::env;
use std::path::PathBuf;

const DESCRIBE: &str = r#"{
  "id": "copy_to_dir",
  "name": "复制到目录",
  "description": "将录制文件复制到指定目录",
  "params": [
    {
      "key": "dest_dir",
      "label": "目标目录路径",
      "type": "string",
      "default": ""
    }
  ]
}"#;

fn run() -> Result<(), String> {
    let input_str = env::var("PP_INPUT").map_err(|_| "PP_INPUT not set".to_string())?;
    let input = PathBuf::from(&input_str);

    if !input.exists() {
        return Err(format!("Input file not found: {}", input.display()));
    }

    let dest_dir = param("dest_dir", "");
    if dest_dir.is_empty() {
        return Err("dest_dir is required".to_string());
    }

    emit_progress_step(0, 2);

    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| format!("Failed to create dest_dir: {}", e))?;

    let file_name = input.file_name().ok_or("Invalid input filename")?;
    let dest_path = PathBuf::from(&dest_dir).join(file_name);

    std::fs::copy(&input, &dest_path)
        .map_err(|e| format!("Copy failed: {}", e))?;

    emit_progress_step(2, 2);

    // 输出原始文件路径，传递给下一个模块
    println!("OUTPUT:{}", input.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--describe") {
        print!("{}", DESCRIBE);
        return;
    }
    if let Err(e) = run() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
```

### 构建

```bash
cargo build --release
# 产物位于 target/release/copy_to_dir
```

---

## 部署模块

将编译好的模块二进制文件放入 Docker 的 `modules` 目录即可被自动发现。

```bash
# 将二进制文件复制到宿主机的 modules 目录
cp ./copy_to_dir ./data/modules/copy_to_dir
chmod +x ./data/modules/copy_to_dir
```

完成后在 Web UI 的「设置 → 后处理流水线」中即可看到并启用新模块。

> **注意：** 每次容器启动时，`modules_default` 中在 `modules` 里尚不存在的文件会被补充复制进来，已存在的文件不会被覆盖。因此自定义模块和手动替换的内置模块在重启后均会保留。

---

## 注意事项

- 模块应为**无状态**的，不依赖上次执行的任何残留状态。
- 模块不应修改或删除 `PP_INPUT` 以外的文件，除非这是其明确功能（如 `filter_short`）。
- 若模块需要临时文件，建议使用 `PP_EXE_DIR/tmp/` 目录，并在退出前清理。
- 模块必须在合理时间内完成，长时间无进度输出可能导致 UI 显示异常。
