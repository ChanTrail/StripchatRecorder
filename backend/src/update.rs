//! 更新检查模块 / Update Check Module
//!
//! 提供版本检查、Docker 环境检测、平台识别功能。
//! 前端通过 GET /api/update/info 获取更新信息，后端负责向 GitHub API 查询最新 Release。
//!
//! Provides version checking, Docker environment detection, and platform identification.
//! The frontend calls GET /api/update/info; the backend queries the GitHub API for the latest release.

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
///
/// 按优先级依次检查：
/// 1. 环境变量 `IS_DOCKER=1`（可由 Dockerfile 手动设置，最可靠）
/// 2. `/.dockerenv` 文件存在
/// 3. `/proc/1/cgroup` 中包含 "docker" 或 "kubepods"（Linux 下 cgroup v1）
///
/// Detects whether the current runtime environment is a Docker container.
///
/// Checked in priority order:
/// 1. Env var `IS_DOCKER=1` (manually set in Dockerfile — most reliable)
/// 2. Presence of `/.dockerenv`
/// 3. `/proc/1/cgroup` contains "docker" or "kubepods" (Linux cgroup v1)
pub fn is_docker() -> bool {
    // 1. 显式环境变量 / explicit env var
    if std::env::var("IS_DOCKER").as_deref() == Ok("1") {
        return true;
    }

    // 2. /.dockerenv 文件（Docker 在容器内自动创建）
    //    /.dockerenv file (automatically created by Docker inside containers)
    if std::path::Path::new("/.dockerenv").exists() {
        return true;
    }

    // 3. /proc/1/cgroup（仅 Linux，cgroup v1）
    //    /proc/1/cgroup (Linux only, cgroup v1)
    #[cfg(target_os = "linux")]
    if let Ok(content) = std::fs::read_to_string("/proc/1/cgroup")
        && (content.contains("docker") || content.contains("kubepods")) {
        return true;
    }

    false
}

// ─── GitHub Release 检查 / GitHub release check ───────────────────────────

/// GitHub Release 信息（仅包含前端需要的字段）
/// GitHub Release information (only fields needed by the frontend)
#[derive(Debug, Serialize)]
pub struct ReleaseInfo {
    /// 最新版本号（去掉 "v" 前缀）/ Latest version (without "v" prefix)
    pub latest_version: String,
    /// Release 页面 URL / Release page URL
    pub release_url: String,
    /// Release body（更新日志，Markdown）/ Release body (changelog, Markdown)
    pub release_notes: String,
    /// 发布时间（ISO 8601）/ Published time (ISO 8601)
    pub published_at: String,
    /// 当前平台对应的 asset 直链（无对应 asset 时为 None）
    /// Direct download URL for the current platform's asset (None if no matching asset)
    pub download_url: Option<String>,
    /// 当前平台对应的 asset 文件大小（字节，无 asset 时为 None）
    /// Asset file size in bytes for the current platform (None if no matching asset)
    pub download_size: Option<u64>,
}

/// GET /api/update/info 的完整响应结构
/// Full response structure for GET /api/update/info
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    /// 当前运行版本 / Current running version
    pub current_version: String,
    /// 运行平台标识 / Runtime platform identifier
    pub platform: String,
    /// 是否在 Docker 容器中运行 / Whether running inside a Docker container
    pub is_docker: bool,
    /// 最新 Release 信息；None 表示检查失败 / Latest release info; None means check failed
    pub release: Option<ReleaseInfo>,
    /// 所有 release asset 名称列表（用于调试匹配问题）
    /// All release asset names (for debugging matching issues)
    pub asset_names: Vec<String>,
}

/// 向 GitHub API 查询最新 Release，使用可选的代理地址。
/// Queries the GitHub API for the latest release, using an optional proxy address.
pub async fn fetch_latest_release(
    proxy_url: Option<&str>,
) -> crate::core::error::Result<ReleaseInfo> {
    let (info, _) = fetch_latest_release_with_assets(proxy_url).await?;
    Ok(info)
}

/// 向 GitHub API 查询最新 Release，同时返回所有 asset 名称列表（用于调试）。
/// Queries the latest release and also returns all asset names (for debugging).
pub async fn fetch_latest_release_with_assets(
    proxy_url: Option<&str>,
) -> crate::core::error::Result<(ReleaseInfo, Vec<String>)> {
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        OWNER, REPO
    );

    // 构造 reqwest Client（可带代理）
    // Build reqwest Client (optionally with proxy)
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
    let release_url = json["html_url"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let release_notes = json["body"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let published_at = json["published_at"]
        .as_str()
        .unwrap_or("")
        .to_string();

    // asset 命名规则：StripchatRecorder-server-{platform}.zip（不含版本号）
    // 同时区分 server/desktop 两种产物，避免误命中 desktop 安装包。
    //
    // Asset naming: StripchatRecorder-server-{platform}.zip (no version number).
    // The "server" infix distinguishes server from desktop installers.
    // asset 命名规则：StripchatRecorder-server-{platform}.zip（不含版本号）
    // Asset naming: StripchatRecorder-server-{platform}.zip (no version number)
    let platform = current_platform();
    let asset_name = format!("StripchatRecorder-server-{}.zip", platform);

    // 收集所有 asset 名称（用于调试）/ Collect all asset names (for debugging)
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
            assets.iter().find(|a| {
                a["name"].as_str() == Some(&asset_name)
            })
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
/// 逐段比较 major.minor.patch；任一段解析失败时退回字符串不等值判断。
///
/// Semantic version comparison: returns true when `latest` > `current`.
/// Compares major.minor.patch segments; falls back to string inequality on parse failure.
pub fn semver_gt(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut it = v.splitn(3, '.');
        let a = it.next()?.parse::<u64>().ok()?;
        let b = it.next()?.parse::<u64>().ok()?;
        let c = it.next().unwrap_or("0").parse::<u64>().ok()?;
        Some((a, b, c))
    }
    match (parse(latest), parse(current)) {
        (Some((la, lb, lc)), Some((ca, cb, cc))) => {
            (la, lb, lc) > (ca, cb, cc)
        }
        _ => latest != current, // 解析失败时退回字符串比较
    }
}

// ─── 更新安装状态 / Update install state ─────────────────────────────────────

/// 更新下载/安装的进度状态，通过 SSE `update-progress` 事件广播给前端。
/// Update download/install progress state, broadcast to frontend via SSE `update-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateProgress {
    /// 空闲（无进行中的更新）/ Idle (no update in progress)
    Idle,
    /// 下载中 / Downloading
    Downloading {
        /// 已下载字节数 / Bytes downloaded so far
        downloaded: u64,
        /// 总字节数（0 表示未知）/ Total bytes (0 if unknown)
        total: u64,
        /// 百分比 0-100（total 为 0 时为 None）/ Percentage 0-100 (None when total is unknown)
        pct: Option<u8>,
    },
    /// 安装中（解压+替换文件）/ Installing (extracting + replacing files)
    Installing,
    /// 完成，即将重启 / Done, restarting soon
    Done,
    /// 出错 / Error
    Error { message: String },
}

/// 进程内更新状态存储（`Arc<RwLock<UpdateProgress>>`），注入到 `AppState`。
/// In-process update state store, injected into `AppState`.
pub type UpdateStateStore = Arc<RwLock<UpdateProgress>>;

/// 创建初始更新状态存储。
pub fn new_update_state() -> UpdateStateStore {
    Arc::new(RwLock::new(UpdateProgress::Idle))
}

// ─── 下载 + 安装 / Download + Install ────────────────────────────────────────

/// 后台下载 zip 包、解压并替换当前可执行文件，然后重启进程。
///
/// 进度通过 `emitter` 广播 `update-progress` SSE 事件。
/// 所有错误都更新 state 并广播，不 panic。
///
/// Downloads the zip in the background, extracts and replaces the current executable,
/// then restarts the process.
///
/// Progress is broadcast via `emitter` as `update-progress` SSE events.
/// All errors update state and broadcast without panicking.
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

    // ── 1. 流式下载 zip / Stream-download zip ────────────────────────────────
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
    let mut zip_bytes: Vec<u8> = if total > 0 { Vec::with_capacity(total as usize) } else { Vec::new() };

    emit_state!(UpdateProgress::Downloading { downloaded: 0, total, pct: if total > 0 { Some(0) } else { None } });

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                zip_bytes.extend_from_slice(&bytes);
                downloaded += bytes.len() as u64;
                let pct = if total > 0 {
                    downloaded.checked_div(total).map(|r| (r * 100).min(100) as u8)
                } else {
                    None
                };
                emit_state!(UpdateProgress::Downloading { downloaded, total, pct });
            }
            Err(e) => bail!(format!("下载中断: {}", e)),
        }
    }

    // ── 2. 解压 + 替换 / Extract + replace ───────────────────────────────────
    emit_state!(UpdateProgress::Installing);

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => bail!(format!("无法获取当前可执行文件路径: {}", e)),
    };
    let exe_dir = match exe_path.parent() {
        Some(d) => d.to_path_buf(),
        None => bail!("无法获取可执行文件所在目录"),
    };

    // 在临时目录解压 zip，找到新的可执行文件
    // Extract the zip to a temp dir and locate the new executable
    let result = tokio::task::spawn_blocking(move || {
        extract_and_replace(&zip_bytes, &exe_path, &exe_dir)
    }).await;

    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!(e),
        Err(e) => bail!(format!("安装任务崩溃: {}", e)),
    }

    // ── 3. 完成，spawn 新进程后立即退出 / Done, spawn new process then exit immediately ──
    emit_state!(UpdateProgress::Done);
    tracing::info!("更新安装完成，准备启动新版本");

    // 等待 SSE 推送完成
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("无法获取可执行文件路径: {}，请手动重启", e);
            std::process::exit(0);
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();

    tracing::info!("正在启动新版本: {} {:?}", exe.display(), args);

    #[cfg(target_os = "windows")]
    let spawn_result = {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_CONSOLE(0x00000010) + CREATE_NEW_PROCESS_GROUP(0x00000200)
        // 新进程获得独立的新控制台窗口，不继承也不依附于父进程的窗口
        // New process gets its own new console window, independent of the parent's
        std::process::Command::new(&exe)
            .args(&args)
            .creation_flags(0x00000010 | 0x00000200)
            .env("STRIPCHAT_RESTART_DELAY_MS", "2000")
            .spawn()
    };

    #[cfg(not(target_os = "windows"))]
    let spawn_result = std::process::Command::new(&exe)
        .args(&args)
        .env("STRIPCHAT_RESTART_DELAY_MS", "2000")
        .spawn();

    match &spawn_result {
        Ok(child) => tracing::info!("新版本进程已启动 PID={}", child.id()),
        Err(e) => tracing::error!("启动新版本失败: {}，请手动重启", e),
    }

    // 立即退出，释放端口，让新进程能绑定
    // Exit immediately to free the port so the new process can bind it
    std::process::exit(0);
}

/// 解压 zip 包并替换 exe 目录下的所有文件（保留 config/ 目录）。
///
/// zip 内层目录结构：`{package-name}/{files...}`，跳过顶层目录直接提取内容。
/// Windows 上运行中的 exe 无法直接覆盖，先 rename 为 `.old` 再写入新文件。
///
/// Extracts the zip and replaces all files under the exe directory (preserves config/).
///
/// Zip structure: `{package-name}/{files...}` — skips the top-level dir and extracts contents.
/// On Windows, the running exe cannot be overwritten directly; rename it to `.old` first.
fn extract_and_replace(
    zip_bytes: &[u8],
    exe_path: &std::path::Path,
    exe_dir: &std::path::Path,
) -> Result<(), String> {
    use std::io::Read;

    let cursor = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| format!("无法读取 zip: {}", e))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .map_err(|e| format!("读取 zip 条目失败: {}", e))?;

        let raw_path = match file.enclosed_name() {
            Some(p) => p,
            None => continue, // 跳过路径遍历风险条目 / skip unsafe paths
        };

        // 跳过顶层目录（zip 内第一层是包名目录）
        // Strip the top-level directory (first component is the package name dir)
        let mut components = raw_path.components();
        components.next(); // 丢弃顶层目录名 / discard top-level dir name
        let relative: std::path::PathBuf = components.collect();
        if relative.as_os_str().is_empty() {
            continue; // 顶层目录本身，跳过 / the top-level dir entry itself, skip
        }

        let dest = exe_dir.join(&relative);

        if file.is_dir() {
            std::fs::create_dir_all(&dest)
                .map_err(|e| format!("创建目录失败 {}: {}", dest.display(), e))?;
            continue;
        }

        // 确保父目录存在 / ensure parent dir exists
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录失败 {}: {}", parent.display(), e))?;
        }

        // Windows：运行中的 exe 不能直接覆盖，先重命名为 .old
        // Windows: rename the running exe to .old before writing the new one
        if dest == exe_path && dest.exists() {
            let old_path = dest.with_extension("old");
            let _ = std::fs::remove_file(&old_path); // 清理上次遗留的 .old / clean up previous .old
            std::fs::rename(&dest, &old_path)
                .map_err(|e| format!("重命名旧 exe 失败: {}", e))?;
        }

        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| format!("读取 zip 文件内容失败: {}", e))?;

        std::fs::write(&dest, &buf)
            .map_err(|e| format!("写入文件失败 {}: {}", dest.display(), e))?;

        // Linux/macOS：为可执行文件设置执行权限
        // Linux/macOS: set executable permission for binary files
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&dest) {
                // 有执行位需求：只对 exe 目录根级文件（而不是 config 下的 json）设置
                // Only set +x on root-level files (not config/*.json etc.)
                if relative.components().count() == 1 {
                    let mut perms = meta.permissions();
                    perms.set_mode(perms.mode() | 0o111);
                    let _ = std::fs::set_permissions(&dest, perms);
                }
            }
        }

        tracing::debug!("已解压: {}", dest.display());
    }

    tracing::info!("zip 解压完成，所有文件已替换");
    Ok(())
}
