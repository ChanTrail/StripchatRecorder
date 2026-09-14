//! 更新检查模块 / Update Check Module
//!
//! 提供版本检查、Docker 环境检测、平台识别功能，以及后台下载并安装更新的逻辑。
//!
//! 下载安装流程（download_and_install）：
//! 1. 用 reqwest 流式下载 zip，每 1%（或 200ms、512KB）广播 SSE `update-progress` 进度
//! 2. 用 self_update::Extract 解压到临时目录
//! 3. 用 self_update::MoveAll 事务性多文件替换（任一步失败全部回滚）
//! 4. 用 duct::cmd! 在独立进程中启动新版本，立即退出当前进程
//!
//! Download/install flow (download_and_install):
//! 1. Stream-download the zip via reqwest, broadcasting SSE `update-progress` every 1% / 200ms / 512KB
//! 2. Extract with self_update::Extract to a temp dir
//! 3. Atomically replace files with self_update::MoveAll (all-or-nothing, rolls back on failure)
//! 4. Spawn the new version in a detached process via duct::cmd!, then exit immediately

use serde::Serialize;
use std::sync::Arc;
use parking_lot::RwLock;

/// 当前应用版本（与 Cargo.toml 一致）
/// Current application version (matches Cargo.toml)
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const OWNER: &str = "ChanTrail";
const REPO: &str = "StripchatRecorder";

// ─── 平台识别 / Platform detection ───────────────────────────────────────────

/// 返回当前编译目标的平台字符串，与 GitHub Release asset 名称对应。
/// Returns the current compile-target platform string, matching GitHub Release asset names.
pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "linux-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "linux-aarch64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "darwin-x86_64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "darwin-aarch64";
    #[allow(unreachable_patterns)]
    #[allow(unreachable_code)]
    "unknown"
}

// ─── Docker 检测 / Docker detection ──────────────────────────────────────────

/// 检测当前运行环境是否为 Docker 容器。
pub fn is_docker() -> bool {
    if std::env::var("IS_DOCKER").as_deref() == Ok("1") {
        return true;
    }
    if std::path::Path::new("/.dockerenv").exists() {
        return true;
    }
    #[cfg(target_os = "linux")]
    if let Ok(content) = std::fs::read_to_string("/proc/1/cgroup")
        && (content.contains("docker") || content.contains("kubepods"))
    {
        return true;
    }
    false
}

// ─── GitHub Release 检查 / GitHub release check ───────────────────────────

/// GitHub Release 信息（仅包含前端需要的字段）
#[derive(Debug, Serialize)]
pub struct ReleaseInfo {
    pub latest_version: String,
    pub release_url: String,
    pub release_notes: String,
    pub published_at: String,
    pub download_url: Option<String>,
    pub download_size: Option<u64>,
}

/// GET /api/update/info 的完整响应结构
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub platform: String,
    pub is_docker: bool,
    pub release: Option<ReleaseInfo>,
    pub asset_names: Vec<String>,
}

/// 向 GitHub API 查询最新 Release，使用可选的代理地址。
pub async fn fetch_latest_release(
    proxy_url: Option<&str>,
) -> crate::core::error::Result<ReleaseInfo> {
    let (info, _) = fetch_latest_release_with_assets(proxy_url).await?;
    Ok(info)
}

/// 向 GitHub API 查询最新 Release，同时返回所有 asset 名称列表（用于调试）。
pub async fn fetch_latest_release_with_assets(
    proxy_url: Option<&str>,
) -> crate::core::error::Result<(ReleaseInfo, Vec<String>)> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        OWNER, REPO
    );

    let mut builder = reqwest::Client::builder()
        .user_agent(format!("StripchatRecorder/{}", APP_VERSION))
        .timeout(std::time::Duration::from_secs(15));

    if let Some(proxy) = proxy_url.filter(|s| !s.is_empty()) {
        let p = reqwest::Proxy::all(proxy)
            .map_err(|e| crate::core::error::AppError::Other(format!("代理配置错误: {}", e)))?;
        builder = builder.proxy(p);
    }

    let client = builder
        .build()
        .map_err(|e| crate::core::error::AppError::Other(format!("HTTP 客户端初始化失败: {}", e)))?;

    let resp = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| crate::core::error::AppError::Other(format!("GitHub API 请求失败: {}", e)))?;

    if !resp.status().is_success() {
        return Err(crate::core::error::AppError::Other(format!(
            "GitHub API 返回 {}",
            resp.status()
        )));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| crate::core::error::AppError::Other(format!("解析响应失败: {}", e)))?;

    let latest_version = json["tag_name"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches('v')
        .to_string();
    let release_url = json["html_url"].as_str().unwrap_or("").to_string();
    let release_notes = json["body"].as_str().unwrap_or("").to_string();
    let published_at = json["published_at"].as_str().unwrap_or("").to_string();

    // asset 命名规则：StripchatRecorder-server-{platform}.zip
    let platform = current_platform();
    let asset_name = format!("StripchatRecorder-server-{}.zip", platform);

    let asset_names: Vec<String> = json["assets"]
        .as_array()
        .map(|assets| {
            assets.iter()
                .filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let (download_url, download_size) = json["assets"]
        .as_array()
        .and_then(|assets| {
            assets.iter().find(|a| a["name"].as_str() == Some(&asset_name))
        })
        .map(|a| {
            let url = a["browser_download_url"].as_str().unwrap_or("").to_string();
            let size = a["size"].as_u64();
            (Some(url), size)
        })
        .unwrap_or((None, None));

    Ok((ReleaseInfo {
        latest_version,
        release_url,
        release_notes,
        published_at,
        download_url,
        download_size,
    }, asset_names))
}

// ─── 语义化版本比较 / Semantic version comparison ────────────────────────────

/// 语义化版本比较：`latest` > `current` 时返回 true。
///
/// 支持预发布后缀（如 `0.4.0-beta`）：比较时只取 `major.minor.patch` 数字部分，
/// 预发布版本的 patch 数字与正式版相同时视为相等（不认为正式版更新）。
/// 这样 `0.4.0-beta` 不会被 `0.3.5` 触发更新提示。
///
/// Supports pre-release suffixes (e.g. `0.4.0-beta`): only the numeric
/// `major.minor.patch` portion is compared. A pre-release version with the
/// same patch number is treated as equal to the stable release, so
/// `0.4.0-beta` will not trigger an update notification for `0.3.5`.
pub fn semver_gt(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.splitn(3, '.');
        let a = it.next()?.parse::<u64>().ok()?;
        let b = it.next()?.parse::<u64>().ok()?;
        // 截断预发布后缀（如 "0-beta" → "0"）再解析
        // Strip any pre-release suffix before parsing (e.g. "0-beta" → "0")
        let patch_str = it.next().unwrap_or("0");
        let patch_num = patch_str.split('-').next().unwrap_or("0");
        let c = patch_num.parse::<u64>().ok()?;
        Some((a, b, c))
    }
    match (parse(latest), parse(current)) {
        (Some((la, lb, lc)), Some((ca, cb, cc))) => (la, lb, lc) > (ca, cb, cc),
        _ => latest != current,
    }
}

// ─── 更新安装状态 / Update install state ─────────────────────────────────────

/// 更新下载/安装的进度状态，通过 SSE `update-progress` 事件广播给前端。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateProgress {
    Idle,
    Downloading {
        downloaded: u64,
        total: u64,
        pct: Option<u8>,
    },
    Installing,
    Done,
    Error { message: String },
}

pub type UpdateStateStore = Arc<RwLock<UpdateProgress>>;

pub fn new_update_state() -> UpdateStateStore {
    Arc::new(RwLock::new(UpdateProgress::Idle))
}

// ─── 下载 + 安装 / Download + Install ────────────────────────────────────────

/// 后台下载 zip 包、用 self_update 解压并多文件事务性替换，然后用 duct 启动新进程后退出。
///
/// 保留与旧版完全一致的 SSE `update-progress` 事件接口，前端无需任何改动。
///
/// Downloads the zip, extracts and atomically replaces files using self_update::MoveAll,
/// then spawns the new process via duct and exits immediately.
/// SSE `update-progress` interface is identical to the old version — no frontend changes needed.
pub async fn download_and_install(
    download_url: String,
    proxy_url: Option<String>,
    state_store: UpdateStateStore,
    emitter: Arc<dyn crate::core::emitter::Emitter>,
) {
    use crate::core::emitter::EmitterExt;
    use futures_util::StreamExt;

    macro_rules! emit_state {
        ($s:expr) => {{
            *state_store.write() = $s.clone();
            emitter.emit("update-progress", &$s);
        }};
    }

    macro_rules! bail {
        ($msg:expr) => {{
            let e = UpdateProgress::Error { message: $msg.to_string() };
            emit_state!(e);
            return;
        }};
    }

    // ── 1. 流式下载 zip，实时广播进度 / Stream-download with live progress ──
    let mut builder = reqwest::Client::builder()
        .user_agent(format!("StripchatRecorder/{}", APP_VERSION))
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(60))
        .timeout(std::time::Duration::from_secs(600));

    if let Some(proxy) = proxy_url.as_deref().filter(|s| !s.is_empty()) {
        match reqwest::Proxy::all(proxy) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => bail!(format!("代理配置错误: {}", e)),
        }
    }

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => bail!(format!("HTTP 客户端初始化失败: {}", e)),
    };

    let resp = match client.get(&download_url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => bail!(format!("下载请求失败: HTTP {}", r.status())),
        Err(e) => bail!(format!("下载失败: {}", e)),
    };

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut zip_bytes: Vec<u8> = if total > 0 {
        Vec::with_capacity(total as usize)
    } else {
        Vec::new()
    };

    emit_state!(UpdateProgress::Downloading {
        downloaded: 0,
        total,
        pct: if total > 0 { Some(0) } else { None },
    });

    // 节流：total 已知时每 1% 或 200ms 广播；未知时每 512KB 或 200ms 广播
    // Throttle: broadcast every 1% or 200ms (known total), or every 512KB or 200ms (unknown)
    const THROTTLE_MS: u128 = 200;
    const BYTES_THRESHOLD: u64 = 512 * 1024;

    let mut last_emit = std::time::Instant::now();
    let mut last_pct: Option<u8> = if total > 0 { Some(0) } else { None };
    let mut last_emit_bytes: u64 = 0;

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                zip_bytes.extend_from_slice(&bytes);
                downloaded += bytes.len() as u64;
                let pct = (downloaded * 100)
                    .checked_div(total)
                    .map(|r| r.min(100) as u8);
                let elapsed = last_emit.elapsed().as_millis() >= THROTTLE_MS;
                let pct_changed = pct != last_pct;
                let bytes_threshold =
                    total == 0 && (downloaded - last_emit_bytes) >= BYTES_THRESHOLD;
                if elapsed || pct_changed || bytes_threshold {
                    emit_state!(UpdateProgress::Downloading { downloaded, total, pct });
                    last_emit = std::time::Instant::now();
                    last_pct = pct;
                    last_emit_bytes = downloaded;
                }
            }
            Err(e) => bail!(format!("下载中断: {}", e)),
        }
    }

    let final_pct = if total > 0 { Some(100u8) } else { None };
    emit_state!(UpdateProgress::Downloading { downloaded, total, pct: final_pct });

    // SSE 推送完成前稍作等待 / Brief pause to let SSE deliver the final download state
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // ── 2. 解压 + 事务性替换 / Extract + atomic replace via self_update ──
    emit_state!(UpdateProgress::Installing);

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => bail!(format!("无法获取当前可执行文件路径: {}", e)),
    };
    let exe_dir = match exe_path.parent() {
        Some(d) => d.to_path_buf(),
        None => bail!("无法获取可执行文件所在目录"),
    };

    // 解压和替换在阻塞线程中执行，避免阻塞 tokio executor
    // Run on a blocking thread so we don't stall the tokio executor
    let result = tokio::task::spawn_blocking(move || {
        extract_and_replace_with_self_update(&zip_bytes, &exe_dir)
    }).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!(e),
        Err(e) => bail!(format!("安装任务崩溃: {}", e)),
    }

    // ── 3. 完成，用 duct 启动新进程后立即退出 / Done: detached spawn via duct, then exit ──
    emit_state!(UpdateProgress::Done);
    tracing::info!("{}", crate::tl!("update.installDone"));

    // 给 SSE 一点时间送达完成状态 / Give SSE a moment to deliver the done state
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("{}", crate::tl!("update.exePathFailed", error = e));
            std::process::exit(0);
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();

    tracing::info!("{}", crate::tl!("update.launching",
        exe = exe.display(), args = format!("{:?}", args)));

    // duct::cmd 构建命令，.unchecked() 不因子进程退出码非零而报错
    // .unchecked() so duct doesn't error if the child exits with a nonzero code
    let cmd = duct::cmd(exe.as_os_str(), &args)
        .env("STRIPCHAT_RESTART_DELAY_MS", "2000")
        .unchecked();

    match cmd.start() {
        Ok(handle) => {
            tracing::info!("{}", crate::tl!("update.launchSuccess", pid = handle.pids()[0]));
            // 分离子进程：遗忘 handle，父进程直接退出，无需等待子进程
            // Detach: forget the handle, parent exits immediately to free the port
            std::mem::forget(handle);
        }
        Err(e) => {
            tracing::error!("{}", crate::tl!("update.launchFailed", error = e));
        }
    }

    std::process::exit(0);
}

// ─── 私有辅助函数 / Private helpers ──────────────────────────────────────────

/// 用 self_update::Extract 解压 zip 到临时目录，再用 self_update::MoveAll 事务性多文件替换。
///
/// zip 内层目录结构：`{package-name}/{files...}`，跳过顶层目录直接提取内容。
/// MoveAll 要求源文件与目标在同一文件系统（使用 rename），因此
/// staging/stash 临时目录均在 exe_dir 内创建。
///
/// Extract zip to a staging temp dir via self_update::Extract, then atomically replace
/// all files via self_update::MoveAll (all-or-nothing; rolls back on failure).
/// Staging and stash are created inside exe_dir to stay on the same filesystem as the install target.
fn extract_and_replace_with_self_update(
    zip_bytes: &[u8],
    exe_dir: &std::path::Path,
) -> Result<(), String> {
    // 将 zip 字节写入临时文件，self_update::Extract 需要文件路径而非内存切片
    // self_update::Extract needs a file path, not a byte slice — write to a temp file first
    let tmp = tempfile::Builder::new()
        .prefix("stripchat_update_")
        .suffix(".zip")
        .tempfile_in(exe_dir)
        .map_err(|e| format!("创建临时 zip 文件失败: {}", e))?;

    std::fs::write(tmp.path(), zip_bytes)
        .map_err(|e| format!("写入临时 zip 失败: {}", e))?;

    // 解压到 staging 临时目录（在 exe_dir 内保证同一文件系统）
    // Extract to a staging temp dir inside exe_dir to guarantee same filesystem
    let staging = tempfile::Builder::new()
        .prefix("stripchat_staging_")
        .tempdir_in(exe_dir)
        .map_err(|e| format!("创建解压目录失败: {}", e))?;

    self_update::Extract::from_source(tmp.path())
        .archive(self_update::ArchiveKind::Zip)
        .extract_into(staging.path())
        .map_err(|e| format!("解压失败: {}", e))?;

    // zip 内第一层是包名目录，跳过它找到实际内容
    // Skip the top-level package-name directory inside the zip
    let content_dir = find_single_subdir(staging.path())
        .unwrap_or_else(|| staging.path().to_path_buf());

    // stash 目录同样在 exe_dir 内，MoveAll 需要它与目标在同一文件系统
    // Stash dir also inside exe_dir — MoveAll requires same filesystem as destinations
    let stash = tempfile::Builder::new()
        .prefix("stripchat_stash_")
        .tempdir_in(exe_dir)
        .map_err(|e| format!("创建 stash 目录失败: {}", e))?;

    // 收集所有 (src → dest) 文件对
    // Collect all (src → dest) file pairs
    let mut pairs: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    collect_file_pairs(&content_dir, exe_dir, &mut pairs);

    // 确保所有目标父目录存在（MoveAll 本身不创建目录）
    // Ensure all destination parent directories exist (MoveAll doesn't create them)
    for (_, dest) in &pairs {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
        }
    }

    // 构建 MoveAll 并提交
    // Build MoveAll and commit
    let mut mover = self_update::MoveAll::from_temp(stash.path());
    for (src, dest) in pairs {
        mover.add(src, dest);
    }
    mover.commit()
        .map_err(|e| format!("文件替换失败（MoveAll）: {}", e))?;

    tracing::info!("{}", crate::tl!("update.unzipDone"));

    // Linux/macOS：为 exe_dir 根级文件设置执行权限
    // Linux/macOS: set executable bit on root-level files in exe_dir
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(entries) = std::fs::read_dir(exe_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() 
                    && let Ok(meta) = std::fs::metadata(&path) {
                        let mut perms = meta.permissions();
                        perms.set_mode(perms.mode() | 0o111);
                        let _ = std::fs::set_permissions(&path, perms);
                    }
            }
        }
    }

    Ok(())
}

/// 如果 `dir` 下恰好只有一个子目录（且没有文件），返回该子目录；否则返回 None。
/// Returns the single subdirectory of `dir` if there is exactly one dir entry and it's a dir.
fn find_single_subdir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    if entries.len() == 1 {
        let entry = &entries[0];
        if entry.file_type().ok()?.is_dir() {
            return Some(entry.path());
        }
    }
    None
}

/// 递归收集 `src_base` 下所有文件的 `(src, dest)` 对，dest 路径相对于 `dest_base`。
/// Recursively collect (src, dest) pairs for all files under `src_base`,
/// with dest paths rooted at `dest_base`.
fn collect_file_pairs(
    src_base: &std::path::Path,
    dest_base: &std::path::Path,
    pairs: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>,
) {
    let Ok(entries) = std::fs::read_dir(src_base) else { return };
    for entry in entries.flatten() {
        let src = entry.path();
        let rel = src.strip_prefix(src_base).unwrap_or(&src);
        let dest = dest_base.join(rel);
        if src.is_dir() {
            collect_file_pairs(&src, &dest_base.join(rel), pairs);
        } else if src.is_file() {
            pairs.push((src, dest));
        }
    }
}
