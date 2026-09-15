# Post-processing Module Development Guide

[简体中文](module-development.md) | [English](module-development.en.md)

This document describes how to write custom post-processing modules for StripchatRecorder.

---

## Table of Contents

1. [Overview](#overview)
2. [Module Protocol](#module-protocol)
3. [Metadata Descriptor](#metadata-descriptor)
4. [Parameter Types](#parameter-types)
5. [Port Type System](#port-type-system)
6. [Internationalization (i18n)](#internationalization-i18n)
7. [Pipeline Mechanics](#pipeline-mechanics)
8. [Progress Reporting](#progress-reporting)
9. [pp_utils Library](#pp_utils-library)
10. [Full Example](#full-example)
11. [Deploying a Module](#deploying-a-module)
12. [Built-in Modules](#built-in-modules)
13. [Notes](#notes)

---

## Overview

Post-processing modules are **standalone executables** invoked by the host application after a recording completes. Each module receives its input via **stdin JSON** and communicates with the host via **stdout**. Modules can be written in any language as long as they follow the protocol defined in this document.

---

## Module Protocol

### Invocation

Modules must support two invocation modes:

| Mode     | Command                 | Description                                        |
| -------- | ----------------------- | -------------------------------------------------- |
| Describe | `./<module> --describe` | Output module metadata JSON, perform no processing |
| Execute  | `./<module>`            | Read stdin JSON and execute processing logic       |

> **Filename format requirement:** The host only discovers and loads executables matching the `{name}-{platform}-{version}` format (`.exe` on Windows), e.g. `my_module-linux-x86_64-1.0.0`. Files not matching this format are ignored.

### stdin Input JSON

In execute mode, the host passes a JSON object to the module via stdin:

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

| Field        | Description                                                                                  |
| ------------ | -------------------------------------------------------------------------------------------- |
| `inputs`     | Input path list, indexed by port order; single-input modules read `inputs[0]`                |
| `params`     | Module parameter key-value pairs (corresponding to `params[].key` in `--describe`)           |
| `exe_dir`    | Backend executable's own directory (`pp_utils::tmp_dir()` uses this to locate the tmp dir)  |
| `max_tmp_mb` | Max tmp directory size in MB; `pp_utils::tmp_dir()` auto-prunes old files based on this      |
| `recording`  | Recording context (`video_path`, `started_at`, `username`; used by notification modules)    |

> **Legacy protocol fallback:** If stdin is empty or fails to parse, `pp_utils::ModuleInput::read()` automatically falls back to the legacy environment variable protocol (`PP_INPUT`, `PP_PARAM_<KEY>`, `PP_EXE_DIR`), preserving backward compatibility.

### Stdout Protocol

Modules communicate with the host by writing to stdout. There are two output types:

**Progress lines** (can be emitted at any time, multiple):

| Output line               | Description                                                                                 |
| ------------------------- | ------------------------------------------------------------------------------------------- |
| `PROGRESS:<done>/<total>` | Progress report; both values are integers, `total` is always `10000` (50% = `done=5000`)   |
| `STATUS:<text>`           | Optional status text (e.g. upload speed), shown next to the progress bar in the UI         |

**Final result line** (last line, must be emitted exactly once):

```json
{"code": "ok", "message": "Done", "outputs": ["/recordings/alice_20240101_120000.ts"]}
```

| `code` value | Description                                                                                        |
| ------------ | -------------------------------------------------------------------------------------------------- |
| `"ok"`       | Success; paths in `outputs` are passed as input to downstream nodes                               |
| `"done"`     | Success and pipeline terminates here (e.g. `cleanup` deletes the file); `outputs` is empty        |
| `"skipped"`  | Processing skipped (e.g. output already exists); path is still passed through via `outputs`       |
| `"error"`    | Failure; pipeline aborts, subsequent nodes are not executed                                        |

> `pp_utils` provides `output_ok()`, `output_done()`, and `output_skipped()` to emit the correct format.

### Stderr

All diagnostic messages, warnings, and errors should be written to stderr. The host captures stderr and displays it to the user when a module fails.

### Exit Code

| Exit code | Meaning                                                                      |
| --------- | ---------------------------------------------------------------------------- |
| `0`       | Success (combine with the final JSON `code` field for full status)           |
| Non-zero  | Failure; pipeline aborts, subsequent modules are not executed                |

---

## Metadata Descriptor

When invoked with `--describe`, the module must print a JSON object to stdout and exit with code `0`.

### JSON Schema

```jsonc
{
  "id": "my_module",              // Unique identifier, lowercase letters, digits, underscores only
  "name": "My Module",            // Display name shown in the UI
  "description": "Does X",        // Short description shown in the UI
  "version": "1.0.0",             // Version (injected by pp_utils::describe_with_version; don't hardcode)
  "inputTypes": ["video_file"],   // Input port types; see Port Type System
  "outputTypes": ["video_file"],  // Output port types; empty means pipeline terminates here
  "official": false,              // Whether this is an official module (UI hint to place after ts_merge)
  "params": [
    {
      "key": "param_key",         // Parameter key; maps to params.param_key in the input JSON
      "label": "Param Label",     // Label shown in the UI
      "type": "string",           // Parameter type; see Parameter Types
      "default": ""               // Default value
    }
  ]
}
```

### Field Reference

| Field         | Type    | Required | Description                                                                          |
| ------------- | ------- | -------- | ------------------------------------------------------------------------------------ |
| `id`          | string  | Yes      | Unique module ID, must not conflict within the same instance                         |
| `name`        | string  | Yes      | Display name in the UI                                                               |
| `description` | string  | Yes      | Description in the UI                                                                |
| `inputTypes`  | array   | Yes      | Input port types (indexed with `inputs[]`); see [Port Type System](#port-type-system) |
| `outputTypes` | array   | Yes      | Output port types (indexed with `outputs[]`); empty array means pipeline terminates  |
| `official`    | boolean | No       | Whether this is an official module; defaults to `false`                              |
| `params`      | array   | Yes      | Parameter definitions; pass empty array if none                                      |

> **Version number:** Do not hardcode `"version"` in the `DESCRIBE` constant. Call `pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION"))` to inject the version from `Cargo.toml` at runtime, keeping it in one place.

---

## Parameter Types

The `type` field of each param object determines how the UI renders it:

| Type      | UI Control      | Value format                          |
| --------- | --------------- | ------------------------------------- |
| `string`  | Text input      | Any string                            |
| `number`  | Number input    | Decimal integer or float string       |
| `boolean` | Toggle switch   | `true` or `false` (JSON)              |
| `select`  | Dropdown select | One of the option value strings       |
| `dir`     | Directory picker| Absolute directory path string        |

`select` type requires an additional `options` field:

```jsonc
{
  "key": "format",
  "label": "Output Format",
  "type": "select",
  "default": "mp4",
  "options": ["mp4", "mkv", "ts"]
}
```

`number` type supports optional `min` / `max` constraints:

```jsonc
{
  "key": "quality",
  "label": "Image Quality",
  "type": "number",
  "default": 85,
  "min": 1,
  "max": 100
}
```

---

## Port Type System

Each module declares its input/output port data types via `inputTypes` and `outputTypes`. In the visual pipeline editor, only ports with compatible types can be connected.

| Identifier      | Description                                                                              |
| --------------- | ---------------------------------------------------------------------------------------- |
| `ts_session_dir`| TS segment recording directory (produced by the recording system, input to `ts_merge`)   |
| `video_file`    | Single video file (mp4 / mkv / ts etc.)                                                  |
| `image_file`    | Single image file (webp / jpg / png etc.)                                                |
| `media_bundle`  | Media bundle (video path + image path separated by `\n`, passed as a single path string) |
| `any_file`      | Any file type (accepts any upstream file type, including `media_bundle`)                 |
| `any_dir`       | Any directory type (accepts any upstream directory type)                                 |

**Compatibility rules:**
- Identical types can always connect
- Any file type (including `media_bundle`) can connect to `any_file`
- Any directory type can connect to `any_dir`

**Multiple ports:** `inputTypes` / `outputTypes` are indexed in order with `inputs[]` / `outputs[]`. For example, `inputTypes: ["video_file", "image_file"]` means the module has two input ports: `inputs[0]` is a video and `inputs[1]` is an image.

If a module omits `inputTypes`/`outputTypes` (legacy compatibility), the host defaults to `["any_file"]`.

---

## Internationalization (i18n)

Modules can declare an optional `i18n` field in their describe JSON to provide translations for `name`, `description`, and parameter `label` values. The host automatically selects the translation matching the user's current UI language, falling back to the original field values when no translation is found.

### JSON Schema

```jsonc
{
  "id": "my_module",
  "name": "我的模块",           // default language (Chinese)
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
    // additional locales can be added, e.g. "ja-JP": { ... }
  }
}
```

Priority: **server-side `locale/modules/<id>/` JSON files** override `--describe` `i18n` → `--describe` `i18n` overrides original defaults.

---

## Pipeline Mechanics

The pipeline is a **DAG (directed acyclic graph)** of nodes and edges, supporting branches and merges. The host executes enabled nodes in topological order:

1. Upstream node `outputs[]` paths are passed to downstream node `inputs[]` according to the wired port indices.
2. The pipeline always starts from the `recording_input` built-in node, which outputs `ts_session_dir` — the TS segment directory from the current recording.
3. If a module returns `{"code": "error"}` or exits with a non-zero code, the pipeline aborts immediately.
4. If a module returns `{"code": "done"}` or `outputs` is empty, that branch terminates and no further downstream nodes receive input.

```
recording_input (TS segment dir)
    │
    ▼  inputs[0] = /ts_fragment/alice_20240101_120000/
┌──────────┐
│ ts_merge │── outputs[0] = /recordings/alice/alice_20240101_120000.mp4
└──────────┘
    │
    ▼  inputs[0] = /recordings/alice/alice_20240101_120000.mp4
┌──────────────┐
│ filter_short │── too short → code: "done" (pipeline terminates)
│              │── duration ok → outputs[0] = same path
└──────────────┘
    │
    ▼  inputs[0] = /recordings/alice/alice_20240101_120000.mp4
┌───────────────┐
│ contact_sheet │── outputs[0] = media_bundle (video_path\nimage_path)
└───────────────┘
    │
    ▼  inputs[0] = media_bundle
┌──────────────────┐
│ __builtin__unpack│── outputs[0] = video_file, outputs[1] = image_file (fork)
└──────────────────┘
   │                └───────────────────────────────────┐
   ▼ video_file                                         ▼ image_file
┌────────────────┐                          ┌─────────────────────┐
│ notify_discord │                          │    test_suffix      │
└────────────────┘                          └─────────────────────┘
```

---

## Progress Reporting

Progress is reported by writing `PROGRESS:<done>/<total>` lines to stdout, where `total` is always `10000`.

```
PROGRESS:0/10000      # 0%
PROGRESS:5000/10000   # 50%
PROGRESS:10000/10000  # 100%
```

**Notes:**

- The host tracks the maximum reported value; progress never goes backwards.
- Emit `PROGRESS:0/10000` before processing starts and `PROGRESS:10000/10000` when done.
- For fixed-step tasks, divide progress evenly (e.g. 3 steps: 0, 3333, 6666, 10000).

---

## pp_utils Library

`pp_utils` is the project's built-in Rust utility library encapsulating common functionality for all modules. It is strongly recommended for Rust-based modules.

Add to `Cargo.toml`:

```toml
[dependencies]
pp_utils = { path = "../pp_utils" }
```

### Reading Input (new protocol)

```rust
use pp_utils::ModuleInput;

// Read and parse stdin JSON; auto-falls back to legacy env-var protocol
let input = ModuleInput::read();

// Get the first input path (single-input modules)
let path = input.first_input().ok_or("inputs[0] required")?;

// Read parameters
let dest_dir: String  = input.param_str("dest_dir", "");
let interval: u32     = input.param_u32("interval", 30);
let min_dur: f64      = input.param_f64("min_duration", 60.0);
let dry_run: bool     = input.param_bool("dry_run", false);

// Recording context (for notification modules)
if let Some(rec) = &input.recording {
    let username = rec.get("username").and_then(|v| v.as_str()).unwrap_or("");
}
```

### Emitting Results (new protocol)

```rust
use pp_utils::{output_ok, output_done, output_skipped};

// Success: pass output path to downstream
output_ok(&[&output_path.to_string_lossy()], "Done");

// Pipeline terminates here (e.g. cleanup module after deleting files)
output_done("Cleaned up associated files");

// Skip processing, pass input through unchanged
output_skipped(&input_path.to_string_lossy(), "Output already exists, skipped");
```

### Video Utilities

```rust
use pp_utils::{video_duration, video_meta};
use std::path::Path;

// Get video duration in seconds via ffprobe
let duration: Option<f64> = video_duration(Path::new("/path/to/video.ts"));

// Get duration, width, and height in one call
let meta: Option<(f64, i32, i32)> = video_meta(Path::new("/path/to/video.ts"));
// Some((duration_secs, width, height))
```

### Formatting Utilities

```rust
use pp_utils::{format_duration, format_bytes, format_speed};

format_duration(3661.0);   // "01:01:01"
format_bytes(1_500_000);   // "1.43 MB"
format_speed(1_048_576.0); // "↑ 1.0 MB/s"
```

### Filename Parsing

Recording filenames follow the format `{model_name}_{YYYYMMDD}_{HHmmss}`.

```rust
use pp_utils::parse_stem;

let (model, timestamp) = parse_stem("alice_20240101_120000");
// model = "alice", timestamp = "2024-01-01 12:00:00"

// Model names with underscores are also supported
let (model, timestamp) = parse_stem("my_streamer_20240101_120000");
// model = "my_streamer", timestamp = "2024-01-01 12:00:00"
```

### Cover Image Lookup

Finds a cover image with the same stem in the same directory as the video. Supports `jpg`, `jpeg`, `webp`, `png`.

```rust
use pp_utils::find_cover;
use std::path::Path;

let cover: Option<PathBuf> = find_cover(Path::new("/recordings/alice_20240101_120000.ts"));
```

### Image Metadata

```rust
use pp_utils::image_dimensions;
use std::path::Path;

let dims: Option<(u32, u32)> = image_dimensions(Path::new("/recordings/cover.webp"));
// Some((1280, 720))
```

### Temporary Directory

Returns `{backend executable's directory}/tmp/`. The directory is created automatically; if `max_tmp_mb` is set, old files are pruned automatically.

```rust
use pp_utils::tmp_dir;

let tmp: PathBuf = tmp_dir();
// e.g. /app/stripchat-recorder/tmp/
```

### Progress Reporting

```rust
use pp_utils::{emit_progress, emit_progress_step, PROGRESS_SCALE};

// Report by done/total (auto-scaled to 10000)
emit_progress(0, 100);   // PROGRESS:0/10000
emit_progress(50, 100);  // PROGRESS:5000/10000
emit_progress(100, 100); // PROGRESS:10000/10000

// Report by fixed steps
emit_progress_step(0, 3); // PROGRESS:0/10000
emit_progress_step(1, 3); // PROGRESS:3333/10000
emit_progress_step(2, 3); // PROGRESS:6667/10000
emit_progress_step(3, 3); // PROGRESS:10000/10000
```

### Injecting Version Number

```rust
// Use this in --describe mode to inject the Cargo.toml version into the JSON
print!("{}", pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION")));
```

---

## Full Example

A complete Rust module that copies the video file to a specified directory.

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
  "name": "Copy to Directory",
  "description": "Copies the recording to a specified directory",
  "inputTypes": ["video_file"],
  "outputTypes": ["video_file"],
  "official": false,
  "params": [
    {
      "key": "dest_dir",
      "label": "Destination Directory",
      "type": "dir",
      "default": ""
    }
  ],
  "i18n": {
    "zh-CN": {
      "name": "复制到目录",
      "description": "将录制文件复制到指定目录",
      "params": {
        "dest_dir": { "label": "目标目录路径" }
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

    // Pass the original file path to the next module
    output_ok(&[&input_path.to_string_lossy()], "Copied successfully");
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

### Build

```bash
cargo build --release --bins
# Binary at target/release/copy_to_dir
```

---

## Deploying a Module

Place the compiled module binary into the `modules` directory to be discovered automatically.

> **Filename format:** The host only loads files matching `{name}-{platform}-{version}[.exe]`. Bare binaries (e.g. just `copy_to_dir`) are ignored. Use the community module CI template (which names files automatically) or rename manually to match the format.

### Docker Deployment

```bash
cp ./copy_to_dir-linux-x86_64-0.1.0 ./data/modules/
chmod +x ./data/modules/copy_to_dir-linux-x86_64-0.1.0
```

After copying, the module will appear in the Web UI under Post-processing Pipeline → Add Module.

> **Note:** On every container start, official module files from `modules.default` that do not yet exist in `modules` (by filename) are copied in. Existing files are never overwritten.

### Desktop Module Install Path

Desktop modules go into the per-user data directory's `modules` subdirectory:

| Platform | Path                                                                              |
| -------- | --------------------------------------------------------------------------------- |
| Windows  | `%APPDATA%\com.chantrail.stripchat-recorder\modules\`                             |
| macOS    | `~/Library/Application Support/com.chantrail.stripchat-recorder/modules/`         |
| Linux    | `~/.local/share/com.chantrail.stripchat-recorder/modules/` (or `$XDG_DATA_HOME`) |

---

## Built-in Modules

The project ships the following official modules:

| Module ID         | Input Type       | Output Type       | Description                                                                                     |
| ----------------- | ---------------- | ----------------- | ----------------------------------------------------------------------------------------------- |
| `ts_merge`        | `ts_session_dir` | `video_file`      | Merges a TS segment directory into a single video file; **the first node in an official pipeline** |
| `filter_short`    | `any_file`       | `any_file`        | Requests the host to delete videos shorter than a threshold; supports `dry_run` preview mode    |
| `contact_sheet`   | `video_file`     | `media_bundle`    | Extracts frames at an interval and tiles them into a preview image; outputs a media_bundle       |
| `notify_discord`  | `any_file`       | `any_file`        | Sends recording info and cover image to Discord via Webhook; supports HTTP/SOCKS5 proxy          |
| `notify_telegram` | `any_file`       | `any_file`        | Sends recording info, cover image, and video to Telegram via MTProto; auto-splits files >2 GB   |
| `cleanup`         | `any_file`       | — (terminates)    | Cleans up recording-related files: optionally deletes video/preview/meta, and tmp cache files   |

> **Built-in DAG nodes** (not executables; handled directly by the backend):
> - `recording_input`: virtual recording input node, always present and non-deletable, outputs `ts_session_dir`
> - `unpack`: splits a `media_bundle` into `video_file` (port 0) and `image_file` (port 1)

### ts_merge parameters

| Parameter          | Type    | Default | Description                                                                |
| ------------------ | ------- | ------- | -------------------------------------------------------------------------- |
| `format`           | select  | `mp4`   | Output format: `mp4`, `mkv`, or `ts`                                       |
| `output_dir`       | dir     | `""`    | Output directory for merged video; empty = parent of the TS session dir    |
| `split_by_streamer`| boolean | `true`  | Create a per-streamer subdirectory under `output_dir` (only meaningful when `output_dir` is set) |

### filter_short parameters

| Parameter      | Type    | Default | Description                                    |
| -------------- | ------- | ------- | ---------------------------------------------- |
| `min_duration` | number  | `60`    | Minimum duration in seconds; shorter files are deleted |
| `dry_run`      | boolean | `false` | Preview only, no actual deletion               |

### contact_sheet parameters

| Parameter    | Type   | Default | Description                                      |
| ------------ | ------ | ------- | ------------------------------------------------ |
| `interval`   | number | `30`    | Frame extraction interval (seconds)              |
| `thumb_width`| number | `320`   | Thumbnail width (px)                             |
| `format`     | select | `webp`  | Image format: `webp`, `jpg`, or `png`            |
| `quality`    | number | `100`   | Image quality (1–100, applies to jpg/webp)       |
| `cols`       | number | `0`     | Number of columns; `0` = auto                   |
| `rows`       | number | `0`     | Number of rows; `0` = auto                      |
| `fontfile`   | string | `""`    | Font file path; leave empty for auto-detection  |
| `fontsize`   | number | `18`    | Timestamp font size                              |

### notify_discord parameters

| Parameter     | Type   | Default        | Description                                      |
| ------------- | ------ | -------------- | ------------------------------------------------ |
| `webhook_url` | string | `""`           | Discord Webhook URL (required)                   |
| `proxy`       | string | `""`           | Proxy address; supports `http://` and `socks5://`|
| `username`    | string | `Recorder Bot` | Bot display name                                 |

### notify_telegram parameters

| Parameter    | Type    | Default | Description                                                    |
| ------------ | ------- | ------- | -------------------------------------------------------------- |
| `api_id`     | string  | `""`    | Telegram API ID from my.telegram.org (required)                |
| `api_hash`   | string  | `""`    | Telegram API Hash (required)                                   |
| `bot_token`  | string  | `""`    | Bot Token from @BotFather (required)                           |
| `chat_id`    | string  | `""`    | Chat ID; supergroup format: `-100xxxxxxxxxx` (required)        |
| `username`   | string  | `""`    | Group username, required for supergroups (without `@`)         |
| `proxy`      | string  | `""`    | Proxy address; supports `http://` and `socks5://`              |
| `send_video` | boolean | `true`  | Also send the video file                                       |

### cleanup parameters

| Parameter                | Type    | Default | Description                                                                              |
| ------------------------ | ------- | ------- | ---------------------------------------------------------------------------------------- |
| `delete_recording_data`  | boolean | `false` | Delete recording data (video + preview image + meta record) as an atomic unit           |
| `delete_tmp_files`       | boolean | `true`  | Delete leftover same-named cache files in the shared tmp directory from other modules   |
| `dry_run`                | boolean | `false` | Preview only, no actual deletion                                                        |

---

## Notes

- Modules should be **stateless** and not rely on any residual state from previous executions.
- Modules should not modify or delete files other than `inputs[0]` unless that is their explicit purpose (e.g. `cleanup`).
- If a module needs temporary files, use `pp_utils::tmp_dir()` and clean up before exiting.
- Modules must complete within a reasonable time; prolonged absence of progress output may cause the UI to appear stalled.
- Executable filenames must match the `{name}-{platform}-{version}[.exe]` format to be discovered by the host.
