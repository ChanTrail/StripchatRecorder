//! 目录浏览工具 / Directory Browser Utilities
//!
//! 供前端"选择目录"浏览器使用的目录列举、驱动器列举和目录创建功能。
//! 被 `server_mod/routes/fs.rs` 调用。
//!
//! Directory listing, drive listing, and directory creation for the
//! frontend's "choose directory" browser.
//! Called by `server_mod/routes/fs.rs`.

use crate::core::error::Result;

/// 目录浏览返回的单个条目 / Single entry returned by directory browsing
#[derive(serde::Serialize)]
pub struct DirEntryInfo {
    /// 条目名称（不含路径）/ Entry name (without path)
    pub name: String,
    /// 完整路径 / Full path
    pub path: String,
}

/// 目录浏览响应：当前路径、父路径（到达根时为 None）、子目录列表。
/// Directory browsing response: current path, parent path (None at root), subdirectory list.
#[derive(serde::Serialize)]
pub struct ListDirResult {
    pub path: String,
    pub parent: Option<String>,
    pub dirs: Vec<DirEntryInfo>,
    /// 是否为"驱动器/根列表"视图（此时 path 无实际意义，不可直接选择）
    /// Whether this is a "drives/roots" listing view (path has no real meaning, not directly selectable)
    #[serde(default)]
    pub is_drives: bool,
}

/// 去除 Windows `canonicalize()` 产生的 `\\?\` 扩展长度路径前缀（以及 UNC 变体
/// `\\?\UNC\`），还原为用户熟悉的普通路径形式（如 `D:\foo` 而非 `\\?\D:\foo`）。
/// 非 Windows 平台或不含该前缀时原样返回。
///
/// Strip the `\\?\` extended-length path prefix (and its UNC variant `\\?\UNC\`)
/// produced by Windows `canonicalize()`, restoring the familiar plain path form
/// (e.g. `D:\foo` instead of `\\?\D:\foo`). Returns the path unchanged on other
/// platforms or when the prefix is absent.
#[cfg(target_os = "windows")]
fn strip_verbatim_prefix(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{}", rest))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

#[cfg(not(target_os = "windows"))]
fn strip_verbatim_prefix(p: &std::path::Path) -> std::path::PathBuf {
    p.to_path_buf()
}

/// 列出指定路径下的子目录（仅目录，不含文件），用于前端"选择目录"浏览器。
///
/// 安全性：仅返回目录名列表，不读取文件内容；路径为空或不存在时回退到系统盘根/用户主目录，
/// 不会因意外路径而报错中断浏览流程。返回路径已去除 Windows 的 `\\?\` 前缀，
/// 呈现为用户熟悉的普通路径格式。
///
/// List subdirectories under the given path (directories only, no files), for the
/// frontend's "choose directory" browser.
pub fn list_dir_inner(path: &str) -> Result<ListDirResult> {
    let requested = std::path::Path::new(path);

    let dir = if path.trim().is_empty() || !requested.is_dir() {
        dirs::home_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    } else {
        requested.to_path_buf()
    };

    let canonical_raw = dir
        .canonicalize()
        .map_err(|e| crate::core::error::AppError::Other(format!("无法访问目录: {}", e)))?;
    let canonical_clean = strip_verbatim_prefix(&canonical_raw);

    let mut dirs_list: Vec<DirEntryInfo> = Vec::new();
    match std::fs::read_dir(&canonical_raw) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                dirs_list.push(DirEntryInfo {
                    path: canonical_clean.join(&name).to_string_lossy().to_string(),
                    name,
                });
            }
        }
        Err(e) => {
            return Err(crate::core::error::AppError::Other(format!(
                "无法读取目录: {}",
                e
            )));
        }
    }
    dirs_list.sort_by_key(|a| a.name.to_lowercase());

    let parent = canonical_clean
        .parent()
        .map(|p| p.to_string_lossy().to_string());

    Ok(ListDirResult {
        path: canonical_clean.to_string_lossy().to_string(),
        parent,
        dirs: dirs_list,
        is_drives: false,
    })
}

/// 列出系统所有可用驱动器（Windows：存在的盘符 A:\~Z:\；Unix：文件系统根 /），
/// 用作目录浏览器的顶层导航入口（"此电脑"）。
///
/// List all available system drives (Windows: existing drive letters A:\~Z:\;
/// Unix: filesystem root /), used as the top-level navigation entry ("This PC")
/// of the directory browser.
pub fn list_drives_inner() -> Result<ListDirResult> {
    let mut dirs_list: Vec<DirEntryInfo> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        for letter in b'A'..=b'Z' {
            let letter = letter as char;
            let root = format!("{}:\\", letter);
            if std::path::Path::new(&root).is_dir() {
                dirs_list.push(DirEntryInfo {
                    name: format!("{}:", letter),
                    path: root,
                });
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        dirs_list.push(DirEntryInfo {
            name: "/".to_string(),
            path: "/".to_string(),
        });
    }

    Ok(ListDirResult {
        path: String::new(),
        parent: None,
        dirs: dirs_list,
        is_drives: true,
    })
}

/// 在指定路径下创建一个新子目录（用于目录浏览器的"新建文件夹"操作）。
/// Create a new subdirectory under the given path (for the directory browser's "new folder" action).
pub fn create_dir_inner(parent: &str, name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::core::error::AppError::Other(
            "目录名不能为空".to_string(),
        ));
    }
    if trimmed.contains(['/', '\\']) || trimmed == ".." {
        return Err(crate::core::error::AppError::Other(
            "目录名不能包含路径分隔符".to_string(),
        ));
    }
    let new_dir = std::path::Path::new(parent).join(trimmed);
    std::fs::create_dir_all(&new_dir)
        .map_err(|e| crate::core::error::AppError::Other(format!("创建目录失败: {}", e)))?;
    Ok(new_dir.to_string_lossy().to_string())
}
