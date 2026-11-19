//! 配置文件与路径 IO 辅助：数据根目录定位、UTF-8 BOM 处理、JSON 配置
//! 文件的容错读取与无效文件备份。
//! Config-file and path IO helpers: data-root resolution, UTF-8 BOM
//! handling, tolerant JSON config loading and invalid-file backup.

use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};

use super::Settings;

/// 运行时覆盖的数据根目录（仅 Desktop 端设置，见 [`set_exe_dir_override`]）。
/// Runtime-overridden data root directory (only set by the Desktop build, see [`set_exe_dir_override`]).
static EXE_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 设置数据根目录覆盖值，替代默认的"可执行文件同目录"约定。
///
/// 仅供 Desktop 端（`desktop-tauri`）在 `setup()` 阶段调用一次：Tauri 应用以系统安装包
/// （NSIS/MSI、AppImage、deb/rpm、dmg）分发时，可执行文件所在目录既可能只读（如 AppImage
/// 的 FUSE 挂载点，每次启动路径还会变化），也可能没有写权限（如 `/usr/bin`），或修改会
/// 破坏代码签名（如 macOS `.app` bundle）。Desktop 端改用 Tauri 的 `app_data_dir()`
/// （操作系统标准的每用户数据目录）作为覆盖值，Server 端不调用此函数，行为不变。
///
/// 必须在任何调用 [`exe_dir`]（或依赖它的 [`crate::config::app_state::AppState::new`]、
/// `log_dir`、`meta_dir` 等）的代码之前调用，且只应调用一次；后续调用会静默忽略
/// （[`OnceLock::set`] 的语义）。
///
/// Set the data root directory override, replacing the default "next to the executable"
/// convention.
///
/// Only meant to be called once by the Desktop build (`desktop-tauri`) during `setup()`:
/// when a Tauri app is distributed as a system installer package (NSIS/MSI, AppImage,
/// deb/rpm, dmg), the executable's directory may be read-only (e.g. AppImage's FUSE mount
/// point, which also changes path on every launch), lack write permission (e.g. `/usr/bin`),
/// or modifying it may break code signing (e.g. macOS `.app` bundles). The Desktop build uses
/// Tauri's `app_data_dir()` (the OS-standard per-user data directory) as the override value.
/// The Server build never calls this, so its behavior is unchanged.
///
/// Must be called before any code that calls [`exe_dir`] (or things that depend on it, like
/// [`crate::config::app_state::AppState::new`], `log_dir`, `meta_dir`, etc.), and only once;
/// subsequent calls are silently ignored (per [`OnceLock::set`] semantics).
pub fn set_exe_dir_override(dir: PathBuf) {
    let _ = EXE_DIR_OVERRIDE.set(dir);
}

/// 返回数据根目录：Desktop 端为 [`set_exe_dir_override`] 设置的每用户数据目录；
/// Server 端（未设置覆盖值）为可执行文件所在目录，用于定位配置文件和模块目录。
///
/// Returns the data root directory: on Desktop, the per-user data directory set via
/// [`set_exe_dir_override`]; on Server (no override set), the directory containing the
/// executable — used to locate config files and module directories.
pub fn exe_dir() -> PathBuf {
    if let Some(dir) = EXE_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// 去掉字符串开头的 UTF-8 BOM（`U+FEFF`）。
///
/// Windows 记事本等编辑器保存 UTF-8 文件时可能在开头加 BOM，serde_json 会把它当成非法字符，
/// 导致整个文件解析失败。读取用户可能手动编辑的 JSON（`config/`、`locale/`）前都先调用本函数。
///
/// Strip a leading UTF-8 BOM (`U+FEFF`) from a string.
///
/// Editors such as Windows Notepad may prepend a BOM when saving UTF-8, which serde_json treats
/// as an invalid character, failing the whole file. Call this before parsing any JSON the user
/// may edit by hand (`config/`, `locale/`).
pub fn strip_utf8_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// 读取 `config/` 下 JSON 配置文件的结果 / Result of reading a JSON config file under `config/`
pub(super) enum ConfigFile<T> {
    /// 文件不存在，或内容为空白（含只有 BOM）/ File missing, or blank (including BOM only)
    Missing,
    /// 解析成功 / Parsed successfully
    Parsed(T),
    /// 文件存在但无法读取（如不是 UTF-8 编码）或解析失败，附错误信息
    /// File exists but can't be read (e.g. not UTF-8) or fails to parse, with the error
    Invalid(String),
}

/// 读取并解析 JSON 配置文件：先去掉 UTF-8 BOM，空白文件视为不存在。
/// Read and parse a JSON config file: the UTF-8 BOM is stripped first and a blank file counts
/// as missing.
pub(super) fn load_config_file<T: DeserializeOwned>(path: &Path) -> ConfigFile<T> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ConfigFile::Missing,
        Err(e) => return ConfigFile::Invalid(e.to_string()),
    };
    let content = strip_utf8_bom(&content);
    if content.trim().is_empty() {
        return ConfigFile::Missing;
    }
    match serde_json::from_str(content) {
        Ok(v) => ConfigFile::Parsed(v),
        Err(e) => ConfigFile::Invalid(e.to_string()),
    }
}

/// 配置文件无法读取或解析时：把原文件复制为 `{文件名}.invalid-{时间}` 备份并记录警告。
///
/// 本次启动对该文件改用默认值；之后任意一次保存（`save()` 会写全部配置文件）都会用默认值
/// 覆盖原文件，备份保证手动编辑过的内容不会就此丢失。
///
/// When a config file can't be read or parsed: copy it to `{file name}.invalid-{time}` as a
/// backup and log a warning.
///
/// This run uses defaults for that file; any later save (`save()` writes every config file)
/// overwrites the original with defaults, so the backup keeps hand-edited content from being lost.
pub(super) fn backup_invalid_config(path: &Path, error: &str) {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config.json");
    let backup = path.with_file_name(format!(
        "{}.invalid-{}",
        file_name,
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    ));
    match fs::copy(path, &backup) {
        Ok(_) => tracing::warn!(
            "{}",
            crate::tl!(
                "startup.configInvalidBackedUp",
                path = path.display(),
                error = error,
                backup = backup.display()
            )
        ),
        Err(e) => tracing::warn!(
            "{}",
            crate::tl!(
                "startup.configInvalidBackupFailed",
                path = path.display(),
                error = error,
                backup_error = e
            )
        ),
    }
}

/// [`crate::config::app_state::AppState::apply_settings_overrides`] 的实现，作用于指定的 settings.json 路径。
/// Implementation of [`crate::config::app_state::AppState::apply_settings_overrides`] for the given settings.json path.
pub(super) fn apply_settings_overrides_in(
    path: &Path,
    language: Option<&str>,
    server_port: Option<u16>,
) -> bool {
    let (mut settings, missing) = match load_config_file::<Settings>(path) {
        ConfigFile::Parsed(s) => (s, false),
        ConfigFile::Missing => (Settings::default(), true),
        ConfigFile::Invalid(err) => {
            tracing::warn!(
                "{}",
                crate::tl!(
                    "startup.envOverrideSkippedInvalid",
                    path = path.display(),
                    error = err
                )
            );
            return false;
        }
    };

    let mut applied: Vec<String> = Vec::new();
    let mut language_changed = false;
    if let Some(lang) = language
        && settings.language != lang
    {
        settings.language = lang.to_string();
        language_changed = true;
        applied.push(format!("LANGUAGE={lang}"));
    }
    if let Some(port) = server_port
        && settings.server_port != port
    {
        settings.server_port = port;
        applied.push(format!("PORT={port}"));
    }
    if applied.is_empty() && !missing {
        return false;
    }

    let write_result = path
        .parent()
        .map_or(Ok(()), fs::create_dir_all)
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::to_string_pretty(&settings).map_err(|e| e.to_string()))
        .and_then(|json| fs::write(path, json).map_err(|e| e.to_string()));
    if let Err(e) = write_result {
        tracing::warn!(
            "{}",
            crate::tl!(
                "startup.envOverrideWriteFailed",
                path = path.display(),
                error = e
            )
        );
        return false;
    }
    if !applied.is_empty() {
        tracing::info!(
            "{}",
            crate::tl!(
                "startup.envOverrideApplied",
                path = path.display(),
                values = applied.join(", ")
            )
        );
    }
    language_changed
}

/// 读取配置文件；无法读取或解析时先备份原文件（见 [`backup_invalid_config`]）。
/// 不存在、为空或无效时返回 `None`。
/// Read a config file, backing it up first when it can't be read or parsed (see
/// [`backup_invalid_config`]). Returns `None` when missing, blank or invalid.
pub(super) fn load_config_or_backup<T: DeserializeOwned>(path: &Path) -> Option<T> {
    match load_config_file(path) {
        ConfigFile::Parsed(v) => Some(v),
        ConfigFile::Missing => None,
        ConfigFile::Invalid(err) => {
            backup_invalid_config(path, &err);
            None
        }
    }
}
