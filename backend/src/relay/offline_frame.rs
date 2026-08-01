//! 离线占位画面文字渲染 / Offline Placeholder Frame Text Rendering
//!
//! 为离线状态下的黑屏占位画面生成 ffmpeg `drawtext` 滤镜表达式，包括跨平台 CJK
//! 字体探测、状态文字转义，以及无 CJK 字体时的英文回退。不涉及 ffmpeg 进程的
//! 启动或转发逻辑（见 `super::streamer`）。
//!
//! Generates ffmpeg `drawtext` filter expressions for the offline black-screen
//! placeholder frame, including cross-platform CJK font detection, status text
//! escaping, and an ASCII English fallback when no CJK font is available. Does not
//! handle spawning the ffmpeg process or relay/feed logic (see `super::streamer`).

/// 探测系统中可用的 CJK 字体路径（按平台常见安装路径依次尝试）。
/// Detect an available CJK font path on the system (tries common per-platform install paths).
pub fn find_cjk_font() -> Option<String> {
    let candidates: &[&str] = &[
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyhbd.ttc",
        "C:/Windows/Fonts/simsun.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/STZHONGS.TTF",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];
    for path in candidates {
        if std::path::Path::new(path).exists() {
            return Some(path.to_string());
        }
    }
    None
}

/// 将中文状态文字转换为英文，供无 CJK 字体环境下的回退显示使用。
/// Convert a Chinese status label to English, used as a fallback when no CJK font is available.
pub fn to_ascii_status(s: &str) -> String {
    match s {
        "公开秀" => "Public Show".to_string(),
        "私密秀" => "Private Show".to_string(),
        "票务秀" => "Ticket Show".to_string(),
        "计时秀" => "Per-Minute Show".to_string(),
        "群组秀" => "Group Show".to_string(),
        "虚拟私密" => "Virtual Private".to_string(),
        "P2P" => "P2P".to_string(),
        "等待" => "Waiting".to_string(),
        "离线" => "Offline".to_string(),
        "获取状态失败" => "Status Unavailable".to_string(),
        _ => s.to_string(),
    }
}

/// 转义字符串中的 ffmpeg drawtext 滤镜特殊字符。
/// Escape characters with special meaning in ffmpeg's drawtext filter syntax.
fn escape_drawtext(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('\'', "\\'")
     .replace(':', "\\:")
     .replace('[', "\\[")
     .replace(']', "\\]")
}

/// 构建离线占位画面的 ffmpeg drawtext 滤镜表达式（用户名 + 状态文字两行）。
/// 若系统存在 CJK 字体则直接渲染中文；否则回退为纯 ASCII 英文文字（不指定字体）。
///
/// Build the ffmpeg drawtext filter expression for the offline placeholder frame
/// (username + status text, two lines). Renders Chinese directly if a CJK font is
/// found; otherwise falls back to plain ASCII English text (no fontfile specified).
pub fn build_drawtext(username: &str, status_text: &str) -> String {
    let username_esc = escape_drawtext(username);
    match find_cjk_font() {
        Some(font_path) => {
            let font_esc = font_path.replace('\\', "/").replace(':', "\\:");
            let status_esc = escape_drawtext(status_text);
            format!(
                "drawtext=fontfile='{}':text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=fontfile='{}':text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                font_esc, username_esc, font_esc, status_esc
            )
        }
        None => {
            let status_ascii = escape_drawtext(&to_ascii_status(status_text));
            format!(
                "drawtext=text='{}':fontcolor=white:fontsize=36:x=(w-text_w)/2:y=(h-text_h)/2-40,\
                 drawtext=text='{}':fontcolor=gray:fontsize=24:x=(w-text_w)/2:y=(h-text_h)/2+20",
                username_esc, status_ascii,
            )
        }
    }
}
