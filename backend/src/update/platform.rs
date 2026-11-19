//! 运行环境识别：编译目标平台字符串 + Docker 容器检测。
//! Runtime environment detection: compile-target platform string + Docker detection.

/// 返回当前编译目标的平台字符串，与 GitHub Release asset 命名
/// （`StripchatRecorder-server-{platform}-*.zip`）对应。
/// Returns the current compile-target platform string, matching the GitHub
/// Release asset naming (`StripchatRecorder-server-{platform}-*.zip`).
pub fn current_platform() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "windows-x86_64";
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "windows-arm64";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "linux-x86_64";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "linux-arm64";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "macos-x86_64";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "macos-arm64";
    #[allow(unreachable_patterns)]
    #[allow(unreachable_code)]
    "unknown"
}

/// 检测当前运行环境是否为 Docker 容器。
/// Detect whether the current process runs inside a Docker container.
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
