//! 社区模块安装与卸载 / Community module install and uninstall
//!
//! 拉取逻辑已移至前端（`stores/community.ts`），后端只负责文件操作：
//! - `install_module`：下载指定 URL 的可执行文件，校验 sha256，写入 `modules/` 目录
//! - `uninstall_module`：从 `modules/` 目录删除对应可执行文件
//!
//! Fetch logic has been moved to the frontend (`stores/community.ts`).
//! The backend only handles file operations:
//! - `install_module`: download the binary, verify sha256, write to `modules/`
//! - `uninstall_module`: remove the corresponding executable from `modules/`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── 数据模型 / Data Model ────────────────────────────────────────────────────

/// 模块维护者仓库中的 registry.json 结构（由前端拉取后传入后端）。
/// Structure of registry.json in the module maintainer's repo
/// (fetched by the frontend and passed to the backend for installation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryModule {
    /// 模块唯一 ID，与 `--describe` 输出的 `id` 字段一致 / Module unique ID
    pub id: String,
    /// 显示名称 / Display name
    pub name: String,
    /// 功能简介 / Brief description
    pub description: String,
    /// 作者 / Author
    #[serde(default)]
    pub author: String,
    /// 标签 / Tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// 许可证（如 "MIT"、"Apache-2.0"）/ License (e.g. "MIT", "Apache-2.0")
    #[serde(default)]
    pub license: String,
    /// 最新版本号 / Latest version
    pub latest_version: String,
    /// 各平台下载 URL / Per-platform download URLs
    pub downloads: HashMap<String, String>,
    /// 各平台 sha256 / Per-platform sha256 checksums
    #[serde(default)]
    pub sha256: HashMap<String, String>,
    /// 模块维护者仓库 URL / Module maintainer's repo URL
    #[serde(default)]
    pub repo: String,
}

// ─── 平台标识符 / Platform Identifier ────────────────────────────────────────

/// 返回当前运行平台的标识符，用于从 downloads/sha256 中选取对应条目。
/// Returns the current platform identifier for selecting the correct download entry.
#[allow(unreachable_code)]
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
    "unknown"
}

/// 将镜像站 URL 应用到给定的 URL 上。
/// 如果 mirror 不为空，则将 mirror 作为前缀拼接到原始 URL：
///   mirror="https://ghproxy.com/"，url="https://github.com/..."
///   → "https://ghproxy.com/https://github.com/..."
///
/// Apply the mirror URL to a given URL by prepending the mirror.
pub fn apply_mirror(url: &str, mirror: Option<&str>) -> String {
    match mirror.filter(|m| !m.is_empty()) {
        Some(m) => format!("{}/{}", m.trim_end_matches('/'), url),
        None => url.to_string(),
    }
}

// ─── 安装 / Install ───────────────────────────────────────────────────────────

/// 安装指定模块。
///
/// 前端已完成模块发现并将完整的 `RegistryModule` 传入，后端只需：
/// 1. 从 `downloads` 中取当前平台的下载 URL
/// 2. 流式下载 → 每块调用 `on_progress(downloaded, total)` → sha256 校验
/// 3. 写临时文件 → 删旧版本 → 重命名为最终文件名
/// 4. Linux/macOS 上设置可执行权限
///
/// Install the specified module.
///
/// The frontend has already resolved the module and passes the complete `RegistryModule`.
/// The backend:
/// 1. Looks up the current platform's download URL from `downloads`
/// 2. Streams the download, calling `on_progress(downloaded, total)` per chunk
/// 3. Writes temp file → removes old versions → renames to final name
/// 4. Sets executable permission on Linux/macOS
pub async fn install_module(
    module: &RegistryModule,
    proxy_url: Option<String>,
    mirror_url: Option<String>,
    on_progress: impl Fn(u64, u64) + Send + Sync,
) -> crate::core::error::Result<()> {
    use sha2::{Digest, Sha256};

    let platform = current_platform();
    let download_url = module.downloads.get(platform).ok_or_else(|| {
        crate::core::error::AppError::Other(format!(
            "模块 '{}' 不支持当前平台 ({})",
            module.id, platform
        ))
    })?;

    let expected_sha256 = module.sha256.get(platform).cloned();

    let modules_dir = crate::postprocess::pipeline::modules_dir();
    std::fs::create_dir_all(&modules_dir).map_err(|e| {
        crate::core::error::AppError::Other(format!("创建 modules 目录失败: {}", e))
    })?;

    #[cfg(target_os = "windows")]
    let file_ext = ".exe";
    #[cfg(not(target_os = "windows"))]
    let file_ext = "";

    let tmp_path = modules_dir.join(format!("{}.tmp", module.id));
    // 命名格式：{id}-{platform}-{version}{ext}
    let final_name = format!("{}-{}-{}{}", module.id, platform, module.latest_version, file_ext);
    let final_path = modules_dir.join(&final_name);

    tracing::info!("正在下载模块 {} v{} ...", module.id, module.latest_version);
    let effective_url = apply_mirror(download_url, mirror_url.as_deref());
    let data = download_file_with_progress(&effective_url, proxy_url, on_progress).await?;

    if let Some(expected) = expected_sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let actual = hasher
            .finalize()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        if actual.to_lowercase() != expected.to_lowercase() {
            return Err(crate::core::error::AppError::Other(format!(
                "模块 '{}' sha256 校验失败：期望 {}，实际 {}",
                module.id, expected, actual
            )));
        }
        tracing::info!("模块 {} sha256 校验通过", module.id);
    } else {
        tracing::warn!("模块 {} 没有提供 sha256 校验值，跳过校验", module.id);
    }

    std::fs::write(&tmp_path, &data)
        .map_err(|e| crate::core::error::AppError::Other(format!("写入临时文件失败: {}", e)))?;

    remove_module_files(&module.id, &modules_dir);

    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        crate::core::error::AppError::Other(format!("重命名模块文件失败: {}", e))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&final_path)
            .map_err(|e| crate::core::error::AppError::Other(e.to_string()))?
            .permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(&final_path, perms)
            .map_err(|e| crate::core::error::AppError::Other(e.to_string()))?;
    }

    tracing::info!(
        "模块 {} v{} 安装成功：{}",
        module.id,
        module.latest_version,
        final_path.display()
    );
    Ok(())
}

// ─── 卸载 / Uninstall ─────────────────────────────────────────────────────────

/// 卸载指定模块（删除 `modules/` 目录中所有匹配该模块 ID 的可执行文件）。
/// 幂等操作：若没有找到任何匹配文件，静默成功。
///
/// Uninstall the specified module (delete all matching executables in `modules/`).
/// Idempotent: silently succeeds if no matching files are found.
pub fn uninstall_module(module_id: &str) -> crate::core::error::Result<()> {
    let modules_dir = crate::postprocess::pipeline::modules_dir();
    remove_module_files(module_id, &modules_dir);
    tracing::info!("模块 {} 已卸载", module_id);
    Ok(())
}

// ─── 内部文件操作 / Internal File Operations ─────────────────────────────────

fn remove_module_files(module_id: &str, modules_dir: &std::path::Path) {
    let entries = match std::fs::read_dir(modules_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_module_file(module_id, &path) {
            tracing::debug!("删除旧模块文件: {}", path.display());
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn is_module_file(module_id: &str, path: &std::path::Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    #[cfg(target_os = "windows")]
    {
        (name == format!("{}.exe", module_id))
            || (name.starts_with(&format!("{}-", module_id)) && name.ends_with(".exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        (name == module_id) || name.starts_with(&format!("{}-", module_id))
    }
}

// ─── HTTP 下载工具 / HTTP Download Utility ────────────────────────────────────

/// 流式下载文件，每收到一个数据块就调用 `on_progress(downloaded_bytes, total_bytes)`。
/// `total_bytes` 为 0 时表示服务器未返回 Content-Length（不可知总大小）。
///
/// Stream-download a file, calling `on_progress(downloaded, total)` for each chunk.
/// `total` is 0 when the server doesn't return Content-Length (unknown total size).
async fn download_file_with_progress(
    url: &str,
    proxy_url: Option<String>,
    on_progress: impl Fn(u64, u64),
) -> crate::core::error::Result<Vec<u8>> {
    use futures_util::StreamExt;

    let mut builder = reqwest::Client::builder()
        // 连接建立超时（TCP + TLS 握手）/ Connection establishment timeout (TCP + TLS)
        .connect_timeout(std::time::Duration::from_secs(30))
        // 单次 chunk 读取超时：超过此时间没有新数据则中断 / Per-chunk read timeout: abort if no new data arrives
        .read_timeout(std::time::Duration::from_secs(60))
        // 整体请求超时（保底）/ Overall request timeout (safety net)
        .timeout(std::time::Duration::from_secs(600));
    if let Some(proxy) = proxy_url.as_deref().filter(|s| !s.is_empty()) {
        match reqwest::Proxy::all(proxy) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => tracing::warn!("下载代理地址无效 {}: {}", proxy, e),
        }
    }
    let client = builder
        .build()
        .map_err(|e| crate::core::error::AppError::Other(e.to_string()))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| crate::core::error::AppError::Other(format!("下载失败: {}", e)))?;

    if !resp.status().is_success() {
        return Err(crate::core::error::AppError::Other(format!(
            "下载请求失败，HTTP {} | {}",
            resp.status(), url
        )));
    }

    // Content-Length 用于计算百分比，若无则 total = 0
    // Content-Length for percentage calculation; 0 if absent
    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut buf: Vec<u8> = if total > 0 { Vec::with_capacity(total as usize) } else { Vec::new() };

    on_progress(0, total);

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            crate::core::error::AppError::Other(format!("读取响应体失败: {} | {}", e, url))
        })?;
        buf.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }

    Ok(buf)
}
