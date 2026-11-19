//! FFmpeg 位置首选项与程序解析 / FFmpeg Location Preference and Program Resolution
//!
//! - [`FFMPEG_PREFERENCE`]：`ffmpeg_preference` 设置的进程内镜像，由 `AppState`
//!   在加载与每次保存设置时同步；解析逻辑逐次读取，保证改完首选项后下一次
//!   调用立即生效
//! - [`preference`]：镜像之上的**生效值**——所选位置组件不齐时回落为 `None`
//!   （保持 tools-存在优先、否则 PATH 的旧行为），组件齐全后的下一次调用
//!   自动切换
//! - [`ffmpeg_program`] / [`ffprobe_program`] / [`ffplay_program`]：按生效
//!   首选项解析——`0` 只走系统 PATH，`1` 只用 `<exe_dir>/tools/` 下的副本，
//!   未设置保持旧行为
//! - [`path_env_with_tools`]：把 tools 目录前置到 PATH 的副本，注入模块子
//!   进程；首选项为系统环境变量时不注入
//! - [`ffmpeg_available`]：ffmpeg 与 ffprobe 均可运行 `-version` 才视为可用
//!
//! - [`FFMPEG_PREFERENCE`]: in-process mirror of the `ffmpeg_preference`
//!   setting, synced by `AppState` on load and on every save; resolution reads
//!   it per call so a changed preference takes effect on the next invocation
//! - [`preference`]: the **effective** value on top of the mirror — an
//!   incomplete chosen location falls back to `None` (legacy
//!   tools-when-present-else-PATH behavior) and the next call once complete
//!   switches automatically
//! - [`ffmpeg_program`] / [`ffprobe_program`] / [`ffplay_program`]: resolve per
//!   the effective preference — `0` only the system PATH, `1` only the copy
//!   under `<exe_dir>/tools/`, unset keeps legacy behavior
//! - [`path_env_with_tools`]: a PATH copy with the tools dir prepended,
//!   injected into module child processes; not injected when the preference
//!   is the system PATH
//! - [`ffmpeg_available`]: both ffmpeg and ffprobe must run `-version`
//!   successfully

use super::paths::{exe_name, tools_dir};
use super::probe::find_on_path;
use crate::core::no_window::NoWindowExt;
use parking_lot::RwLock;
use std::ffi::OsString;
use std::process::{Command, Stdio};

/// `ffmpeg_preference` 设置的进程内镜像：由 `AppState::new`（加载时）与
/// `AppState::update_settings`（每次保存时）同步；[`program_by_preference`]
/// 逐次读取，保证"用户改完设置，下一次调用 ffmpeg 立即用新选择"。
/// 取值：`Some(0)` = 系统环境变量，`Some(1)` = tools 文件夹，`None` = 未设置。
///
/// In-process mirror of the `ffmpeg_preference` setting: synced by
/// `AppState::new` (on load) and `AppState::update_settings` (on every save);
/// [`program_by_preference`] reads it per call, so a changed preference takes
/// effect on the very next ffmpeg invocation.
/// Values: `Some(0)` = system PATH, `Some(1)` = tools folder, `None` = unset.
static FFMPEG_PREFERENCE: RwLock<Option<u8>> = RwLock::new(None);

/// 同步 FFmpeg 位置首选项镜像（由 `AppState` 在加载/保存设置时调用）。
/// Sync the FFmpeg location preference mirror (called by `AppState` when
/// settings load or are saved).
pub fn set_preference(preference: Option<u8>) {
    *FFMPEG_PREFERENCE.write() = preference;
}

/// 镜像中的原始选择（无存在性门控）：仅用于"用户是否选择了 PATH"这类
/// 策略判定——选了 PATH 即使 PATH 缺组件也不应自动下载 tools 兜底；
/// 程序路径解析请用 [`preference`]。
///
/// The raw mirrored choice without the existence gate: only for policy
/// checks like "did the user pick PATH" — a PATH choice must never trigger a
/// tools auto-download even while PATH lacks components; use [`preference`]
/// for program resolution.
pub(crate) fn raw_preference() -> Option<u8> {
    *FFMPEG_PREFERENCE.read()
}

/// 判断指定位置的 ffmpeg / ffprobe / ffplay 三组件是否齐全：
/// `0` = 系统 PATH（[`find_on_path`] 能找到全部三个），`1` = tools 目录
/// （三个可执行文件都存在）。
///
/// Check whether the given location has all three ffmpeg components:
/// `0` = system PATH (all three found via [`find_on_path`]), `1` = the tools
/// directory (all three executables present).
pub(crate) fn location_complete(location: u8) -> bool {
    const BINS: [&str; 3] = ["ffmpeg", "ffprobe", "ffplay"];
    match location {
        0 => BINS.iter().all(|b| find_on_path(b).is_some()),
        1 => {
            let tools = tools_dir();
            BINS.iter().all(|b| tools.join(exe_name(b)).is_file())
        }
        _ => false,
    }
}

/// 当前 FFmpeg 位置首选项的**生效值**（`0` = 系统 PATH，`1` = tools，
/// `None` = 未设置/组件不齐按未设置处理）。镜像里存的是用户的持久化选择，
/// 这里再加一层存在性判断：所选位置组件不齐时回落为 `None`（保持
/// tools-存在优先、否则 PATH 的旧行为），等组件齐全后的下一次调用自动切换——
/// 例如 tools 下载进行中，期间仍按旧逻辑解析，不会切到尚不完整的 tools。
///
/// The **effective** FFmpeg location preference (`0` = system PATH, `1` =
/// tools, `None` = unset/incomplete). The mirror stores the user's persisted
/// choice; on top of it we apply an existence check: an incomplete chosen
/// location falls back to `None` (legacy tools-first-then-PATH behavior) and
/// the next call after the components become complete switches automatically —
/// e.g. during a tools download the old resolution keeps working instead of
/// jumping to the not-yet-complete tools copy.
pub fn preference() -> Option<u8> {
    match *FFMPEG_PREFERENCE.read() {
        Some(loc) if !location_complete(loc) => None,
        p => p,
    }
}

/// 解析要使用的程序路径，按 [`preference`] 决定来源：
/// - `0`（系统环境变量）：返回裸命令名，由操作系统按 PATH 解析
/// - `1`（tools 文件夹）：返回 `tools_dir()` 下的绝对路径（即使文件尚不存在，
///   这样启动失败时的报错会指向用户选择的预期位置）
/// - 未设置（向导尚未选择）：保持旧行为——tools 中存在则用其绝对路径，
///   否则返回裸命令名
///
/// 每次调用都读取首选项镜像并做文件存在性检查，设置或下载完成后立即生效，
/// 无需重启进程。
///
/// Resolve the program to use, honoring [`preference`]:
/// - `0` (system PATH): return the bare command name, resolved via PATH
/// - `1` (tools folder): return the absolute path under `tools_dir()` (even if
///   the file does not exist yet, so a launch failure points at the location
///   the user actually chose)
/// - unset (wizard not answered yet): legacy behavior — the tools path when the
///   executable exists there, otherwise the bare command name
///
/// Preference and existence are checked on every call, so new settings or a
/// completed download take effect immediately without restarting.
pub fn ffmpeg_program() -> OsString {
    program_by_preference("ffmpeg")
}

/// 同 [`ffmpeg_program`]，解析 ffprobe。/ Same as [`ffmpeg_program`], for ffprobe.
pub fn ffprobe_program() -> OsString {
    program_by_preference("ffprobe")
}

/// 同 [`ffmpeg_program`]，解析 ffplay。/ Same as [`ffmpeg_program`], for ffplay.
pub fn ffplay_program() -> OsString {
    program_by_preference("ffplay")
}

/// [`ffmpeg_program`] / [`ffprobe_program`] / [`ffplay_program`] 的公共实现。
/// Shared implementation of [`ffmpeg_program`] / [`ffprobe_program`] / [`ffplay_program`].
fn program_by_preference(base: &str) -> OsString {
    let tools_path = tools_dir().join(exe_name(base));
    match preference() {
        // 仅系统 PATH / system PATH only
        Some(0) => OsString::from(base),
        // 仅 tools 目录（缺失也返回该路径）/ tools dir only (path returned even when absent)
        Some(1) => tools_path.into_os_string(),
        // 未设置：tools 存在优先，否则 PATH / unset: tools when present, else PATH
        _ if tools_path.exists() => tools_path.into_os_string(),
        _ => OsString::from(base),
    }
}

/// 返回一份把 `tools_dir()` 前置到 PATH 的环境变量值，仅当 tools 中存在
/// ffmpeg/ffprobe/ffplay 之一时返回 `Some`；首选项为系统环境变量（`0`）时返回
/// `None`——模块（含社区模块）以裸 `ffmpeg` 启动子进程时应同样使用系统副本。
///
/// Returns a PATH value with `tools_dir()` prepended, but only when tools
/// actually contains ffmpeg/ffprobe/ffplay; returns `None` when the preference
/// is the system PATH (`0`) — modules (including community modules) spawning
/// plain `ffmpeg` should then resolve the system copy too.
pub fn path_env_with_tools() -> Option<OsString> {
    if preference() == Some(0) {
        return None;
    }
    let tools = tools_dir();
    if !["ffmpeg", "ffprobe", "ffplay"]
        .iter()
        .any(|b| tools.join(exe_name(b)).exists())
    {
        return None;
    }
    let current = std::env::var_os("PATH").unwrap_or_default();
    let merged = std::iter::once(tools)
        .chain(std::env::split_paths(&current))
        .collect::<Vec<_>>();
    std::env::join_paths(merged).ok()
}

/// 检查 ffmpeg 与 ffprobe 是否都可用（两者都能成功启动 `-version`）。
/// Check whether both ffmpeg and ffprobe are usable (each can launch `-version`).
pub fn ffmpeg_available() -> bool {
    fn runs(prog: &OsString) -> bool {
        Command::new(prog)
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .no_window()
            .status()
            .is_ok()
    }
    runs(&ffmpeg_program()) && runs(&ffprobe_program())
}
