//! 字体查找与 ffmpeg drawtext 路径转义 / Font Discovery and ffmpeg drawtext Path Escaping

use std::path::Path;

/// 在常见系统路径中查找可用的字体文件（Windows / macOS / Linux）。
/// 返回已转义为 ffmpeg drawtext 格式的路径字符串，未找到返回 `None`。
///
/// Find an available font file in common system paths (Windows / macOS / Linux).
/// Returns the path escaped for ffmpeg drawtext, or `None` if not found.
pub fn find_font() -> Option<String> {
    let candidates: &[&str] = &[
        // Windows
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\consola.ttf",
        // macOS
        "/System/Library/Fonts/Helvetica.ttc",
        "/Library/Fonts/Arial.ttf",
        // Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    candidates
        .iter()
        .find(|p| Path::new(p).exists())
        .map(|p| escape_for_drawtext(p))
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
