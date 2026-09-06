//! 磁盘空间查询 / Disk Space Query
//!
//! 提供跨平台的磁盘空间查询（Windows API / Unix statvfs）。
//! Provides cross-platform disk space querying (Windows API / Unix statvfs).

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
