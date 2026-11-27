//! 字体查找与 ffmpeg drawtext 路径转义 / Font Discovery and ffmpeg drawtext Path Escaping

use std::path::{Path, PathBuf};
use std::process::Command;

/// 在系统中查找可用的字体文件，三级回退：
///   1. `fc-match`（fontconfig）——Linux/macOS 自带，Windows 构建链也
///      提供 fontconfig；能找到任意已安装字体。
///   2. 已知绝对路径候选（Windows/macOS/Linux 常见字体，含 CJK）。
///   3. 递归扫描字体目录，挑第一个可用 .ttf/.ttc/.otf。
///
/// 返回已转义为 ffmpeg drawtext 格式的路径字符串，未找到返回 `None`。
///
/// Find a usable font file with a three-level fallback:
///   1. `fc-match` (fontconfig) — present on most Linux/macOS systems
///      and in the bundled toolchain; resolves any installed font.
///   2. Known absolute-path candidates (Windows/macOS/Linux incl. CJK).
///   3. Recursive scan of font directories for any .ttf/.ttc/.otf.
///
/// Returns the path escaped for ffmpeg drawtext, or `None`.
pub fn find_font() -> Option<String> {
    find_via_fontconfig()
        .or_else(find_in_candidates)
        .or_else(find_by_scan)
        .map(|p| escape_for_drawtext(&p))
}

/// 经 `fc-match` 查询字体——fontconfig 返回匹配字体的文件路径。
/// Query a font via `fc-match` — fontconfig prints the matched file.
fn find_via_fontconfig() -> Option<String> {
    for pat in ["sans", "serif"] {
        let out = Command::new("fc-match")
            .args(["-f", "%{file}", pat])
            .output()
            .ok()?;
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Windows 字体目录——优先环境变量（SystemRoot/WINDIR），回退 C:\Windows。
/// The Windows fonts dir — env first, `C:\Windows` fallback.
#[cfg(target_os = "windows")]
fn windows_fonts_dir() -> PathBuf {
    for key in ["SystemRoot", "WINDIR"] {
        if let Ok(root) = std::env::var(key) {
            let d = PathBuf::from(root).join("Fonts");
            if d.is_dir() {
                return d;
            }
        }
    }
    PathBuf::from(r"C:\Windows\Fonts")
}

/// 已知字体文件名候选（按常见度排序，含中文系统字体）。
/// Known font file candidates (most common first, incl. CJK faces).
const FONT_NAMES: &[&str] = &[
    // Windows — Latin faces / 西文
    "arial.ttf",
    "segoeui.ttf",
    "calibri.ttf",
    "tahoma.ttf",
    "verdana.ttf",
    "consola.ttf",
    // Windows — CJK（精简/中文版 Windows 可能缺西文字体）
    "msyh.ttc",
    "simhei.ttf",
    "simsun.ttc",
    "msjh.ttc",
    // macOS
    "Helvetica.ttc",
    "Arial.ttf",
    "PingFang.ttc",
    "STHeiti Light.ttc",
    // Linux — Debian/Ubuntu/Noto/文泉驿
    "DejaVuSans.ttf",
    "LiberationSans-Regular.ttf",
    "NotoSans-Regular.ttf",
    "NotoSansCJK-Regular.ttc",
    "wqy-microhei.ttc",
    "wqy-zenhei.ttc",
    "Ubuntu-R.ttf",
    "DroidSansFallbackFull.ttf",
    "FreeSans.ttf",
];

/// 候选目录——逐文件名探测。/ Candidate dirs probed per file name.
fn font_dirs() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        vec![windows_fonts_dir()]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/System/Library/Fonts"),
            PathBuf::from("/Library/Fonts"),
            dirs_home_fonts(),
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
            PathBuf::from("/usr/share/fonts/truetype/dejavu"),
            PathBuf::from("/usr/share/fonts/truetype/liberation"),
            PathBuf::from("/usr/share/fonts/opentype/noto"),
            PathBuf::from("/usr/share/fonts/truetype/wqy"),
            dirs_home_fonts(),
        ]
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        vec![]
    }
}

/// `~/.local/share/fonts`（用户级字体目录，类 Unix 系统）。
/// Per-user font dir on Unix-likes.
#[allow(dead_code)] // 仅 macOS/Linux 分支引用 / only used by unix cfg arms
fn dirs_home_fonts() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/fonts"))
        .unwrap_or_default()
}

/// 在候选目录中按已知文件名逐一查找。
/// Probe known file names inside the candidate dirs.
fn find_in_candidates() -> Option<String> {
    for dir in font_dirs() {
        if !dir.is_dir() {
            continue;
        }
        for name in FONT_NAMES {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }
    // Windows 也直接在 %WINDIR%\Fonts 下按名探测（含未枚举到的目录）/
    // Windows: also probe the raw dir for names not yet enumerated.
    #[cfg(target_os = "windows")]
    {
        let dir = windows_fonts_dir();
        for name in FONT_NAMES {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 深度受限地递归扫描字体目录，返回第一个可用字体文件
/// （兜底——哪怕不是理想字体，也比跳过时间戳覆盖层强）。
/// Depth-limited recursive scan returning the first usable font file
/// (last resort — any font beats skipping the timestamp overlay).
fn find_by_scan() -> Option<String> {
    for dir in font_dirs() {
        if let Some(p) = scan_dir(&dir, 4) {
            return Some(p);
        }
    }
    None
}

/// 递归扫描：优先命中文件名含常见 sans 关键字的字体，否则收第一个字体。
/// Recursive scan preferring well-known sans names, else the first font.
fn scan_dir(dir: &Path, depth: u32) -> Option<String> {
    if depth == 0 || !dir.is_dir() {
        return None;
    }
    const PREFER: &[&str] = &[
        "dejavu", "notosans", "noto", "liberation", "freesans", "arial",
        "helvetica", "msyh", "simhei", "wqy", "droid", "ubuntu",
    ];
    let mut first: Option<String> = None;
    let mut entries: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        entries.push(e.path());
    }
    // 先查文件，再下钻子目录 / files first, then descend.
    for p in &entries {
        if !p.is_file() {
            continue;
        }
        let ext_ok = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "ttf" | "ttc" | "otf"))
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        let lower = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if PREFER.iter().any(|k| lower.contains(k)) {
            return Some(p.to_string_lossy().to_string());
        }
        if first.is_none() {
            first = Some(p.to_string_lossy().to_string());
        }
    }
    for p in &entries {
        if p.is_dir()
            && let Some(hit) = scan_dir(p, depth - 1)
        {
            return Some(hit);
        }
    }
    first
}

/// 将文件路径转换为 ffmpeg drawtext 过滤器可接受的格式。
/// Windows 驱动器路径的反斜杠改为正斜杠，冒号转义为 `\:`。
///
/// Convert a file path to a format accepted by ffmpeg's drawtext filter.
/// On Windows: backslashes → forward slashes, drive-letter colon → `\:`.
pub fn escape_for_drawtext(path: &str) -> String {
    let fwd = path.replace('\\', "/");
    if fwd.len() >= 2 && fwd.as_bytes()[1] == b':' {
        format!("{}\\:{}", &fwd[..1], &fwd[2..])
    } else {
        fwd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive-letter escape for drawtext. / drawtext 盘符转义。
    #[test]
    fn escape_windows_path() {
        assert_eq!(escape_for_drawtext(r"C:\Fonts\a.ttf"), "C\\:/Fonts/a.ttf");
        assert_eq!(escape_for_drawtext("/usr/x/a.ttf"), "/usr/x/a.ttf");
    }

    /// The fallback chain must resolve a real font on any CI/dev box.
    /// 回退链在开发/CI 机上必须能解析出真实字体。
    #[test]
    fn finds_some_font() {
        let found = find_font().expect("a font should resolve on this system");
        // Escaped drive colon: un-escape to check existence.
        // 反转义盘符冒号后验证文件存在。
        let raw = found.replace("\\:", ":");
        assert!(Path::new(&raw).exists(), "font path should exist: {raw}");
    }
}
