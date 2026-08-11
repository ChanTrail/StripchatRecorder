//! 磁盘空间查询与目录浏览命令 / Disk Space Query and Directory Browsing Commands
//!
//! 提供跨平台的磁盘空间查询，以及供前端"选择目录"浏览器使用的目录列举/创建功能，
//! 均由 HTTP server 的设置路由复用。
//! Provides cross-platform disk space querying, plus directory listing/creation for the
//! frontend's "choose directory" browser. Reused by the settings HTTP route.

use crate::core::error::Result;

/// 磁盘空间信息 / Disk space information
#[derive(serde::Serialize)]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}

/// 获取指定路径所在磁盘的空间信息（跨平台实现）。
/// Get disk space information for the drive containing the given path (cross-platform implementation).
pub fn get_disk_space_inner(output_dir: &str) -> Result<DiskSpace> {
    let path = std::path::Path::new(output_dir);

    let existing = std::iter::successors(Some(path), |p| p.parent())
        .find(|p| p.exists())
        .unwrap_or(path);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = existing
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes: u64 = 0;
        let mut total_bytes: u64 = 0;
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(
                lp_directory_name: *const u16,
                lp_free_bytes_available_to_caller: *mut u64,
                lp_total_number_of_bytes: *mut u64,
                lp_total_number_of_free_bytes: *mut u64,
            ) -> i32;
        }
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free_bytes,
                &mut total_bytes,
                std::ptr::null_mut(),
            )
        };
        if ok != 0 {
            return Ok(DiskSpace {
                total_bytes,
                available_bytes: free_bytes,
                used_bytes: total_bytes.saturating_sub(free_bytes),
            });
        }
    }

    #[cfg(unix)]
    {
        use std::mem::MaybeUninit;
        let path_cstr = std::ffi::CString::new(existing.to_string_lossy().as_bytes()).unwrap();
        let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
        let ret = unsafe { libc::statvfs(path_cstr.as_ptr(), stat.as_mut_ptr()) };
        if ret == 0 {
            let stat = unsafe { stat.assume_init() };
            #[cfg(target_os = "macos")]
            let block = stat.f_frsize as u64;
            #[cfg(not(target_os = "macos"))]
            let block = stat.f_frsize;
            #[cfg(target_os = "macos")]
            let total = stat.f_blocks as u64 * block;
            #[cfg(not(target_os = "macos"))]
            let total = stat.f_blocks * block;
            #[cfg(target_os = "macos")]
            let avail = stat.f_bavail as u64 * block;
            #[cfg(not(target_os = "macos"))]
            let avail = stat.f_bavail * block;
            return Ok(DiskSpace {
                total_bytes: total,
                available_bytes: avail,
                used_bytes: total.saturating_sub(avail),
            });
        }
    }

    Err(crate::core::error::AppError::Other(
        "无法获取磁盘空间信息".to_string(),
    ))
}

// ─── 目录浏览 / Directory Browsing ────────────────────────────────────────────

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
///
/// Safety: only returns directory name listings, never reads file contents; falls back to
/// drive roots / user home when the path is empty or doesn't exist, rather than erroring out.
/// Returned paths have the Windows `\\?\` prefix stripped, presented in the familiar
/// plain path format.
pub fn list_dir_inner(path: &str) -> Result<ListDirResult> {
    let requested = std::path::Path::new(path);

    // 空路径或不存在的路径：回退到用户主目录（存在则用），否则回退到当前工作目录
    // Empty or non-existent path: fall back to the user's home directory, else the cwd
    let dir = if path.trim().is_empty() || !requested.is_dir() {
        dirs::home_dir().unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    } else {
        requested.to_path_buf()
    };

    let canonical_raw = dir
        .canonicalize()
        .map_err(|e| crate::core::error::AppError::Other(format!("无法访问目录: {}", e)))?;
    // 用于实际文件系统访问（保留 \\?\ 前缀，支持超长路径）；对外展示时再清理前缀
    // Used for actual filesystem access (keeps \\?\ prefix to support long paths);
    // the prefix is stripped only when presenting to the outside
    let canonical_clean = strip_verbatim_prefix(&canonical_raw);

    let mut dirs_list: Vec<DirEntryInfo> = Vec::new();
    match std::fs::read_dir(&canonical_raw) {
        Ok(entries) => {
            for entry in entries.flatten() {
                // 跳过因权限问题无法读取元数据的条目，而不是整体报错
                // Skip entries whose metadata can't be read (permission issues) instead of failing entirely
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                // 跳过隐藏目录（以 . 开头），减少无关噪音 / Skip hidden directories (dotfiles), reduce noise
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
    dirs_list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

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
    // 禁止路径分隔符和上级目录引用，防止越出 parent 目录
    // Disallow path separators and parent-dir references to prevent escaping the parent directory
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
