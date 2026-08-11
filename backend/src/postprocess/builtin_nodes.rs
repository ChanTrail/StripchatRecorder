//! 内置节点描述与执行 / Built-in Node Descriptions and Execution
//!
//! 内置节点不对应任何外部可执行文件，由后端直接处理其逻辑。
//! 它们通过 `discover_modules` 注入到可用模块列表，让前端可以像普通模块一样放置它们。
//!
//! Built-in nodes have no corresponding external executable; their logic is handled
//! directly by the backend. They are injected into the module list by `discover_modules`
//! so the frontend can place them like any regular module.
//!
//! ## 当前内置节点 / Current built-in nodes
//!
//! ### `recording_input`
//! 虚拟录制输入节点。在前端画布上始终存在，不可删除。
//! 输出 `ts_session_dir`（port 0），连接到 `ts_merge` 或其他接受目录输入的节点。
//!
//! Virtual recording input node. Always present on the frontend canvas and cannot be removed.
//! Outputs `ts_session_dir` (port 0), wired to `ts_merge` or any node accepting a directory input.
//!
//! ### `unpack`
//! MediaBundle 解组节点。接收 `media_bundle` 输入（port 0），
//! 将其拆分为 `video_file`（port 0）和 `image_file`（port 1）两个独立输出端口。
//! 若 bundle 中没有图片路径，port 1 输出为空，后续节点不会被触发。
//!
//! MediaBundle unpack node. Accepts `media_bundle` input (port 0),
//! splits it into `video_file` (port 0) and `image_file` (port 1) output ports.
//! If the bundle contains no image path, port 1 is not emitted and downstream nodes won't run.

use super::pipeline::{ModuleInfo, NodeResult, PortType};
use crate::recording::meta::PpExecCode;
use std::path::PathBuf;

/// 内置节点的模块 ID 前缀，用于运行时识别。
/// Module ID prefix for built-in nodes, used for runtime identification.
pub const BUILTIN_PREFIX: &str = "__builtin__";

/// `recording_input` 内置节点 ID。
pub const ID_RECORDING_INPUT: &str = "__builtin__recording_input";

/// `unpack` 内置节点 ID。
pub const ID_UNPACK: &str = "__builtin__unpack";

/// MediaBundle 字段分隔符：视频路径 + `\n` + 图片路径。
/// MediaBundle field separator: video path + `\n` + image path.
pub const BUNDLE_SEP: char = '\n';

/// 将视频路径和图片路径打包为 MediaBundle 路径字符串。
/// Pack video and image paths into a MediaBundle path string.
pub fn pack_bundle(video: &std::path::Path, image: &std::path::Path) -> PathBuf {
    PathBuf::from(format!(
        "{}{}{}",
        video.to_string_lossy(),
        BUNDLE_SEP,
        image.to_string_lossy()
    ))
}

/// 从 MediaBundle 路径字符串中解包视频路径和可选的图片路径。
/// Unpack a MediaBundle path string into the video path and an optional image path.
pub fn unpack_bundle(bundle: &std::path::Path) -> (PathBuf, Option<PathBuf>) {
    let s = bundle.to_string_lossy();
    if let Some(sep) = s.find(BUNDLE_SEP) {
        let video = PathBuf::from(&s[..sep]);
        let image_str = s[sep + BUNDLE_SEP.len_utf8()..].trim();
        let image = if image_str.is_empty() {
            None
        } else {
            Some(PathBuf::from(image_str))
        };
        (video, image)
    } else {
        // 没有分隔符，整体视为视频路径 / No separator — treat entire string as video path
        (bundle.to_path_buf(), None)
    }
}

/// 返回所有内置节点的 `ModuleInfo` 描述，注入到 `discover_modules` 返回列表。
/// name 和 description 从合并的 `__builtin__` locale 文件读取，回退到内嵌的英文默认值。
///
/// Return `ModuleInfo` for all built-in nodes, injected into the `discover_modules` list.
/// name and description are read from the merged `__builtin__` locale file,
/// falling back to embedded English defaults.
pub fn builtin_module_infos() -> Vec<ModuleInfo> {
    // 读取当前语言设置 / Read current language setting
    let locale_code = crate::config::settings::AppState::new()
        .ok()
        .map(|s| s.get_settings().language)
        .unwrap_or_else(|| "en-US".to_string());

    // 从合并的 __builtin__ locale 文件中取指定节点 key 的翻译
    // Read name/description for a node key from the merged __builtin__ locale file
    let tr = |node_key: &str, name_default: &str, desc_default: &str| -> (String, String) {
        crate::locale::manager::read_module_locale("__builtin__", &locale_code)
            .and_then(|v| v.get(node_key).cloned())
            .map(|entry| {
                let name = entry.get("name").and_then(|n| n.as_str()).unwrap_or(name_default).to_string();
                let desc = entry.get("description").and_then(|d| d.as_str()).unwrap_or(desc_default).to_string();
                (name, desc)
            })
            .unwrap_or_else(|| (name_default.to_string(), desc_default.to_string()))
    };

    let (ri_name, ri_desc) = tr(
        "recording_input",
        "Recording Input",
        "Virtual recording input node — always the pipeline start point",
    );
    let (unpack_name, unpack_desc) = tr(
        "unpack",
        "Unpack Media Bundle",
        "Split a media bundle into a video file (port 0) and an image file (port 1)",
    );

    // 内置节点没有独立的 Cargo.toml，也不随 backend 版本单独展示版本号——
    // 它们与 backend 是同一次编译产物，展示版本号对用户没有实际意义（无法单独升级），
    // 留空即可，前端在 version 为空字符串时不渲染版本号徽章。
    //
    // Built-in nodes have no separate Cargo.toml and don't display a version on their
    // own — they're compiled as part of the backend itself, so showing a version number
    // would be meaningless to the user (they can't be upgraded independently). Left empty;
    // the frontend skips rendering the version badge when it's an empty string.
    vec![
        ModuleInfo {
            id: ID_RECORDING_INPUT.to_string(),
            name: ri_name,
            version: String::new(),
            description: ri_desc,
            input_types: vec![],
            output_types: vec![PortType::TsSessionDir],
            params: vec![],
            i18n: None,
            official: false,
            reusable: false,
            exe_path: PathBuf::new(),
        },
        ModuleInfo {
            id: ID_UNPACK.to_string(),
            name: unpack_name,
            version: String::new(),
            description: unpack_desc,
            input_types: vec![PortType::MediaBundle],
            output_types: vec![PortType::VideoFile, PortType::ImageFile],
            params: vec![],
            i18n: None,
            official: true,
            reusable: true,
            exe_path: PathBuf::new(),
        },
    ]
}

/// 执行内置 `unpack` 节点：将 MediaBundle 拆分为视频和图片两个输出端口。
///
/// Execute the built-in `unpack` node: splits a MediaBundle into video and image output ports.
///
/// - port 0 (VideoFile): 视频路径，始终存在 / Video path, always present
/// - port 1 (ImageFile): 图片路径，bundle 中无图片时不输出 / Image path, absent when bundle has no image
pub fn run_unpack(effective_id: &str, inputs: &[PathBuf]) -> NodeResult {
    let bundle = match inputs.first() {
        Some(p) => p,
        None => {
            return NodeResult {
                effective_id: effective_id.to_string(),
                module_id: ID_UNPACK.to_string(),
                code: PpExecCode::Error,
                message: "unpack: no input provided".to_string(),
                outputs: vec![],
                inputs: inputs.to_vec(),
            };
        }
    };

    let (video, image) = unpack_bundle(bundle);

    if !video.exists() {
        return NodeResult {
            effective_id: effective_id.to_string(),
            module_id: ID_UNPACK.to_string(),
            code: PpExecCode::Error,
            message: format!("unpack: video path not found: {}", video.display()),
            outputs: vec![],
            inputs: inputs.to_vec(),
        };
    }

    let mut outputs = vec![video];
    if let Some(img) = image {
        if img.exists() {
            outputs.push(img);
        } else {
            tracing::warn!("unpack: image path not found (skipping port 1): {}", img.display());
        }
    }

    NodeResult {
        effective_id: effective_id.to_string(),
        module_id: ID_UNPACK.to_string(),
        code: PpExecCode::Ok,
        message: format!("unpacked {} output(s)", outputs.len()),
        outputs,
        inputs: inputs.to_vec(),
    }
}
