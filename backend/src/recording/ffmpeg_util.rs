//! ffmpeg/ffprobe 底层工具函数 / Low-level ffmpeg/ffprobe Utility Functions
//!
//! 提供分片 remux（fMP4→fMP4/.m4s，含 tfdt 补丁）、m3u8 维护、目录大小
//! 计算、视频时长/分辨率探测等纯 ffmpeg/ffprobe 操作，不涉及录制会话
//! 生命周期管理（会话生命周期见 `recording::recorder`）。
//!
//! Provides low-level ffmpeg/ffprobe operations: segment remuxing
//! (fMP4→fMP4/.m4s with tfdt patching), m3u8 maintenance, directory size
//! calculation, and video duration/resolution probing. Does not manage
//! recording session lifecycle (see `recording::recorder` for that).

use crate::core::error::{AppError, Result};
use crate::core::no_window::NoWindowExt;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// 检查 ffmpeg 与 ffprobe 是否可用（lib 目录副本优先，其次 PATH）。
/// 实际检测逻辑在 `crate::ffmpeg::ffmpeg_available`。
///
/// Check whether ffmpeg and ffprobe are usable (lib-dir copy first, then PATH).
/// The actual check lives in `crate::ffmpeg::ffmpeg_available`.
pub fn ffmpeg_available() -> bool {
    crate::ffmpeg::ffmpeg_available()
}

/// 使用 ffmpeg 将 fMP4 数据 remux 为 fragmented-MP4 分片（通过 stdin 管道传入）。
///
/// 历史沿革：早期产物是 MPEG-TS（`-f mpegts`），但 TS 没有官方 AV1 codec
/// 注册、AV1 变体流直接无法封装，故改存原生 fMP4——mp4 对
/// h264/H.265/AV1 全兼容。真正修复"时间码错乱"的并不是换容器，而是
/// ffmpeg 非 copyts 默认重基线（每分片归零）；fMP4 分片的时间轴由 tfdt
/// 承载，remux 后由 [`patch_m4s_tfdt`] 改写为该分片在清单里的累计位置，
/// 合并端获得确定性连续时间轴。
///
/// 录制分片转码是实时路径，不受后处理并发许可约束——后处理任务的并发由
/// `PpQueue` 的任务级许可统一控制，录制完整性优先，不与排队任务竞争。
///
/// Remux fMP4 data into a fragmented-MP4 segment via ffmpeg (piped via stdin).
///
/// History: segments used to be converted to MPEG-TS (`-f mpegts`), but TS
/// has no official AV1 codec registration — AV1 variant streams cannot be
/// muxed at all, so we now store native fMP4 (h264/H.265/AV1 all fit).
/// What actually fixed the "timestamp disorder" was never the container
/// swap but ffmpeg's default non-copyts rebase (zeroing each segment);
/// an fMP4 segment's timeline rides on tfdt, which [`patch_m4s_tfdt`]
/// rewrites to the segment's cumulative playlist position after remux,
/// giving the merge side a deterministic continuous timeline.
///
/// Segment remuxing is on the real-time recording path and is not subject
/// to post-processing concurrency permits — PP concurrency is controlled at
/// the task level by `PpQueue`; recording integrity takes priority and never
/// competes with queued tasks.
pub(crate) async fn convert_to_m4s(
    fmp4_data: Vec<u8>,
    m4s_path: &PathBuf,
    tfdt_offset_secs: f64,
) -> Result<()> {
    // 标记 ffmpeg 使用中，阻止自动更新在转码期间替换二进制
    // Mark ffmpeg in use so auto-update won't swap the binary mid-transcode
    let _ffmpeg_guard = crate::ffmpeg::in_use();
    let mut child = tokio::process::Command::new(crate::ffmpeg::ffmpeg_program())
        // -hide_banner -v warning：stderr 只留警告/错误，去掉版本横幅与 Input #0
        // 探测信息——失败日志不再被整版 banner/config 淹没。
        // -hide_banner -v warning keeps stderr to warnings/errors only —
        // drops the version banner and the Input #0 probe dump so failure
        // logs aren't drowned in pages of banner/config noise.
        //
        // 输出为自初始化 fMP4 分片：ftyp+moov(empty)+moof+mdat，每文件自带
        // 完整 trak 定义，HLS 清单无需 EXT-X-MAP 即可被 ffmpeg demuxer /
        // 浏览器 MSE 消费。
        // Output is a self-initializing fMP4 segment:
        // ftyp+moov(empty)+moof+mdat — each file carries full track
        // definitions so the playlist needs no EXT-X-MAP for the ffmpeg
        // demuxer or browser MSE to consume it.
        .args([
            "-hide_banner", "-v", "warning", "-y", "-i", "pipe:0", "-c", "copy",
            "-f", "mp4",
            "-movflags", "frag_keyframe+empty_moov+default_base_moof",
        ])
        .arg(m4s_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped()) // 捕获 stderr，失败时记录原因 / Capture stderr for failure diagnostics
        .no_window()
        .spawn()
        .map_err(|e| AppError::Other(format!("Failed to spawn ffmpeg: {}", e)))?;

    // 写入 stdin：写完后必须主动 drop 关闭管道，ffmpeg 才能感知到 EOF 并继续处理。
    // 若 ffmpeg 在写完前就因输入非法而提前退出，write_all 会收到 Broken pipe——
    // 这是 ffmpeg 自身检测到数据问题的信号，不应立即返回错误，而是继续 wait()
    // 获取真实的退出码和 stderr，再决定如何上报。
    //
    // Write to stdin: must explicitly drop (close) the pipe after writing so ffmpeg
    // sees EOF and can continue. If ffmpeg exits early due to invalid input, write_all
    // returns Broken pipe — this signals a data problem detected by ffmpeg itself.
    // Don't return immediately; instead fall through to wait() to get the real exit
    // code and stderr before deciding how to report the error.
    let write_err = if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let result = stdin.write_all(&fmp4_data).await;
        // drop(stdin) 隐式发生在这里，关闭管道写端，ffmpeg 收到 EOF
        // drop(stdin) happens implicitly here, closing the write end so ffmpeg gets EOF
        result.err()
    } else {
        None
    };

    // 同时等待进程退出并收集 stderr / Wait for exit and collect stderr concurrently
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Other(format!("ffmpeg wait: {}", e)))?;

    if !output.status.success() {
        // 优先展示 ffmpeg 自身的错误信息，方便排查数据问题；只取末尾几行——
        // 真正的错误总在尾部，前面的警告流（异常时间戳等）可能刷满整屏
        // Prefer ffmpeg's own message for diagnosing data issues; keep only
        // the tail lines — the real error is always at the end while the
        // earlier warning stream (bad timestamps etc.) can fill the screen
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        let detail = if !stderr.is_empty() {
            stderr_tail(stderr)
        } else if let Some(e) = write_err {
            format!("stdin write: {}", e)
        } else {
            format!("exit {}", output.status)
        };
        return Err(AppError::Other(format!("ffmpeg: {}", detail)));
    }

    // 进程成功退出，但写入时发生过错误（理论上不应出现，防御性处理）
    // Process exited successfully but a write error occurred (shouldn't happen; defensive)
    if let Some(e) = write_err {
        tracing::warn!("{}", crate::tl!("ffmpegUtil.stdinWriteIgnored", error = e));
    }

    // 把 moof/traf/tfdt 改写为清单累计位置：remux 的非 copyts 重基线只负责
    // 把分片归零，段间连续时间轴必须由我们显式写入——否则 fmp4 合并端按
    // tfdt 排时间轴，全 0 的分片会塌缩叠在同一位置。
    // Rewrite moof/traf/tfdt to the cumulative playlist position: remux's
    // non-copyts rebase only zeroes each segment — the cross-segment
    // continuous timeline must be written explicitly, otherwise the fmp4
    // merge demuxer lays segments by tfdt and all-zero segments collapse
    // onto one spot.
    if let Err(e) = patch_m4s_tfdt(m4s_path, tfdt_offset_secs) {
        // 补丁失败不致命：文件本身仍是合法 fMP4，只是合并时间轴可能偏移
        // Patching failure is non-fatal: the file is still valid fMP4,
        // only the merged timeline may be off
        tracing::warn!("{}", crate::tl!("ffmpegUtil.tfdtPatchFailed", error = e));
    }

    Ok(())
}

// ─── fMP4 tfdt 补丁 / fMP4 tfdt patching ────────────────────────────────────

/// 就地重写 .m4s 文件内每个 `moof/traf/tfdt` 的 `base_media_decode_time`。
///
/// `offset_secs` 为该分片在会话清单中的累计时间轴位置（秒）。每个轨道的
/// tfdt 按自身 `mdhd` timescale 换算；多 moof（一片多关键帧）时按 trun
/// 样本时长累计推进，保证 fragment 内时间轴连续。
///
/// Rewrite `base_media_decode_time` of every `moof/traf/tfdt` in a .m4s
/// file in place.
///
/// `offset_secs` is the segment's cumulative timeline position within the
/// session playlist (seconds). Each track's tfdt is scaled by its own
/// `mdhd` timescale; for multi-moof files (several keyframes in one
/// segment) the base advances by summed trun sample durations so the
/// in-fragment timeline stays continuous.
fn patch_m4s_tfdt(path: &PathBuf, offset_secs: f64) -> Result<()> {
    let mut data = fs::read(path)
        .map_err(|e| AppError::Other(format!("read m4s: {}", e)))?;

    // ISO-BMFF 顶层盒子迭代：size(4)+type(4)，size==1 用 64 位 largesize，
    // size==0 表示延伸到文件尾。
    // Top-level ISO-BMFF box iteration: size(4)+type(4); size==1 uses the
    // 64-bit largesize, size==0 extends to EOF.
    fn each_box(buf: &[u8], mut i: usize, end: usize) -> Vec<(usize, usize, [u8; 4], usize)> {
        let mut out = Vec::new();
        while i + 8 <= end {
            let mut size = u32::from_be_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            let typ: [u8; 4] = buf[i + 4..i + 8].try_into().unwrap();
            let mut hdr = 8usize;
            if size == 1 {
                if i + 16 > end {
                    break;
                }
                size = u64::from_be_bytes(buf[i + 8..i + 16].try_into().unwrap()) as usize;
                hdr = 16;
            } else if size == 0 {
                size = end - i;
            }
            if size == 0 {
                break;
            }
            out.push((i, size, typ, hdr));
            i += size;
        }
        out
    }
    let u32be = |buf: &[u8], p: usize| -> u32 {
        u32::from_be_bytes(buf[p..p + 4].try_into().unwrap())
    };

    // 第一遍：moov 里收集 track_id → mdhd timescale。
    // Pass 1: collect track_id → mdhd timescale from the moov.
    let mut timescales: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for (off, size, typ, hdr) in each_box(&data, 0, data.len()) {
        if &typ != b"moov" {
            continue;
        }
        for (toff, tsize, ttyp, thdr) in each_box(&data, off + hdr, off + size) {
            if &ttyp != b"trak" {
                continue;
            }
            let mut track_id = None;
            let mut tscale = None;
            for (coff, csize, ctyp, chdr) in each_box(&data, toff + thdr, toff + tsize) {
                match &ctyp {
                    b"tkhd" => {
                        // tkhd: version(1)+flags(3) 后，v0 track_id 在 +12 /
                        // after version+flags, v0 track_id sits at +12
                        track_id = Some(u32be(&data, coff + chdr + 12));
                    }
                    b"mdia" => {
                        for (doff, _dsize, dtyp, dhdr) in
                            each_box(&data, coff + chdr, coff + csize)
                        {
                            if &dtyp != b"mdhd" {
                                continue;
                            }
                            // mdhd v0: timescale 在 version+flags 后 +12；
                            // v1（64位时间）在 +20
                            // mdhd v0: timescale at +12 after version+flags;
                            // v1 (64-bit times) at +20
                            let ver = data[doff + dhdr];
                            tscale = Some(if ver == 1 {
                                u32be(&data, doff + dhdr + 20)
                            } else {
                                u32be(&data, doff + dhdr + 12)
                            } as u64);
                        }
                    }
                    _ => {}
                }
            }
            if let (Some(id), Some(ts)) = (track_id, tscale) {
                timescales.insert(id, ts);
            }
        }
    }
    if timescales.is_empty() {
        return Err(AppError::Other("no track timescale found in moov".into()));
    }

    // 第二遍：按序遍历 moof→traf，改写 tfdt 并按 trun 时长推进 elapsed。
    // Pass 2: walk moof→traf in order, rewrite tfdt and advance the
    // per-track elapsed base by summed trun durations.
    let mut elapsed: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    let mut patched = 0usize;
    for (off, size, typ, hdr) in each_box(&data, 0, data.len()) {
        if &typ != b"moof" {
            continue;
        }
        for (toff, tsize, ttyp, thdr) in each_box(&data, off + hdr, off + size) {
            if &ttyp != b"traf" {
                continue;
            }
            let mut track_id = None;
            let mut tfdt_pos: Option<(usize, u8)> = None;
            let mut frag_duration: u64 = 0;
            // tfhd 的 default_sample_duration：trun 缺逐样本时长字段时的兜底
            // tfhd's default_sample_duration: fallback when trun omits
            // per-sample durations
            let mut default_dur: Option<u64> = None;
            for (boff, _bsize, btyp, bhdr) in each_box(&data, toff + thdr, toff + tsize) {
                match &btyp {
                    b"tfhd" => {
                        let flags = u32be(&data, boff + bhdr) & 0x00FF_FFFF;
                        track_id = Some(u32be(&data, boff + bhdr + 4));
                        // 可选字段按 flag 位顺排：base_data_offset(0x1,8B)、
                        // sample_desc_index(0x2,4B)、default_duration(0x8,4B)
                        // Optional fields in flag order: base_data_offset
                        // (0x1, 8B), sample_desc_index (0x2, 4B),
                        // default_sample_duration (0x8, 4B)
                        let mut p = boff + bhdr + 8;
                        if flags & 0x1 != 0 {
                            p += 8;
                        }
                        if flags & 0x2 != 0 {
                            p += 4;
                        }
                        if flags & 0x8 != 0 && p + 4 <= data.len() {
                            default_dur = Some(u32be(&data, p) as u64);
                        }
                    }
                    b"tfdt" => {
                        tfdt_pos = Some((boff + bhdr + 4, data[boff + bhdr]));
                    }
                    b"trun" => {
                        frag_duration +=
                            trun_duration(&data, boff + bhdr, &u32be, default_dur)
                                .unwrap_or(0);
                    }
                    _ => {}
                }
            }
            let Some(id) = track_id else { continue };
            let Some(&scale) = timescales.get(&id) else { continue };
            // 本轨道累计起点 = offset + 该轨道已走过的 fragment 时长；
            // 同 moof 内不同轨道各自独立推进。
            // This track's base = offset + media already consumed by its
            // earlier fragments; tracks inside one moof advance
            // independently.
            let base = elapsed.entry(id).or_insert_with(|| {
                (offset_secs.max(0.0) * scale as f64).round() as u64
            });
            if let Some((pos, ver)) = tfdt_pos {
                if ver == 1 {
                    data[pos..pos + 8].copy_from_slice(&base.to_be_bytes());
                } else {
                    data[pos..pos + 4].copy_from_slice(&(*base as u32).to_be_bytes());
                }
                patched += 1;
            }
            *base += frag_duration;
        }
    }
    if patched == 0 {
        return Err(AppError::Other("no tfdt box found in m4s".into()));
    }

    fs::write(path, &data).map_err(|e| AppError::Other(format!("write m4s: {}", e)))?;
    Ok(())
}

/// 求 trun 的样本总时长（轨道 timescale 单位）。
///
/// trun 布局：version+flags(4)、sample_count(4)、可选 data_offset(4,
/// flag 0x1)、可选 first_sample_flags(4, flag 0x4)，其后每样本条目按
/// flag 位含 duration(0x100)/size(0x200)/flags(0x400)/cts(0x800)。
/// `tfhd_default` 未携带时退化为 `sample_count × default_sample_duration`
/// 的调用方兜底——此处在 trun 内自足解析，条目不完整则返回 None。
///
/// Total sample duration of a trun box in track-timescale units.
///
/// Layout: version+flags(4), sample_count(4), optional data_offset(4,
/// flag 0x1), optional first_sample_flags(4, flag 0x4), then per-sample
/// entries with duration(0x100)/size(0x200)/flags(0x400)/cts(0x800) by
/// flag bits. Falls back to `sample_count × default_sample_duration` when
/// durations are absent — the caller supplies tfhd's default; returns
/// None on a structurally unreadable box.
fn trun_duration(
    buf: &[u8],
    body: usize,
    u32be: &impl Fn(&[u8], usize) -> u32,
    tfhd_default: Option<u64>,
) -> Option<u64> {
    if body + 8 > buf.len() {
        return None;
    }
    let flags = u32be(buf, body) & 0x00FF_FFFF;
    let count = u32be(buf, body + 4) as usize;
    if flags & 0x100 == 0 {
        // 无逐样本时长：sample_count × tfhd 默认值兜底
        // No per-sample durations: fall back to sample_count × tfhd default
        return tfhd_default.map(|d| d * count as u64);
    }
    let mut p = body + 8;
    if flags & 0x1 != 0 {
        p += 4; // data_offset
    }
    if flags & 0x4 != 0 {
        p += 4; // first_sample_flags
    }
    // 逐样本条目按 flag 位含 duration(0x100)/size(0x200)/flags(0x400)/
    // cts(0x800)，duration 恒为首字段
    // Per-sample entries carry duration(0x100)/size(0x200)/flags(0x400)/
    // cts(0x800) by flag bits; duration is always the first field
    let per_sample = ((flags & 0x100 != 0) as usize)
        + ((flags & 0x200 != 0) as usize)
        + ((flags & 0x400 != 0) as usize)
        + ((flags & 0x800 != 0) as usize);
    let mut total = 0u64;
    for _ in 0..count {
        if p + per_sample * 4 > buf.len() {
            return None;
        }
        total += u32be(buf, p) as u64;
        p += per_sample * 4;
    }
    Some(total)
}

/// 失败日志保留的 stderr 末尾行数。ffmpeg 的有效错误信息总在尾部；
/// 行数过少可能丢失"多条错误"中的上文，取 12 行兼顾。
/// Number of trailing stderr lines kept in failure logs. ffmpeg's real
/// error is always at the tail; too few lines can drop context when
/// several errors are reported — 12 lines is a reasonable middle ground.
const STDERR_TAIL_LINES: usize = 12;

/// 截取 stderr 末尾 [`STDERR_TAIL_LINES`] 行；被截断时以 `…` 前缀标记，
/// 便于区分"完整输出"与"截尾输出"。
///
/// Keep the last [`STDERR_TAIL_LINES`] lines of `text`; when truncated,
/// a leading `…` marks it so logs distinguish full output from a tail cut.
fn stderr_tail(text: &str) -> String {
    let count = text.lines().count();
    if count <= STDERR_TAIL_LINES {
        return text.to_string();
    }
    // 跳过前 count - N 行，取剩余后缀 / Skip the first count - N lines and keep the suffix
    let skip = count - STDERR_TAIL_LINES;
    let start = text
        .match_indices('\n')
        .nth(skip - 1)
        .map(|(i, _)| i + 1)
        .unwrap_or(0);
    format!("…\n{}", &text[start..])
}

/// 串行化 playlist.m3u8 的所有写入：普通追加与重试乱序插入共用一把锁，
/// 防止插入路径的"读-改-写"与 append 交错导致条目丢失。写入极稀有且文件
/// 很小（每条目约 50 字节），全局锁的争用可忽略。
///
/// Serializes every playlist.m3u8 write: normal appends and out-of-order retry
/// insertions share one lock so the insertion path's read-modify-write cannot
/// interleave with an append and lose entries. Writes are rare and the file is
/// small (~50 bytes per entry), so global-lock contention is negligible.
static M3U8_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 获取 m3u8 写锁；中毒后仍继续——最坏只是文件内容短暂不一致，
/// 不应让整个写入路径停摆。
///
/// Take the m3u8 write lock; proceeds despite poisoning — the worst case is a
/// briefly inconsistent file, which must not halt the whole write path.
fn m3u8_lock() -> std::sync::MutexGuard<'static, ()> {
    M3U8_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 从 `{username}_segment{seq:06}.{m4s|ts}` 文件名解析分片序号。
/// 兼容旧会话残留的 .ts 文件名（只影响插入定位，不区分容器）。
/// Parse the segment sequence out of a `{username}_segment{seq:06}.{m4s|ts}`
/// filename. Legacy .ts names are accepted too — it only affects insert
/// positioning, not container handling.
fn seq_from_seg_name(name: &str) -> Option<u32> {
    let pos = name.rfind("_segment")?;
    let stem = &name[pos + "_segment".len()..];
    let digits = stem
        .strip_suffix(".m4s")
        .or_else(|| stem.strip_suffix(".ts"))?;
    digits.parse().ok()
}

/// 将分片按 seq 顺序写入 playlist.m3u8（标准 HLS 格式）：
/// - 其 seq 大于所有现有条目时退化为末尾追加（常规路径：并发任务虽乱序
///   完成，但多数情况下序号本就递增，落在文件尾即可）
/// - 否则把条目插到第一个 seq 更大的 `#EXTINF` 条目之前——文件不支持
///   原地插入，实为重写插入点之后的字节（m3u8 很小，成本可忽略）
/// - 幂等：条目文件名已存在时直接跳过
/// - 首次写入时自动添加 M3U8 文件头（`#EXTM3U` 和 `#EXT-X-VERSION:3`）
///
/// 并发分片任务全部经本函数写清单，多任务写入共用 [`M3U8_LOCK`]，读写
/// 互不交错。
///
/// Insert a segment into playlist.m3u8 in sequence order (standard HLS):
/// - Falls back to a plain append when its seq exceeds every existing
///   entry (the common path: concurrent jobs finish out of order but seqs
///   are still mostly increasing, landing at the tail)
/// - Otherwise inserts the entry before the first `#EXTINF` entry whose seq is
///   greater — files don't support in-place insertion, so this rewrites the
///   bytes from the insert point onward (trivial for a small m3u8)
/// - Idempotent: skips when the filename is already present
/// - Automatically writes the M3U8 header (`#EXTM3U` and `#EXT-X-VERSION:7`)
///   on first write
///
/// All concurrent segment jobs write the playlist via this function, sharing
/// [`M3U8_LOCK`] so reads and writes never interleave.
///
/// `duration_secs` 为该分片的真实 `#EXTINF` 时长（来自上游清单）；
/// 历史上曾写死 `#EXTINF:0`——每段声明时长都为 0，播放器/探测工具
/// 拿不到真实时长，消费端做时长/时间轴运算时也更容易踩除零路径。
///
/// `duration_secs` is the segment's true `#EXTINF` duration (from the upstream
/// playlist); historically this was hardcoded `#EXTINF:0` — every segment
/// declared zero length, so players/probers saw no real duration and
/// duration/timeline math in consumers was one step closer to
/// divide-by-zero paths.
pub(crate) fn insert_to_m3u8_sorted(
    session_dir: &std::path::Path,
    seg_path: &std::path::Path,
    seq: u32,
    duration_secs: f64,
) {
    let _guard = m3u8_lock();
    let m3u8_path = session_dir.join("playlist.m3u8");
    let Some(filename) = seg_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };

    let existing = fs::read_to_string(&m3u8_path).unwrap_or_default();
    // 幂等：条目已存在则跳过 / Idempotent: skip when the entry already exists
    if existing.contains(filename) {
        return;
    }

    let entry = format!("#EXTINF:{:.3},\n{}\n", duration_secs.max(0.0), filename);

    // 尚无清单（首个分片即由重试写回的边界）：写头部 + 条目。
    // VERSION 7：fMP4 自初始化分片建议 ≥6（EXT-X-MAP 时代的要求），
    // ffmpeg demuxer 对版本号不挑剔，按规范写 7。
    // No playlist yet (edge: the very first segment lands via retry): header
    // + entry. VERSION 7: fMP4 self-initializing segments want ≥6 per spec
    // (an EXT-X-MAP-era requirement); the ffmpeg demuxer doesn't care but
    // we stay spec-faithful.
    if existing.is_empty() {
        let content = format!(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-MEDIA-SEQUENCE:0\n{}",
            entry
        );
        if let Err(e) = fs::write(&m3u8_path, content) {
            tracing::error!("{}", crate::tl!("ffmpegUtil.updateM3u8Failed", error = e));
        }
        return;
    }

    // 扫描条目行，找第一个 seq 更大的分片；其 `#EXTINF` 行起点即插入位置。
    // prev_start 始终指向前一行起点——条目结构固定为 "#EXTINF:{dur},\n{file}\n"。
    // Scan entry lines for the first segment with a greater seq; the start of
    // its `#EXTINF` line is the insert point. `prev_start` always points at the
    // previous line's start — entries are fixed "#EXTINF:{dur},\n{file}\n" pairs.
    let mut insert_at: Option<usize> = None;
    let mut line_start = 0usize;
    let mut prev_start = 0usize;
    for line in existing.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        if !text.starts_with('#')
            && let Some(other_seq) = seq_from_seg_name(text)
            && other_seq > seq
        {
            insert_at = Some(prev_start);
            break;
        }
        prev_start = line_start;
        line_start += line.len();
    }

    match insert_at {
        Some(pos) => {
            // 把插入点之后的字节整体后移，写入新条目 / Shift the tail bytes and
            // splice the new entry in place
            let mut content = existing.into_bytes();
            content.splice(pos..pos, entry.bytes());
            if let Err(e) = fs::write(&m3u8_path, content) {
                tracing::error!("{}", crate::tl!("ffmpegUtil.updateM3u8Failed", error = e));
            }
        }
        None => {
            // seq 最大：与 append 相同的末尾追加 / Largest seq: plain append
            if let Err(e) = fs::OpenOptions::new()
                .append(true)
                .open(&m3u8_path)
                .and_then(|mut f| f.write_all(entry.as_bytes()))
            {
                tracing::error!("{}", crate::tl!("ffmpegUtil.updateM3u8Failed", error = e));
            }
        }
    }
}

/// 计算目录中所有文件的总大小（字节）。
/// Calculate the total size of all files in a directory (bytes).
pub fn dir_size_bytes(dir: &PathBuf) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        }
    }
    Ok(total)
}

/// 使用 ffprobe 获取视频文件的时长（秒）。
/// Get the duration of a video file in seconds using ffprobe.
pub fn get_video_duration(path: &std::path::Path) -> Option<u64> {
    let _ffmpeg_guard = crate::ffmpeg::in_use();
    let output = Command::new(crate::ffmpeg::ffprobe_program())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_window()
        .output()
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse::<f64>().ok().map(|d| d as u64)
}

/// 使用 ffprobe 获取视频文件的分辨率（如 "1920x1080"）。
/// Get the resolution of a video file (e.g. "1920x1080") using ffprobe.
pub fn get_video_resolution(path: &std::path::Path) -> Option<String> {
    let _ffmpeg_guard = crate::ffmpeg::in_use();
    let output = Command::new(crate::ffmpeg::ffprobe_program())
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=s=x:p=0",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .no_window()
        .output()
        .ok()?;

    let s = String::from_utf8_lossy(&output.stdout);
    let trimmed = s.trim();
    // 格式为 "WxH"，过滤无效值（含 0 的结果） / Format is "WxH"; filter out invalid results (containing 0)
    if trimmed.is_empty() || trimmed == "x" || trimmed.starts_with('x') || trimmed.ends_with('x') {
        return None;
    }
    let parts: Vec<&str> = trimmed.split('x').collect();
    if parts.len() == 2
        && parts
            .iter()
            .all(|p| p.parse::<u32>().map(|v| v > 0).unwrap_or(false))
    {
        Some(trimmed.to_string())
    } else {
        None
    }
}
