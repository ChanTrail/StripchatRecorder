//! 磁盘空间查询 / Disk Space Query
//!
//! 提供跨平台的磁盘空间查询（Windows API / Unix statvfs）。
//! Provides cross-platform disk space querying (Windows API / Unix statvfs).

use crate::core::error::Result;

/// 磁盘空间信息（含目录标签）/ Disk space information (with directory label)
#[derive(serde::Serialize)]
pub struct DiskSpaceEntry {
    /// 目录用途标识（"ts_fragment" 或 "ts_merge"）/ Directory purpose label
    pub label: String,
    /// 目录路径（供前端悬停提示）/ Directory path (for frontend tooltip)
    pub path: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}

/// 单目录磁盘空间（内部使用，不带标签）/ Single-directory disk space (internal, no label)
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

/// 构建录制页面所需的磁盘空间列表。
///
/// - 若 ts_merge 的目录与 ts_fragment 目录位于同一磁盘（或未配置 ts_merge 目录），
///   则只返回一条，标签为 `"all"`（表示"所有录制文件所在磁盘"）。
/// - 若两者位于不同磁盘，则返回两条，分别标签为 `"ts_fragment"` 和 `"ts_merge"`。
/// - 同一磁盘的判断用 `total_bytes` 相等作为近似（跨平台，无需挂载点解析）。
///
/// Build the disk space entry list for the recordings page.
///
/// - If the ts_merge directory is on the same disk as the ts_fragment directory
///   (or no ts_merge directory is configured), only one entry is returned, labeled
///   `"all"` (meaning "the disk that holds all recording output").
/// - If they are on different disks, two entries are returned, labeled `"ts_fragment"`
///   and `"ts_merge"` respectively.
/// - Same-disk detection uses equal `total_bytes` as a cross-platform approximation
///   (no mount-point resolution needed).
pub fn get_disk_space_entries(
    ts_fragment_dir: &str,
    ts_merge_dir: Option<&str>,
) -> Vec<DiskSpaceEntry> {
    let ts = match get_disk_space_inner(ts_fragment_dir) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    // ts_merge 目录有效时才尝试查询
    // Only query ts_merge disk when the directory is configured
    let merge_space = ts_merge_dir
        .filter(|s| !s.is_empty())
        .and_then(|dir| get_disk_space_inner(dir).ok().map(|sp| (dir, sp)));

    match merge_space {
        Some((merge_dir, tm)) if tm.total_bytes != ts.total_bytes => {
            // 不同磁盘：两条，分别标注用途
            // Different disks: two entries with distinct labels
            vec![
                DiskSpaceEntry {
                    label: "ts_fragment".to_string(),
                    path: ts_fragment_dir.to_string(),
                    total_bytes: ts.total_bytes,
                    available_bytes: ts.available_bytes,
                    used_bytes: ts.used_bytes,
                },
                DiskSpaceEntry {
                    label: "ts_merge".to_string(),
                    path: merge_dir.to_string(),
                    total_bytes: tm.total_bytes,
                    available_bytes: tm.available_bytes,
                    used_bytes: tm.used_bytes,
                },
            ]
        }
        _ => {
            // 同一磁盘或未配置 ts_merge 目录：单条，标为"全部"
            // Same disk or no ts_merge dir configured: single entry labeled "all"
            vec![DiskSpaceEntry {
                label: "all".to_string(),
                path: ts_fragment_dir.to_string(),
                total_bytes: ts.total_bytes,
                available_bytes: ts.available_bytes,
                used_bytes: ts.used_bytes,
            }]
        }
    }
}
