//! 设置管理命令 / Settings Management Commands
//!
//! 提供应用设置的读取/保存、Mouflon 密钥管理、启动警告查询和磁盘空间查询等功能。
//! Provides commands for reading/saving app settings, Mouflon key management,
//! startup warning queries, and disk space queries.
//! These functions are called directly by the HTTP server handlers in server_mod/server.rs.

use crate::config::settings::{AppState, Settings};
use crate::core::error::Result;
use std::sync::Arc;

/// 获取当前应用设置。
/// Get the current application settings.
pub async fn get_settings(state: &Arc<AppState>) -> Result<Settings> {
    Ok(state.get_settings())
}

/// 保存新的应用设置。
/// Save new application settings.
pub async fn save_settings(new_settings: Settings, state: &Arc<AppState>) -> Result<()> {
    state.update_settings(new_settings)?;
    Ok(())
}

/// 启动警告数据结构 / Startup warnings data structure
#[derive(serde::Serialize)]
pub struct StartupWarnings {
    pub missing_streamers: Vec<String>,
    pub missing_pp_results: Vec<String>,
}

/// 查询启动警告：检查 pp_results 中是否有对应文件已不存在的孤立记录。
/// Query startup warnings: check for orphaned pp_results entries whose files no longer exist.
pub async fn get_startup_warnings(state: &Arc<AppState>) -> Result<StartupWarnings> {
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || {
        let data = state.data.read();

        let missing_pp_results: Vec<String> = data
            .pp_results
            .iter()
            .filter(|path| !std::path::Path::new(path.as_str()).exists())
            .cloned()
            .collect();

        Ok(StartupWarnings {
            missing_streamers: Vec::new(),
            missing_pp_results,
        })
    })
    .await
    .map_err(|e| crate::core::error::AppError::Other(e.to_string()))?
}

/// 磁盘空间信息 / Disk space information
#[derive(serde::Serialize)]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
}

/// 查询录制输出目录所在磁盘的空间信息。
/// Query disk space information for the drive containing the recording output directory.
pub async fn get_disk_space(state: &Arc<AppState>) -> Result<DiskSpace> {
    let output_dir = state.get_settings().output_dir;
    get_disk_space_inner(&output_dir)
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
