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
/// Scan the modules/ directory to discover all available post-processing modules,
/// then append built-in node descriptions.
pub fn discover_modules() -> Vec<ModuleInfo> {
    let dir = modules_dir();
    let mut modules = Vec::new();

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
                    info.exe_path = path;
                    modules.push(info);
                }
                Err(e) => tracing::error!("Failed to describe module {:?}: {}", path, e),
            }
        }
    }

    // 追加内置节点描述（recording_input、unpack）
    // Append built-in node descriptions (recording_input, unpack)
    modules.extend(crate::postprocess::builtin_nodes::builtin_module_infos());
    modules
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
