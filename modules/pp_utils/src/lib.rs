//! 后处理工具库 / Post-processing Utility Library
//!
//! 为所有后处理模块提供共享的工具函数，包括：
//! - 通过 ffprobe 获取视频时长
//! - 格式化时长、文件大小和传输速度
//! - 解析录制文件名中的主播名和时间戳
//! - 查找视频对应的封面图
//! - 向标准输出发送进度信息
//!
//! Provides shared utility functions for all post-processing modules, including:
//! - Getting video duration via ffprobe
//! - Formatting duration, file size, and transfer speed
//! - Parsing streamer name and timestamp from recording filenames
//! - Finding cover images for videos
//! - Emitting progress information to stdout

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 使用 ffprobe 获取视频文件的时长（秒）。
/// Get the duration of a video file in seconds using ffprobe.
///
/// # 返回值 / Returns
/// 视频时长（秒），ffprobe 不可用或解析失败时返回 `None`。
/// Video duration in seconds, or `None` if ffprobe is unavailable or parsing fails.
pub fn video_duration(input: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}

/// 将秒数格式化为 `HH:MM:SS` 格式的时长字符串。
/// Format seconds as a duration string in `HH:MM:SS` format.
pub fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 将字节数格式化为人类可读的大小字符串（如 "1.23 GB"）。
/// Format bytes as a human-readable size string (e.g. "1.23 GB").
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut val = bytes as f64;
    let mut i = 0;
    while val >= 1024.0 && i < UNITS.len() - 1 {
        val /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", val, UNITS[i])
}

/// 将每秒字节数格式化为带上传箭头的速度字符串（如 "↑ 1.5 MB/s"）。
/// Format bytes per second as an upload speed string (e.g. "↑ 1.5 MB/s").
pub fn format_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024.0 * 1024.0 {
        format!("↑ {:.1} MB/s", bytes_per_sec / (1024.0 * 1024.0))
    } else if bytes_per_sec >= 1024.0 {
        format!("↑ {:.1} KB/s", bytes_per_sec / 1024.0)
    } else {
        format!("↑ {:.0} B/s", bytes_per_sec)
    }
}

/// 从录制文件名的 stem 中解析主播名和录制时间戳。
/// 文件名格式为 `{model_name}_{YYYYMMDD}_{HHmmss}`。
///
/// Parse the model name and recording timestamp from a recording filename stem.
/// Filename format: `{model_name}_{YYYYMMDD}_{HHmmss}`
///
/// # 返回值 / Returns
/// `(model_name, timestamp_str)` 元组，解析失败时 timestamp 为空字符串。
/// Tuple of `(model_name, timestamp_str)`, timestamp is empty string on parse failure.
pub fn parse_stem(stem: &str) -> (String, String) {
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() >= 3 {
        let date = parts[parts.len() - 2];
        let time = parts[parts.len() - 1];
        // 验证日期（8位数字）和时间（6位数字）格式
        // Validate date (8 digits) and time (6 digits) format
        if date.len() == 8
            && date.chars().all(|c| c.is_ascii_digit())
            && time.len() == 6
            && time.chars().all(|c| c.is_ascii_digit())
        {
            let model = parts[..parts.len() - 2].join("_");
            let ts = format!(
                "{}-{}-{} {}:{}:{}",
                &date[..4],
                &date[4..6],
                &date[6..8],
                &time[..2],
                &time[2..4],
                &time[4..6]
            );
            return (model, ts);
        }
    }
    (stem.to_string(), String::new())
}

/// 在视频文件同目录下查找对应的封面图（支持 jpg/jpeg/webp/png）。
/// Find the cover image for a video in the same directory (supports jpg/jpeg/webp/png).
///
/// # 返回值 / Returns
/// 封面图路径，未找到时返回 `None`。
/// Cover image path, or `None` if not found.
pub fn find_cover(video: &Path) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_str()?;
    let dir = video.parent()?;
    for ext in &["jpg", "jpeg", "webp", "png"] {
        let p = dir.join(format!("{}.{}", stem, ext));
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// 获取临时文件目录：`{后端可执行文件所在目录}/tmp/`。
///
/// 优先使用 `PP_EXE_DIR` 环境变量指定的目录（该变量由 [`ModuleInput::read`] 在解析
/// stdin JSON 后自动设置为 `exe_dir` 字段的值——即**后端**可执行文件所在目录，
/// 由后端 `exec.rs` 通过 `crate::config::settings::exe_dir()` 传入，而不是本模块
/// 自身可执行文件所在目录）。仅当 `PP_EXE_DIR` 未被设置（如模块脱离主程序单独
/// 手动调用、或走的是旧版环境变量协议）时，才回退到 `env::current_exe()` 推断的
/// 模块自身可执行文件目录。
///
/// 之所以要用后端可执行文件目录而非模块自身目录：所有模块二进制文件被统一复制到
/// 同一个共享目录 `{后端目录}/modules/`（见 `discovery.rs` 的 `modules_dir()`），
/// 若临时目录挂在这个共享目录下（`modules/tmp/`），会与 `modules/` 目录本身的
/// 语义混淆（`modules/` 应该只存放可执行文件，不该混入运行时产生的缓存文件）；
/// 挂在后端目录下（`{后端目录}/tmp/`）则与 `meta/`、`locale/`、`logs/` 等其他
/// 运行时目录保持同一层级，语义更清晰，也不会因构建脚本清理/替换模块二进制文件
/// 而影响到缓存内容。
///
/// 若设置了 `PP_MAX_TMP_MB` 环境变量，会在返回前自动清理超出限制的旧文件。
///
/// Get the temporary file directory: `{backend executable's directory}/tmp/`.
///
/// Prefers the directory specified by the `PP_EXE_DIR` env var (automatically set by
/// [`ModuleInput::read`] to the `exe_dir` field's value after parsing the stdin JSON —
/// i.e. the **backend's own** executable directory, passed in by the backend's
/// `exec.rs` via `crate::config::settings::exe_dir()`, not this module's own executable
/// directory). Only falls back to the module's own executable directory (via
/// `env::current_exe()`) when `PP_EXE_DIR` isn't set (e.g. the module is invoked
/// standalone outside the host app, or via the legacy env-var protocol).
///
/// Why the backend's directory rather than the module's own: all module binaries are
/// copied into one shared directory, `{backend dir}/modules/` (see `modules_dir()` in
/// `discovery.rs`). Nesting the tmp directory under that shared directory
/// (`modules/tmp/`) would blur its meaning (`modules/` should only ever hold
/// executables, not runtime-generated cache files); nesting it under the backend's own
/// directory (`{backend dir}/tmp/`) keeps it at the same level as the other runtime
/// directories (`meta/`, `locale/`, `logs/`, etc.), which is clearer and keeps cache
/// contents unaffected by the build script cleaning up/replacing module binaries.
///
/// If `PP_MAX_TMP_MB` is set, automatically prunes old files that exceed the size limit before returning.
pub fn tmp_dir() -> PathBuf {
    let base = env::var("PP_EXE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let tmp = base.join("tmp");
    std::fs::create_dir_all(&tmp).ok();

    // 若设置了最大大小限制，清理超出的旧文件
    // If a max size limit is set, prune old files that exceed it
    if let Ok(max_mb_str) = env::var("PP_MAX_TMP_MB")
        && let Ok(max_mb) = max_mb_str.trim().parse::<u64>()
        && max_mb > 0
    {
        cleanup_tmp_dir(&tmp, max_mb);
    }

    tmp
}

/// 清理 tmp 目录，按最后修改时间从旧到新删除文件，直到目录总大小低于 `max_mb`。
/// 只删除直接子文件，不递归删除子目录（子目录由各模块自行管理）。
///
/// Prune the tmp directory by deleting files from oldest to newest until the total
/// directory size is below `max_mb`. Only direct child files are deleted; subdirectories
/// are left for modules to manage themselves.
pub fn cleanup_tmp_dir(tmp: &Path, max_mb: u64) {
    let max_bytes = max_mb * 1024 * 1024;

    // 收集所有直接子文件及其元数据 / Collect all direct child files with metadata
    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = std::fs::read_dir(tmp)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let meta = std::fs::metadata(&path).ok()?;
            let size = meta.len();
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Some((path, size, modified))
        })
        .collect();

    // 计算当前总大小 / Calculate current total size
    let total: u64 = entries.iter().map(|(_, s, _)| s).sum();
    if total <= max_bytes {
        return;
    }

    // 按修改时间升序排列（最旧的在前）/ Sort by modification time ascending (oldest first)
    entries.sort_by_key(|(_, _, t)| *t);

    let mut remaining = total;
    for (path, size, _) in &entries {
        if remaining <= max_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            remaining = remaining.saturating_sub(*size);
        }
    }
}

/// 使用 ffprobe 获取图片的宽度和高度。
/// Get image width and height using ffprobe.
///
/// # 返回值 / Returns
/// `(width, height)`，失败时返回 `None`。
/// `(width, height)`, or `None` on failure.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    use std::process::Command;
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut parts = s.trim().splitn(2, ',');
    let w: u32 = parts.next()?.trim().parse().ok()?;
    let h: u32 = parts.next()?.trim().parse().ok()?;
    Some((w, h))
}

/// 使用 ffprobe 获取视频的时长、宽度和高度。
/// Get video duration, width, and height using ffprobe.
///
/// # 返回值 / Returns
/// `(duration_secs, width, height)`，失败时返回 `None`。
/// `(duration_secs, width, height)`, or `None` on failure.
pub fn video_meta(input: &Path) -> Option<(f64, i32, i32)> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "format=duration:stream=width,height",
            "-of", "csv=p=0",
        ])
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let dims_line = lines.next()?;
    let dur_line = lines.next()?;
    let mut dims = dims_line.splitn(2, ',');
    let w: i32 = dims.next()?.trim().parse().ok()?;
    let h: i32 = dims.next()?.trim().parse().ok()?;
    let dur: f64 = dur_line.trim().parse().ok()?;
    Some((dur, w, h))
}

/// 进度上报的缩放基数（10000 = 100.00%）。
/// Progress reporting scale base (10000 = 100.00%).
pub const PROGRESS_SCALE: u32 = 10_000;

/// 向标准输出发送进度信息（格式：`PROGRESS:{scaled}/{PROGRESS_SCALE}`）。
/// Emit progress to stdout (format: `PROGRESS:{scaled}/{PROGRESS_SCALE}`).
///
/// # 参数 / Parameters
/// - `done`: 已完成的工作量 / Amount of work done
/// - `total`: 总工作量 / Total amount of work
pub fn emit_progress(done: u32, total: u32) {
    let scaled = if total == 0 {
        0
    } else {
        ((done as u64) * (PROGRESS_SCALE as u64) / (total as u64)).min(PROGRESS_SCALE as u64) as u32
    };
    println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
}

/// 按步骤发送进度信息，适用于固定步骤数的任务（四舍五入到最近整数步）。
/// Emit progress by step count, suitable for tasks with a fixed number of steps (rounded to nearest step).
///
/// # 参数 / Parameters
/// - `step`: 当前步骤序号（0-based）/ Current step index (0-based)
/// - `total_steps`: 总步骤数 / Total number of steps
pub fn emit_progress_step(step: u32, total_steps: u32) {
    let scaled = if total_steps == 0 {
        0
    } else {
        (((step as u64) * (PROGRESS_SCALE as u64) + ((total_steps as u64) / 2))
            / (total_steps as u64))
            .min(PROGRESS_SCALE as u64) as u32
    };
    println!("PROGRESS:{}/{}", scaled, PROGRESS_SCALE);
}

// ─── 新 JSON 协议帮助函数 / New JSON Protocol Helpers ────────────────────────

/// 模块 stdin 输入 JSON 结构 / Module stdin input JSON structure
#[derive(Debug, serde::Deserialize)]
pub struct ModuleInput {
    /// 输入路径列表（文件或目录）/ Input path list (files or directories)
    #[serde(default)]
    pub inputs: Vec<String>,
    /// 模块参数 / Module parameters
    #[serde(default)]
    pub params: serde_json::Value,
    /// 模块可执行文件所在目录 / Module executables directory
    #[serde(default)]
    pub exe_dir: Option<String>,
    /// tmp 目录最大占用（MB）/ Max tmp directory size (MB)
    #[serde(default)]
    pub max_tmp_mb: Option<u64>,
    /// 录制上下文 / Recording context
    #[serde(default)]
    pub recording: Option<serde_json::Value>,
}

impl ModuleInput {
    /// 从 stdin 读取并解析 JSON 输入。
    /// 若 stdin 为空或解析失败，回退到旧协议（`PP_INPUT` 环境变量）。
    ///
    /// 解析成功后，若 JSON 中的 `exe_dir` 字段非空，会将其写入 `PP_EXE_DIR`
    /// 环境变量——这样 [`tmp_dir`] 无论在 `ModuleInput::read()` 调用前还是调用后
    /// 被使用，都能读到同一个值，不需要每个模块自己在 `main()` 里手动转发这个字段。
    /// `exe_dir` 字段的值来自后端 `exec.rs`（`crate::config::settings::exe_dir()`），
    /// 是后端可执行文件所在目录，不是本模块自身可执行文件所在目录——[`tmp_dir`] 由此
    /// 得到的临时目录路径是 `{后端目录}/tmp/`。
    ///
    /// Read and parse JSON input from stdin.
    /// Falls back to legacy protocol (`PP_INPUT` env var) if stdin is empty or parse fails.
    ///
    /// On successful parse, if the JSON's `exe_dir` field is non-empty, it's written into
    /// the `PP_EXE_DIR` env var — so [`tmp_dir`] sees the same value whether it's called
    /// before or after `ModuleInput::read()`, without every module having to manually
    /// forward this field in `main()`. The `exe_dir` field's value comes from the
    /// backend's `exec.rs` (`crate::config::settings::exe_dir()`) — the backend's own
    /// executable directory, not this module's own — so the tmp directory path
    /// [`tmp_dir`] resolves to is `{backend dir}/tmp/`.
    pub fn read() -> Self {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().lock().read_to_string(&mut buf).ok();
        let trimmed = buf.trim();
        let parsed = if trimmed.is_empty() {
            Self::from_legacy_env()
        } else {
            serde_json::from_str(trimmed).unwrap_or_else(|_| Self::from_legacy_env())
        };
        if let Some(ref dir) = parsed.exe_dir
            && !dir.is_empty()
        {
            // SAFETY: 本进程是一次性 CLI 子进程，且此时刚从 stdin 完成一次性解析，
            // 尚未 spawn 任何线程，不存在与其他线程并发读写环境变量的竞争。
            // SAFETY: this process is a one-shot CLI subprocess, and we're right after a
            // one-time stdin parse with no threads spawned yet, so there's no concurrent
            // read/write race on the environment from other threads.
            unsafe {
                env::set_var("PP_EXE_DIR", dir);
            }
        }
        parsed
    }

    /// 从旧协议环境变量构建输入（`PP_INPUT`、`PP_EXE_DIR`、`PP_MAX_TMP_MB`）。
    /// Build input from legacy env vars (`PP_INPUT`, `PP_EXE_DIR`, `PP_MAX_TMP_MB`).
    fn from_legacy_env() -> Self {
        let inputs = env::var("PP_INPUT")
            .map(|v| vec![v])
            .unwrap_or_default();
        let max_tmp_mb = env::var("PP_MAX_TMP_MB")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        let exe_dir = env::var("PP_EXE_DIR").ok();
        Self { inputs, params: serde_json::Value::Null, exe_dir, max_tmp_mb, recording: None }
    }

    /// 获取第一个输入路径（单输入模块使用）/ Get the first input path (for single-input modules)
    pub fn first_input(&self) -> Option<std::path::PathBuf> {
        self.inputs.first().map(std::path::PathBuf::from)
    }

    /// 读取字符串参数（从 JSON params 读取，未设置时返回 fallback）。
    /// Read a string parameter from JSON params, returning fallback if not set.
    pub fn param_str(&self, key: &str, fallback: &str) -> String {
        if let Some(v) = self.params.get(key).and_then(|v| v.as_str()) {
            return v.to_string();
        }
        if let serde_json::Value::Object(ref map) = self.params {
            if let Some(v) = map.get(key) {
                if !matches!(v, serde_json::Value::Null) {
                    return match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                }
            }
        }
        fallback.to_string()
    }

    /// 读取 f64 参数 / Read an f64 parameter
    pub fn param_f64(&self, key: &str, fallback: f64) -> f64 {
        self.param_str(key, &fallback.to_string()).parse().unwrap_or(fallback)
    }

    /// 读取 u32 参数 / Read a u32 parameter
    pub fn param_u32(&self, key: &str, fallback: u32) -> u32 {
        self.param_str(key, &fallback.to_string()).parse().unwrap_or(fallback)
    }

    /// 读取 bool 参数 / Read a bool parameter
    pub fn param_bool(&self, key: &str, fallback: bool) -> bool {
        matches!(
            self.param_str(key, if fallback { "true" } else { "false" })
                .to_lowercase()
                .as_str(),
            "true" | "1" | "yes"
        )
    }

    /// 获取有效的 max_tmp_mb（优先 JSON，回退环境变量）/ Get effective max_tmp_mb
    pub fn max_tmp_mb(&self) -> Option<u64> {
        self.max_tmp_mb.or_else(|| {
            env::var("PP_MAX_TMP_MB").ok()?.trim().parse().ok()
        })
    }
}

/// 输出最终 JSON 结果（`ok` 状态，有输出路径）并写入 stdout。
/// Print final JSON result to stdout (`ok` code with output paths).
pub fn output_ok(outputs: &[&str], message: &str) {
    let json = serde_json::json!({
        "code": "ok",
        "message": message,
        "outputs": outputs,
    });
    println!("{}", json);
}

/// 输出最终 JSON 结果（`done` 状态，流水线终止）并写入 stdout。
/// Print final JSON result to stdout (`done` code, pipeline terminates).
pub fn output_done(message: &str) {
    let json = serde_json::json!({
        "code": "done",
        "message": message,
        "outputs": [],
    });
    println!("{}", json);
}

/// 输出最终 JSON 结果（`skipped` 状态，原样传递输入）并写入 stdout。
/// Print final JSON result to stdout (`skipped` code, passes input through).
pub fn output_skipped(input: &str, message: &str) {
    let json = serde_json::json!({
        "code": "skipped",
        "message": message,
        "outputs": [input],
    });
    println!("{}", json);
}

/// 解析模块的 `DESCRIBE` JSON 模板并注入版本号，返回可直接打印到 stdout 的完整 JSON 字符串。
///
/// 各模块的 `DESCRIBE` 常量本身不再手写版本号；版本号统一在编译期通过
/// `env!("CARGO_PKG_VERSION")` 从模块自身 `Cargo.toml` 的 `version` 字段读取，
/// 调用方式为 `pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION"))`。
/// 这样升级模块版本时只需改动一处（Cargo.toml），不会再出现 Cargo.toml、DESCRIBE 的
/// `name` 字段、locale 翻译文件三处版本号各自漂移、互相不一致的问题。
///
/// Parse a module's `DESCRIBE` JSON template and inject the version, returning a JSON
/// string ready to print to stdout.
///
/// Each module's `DESCRIBE` constant no longer hardcodes a version number; the version is
/// read once at compile time from the module's own `Cargo.toml` `version` field via
/// `env!("CARGO_PKG_VERSION")`, called as
/// `pp_utils::describe_with_version(DESCRIBE, env!("CARGO_PKG_VERSION"))`. This way,
/// bumping a module's version only requires editing one place (Cargo.toml), eliminating
/// the previous drift between Cargo.toml, the DESCRIBE `name` field, and locale
/// translation files.
///
/// # Panics
/// Panics if `template` is not valid JSON — this indicates a bug in the module's own
/// `DESCRIBE` constant that should surface immediately during development rather than
/// silently emitting broken metadata to the host.
pub fn describe_with_version(template: &str, version: &str) -> String {
    let mut value: serde_json::Value = serde_json::from_str(template)
        .expect("pp_utils::describe_with_version: DESCRIBE template is not valid JSON");
    if let serde_json::Value::Object(map) = &mut value {
        map.insert(
            "version".to_string(),
            serde_json::Value::String(version.to_string()),
        );
    }
    serde_json::to_string(&value).unwrap_or_else(|_| template.to_string())
}
