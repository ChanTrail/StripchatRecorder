//! 设置管理 Tauri 命令 / Settings Management Tauri Commands
//!
//! 提供应用设置的读取/保存、目录选择器、Mouflon 密钥管理、启动警告查询和磁盘空间查询等命令。
//! Provides commands for reading/saving app settings, directory picker, Mouflon key management,
//! startup warning queries, and disk space queries.

use crate::core::error::Result;
use crate::streaming::monitor::StatusMonitor;
use crate::config::settings::{AppState, Settings};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

/// 获取当前应用设置。
/// Get the current application settings.
#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> Result<Settings> {
    Ok(state.get_settings())
}

/// 保存新的应用设置，广播 `settings-updated` 事件，并在并发限制放宽时尝试启动待录制的主播。
/// Save new application settings, broadcast `settings-updated` event,
/// and try to start pending recordings if the concurrency limit was relaxed.
#[tauri::command]
pub async fn save_settings_cmd(
    new_settings: Settings,
    app_handle: AppHandle,
    state: State<'_, Arc<AppState>>,
    monitor: State<'_, Arc<StatusMonitor>>,
) -> Result<()> {
    let old_max = state.get_settings().max_concurrent;
    let new_max = new_settings.max_concurrent;
    state.update_settings(new_settings)?;
    let _ = app_handle.emit("settings-updated", state.get_settings());

    // 若并发限制放宽（从有限制变为无限制，或上限增大），尝试启动等待中的录制
    // If concurrency limit was relaxed (limited -> unlimited, or limit increased), try starting pending recordings
    let limit_relaxed = new_max == 0 || (old_max > 0 && new_max > old_max);
    if limit_relaxed {
        let monitor = Arc::clone(&monitor);
        let app_handle = app_handle.clone();
        tokio::spawn(async move {
            monitor.try_start_pending(&app_handle).await;
        });
    }

    Ok(())
}

/// 打开系统目录选择对话框，返回用户选择的目录路径（取消则返回 `None`）。
/// Open the system directory picker dialog; returns the selected path or `None` if cancelled.
#[tauri::command]
pub async fn pick_output_dir(app_handle: AppHandle) -> Result<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();

    app_handle
        .dialog()
        .file()
        .set_title("选择输出目录")
        .pick_folder(move |path| {
            let _ = tx.send(path);
        });

    match rx.recv() {
        Ok(Some(path)) => Ok(Some(path.to_string())),
        Ok(None) => Ok(None),
        Err(_) => Ok(None),
    }
}

/// 列出所有 Mouflon HLS 解密密钥（pkey -> pdkey 映射）。
/// List all Mouflon HLS decryption keys (pkey -> pdkey map).
#[tauri::command]
pub async fn list_mouflon_keys(state: State<'_, Arc<AppState>>) -> Result<HashMap<String, String>> {
    Ok(state.get_mouflon_keys())
}

/// 添加或更新一个 Mouflon 密钥对。
/// Add or update a Mouflon key pair.
#[tauri::command]
pub async fn add_mouflon_key(
    pkey: String,
    pdkey: String,
    state: State<'_, Arc<AppState>>,
) -> Result<()> {
    state.add_mouflon_key(&pkey, &pdkey)
}

/// 删除指定 pkey 的 Mouflon 密钥。
/// Remove the Mouflon key with the given pkey.
#[tauri::command]
pub async fn remove_mouflon_key(pkey: String, state: State<'_, Arc<AppState>>) -> Result<()> {
    state.remove_mouflon_key(&pkey)
}

/// 启动警告数据结构（序列化后返回给前端）/ Startup warnings data structure (serialized and returned to the frontend)
#[derive(serde::Serialize)]
pub struct StartupWarnings {
    /// 不存在的主播用户名列表 / List of non-existent streamer usernames
    pub missing_streamers: Vec<String>,
    /// 对应文件已删除的后处理记录路径列表 / List of pp_result paths whose files have been deleted
    pub missing_pp_results: Vec<String>,
}

/// 查询启动警告：检查 pp_results 中是否有对应文件已不存在的孤立记录。
/// Query startup warnings: check for orphaned pp_results entries whose files no longer exist.
#[tauri::command]
pub async fn get_startup_warnings(state: State<'_, Arc<AppState>>) -> Result<StartupWarnings> {
    let state = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let data = state.data.read();

        let missing_pp_results: Vec<String> = data
            .pp_results
            .keys()
            .filter(|path| !std::path::Path::new(path).exists())
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

/// 从 pp_results 中删除指定路径的孤立记录并保存。
/// Remove orphaned pp_result entries for the given paths and save.
#[tauri::command]
pub async fn remove_missing_pp_results(
    paths: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<()> {
    let mut data = state.data.write();
    for path in &paths {
        data.pp_results.remove(path);
    }
    drop(data);
    state.save()
}

/// 磁盘空间信息（序列化后返回给前端）/ Disk space information (serialized and returned to the frontend)
#[derive(serde::Serialize)]
pub struct DiskSpace {
    /// 磁盘总容量（字节）/ Total disk capacity (bytes)
    pub total_bytes: u64,
    /// 可用空间（字节）/ Available space (bytes)
    pub available_bytes: u64,
    /// 已用空间（字节）/ Used space (bytes)
    pub used_bytes: u64,
}

/// 查询录制输出目录所在磁盘的空间信息。
/// Query disk space information for the drive containing the recording output directory.
#[tauri::command]
pub async fn get_disk_space(state: State<'_, Arc<AppState>>) -> Result<DiskSpace> {
    let output_dir = state.get_settings().output_dir;
    get_disk_space_inner(&output_dir)
}

/// 获取指定路径所在磁盘的空间信息（跨平台实现）。
/// Get disk space information for the drive containing the given path (cross-platform implementation).
pub fn get_disk_space_inner(output_dir: &str) -> Result<DiskSpace> {
    let path = std::path::Path::new(output_dir);

    // 向上查找第一个实际存在的祖先目录 / Walk up to find the first existing ancestor directory
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
            let block = stat.f_frsize as u64;
            let total = stat.f_blocks as u64 * block;
            let avail = stat.f_bavail as u64 * block;
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
