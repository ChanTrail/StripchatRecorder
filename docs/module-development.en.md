# Post-processing Module Development Guide

[简体中文](module-development.md) | [English](module-development.en.md)

This document describes how to write custom post-processing modules for StripchatRecorder.

---

## Table of Contents

1. [Overview](#overview)
2. [Module Protocol](#module-protocol)
3. [Metadata Descriptor](#metadata-descriptor)
4. [Parameter Types](#parameter-types)
5. [Pipeline Mechanics](#pipeline-mechanics)
6. [Progress Reporting](#progress-reporting)
7. [pp_utils Library](#pp_utils-library)
8. [Full Example](#full-example)
9. [Deploying a Module](#deploying-a-module)
10. [Notes](#notes)

---

## Overview

Post-processing modules are **standalone executables** invoked sequentially by the host application after a recording completes. Each module receives its input via environment variables and communicates with the host via stdout. Modules can be written in any language as long as they follow the protocol defined in this document.

---

## Module Protocol

### Invocation

The host invokes a module as follows:

```
PP_INPUT=<path>  PP_PARAM_<KEY>=<value> ...  ./<module_binary>
```

A module must support two invocation modes:

| Mode     | Command                 | Description                                        |
| -------- | ----------------------- | -------------------------------------------------- |
| Describe | `./<module> --describe` | Output module metadata JSON, perform no processing |
| Execute  | `./<module>`            | Read env vars and execute processing logic         |

### Environment Variables

| Variable         | Required | Description                                                   |
| ---------------- | -------- | ------------------------------------------------------------- |
| `PP_INPUT`       | Yes      | Absolute path to the input video file                         |
| `PP_PARAM_<KEY>` | No       | Module parameter; `<KEY>` is the uppercased parameter key     |
| `PP_EXE_DIR`     | No       | Directory containing the module binary, useful for temp files |

### Stdout Protocol

The module communicates with the host by writing lines with specific prefixes to stdout:

| Output line               | Description                                                                                                                                                                         |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OUTPUT:<path>`           | **Must be emitted once.** Passes `<path>` as input to the next module. If the module deletes the file (e.g. `filter_short`), omit this line and subsequent modules will be skipped. |
| `PROGRESS:<done>/<total>` | Progress report; both values are integers, `total` is always `10000` (i.e. `done=5000` means 50%).                                                                                  |
| `STATUS:<text>`           | Optional status text (e.g. upload speed), shown next to the progress bar in the UI.                                                                                                 |
| `SKIP:<reason>`           | Optional. Notifies the host that processing was skipped (e.g. output already exists). Still requires an `OUTPUT:` line.                                                             |

Lines not starting with one of the above prefixes are ignored by the host.

### Stderr

All diagnostic messages, warnings, and errors should be written to stderr. The host captures stderr and displays it to the user when a module fails.

### Exit Code

| Exit code | Meaning                                                           |
| --------- | ----------------------------------------------------------------- |
| `0`       | Success                                                           |
| Non-zero  | Failure; pipeline is aborted, subsequent modules are not executed |

---

## Metadata Descriptor

When invoked with `--describe`, the module must print a JSON object to stdout and exit with code `0`.

### JSON Schema

```jsonc
{
	"id": "my_module", // Unique identifier, lowercase letters, digits, underscores only
	"name": "My Module", // Display name shown in the UI
	"description": "Does X", // Short description shown in the UI
	"params": [
		// Parameter list, may be empty array
		{
			"key": "param_key", // Parameter key, maps to env var PP_PARAM_PARAM_KEY
			"label": "Param Label", // Label shown in the UI
			"type": "string", // Parameter type, see below
			"default": "", // Default value
		},
	],
}
```

### Field Reference

| Field         | Type   | Required | Description                                                  |
| ------------- | ------ | -------- | ------------------------------------------------------------ |
| `id`          | string | Yes      | Unique module ID, must not conflict within the same instance |
| `name`        | string | Yes      | Display name in the UI                                       |
| `description` | string | Yes      | Description in the UI                                        |
| `params`      | array  | Yes      | Parameter definitions; pass empty array if none              |

---

## Parameter Types

The `type` field of each param object determines how the UI renders it and the format of the env var value.

| Type      | UI Control      | Env var value format            |
| --------- | --------------- | ------------------------------- |
| `string`  | Text input      | Any string                      |
| `number`  | Number input    | Decimal integer or float string |
| `boolean` | Toggle switch   | `"true"` or `"false"`           |
| `select`  | Dropdown select | One of the option value strings |

`select` type requires an additional `options` field:

```jsonc
{
	"key": "format",
	"label": "Output Format",
	"type": "select",
	"default": "webp",
	"options": ["webp", "jpg", "png"],
}
```

---

## Pipeline Mechanics

A pipeline consists of an ordered list of module nodes. The host executes each enabled node in sequence:

1. The `OUTPUT:` path from the previous module becomes the `PP_INPUT` for the next. The first module's `PP_INPUT` is the path of the completed recording.
2. If a module exits with a non-zero code, the pipeline aborts immediately and subsequent modules are not executed.
3. If a module emits no `OUTPUT:` line (e.g. the file was deleted), the pipeline also aborts.

```
Recording done
    │
    ▼  PP_INPUT=/recordings/alice_20240101_120000.ts
┌─────────────┐
│ filter_short│ ── too short, file deleted, no OUTPUT → pipeline aborts
│             │ ── duration ok → OUTPUT:/recordings/alice_20240101_120000.ts
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
   Done
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

### Reading Parameters

```rust
use pp_utils::{param, param_u32, param_f64, param_bool};

let url: String   = param("webhook_url", "");        // string parameter
let interval: u32 = param_u32("interval", 30);       // u32 parameter
let min_dur: f64  = param_f64("min_duration", 60.0); // f64 parameter
let dry_run: bool = param_bool("dry_run", false);    // bool ("true"/"1"/"yes" → true)
```

Parameter keys are case-insensitive; `param("interval", ...)` maps to env var `PP_PARAM_INTERVAL`.

### Video Utilities

```rust
use pp_utils::video_duration;
use std::path::Path;

// Get video duration in seconds via ffprobe; returns None if ffprobe is unavailable
let duration: Option<f64> = video_duration(Path::new("/path/to/video.ts"));
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
// Returns e.g. /recordings/alice_20240101_120000.webp if it exists
```

### Progress Reporting

```rust
use pp_utils::{emit_progress, emit_progress_step};

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
use pp_utils::{emit_progress_step, param};
use std::env;
use std::path::PathBuf;

const DESCRIBE: &str = r#"{
  "id": "copy_to_dir",
  "name": "Copy to Directory",
  "description": "Copies the recording to a specified directory",
  "params": [
    {
      "key": "dest_dir",
      "label": "Destination Directory",
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

    // Pass the original file path to the next module
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

### Build

```bash
cargo build --release --bins
# Binary at target/release/copy_to_dir
```

---

## Deploying a Module

Place the compiled module binary into the `modules` Docker volume directory to be discovered automatically.

```bash
# Copy the binary to the host modules directory
cp ./copy_to_dir ./data/modules/copy_to_dir
chmod +x ./data/modules/copy_to_dir
```

After copying, the new module will appear in the Web UI under Settings → Post-processing Pipeline.

> **Note:** On every container start, files from `modules_default` that do not yet exist in `modules` are copied in. Existing files are never overwritten, so custom modules and manually replaced built-in modules are preserved across restarts.

---

## Notes

- Modules should be **stateless** and not rely on any residual state from previous executions.
- Modules should not modify or delete files other than `PP_INPUT` unless that is their explicit purpose (e.g. `filter_short`).
- If a module needs temporary files, use `PP_EXE_DIR/tmp/` and clean up before exiting.
- Modules must complete within a reasonable time; prolonged absence of progress output may cause the UI to appear stalled.
