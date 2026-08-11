//! 模块发现 / Module Discovery
//!
//! 扫描 `modules/` 目录，调用每个可执行文件的 `--describe` 参数获取模块元数据，
//! 并追加内置节点描述（见 `postprocess::builtin_nodes`）。不涉及 DAG 数据结构
//! 定义（见 `super::model`）或流水线执行（见 `super::exec`）。
//!
//! Scans the `modules/` directory, invokes each executable's `--describe` flag to
//! retrieve module metadata, and appends built-in node descriptions (see
//! `postprocess::builtin_nodes`). Does not define DAG data structures (see
//! `super::model`) or execute the pipeline (see `super::exec`).

use super::model::{ModuleInfo, PortType};
use crate::config::settings::exe_dir;
use std::path::PathBuf;
use std::process::Stdio;

/// 返回模块可执行文件所在目录（可执行文件同目录下的 modules/ 文件夹）。
/// Returns the modules directory (modules/ folder next to the executable).
pub fn modules_dir() -> PathBuf {
    exe_dir().join("modules")
}

/// 扫描 modules/ 目录，发现所有可用的后处理模块，并追加内置节点描述。
///
/// 构建脚本（scripts/common.js 的 `buildModules`）会把每个模块的可执行文件复制为
/// 带版本号后缀的文件名（如 `notify_telegram-0.5.0.exe`），方便用户直接从文件名
/// 分辨版本；每次构建都会先清理该模块的所有旧文件名（不论是否带版本号），正常
/// 情况下 modules/ 目录里同一模块任何时候只应有一个文件。
///
/// 但如果用户手动往 modules/ 目录里塞文件、或旧版本构建脚本/升级流程遗留了文件
/// 未被清理，仍可能出现同一个 `id` 被多个可执行文件同时声明的情况。这里作为
/// 防御性兜底：只保留 `version` 字段（语义化版本号，按点号分隔的数字逐段比较）
/// 更高的一个，避免下游 `modules.iter().find(|m| m.id == ...)` 类查找因目录遍历
/// 顺序不确定而随机选中新旧版本中的任意一个（进而导致版本号显示不一致、或实际
/// 执行的是已修复 bug 之前的旧版本可执行文件——这是本项目实际发生过的真实故障，
/// 当时的构建脚本还没有自动清理旧文件这一步）。
///
/// Scan the modules/ directory to discover all available post-processing modules,
/// then append built-in node descriptions.
///
/// The build script (`buildModules` in scripts/common.js) copies each module's
/// executable under a version-suffixed filename (e.g. `notify_telegram-0.5.0.exe`), so
/// users can tell versions apart directly from the filename; every build first removes
/// all of that module's old filenames (versioned or not), so under normal operation
/// modules/ should only ever contain one file per module at a time.
///
/// However, if a user manually drops files into modules/, or an older version of the
/// build/upgrade process left a file behind uncleaned, the same `id` could still end up
/// declared by multiple executables at once. This is handled here as a defensive
/// fallback: only the one with the higher `version` field (semantic version, compared
/// numerically per dot-separated segment) is kept. This avoids downstream lookups like
/// `modules.iter().find(|m| m.id == ...)` nondeterministically picking either the old or
/// new version depending on directory iteration order (which actually happened in this
/// project before the build script gained its auto-cleanup step — causing inconsistent
/// version display, and executing a stale binary predating a bug fix).
pub fn discover_modules() -> Vec<ModuleInfo> {
    let dir = modules_dir();
    let mut by_id: std::collections::HashMap<String, ModuleInfo> = std::collections::HashMap::new();

    if dir.exists() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return crate::postprocess::builtin_nodes::builtin_module_infos(),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            #[cfg(target_os = "windows")]
            let is_exec = path.extension().and_then(|e| e.to_str()) == Some("exe");
            #[cfg(not(target_os = "windows"))]
            let is_exec = {
                use std::os::unix::fs::PermissionsExt;
                path.metadata()
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            };
            if !is_exec {
                continue;
            }
            match describe_module(&path) {
                Ok(mut info) => {
                    info.exe_path = path.clone();
                    match by_id.get(&info.id) {
                        Some(existing) if compare_versions(&existing.version, &info.version) >= 0 => {
                            tracing::warn!(
                                "Module id {:?} declared by multiple executables ({:?} and {:?}); keeping the higher version ({} >= {})",
                                info.id, existing.exe_path, path, existing.version, info.version
                            );
                        }
                        Some(existing) => {
                            tracing::warn!(
                                "Module id {:?} declared by multiple executables ({:?} and {:?}); keeping the higher version ({} > {})",
                                info.id, path, existing.exe_path, info.version, existing.version
                            );
                            by_id.insert(info.id.clone(), info);
                        }
                        None => {
                            by_id.insert(info.id.clone(), info);
                        }
                    }
                }
                Err(e) => tracing::error!("Failed to describe module {:?}: {}", path, e),
            }
        }
    }

    let mut modules: Vec<ModuleInfo> = by_id.into_values().collect();
    modules.sort_by(|a, b| a.id.cmp(&b.id));

    // 追加内置节点描述（recording_input、unpack）
    // Append built-in node descriptions (recording_input, unpack)
    modules.extend(crate::postprocess::builtin_nodes::builtin_module_infos());
    modules
}

/// 比较两个点号分隔的语义化版本号字符串，逐段按数字比较。
/// 返回值语义与 `Ordering` 一致：负数表示 `a < b`，0 表示相等，正数表示 `a > b`。
/// 无法解析为数字的段视为 0，空字符串视为 `0.0.0`（确保未提供版本号的旧模块
/// 始终被判定为版本最低，让任何带版本号的重复声明都能覆盖它）。
///
/// Compare two dot-separated semantic version strings, segment by segment as integers.
/// Return value follows `Ordering` semantics: negative means `a < b`, zero means equal,
/// positive means `a > b`. Unparseable segments are treated as 0; an empty string is
/// treated as `0.0.0` (ensuring older modules that never provided a version are always
/// considered lowest, so any versioned duplicate declaration takes precedence over them).
fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.').map(|seg| seg.parse::<u64>().unwrap_or(0)).collect()
    };
    let va = parse(a);
    let vb = parse(b);
    let len = va.len().max(vb.len());
    for i in 0..len {
        let xa = va.get(i).copied().unwrap_or(0);
        let xb = vb.get(i).copied().unwrap_or(0);
        match xa.cmp(&xb) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => continue,
        }
    }
    0
}

/// 调用模块的 `--describe` 参数，解析并返回模块元数据。
/// Call the module with `--describe` and parse the returned module metadata.
fn describe_module(exe: &PathBuf) -> crate::core::error::Result<ModuleInfo> {
    let output = std::process::Command::new(exe)
        .arg("--describe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| crate::core::error::AppError::Other(format!("spawn: {}", e)))?;
    if !output.status.success() {
        return Err(crate::core::error::AppError::Other(format!(
            "exit {}", output.status
        )));
    }
    let mut info: ModuleInfo = serde_json::from_slice(&output.stdout)
        .map_err(|e| crate::core::error::AppError::Other(format!("json: {}", e)))?;
    // 若模块未声明 input_types/output_types，使用兼容默认值（单 AnyFile 输入/输出）
    // If module doesn't declare port types, use compatible defaults (single AnyFile in/out)
    if info.input_types.is_empty() {
        info.input_types = vec![PortType::AnyFile];
    }
    if info.output_types.is_empty() {
        info.output_types = vec![PortType::AnyFile];
    }
    Ok(info)
}
