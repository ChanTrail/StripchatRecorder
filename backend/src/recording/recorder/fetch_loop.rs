//! 录制主循环与分片下载子模块：播放清单轮询与分片执行解耦。
//! 主循环只做三件事——轮询清单、缺口检测、把新分片入队；分片本体由
//! 信号量有界的后台任务完成（下载→init→转码→有序写入 m3u8→回报），
//! 因此轮询节奏只受清单拉取耗时约束，分片慢/个别失败不再拖慢滑窗
//! 快照密度（窗口仅 ~3 条时，这是不被甩出窗口的关键）。
//! Recording main-loop and segment-download submodule: playlist polling
//! decoupled from segment execution. The main loop only polls playlists,
//! detects window gaps and enqueues new segments; the segment body is
//! handled by semaphore-bounded background jobs (download → init →
//! convert → sorted m3u8 insert → report), so poll cadence is bound only
//! by playlist-fetch latency — slow segments or isolated failures can no
//! longer stretch the snapshot cadence (decisive when the window is ~3
//! entries).

use super::{PlaylistRefresh, RecorderManager, refresh_playlist_target};
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use crate::platform::stripchat::StripchatApi;
use crate::recording::ffmpeg_util::{convert_to_ts, dir_size_bytes, insert_to_m3u8_sorted};
use crate::recording::hls::{HlsSegment, MouflonKeys, get_url_prefix, parse_playlist};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OnceCell, Semaphore, mpsc};

/// 播放清单轮询间隔：live 媒体清单 `TARGETDURATION:2`（窗口 ~3 条），
/// 1.2s 一拍让相邻快照有约 40% 重叠——单条分片至多被错过一拍。
/// Playlist poll interval: live media playlists run `TARGETDURATION:2`
/// (~3-entry window); a 1.2s beat overlaps adjacent snapshots by ~40% —
/// a segment can be missed by at most one beat.
const POLL_INTERVAL: Duration = Duration::from_millis(1200);

/// 同时在飞的分片任务数上限：实测单连接 ~0.16MB/s、聚合需求 ~0.5MB/s，
/// 8 路并发留足余量，又不会在 CDN 抖动时把连接风暴打到全灭。
/// Max in-flight segment jobs: measured ~0.16MB/s per connection with a
/// ~0.5MB/s aggregate need — 8 lanes leaves ample headroom without
/// turning CDN turbulence into a self-inflicted connection storm.
const SEG_JOB_PERMITS: usize = 8;

/// 「无进展」判停阈值：既无新分片入队也无分片写盘持续该时长，认定直播
/// 流实质停滞（清单冻结或窗口空转），正常下播/刷新路径另有计数兜底。
/// Stall verdict: with neither enqueues nor writes for this long the
/// stream is treated as effectively stalled (frozen playlist or a window
/// sliding past with nothing to do); normal end/refresh paths have their
/// own counters.
const STALL_TIMEOUT: Duration = Duration::from_secs(45);

/// 分片下载重试（首次失败之后）：500ms → 1s → 2s 退避，共 3 次。
/// Segment retries after the first failure: 500ms → 1s → 2s backoff, 3 total.
const RETRY_DELAYS_MS: [u64; 3] = [500, 1000, 2000];

/// init 段缓存容量上限：超出即清空（在飞任务持有自己的 Arc 不受影响，
/// 条目按需重建——init 只有 ~1KB，重建代价可忽略）。
/// Init-cache capacity cap: cleared past this size (in-flight jobs hold
/// their own Arc and are unaffected; entries rebuild on demand — an init
/// is ~1KB so rebuilding is negligible).
const INIT_CACHE_CAP: usize = 16;

/// 共享 init 段缓存：按 init URL 去重的 [`OnceCell`] 映射——并发任务对
/// 同一 URL 共享同一次下载；失败时 cell 保持未初始化，下一个访问者
/// 自动重试。
/// Shared init-segment cache: a per-URL [`OnceCell`] map — concurrent jobs
/// share a single download per URL; on failure the cell stays
/// uninitialized and the next accessor retries.
type InitCache = Arc<parking_lot::Mutex<HashMap<String, Arc<OnceCell<Vec<u8>>>>>>;

/// 待下载分片任务：入队时刻快照——会话目录、生效 init URL、是否清单尾
/// 条目都按入队当时固定（轮候期间清单/目录可能已轮换）。
/// Queued segment job: snapshotted at enqueue — session dir, effective
/// init URL and the tail flag are frozen at that moment (the playlist and
/// the dir may have moved on while the job waited for a permit).
struct PendingSeg {
    /// 分片序号 / segment sequence number
    seq: u32,
    /// 分片 URL / segment URL
    url: String,
    /// 入队时生效的 init URL（沿用最近一次 EXT-X-MAP 变更点）
    /// Effective init URL at enqueue (inherited from the latest EXT-X-MAP
    /// change point)
    init_url: Option<String>,
    /// 分片时长（#EXTINF，秒）/ segment duration in seconds
    duration_secs: f64,
    /// 入队时是否清单最后一条（尾条常常尚未发布完，404 时按退避继续重试
    /// 而非立即判逐出——给发布延迟留余地）
    /// Whether it was the last playlist entry at enqueue (the tail is
    /// often not fully published yet — a 404 keeps backing off instead of
    /// being called evicted outright, giving the publish lag room)
    is_tail: bool,
    /// 入队时的会话目录快照（轮转后晚到的写仍落旧目录，由收尾合并兜底）
    /// Session-dir snapshot at enqueue (a write landing after rotation
    /// still goes to the old dir — the exit merge takes care of it)
    session_dir: PathBuf,
}

/// 分片任务回报：主循环在下一轮开头统一收取并更新统计/轮转/去标记。
/// Segment-job report: the main loop collects these at the top of the
/// next iteration for stats, rotation and un-marking.
enum SegReport {
    /// 已写盘：目录随报——轮转后落进旧目录的回报不计入新目录时长
    /// Written: the dir travels with it — a report landing in a rotated-out
    /// dir must not feed the new dir's duration counter
    Written {
        /// 实际写入的会话目录 / the session dir actually written to
        dir: PathBuf,
        /// 分片时长（计入录制时长）/ segment duration (feeds recorded time)
        duration_secs: f64,
    },
    /// 已被 CDN 逐出（非尾条目首轮即 404，或重试期间 404）：终判缺失，
    /// 计入失败但不喂连续失败阈值——清单本身没病，不触发 URL 刷新。
    /// Evicted by the CDN (first-attempt 404 on a non-tail entry, or a 404
    /// during retries): terminal loss, counted as failed but NOT fed into
    /// the consecutive-failure threshold — the playlist is healthy, so no
    /// URL refresh is warranted.
    Evicted,
    /// 下载/init/转码重试耗尽：计入失败并喂连续失败阈值（达到阈值触发
    /// 清单 URL 刷新——与旧路径一致把转码类本地失败也归并进计数）。
    /// Download/init/convert attempts exhausted: counted as failed and fed
    /// into the consecutive-failure threshold (matching the old path, which
    /// also lumped local convert failures into that count).
    DownloadFailed,
}

impl RecorderManager {
    /// 录制主循环：持续拉取 HLS 播放列表、把新分片派发给有界后台任务，
    /// 并统一收取任务回报更新统计与轮转。代理设置和 Mouflon 密钥在每次
    /// 循环迭代时动态读取，变更后立即生效（新派发的任务使用新 api）。
    ///
    /// Recording main loop: continuously fetches HLS playlists, dispatches
    /// new segments to bounded background jobs and collects job reports for
    /// stats and rotation. Proxy settings and Mouflon keys are read
    /// dynamically each iteration and take effect immediately (newly
    /// dispatched jobs pick up the new api).
    pub(super) async fn recording_loop(
        self: &Arc<Self>,
        username: &str,
        playlist_url: &str,
        session_dir: &mut PathBuf,
        mut stop_rx: mpsc::Receiver<()>,
        emitter: Arc<dyn Emitter>,
    ) -> Result<()> {
        // 每次刷新 playlist 时都重新读取 model_id：录制过程中若因主播改名触发过
        // AppState::rename_streamer，追踪列表里该主播的用户名会变化，此处需要
        // 用当前实际记录的 model_id（而非录制开始时的快照）传给 get_stream_info
        // 做改名反查兜底。
        //
        // Re-read model_id on every playlist refresh: if a rename triggered
        // AppState::rename_streamer during recording, the tracked entry's username may
        // have changed. Use the currently recorded model_id (not a snapshot taken at
        // recording start) as the get_stream_info rename-lookup fallback.
        let known_model_id = || -> Option<i64> {
            self.state
                .get_streamers()
                .into_iter()
                .find(|s| s.username == username)
                .and_then(|s| s.model_id)
        };
        // 初始设置快照，用于检测变更 / Initial settings snapshot for change detection
        let mut last_settings = self.state.get_settings();
        let mut api = StripchatApi::new(
            last_settings.api_proxy_url.as_deref(),
            last_settings.cdn_proxy_url.as_deref(),
            last_settings.sc_mirror_url.as_deref(),
            Some(last_settings.sc_mirror_scheme.as_str()),
            Arc::clone(&self.preferred_tld_by_node),
            last_settings.ech_enabled,
        )?
        .with_mouflon_keys(self.state.get_mouflon_keys())
        .with_resolution_selection(
            last_settings.preferred_resolution,
            &last_settings.resolution_preference,
        )
        .with_codec_selection(&last_settings.preferred_codec);
        let mut current_playlist_url = playlist_url.to_string();
        let mut url_prefix = get_url_prefix(&current_playlist_url);

        let mut downloaded_sequences: HashSet<u32> = HashSet::new();
        // 清单窗口追踪：缺口检测/编号步长/媒体序号覆盖的跨轮状态
        // Playlist-window tracking: cross-round state for gap detection,
        // numbering stride and media-sequence coverage
        let mut window_track = WindowTrack::default();
        // 轮询间隙预取的下一轮清单快照（跨迭代传递；URL 不符时在消费侧作废）
        // Next-poll playlist snapshot prefetched between polls (threaded
        // across iterations; a URL mismatch discards it on the consume side)
        let mut playlist_prefetch: Option<PlaylistPrefetch> = None;
        // 当前生效的 init URL：EXT-X-MAP 变更点更新，跨轮沿用（分片任务在
        // 入队时快照它）/ Effective init URL: updated at EXT-X-MAP change
        // points, carried across polls (jobs snapshot it at enqueue)
        let mut cur_init_url: Option<String> = None;
        // 分片任务并发上限与回报通道 / Bounded job permits and the report channel
        let seg_permits = Arc::new(Semaphore::new(SEG_JOB_PERMITS));
        let (report_tx, mut report_rx) = mpsc::unbounded_channel::<SegReport>();
        // 按 URL 去重的共享 init 缓存 / Per-URL deduplicated init cache
        let init_cache: InitCache = Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let mut recorded_secs = 0.0f64;
        let mut playlist_refresh_failures = 0;
        let mut consecutive_cdn_failures: usize = 0;
        let mut last_size_snapshot: Option<(u64, Instant)> = None;
        // 累计成功下载的分片数 / Total successfully downloaded segments
        let mut total_downloaded: u64 = 0;
        // 累计下载失败的分片数 / Total failed segment downloads
        let mut total_failed: u64 = 0;
        // 上次有进展（新分片入队或有分片写盘）的时间：无进展超时判停
        // Last-progress timestamp (a new enqueue or a write refreshes it):
        // the stall verdict
        let mut last_progress = Instant::now();
        const MAX_PLAYLIST_REFRESH_FAILURES: u32 = 5;
        const CDN_FAILURE_REFRESH_THRESHOLD: usize = 3;
        /// 播放列表 URL 预热刷新间隔：周期性地在后台重取 URL 并换上，
        /// 使在用的 URL「年龄」有上界——旧 URL 大概率在失效前就被换掉。
        /// Warm-refresh interval for the playlist URL: periodically re-fetches
        /// the URL in the background and swaps it in, bounding the in-use URL's
        /// age — the old URL is almost always replaced before it can expire.
        const WARM_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
        let mut last_warm_refresh = Instant::now();
        // 在飞的预热刷新任务（one-shot spawn；结果在下一轮循环开头收取）
        // In-flight warm-refresh task (one-shot spawn; its result is collected
        // at the top of the next loop round)
        let mut warm_task: Option<tokio::task::JoinHandle<Result<String>>> = None;

        tracing::info!(
            "{}",
            crate::tl!(
                "recorder.started",
                username = username,
                dir = session_dir.display()
            )
        );

        loop {
            // 检测代理/密钥设置变更，变更时重建 api 实例使其立即生效
            // Detect proxy/key setting changes and rebuild api instance for immediate effect
            let current_settings = self.state.get_settings();
            let current_mouflon_keys = self.state.get_mouflon_keys();
            let proxy_changed = current_settings.api_proxy_url != last_settings.api_proxy_url
                || current_settings.cdn_proxy_url != last_settings.cdn_proxy_url
                || current_settings.sc_mirror_url != last_settings.sc_mirror_url
                || current_settings.sc_mirror_scheme != last_settings.sc_mirror_scheme
                // ECH 开关改变客户端 TLS 栈，属同一重建条件 /
                // The ECH toggle changes the client TLS stack — same rebuild condition
                || current_settings.ech_enabled != last_settings.ech_enabled;
            let stream_selection_changed = current_settings.preferred_resolution
                != last_settings.preferred_resolution
                || current_settings.resolution_preference != last_settings.resolution_preference
                || current_settings.preferred_codec != last_settings.preferred_codec;
            let keys_changed = current_mouflon_keys != *api.mouflon_keys();
            if proxy_changed || stream_selection_changed || keys_changed {
                match StripchatApi::new(
                    current_settings.api_proxy_url.as_deref(),
                    current_settings.cdn_proxy_url.as_deref(),
                    current_settings.sc_mirror_url.as_deref(),
                    Some(current_settings.sc_mirror_scheme.as_str()),
                    Arc::clone(&self.preferred_tld_by_node),
                    current_settings.ech_enabled,
                ) {
                    Ok(new_api) => {
                        api = new_api
                            .with_mouflon_keys(current_mouflon_keys)
                            .with_resolution_selection(
                                current_settings.preferred_resolution,
                                &current_settings.resolution_preference,
                            )
                            .with_codec_selection(&current_settings.preferred_codec);
                        tracing::info!(
                            "{}",
                            crate::tl!("recorder.apiRebuilt", username = username)
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "{}",
                            crate::tl!("recorder.apiRebuildFailed", username = username, error = e)
                        );
                    }
                }
                last_settings = current_settings.clone();
            }
            let mouflon_keys = api.mouflon_keys().clone();

            // 预热刷新：收取上一轮后台任务结果；到期则 spawn 新一轮。
            // 每次 spawn 克隆的是当前 api 实例——代理/ECH/清晰度等设置变更随
            // 上面的重建逻辑自动传导到刷新任务。任务挂起超过一个间隔时只需等它
            // 自然完成，不叠加并发。失败静默（debug 日志），旧 URL 继续服役，
            // 真正的失效仍由下方响应式路径兜底。
            //
            // Warm refresh: collect last round's task result; spawn a new round
            // when due. Each spawn clones the CURRENT api instance — proxy/ECH/
            // resolution changes propagate automatically via the rebuild logic
            // above. A task outliving an interval simply finishes on its own —
            // no concurrent stacking. Failures stay quiet (debug log) and the
            // old URL keeps serving; the reactive path below remains the real
            // failure handler.
            if warm_task.as_ref().is_none_or(|h| h.is_finished()) {
                if let Some(h) = warm_task.take() {
                    match h.await {
                        Ok(Ok(new_url)) if new_url != current_playlist_url => {
                            tracing::info!(
                                "{}",
                                crate::tl!("recorder.playlistRefreshed", username = username)
                            );
                            url_prefix = get_url_prefix(&new_url);
                            current_playlist_url = new_url;
                            // URL 换了，MEDIA-SEQUENCE 可能换基线：重置覆盖端点防止误报
                            // URL swapped — media-sequence may rebase: reset coverage to avoid false gaps
                            window_track.covered_end = None;
                            playlist_refresh_failures = 0;
                        }
                        Ok(Err(e)) => {
                            tracing::debug!(
                                "{}",
                                crate::tl!(
                                    "recorder.playlistWarmRefreshFailed",
                                    username = username,
                                    error = e
                                )
                            );
                        }
                        _ => {}
                    }
                }
                if last_warm_refresh.elapsed() >= WARM_REFRESH_INTERVAL {
                    last_warm_refresh = Instant::now();
                    if let Some(mid) = known_model_id() {
                        let warm_api = api.clone();
                        let warm_username = username.to_string();
                        warm_task = Some(tokio::spawn(async move {
                            warm_api.get_playlist_url(&warm_username, mid).await
                        }));
                    }
                }
            }

            // 收取分片任务回报：写盘计入时长与成功数、尾条未就绪撤销标记、
            // 逐出/失败计入失败数（失败还喂连续失败阈值）。随后做轮转检查——
            // 轮转只发生在回报边界，天然对齐分片边界。
            // Collect segment-job reports: writes feed duration/success,
            // a not-ready tail clears its handled mark, evictions/failures
            // count as failed (failures also feed the threshold). Rotation
            // is then checked at the report boundary — naturally aligned to
            // segment boundaries.
            let mut stats_dirty = false;
            while let Ok(report) = report_rx.try_recv() {
                stats_dirty = true;
                match report {
                    SegReport::Written {
                        dir,
                        duration_secs,
                        ..
                    } => {
                        total_downloaded += 1;
                        consecutive_cdn_failures = 0;
                        // 只把落进当前会话目录的写入计入时长——轮转后晚到的
                        // 旧目录回报属于上一会话
                        // Only writes landing in the CURRENT session dir count
                        // toward duration — a late report for a rotated-out dir
                        // belongs to the previous session
                        if dir == *session_dir {
                            recorded_secs += duration_secs;
                        }
                        last_progress = Instant::now();
                    }
                    SegReport::Evicted => {
                        total_failed += 1;
                    }
                    SegReport::DownloadFailed => {
                        total_failed += 1;
                        consecutive_cdn_failures += 1;
                    }
                }
            }

            // 录制时长上限轮转（在分片回报边界切换）：成功才归零，失败
            // 保留下轮再试 / Rotate at the report boundary once the duration
            // cap is reached: reset only on success — kept for a retry next
            // round on failure
            let limit = self.state.get_settings().max_recording_duration_secs;
            if limit > 0 && recorded_secs >= limit as f64 {
                match self.rotate_session(username, session_dir, &emitter) {
                    Ok(()) => recorded_secs = 0.0,
                    Err(e) => {
                        tracing::warn!(
                            "{}",
                            crate::tl!("recorder.rotateFailed", username = username, error = e)
                        );
                    }
                }
            }

            let mut skip_sleep = false;

            // 连续分片失败达到阈值：刷新清单 URL（响应式路径——清单 URL
            // 可能已失效，分片全灭多半由此而来）/ Consecutive segment
            // failures past the threshold: refresh the playlist URL
            // (reactive path — a stale URL is the usual cause of an
            // all-fail burst)
            if consecutive_cdn_failures >= CDN_FAILURE_REFRESH_THRESHOLD {
                tracing::error!("{}", crate::tl!("recorder.fetchErrCdnRefresh", username = username, count = consecutive_cdn_failures)
                );
                consecutive_cdn_failures = 0;
                match refresh_playlist_target(&api, username, known_model_id(), &current_playlist_url)
                    .await
                {
                    PlaylistRefresh::Swapped(new_url) => {
                        // 刷新后 URL 未变 = 媒体列表仍是刚失败的那条：
                        // 按刷新失败计数、不提前重试（保持正常轮询节奏）
                        // URL unchanged = media playlist is still the
                        // just-failed one: count as refresh failure and
                        // keep the normal cadence (no early retry)
                        if new_url == current_playlist_url {
                            playlist_refresh_failures += 1;
                        } else {
                            tracing::info!("{}", crate::tl!("recorder.playlistRefreshed", username = username));
                            url_prefix = get_url_prefix(&new_url);
                            current_playlist_url = new_url;
                            // URL 换了，MEDIA-SEQUENCE 可能换基线：重置覆盖端点防止误报
                            // URL swapped — media-sequence may rebase: reset coverage to avoid false gaps
                            window_track.covered_end = None;
                            playlist_refresh_failures = 0;
                            last_progress = Instant::now();
                            skip_sleep = true;
                        }
                    }
                    PlaylistRefresh::NotRecordable(status) => {
                        tracing::warn!("{}", crate::tl!("recorder.streamNotRecordable", username = username, status = status));
                        break;
                    }
                    PlaylistRefresh::NoUrl(_) => {
                        playlist_refresh_failures += 1;
                    }
                    PlaylistRefresh::Failed(err) => {
                        tracing::error!("{}", crate::tl!("recorder.playlistRefreshFailed", username = username, error = err));
                        playlist_refresh_failures += 1;
                    }
                }
                if playlist_refresh_failures >= MAX_PLAYLIST_REFRESH_FAILURES {
                    tracing::warn!("{}", crate::tl!("recorder.streamEndedPlaylistFail", username = username, count = playlist_refresh_failures));
                    break;
                }
            }

            // 无进展判停：清单能拉但一直无新分片入队也无写盘——流实质停滞
            // Stall verdict: the playlist still fetches but nothing new was
            // enqueued and nothing was written for STALL_TIMEOUT
            if last_progress.elapsed() >= STALL_TIMEOUT {
                tracing::warn!(
                    "{}",
                    crate::tl!("recorder.streamStalled", username = username)
                );
                break;
            }

            tokio::select! {
                _ = stop_rx.recv() => {
                    tracing::info!("{}", crate::tl!("recorder.stopSignal", username = username));
                    break;
                }
                result = poll_once(
                    &api,
                    &current_playlist_url,
                    &url_prefix,
                    &mouflon_keys,
                    username,
                    session_dir,
                    &mut downloaded_sequences,
                    &mut window_track,
                    &mut cur_init_url,
                    &init_cache,
                    &seg_permits,
                    &report_tx,
                    playlist_prefetch.take(),
                ) => {
                    match result {
                        Ok((enqueued, gap_failures, next_prefetch)) => {
                            // 轮间预取的下一拍清单快照，存给下一次迭代消费
                            // Inter-poll playlist snapshot — consumed by the
                            // next iteration (URL-matched there; stale ones
                            // are aborted on consume)
                            playlist_prefetch = next_prefetch;
                            // 滑窗缺口：不是 CDN 故障（不触发清单刷新），但
                            // 是文件里的真实缺失，同样计入失败统计 /
                            // window gaps aren't CDN failures (no playlist
                            // refresh) but are real file gaps — counted all
                            // the same
                            if gap_failures > 0 {
                                total_failed += gap_failures;
                                stats_dirty = true;
                            }
                            // 注意入队/缺口不刷新 last_progress：只有真正
                            // 写盘才算进展，否则「清单健康但分片永远写不进」
                            // （CDN 全死）会无限空转不退出
                            // Enqueues/gaps deliberately do NOT refresh
                            // last_progress: only an actual write counts as
                            // progress — otherwise "playlist healthy but
                            // nothing ever writes" (CDN dead) would spin
                            // forever
                            if enqueued > 0 || gap_failures > 0 {
                                // 有活干的拍跳过轮间等待：落后清单窗口时下一拍
                                // 立即消化在飞预取 / busy beats skip the wait:
                                // when behind the window the next beat drains
                                // the in-flight prefetch right away
                                skip_sleep = true;
                            }
                        }
                        Err(e) => {
                            tracing::error!("{}", crate::tl!("recorder.fetchErrRefresh", username = username, error = e));
                            consecutive_cdn_failures = 0;
                            match refresh_playlist_target(&api, username, known_model_id(), &current_playlist_url).await {
                                PlaylistRefresh::Swapped(new_url) => {
                                    // 同 URL = 无进展，按刷新失败计数 / Same URL = no progress, count as failure
                                    if new_url == current_playlist_url {
                                        playlist_refresh_failures += 1;
                                    } else {
                                        tracing::info!("{}", crate::tl!("recorder.playlistRefreshed", username = username));
                                        url_prefix = get_url_prefix(&new_url);
                                        current_playlist_url = new_url;
                                        // URL 换了，MEDIA-SEQUENCE 可能换基线：重置覆盖端点防止误报
                                        // URL swapped — media-sequence may rebase: reset coverage to avoid false gaps
                                        window_track.covered_end = None;
                                        playlist_refresh_failures = 0;
                                        last_progress = Instant::now();
                                        skip_sleep = true;
                                    }
                                }
                                PlaylistRefresh::NotRecordable(status) => {
                                    tracing::warn!("{}", crate::tl!("recorder.streamNotRecordableNoUrl", username = username, status = status));
                                    break;
                                }
                                PlaylistRefresh::NoUrl(status) => {
                                    tracing::warn!("{}", crate::tl!("recorder.noPlaylistUrl", username = username, status = status));
                                    playlist_refresh_failures += 1;
                                }
                                PlaylistRefresh::Failed(err) => {
                                    tracing::error!("{}", crate::tl!("recorder.playlistRefreshFailed", username = username, error = err));
                                    playlist_refresh_failures += 1;
                                }
                            }
                            if playlist_refresh_failures >= MAX_PLAYLIST_REFRESH_FAILURES {
                                tracing::warn!("{}", crate::tl!("recorder.streamEndedPlaylistFail", username = username, count = playlist_refresh_failures));
                                break;
                            }
                        }
                    }
                }
            }

            // 任一回报告或缺口即刷新统计：此前整个落盘/上报块只在成功轮
            // 写盘，全失败轮与收尾前的失败计数永远不会写进 meta/stats
            // （分片缺失但失败数没写入的根因）
            // Flush stats whenever a report OR a gap occurred: this block
            // previously ran only on successful rounds, so all-failed rounds
            // and last-moment failures were never persisted (the "segments
            // missing but no failed count" bug)
            if stats_dirty {
                let size_bytes = dir_size_bytes(&*session_dir).unwrap_or(0);
                let now = Instant::now();
                let speed_bps = last_size_snapshot.map(|(prev_size, prev_time)| {
                    let dt = now.duration_since(prev_time).as_secs_f64();
                    let ds = size_bytes.saturating_sub(prev_size) as f64;
                    if dt > 0.0 { ds / dt } else { 0.0 }
                });
                last_size_snapshot = Some((size_bytes, now));

                // session_dir 路径同时用于 meta 更新和前端匹配
                // session_dir path is used for both meta update and frontend matching
                let session_dir_str = session_dir.to_string_lossy().to_string();

                // 将实时文件大小和分片统计写入 meta JSON
                // Write real-time file size and segment stats to meta JSON
                if let Some(mut meta) = crate::recording::meta::read_meta(session_dir) {
                    let mut changed = false;
                    if meta.size_bytes != size_bytes {
                        meta.size_bytes = size_bytes;
                        changed = true;
                    }
                    if meta.segments_downloaded != Some(total_downloaded) {
                        meta.segments_downloaded = Some(total_downloaded);
                        changed = true;
                    }
                    if meta.segments_failed != Some(total_failed) {
                        meta.segments_failed = Some(total_failed);
                        changed = true;
                    }
                    if changed {
                        crate::recording::meta::write_meta(session_dir, &meta);
                    }
                }

                // 更新管理器中的实时分片统计 / Update real-time segment stats in manager
                self.segment_stats.write().insert(
                    session_dir_str.clone(),
                    (total_downloaded, total_failed),
                );

                let mut payload = serde_json::json!({
                    "path": session_dir_str,
                    "segment_count": downloaded_sequences.len(),
                    "size_bytes": size_bytes,
                    "segments_downloaded": total_downloaded,
                    "segments_failed": total_failed,
                });
                if let Some(spd) = speed_bps {
                    payload["speed_bps"] = serde_json::json!(spd);
                }
                emitter.emit("recording-file-update", &payload);
            }

            if !skip_sleep {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }

        // 退出时丢弃可能在飞的预热任务，避免泄漏 / Abort any in-flight warm task on exit
        if let Some(h) = warm_task {
            h.abort();
        }
        // 在飞的分片任务不逐个取消：它们持有自己的目录快照，写完即自然
        // 结束（合并按 playlist.m3u8 清单读，晚到条目写得上就进合并，
        // 赶不上也不影响正确性）/ In-flight segment jobs are not aborted:
        // they hold their own dir snapshot and finish on their own — the
        // merge reads playlist.m3u8, so a late entry either makes the merge
        // or is harmlessly left behind

        // 收尾统计落盘：末轮的失败/成功计数可能尚未写入（此前只有成功轮
        // 才触发写盘），退出前统一补写 meta 与管理器缓存
        // Final stats flush on exit: the last rounds' counts may not have
        // been persisted yet (only successful rounds used to trigger
        // writes) — backfill meta and the manager cache before leaving
        {
            let session_dir_str = session_dir.to_string_lossy().to_string();
            if let Some(mut meta) = crate::recording::meta::read_meta(session_dir) {
                let mut changed = false;
                if meta.segments_downloaded != Some(total_downloaded) {
                    meta.segments_downloaded = Some(total_downloaded);
                    changed = true;
                }
                if meta.segments_failed != Some(total_failed) {
                    meta.segments_failed = Some(total_failed);
                    changed = true;
                }
                if changed {
                    crate::recording::meta::write_meta(session_dir, &meta);
                }
            }
            self.segment_stats
                .write()
                .insert(session_dir_str, (total_downloaded, total_failed));
        }

        // 缺口终报：total_failed 语义为「累计失败」，即文件里的真实缺口
        // 分片数 / exit report of the real gap count: total_failed is the
        // segments truly missing from the file
        if total_failed > 0 {
            tracing::warn!(
                "{}",
                crate::tl!(
                    "recorder.segmentsLostOnExit",
                    username = username,
                    count = total_failed
                )
            );
        }

        tracing::info!(
            "{}",
            crate::tl!(
                "recorder.finished",
                username = username,
                dir = session_dir.display()
            )
        );
        Ok(())
    }
}

/// 轮询一次播放清单并把新分片派发为后台任务。
/// 返回 `(入队分片数, 滑窗缺口分片数, 下一拍清单预取)`。
///
/// Poll the playlist once and dispatch new segments as background jobs.
/// Returns `(segments enqueued, playlist-window gaps, next-poll prefetch)`.
///
/// `downloaded_sequences` 是跨轮次的"已处理"集合（含失败预标记与滑窗缺口
/// 标记）；`window_track` 追踪媒体序号覆盖端点/编号步长，用于检测滑窗
/// 缺口——媒体序号跳变说明中间条目未进过任何快照就滑出窗口，记为缺口。
///
/// `downloaded_sequences` is the cross-poll "handled" set (including failed
/// and window-gap marks); `window_track` carries the media-sequence coverage
/// edge / numbering stride for gap detection — an ms jump means entries slid
/// out before ANY snapshot saw them.
///
/// `playlist_prefetch_in` 是上一拍在轮询间隙预取的清单快照（见
/// [`PlaylistPrefetch`]）：URL 一致则消费它（预取失败回退实时拉取），
/// 不一致则 abort 后实时拉取。返回值的第三个元素是本拍为下一拍预取的
/// 快照，由调用方跨迭代传递。
///
/// `playlist_prefetch_in` is the playlist snapshot prefetched between polls
/// (see [`PlaylistPrefetch`]): consumed when the URL matches (a failed
/// prefetch falls back to a live fetch), aborted + live-fetched otherwise.
/// The 3rd return element is this poll's snapshot for the next one —
/// threaded across iterations by the caller.
///
/// ## Init segment 缓存策略 / Init segment caching strategy
///
/// 每个 `HlsSegment` 在 `init_url` 字段中携带 EXT-X-MAP 变更点（仅变更处
/// 为 `Some`）。扫描清单时维护"当前生效 init"：变更点立即后台预热
/// （[`prefetch_init`] 与 [`OnceCell`] 去重——多拍/多任务同 URL 只下一
/// 次），任务入队时快照当前生效值，写盘时经 [`cached_init`] 取用——
/// 几乎总是已在缓存中。
///
/// Each `HlsSegment` carries an EXT-X-MAP change point in `init_url`
/// (`Some` only at changes). The playlist scan tracks the "effective
/// init": a change point is prefetched in the background right away
/// ([`prefetch_init`] with [`OnceCell`] dedup — one download per URL
/// across polls/jobs), jobs snapshot the effective value at enqueue and
/// pull the bytes via [`cached_init`] at write time — almost always warm.
#[allow(clippy::too_many_arguments)]
async fn poll_once(
    api: &StripchatApi,
    playlist_url: &str,
    url_prefix: &str,
    mouflon_keys: &MouflonKeys,
    username: &str,
    session_dir: &std::path::Path,
    downloaded_sequences: &mut HashSet<u32>,
    window_track: &mut WindowTrack,
    cur_init_url: &mut Option<String>,
    init_cache: &InitCache,
    seg_permits: &Arc<Semaphore>,
    report_tx: &mpsc::UnboundedSender<SegReport>,
    playlist_prefetch_in: Option<PlaylistPrefetch>,
) -> Result<(usize, u64, Option<PlaylistPrefetch>)> {
    // 消费上一拍在轮询间隙预取的清单快照：URL 一致才能用（清单刷新/
    // 轮换后旧快照作废重建）；预取任务失败/恐慌时回退实时拉取，语义不变。
    // Consume the playlist snapshot prefetched between polls — usable only
    // for the same URL (stale snapshots from before a refresh/rotation are
    // discarded); a failed/panicked prefetch falls back to a live fetch so
    // semantics stay unchanged.
    let playlist = match playlist_prefetch_in {
        Some((url, handle)) if url == playlist_url => match handle.await {
            Ok(Ok(text)) => text,
            _ => api.fetch_playlist(playlist_url).await?,
        },
        Some((_, handle)) => {
            handle.abort();
            api.fetch_playlist(playlist_url).await?
        }
        None => api.fetch_playlist(playlist_url).await?,
    };
    let segments = parse_playlist(&playlist, url_prefix, mouflon_keys)?;

    // 清单滑窗缺口检测——两条路径：
    //
    // ① 媒体序号覆盖（首选）：`#EXT-X-MEDIA-SEQUENCE` 是发布侧"已下发
    // 条目数"的权威计数。历次清单覆盖区间 [ms, ms+count)，新清单 ms 超出
    // 历史右端 = 有条目未进过我们任何快照就滑出窗口——这才是真实缺失。
    // 文件名序号跳号但条目一条不少（源侧重编号）→ 覆盖无缺口 → 不报。
    //
    // ② 文件名序号检测（无该标签时回退）：取清单内相邻文件名序号最小
    // 正差为步长（实测有流 +2 编号），prev+k*step 枚举缺失槽位。
    //
    // Playlist window-gap detection — two paths:
    //
    // ① media-sequence coverage (preferred): EXT-X-MEDIA-SEQUENCE is the
    // publisher's authoritative "entries served" counter. Each playlist
    // covers [ms, ms+count); a new ms beyond the running right edge means
    // entries slid out before ANY of our snapshots — the true loss. A
    // filename-seq jump with full entry coverage = publisher renumbering,
    // not a loss — correctly unreported.
    //
    // ② filename-seq fallback (tag absent): learn the stride from the
    // smallest positive adjacent delta (+2 numbering exists in the wild)
    // and enumerate missing slots at prev+k*step.
    let media_seq: Option<u64> = playlist.lines().find_map(|l| {
        l.strip_prefix("#EXT-X-MEDIA-SEQUENCE:")
            .and_then(|v| v.trim().parse().ok())
    });

    let mut gap_missing: Vec<u32> = Vec::new();
    let mut gap_failures = 0u64;
    if let Some(ms) = media_seq {
        let mut lost = 0u64;
        let mut lost_from = 0u64;
        if let Some(end) = window_track.covered_end {
            lost = ms.saturating_sub(end);
            lost_from = end;
        }
        // 覆盖端点单调前进：取 max(end, ms+count)——窗口回退/重列不算
        // Advance the coverage edge monotonically: max(end, ms+count) —
        // window regressions/re-listings don't count
        window_track.covered_end = Some(
            window_track
                .covered_end
                .unwrap_or(0)
                .max(ms + segments.len() as u64),
        );
        if lost > 0 {
            gap_failures += lost;
            // 滑出的条目其文件名序号未知（从未见过），报告发布侧条目
            // 索引区间 [end, ms) 作为 range / the slid entries' filename
            // seqs are unknowable (never seen) — report the publisher's
            // entry-index range [end, ms) instead
            tracing::warn!(
                "{}",
                crate::tl!(
                    "recorder.playlistGap",
                    username = username,
                    count = lost,
                    range = format!("ms {lost_from}..{ms}")
                )
            );
        }
    } else {
        let mut prev_listed: Option<u32> = None;
        for seg in &segments {
            // 清单内相邻序号差更新步长（只取更小值；大差值多半是真实缺口，
            // 不污染步长）/ shrink the stride with adjacent deltas (larger
            // deltas are likely real gaps — must not corrupt the stride)
            if let Some(p) = prev_listed {
                let d = seg.sequence.saturating_sub(p);
                if d > 0 && window_track.seq_step.is_none_or(|s| d < s) {
                    window_track.seq_step = Some(d);
                }
            }
            prev_listed = Some(seg.sequence);

            // 相对历史最大值的跳变：按步长枚举中间缺失槽位 /
            // jump vs. the running max → enumerate missing slots by stride
            let step = window_track.seq_step.unwrap_or(1).max(1);
            if let Some(prev) = window_track.max_seq_seen
                && seg.sequence > prev + step
            {
                let mut missing = prev + step;
                while missing < seg.sequence {
                    if downloaded_sequences.insert(missing) {
                        gap_missing.push(missing);
                    }
                    missing += step;
                }
            }
            if window_track.max_seq_seen.is_none_or(|m| seg.sequence > m) {
                window_track.max_seq_seen = Some(seg.sequence);
            }
        }
        gap_failures += gap_missing.len() as u64;
        if !gap_missing.is_empty() {
            tracing::warn!(
                "{}",
                crate::tl!(
                    "recorder.playlistGap",
                    username = username,
                    count = gap_missing.len(),
                    range = fmt_seq_ranges(&gap_missing)
                )
            );
        }
    }

    // 派发新分片为后台任务：扫描时同步推进生效 init（含已下载分片的
    // 变更点——它决定后续新分片携带哪个 init），变更点顺手后台预热；
    // 每个新分片入队时快照目录/init/尾条标记并标记为已处理，回报经
    // report_tx 统一送达。
    // Dispatch new segments as background jobs: advance the effective init
    // while scanning (including change points on already-downloaded segs —
    // they decide which init later new segs carry) and warm every change
    // point in the background; each new seg is marked handled and enqueued
    // with a dir/init/tail snapshot, reporting back over report_tx.
    let last_idx = segments.len().saturating_sub(1);
    let mut enqueued = 0usize;
    for (idx, seg) in segments.iter().enumerate() {
        if let Some(new_init) = &seg.init_url {
            *cur_init_url = Some(new_init.clone());
            prefetch_init(init_cache, api, new_init);
        }
        if downloaded_sequences.contains(&seg.sequence) {
            continue;
        }
        // 立即标记为"已处理"：任务在飞期间清单再次列出它不会重复派发；
        // 终判（写盘/逐出/失败）都在任务内完成，标记一旦设置便保持
        // Mark "handled" right away: re-listings while the job is in flight
        // won't re-dispatch; terminal verdicts (written/evicted/failed) are
        // all reached inside the job — a mark, once set, stays
        downloaded_sequences.insert(seg.sequence);
        enqueued += 1;
        spawn_segment_job(
            api,
            username,
            seg,
            idx == last_idx,
            cur_init_url.clone(),
            session_dir,
            init_cache,
            seg_permits,
            report_tx,
        );
    }

    // 返回前确保带一个下一拍清单预取：与轮间等待并行，快照密度由
    // 「轮询间隔 + 清单拉取耗时」决定，分片下载快慢不再参与。
    // Always ride a next-poll playlist prefetch out the door: it overlaps
    // the inter-poll wait, so snapshot density is governed by "poll
    // interval + playlist fetch time" — segment download speed no longer
    // participates.
    Ok((
        enqueued,
        gap_failures,
        Some(spawn_playlist_prefetch(api, playlist_url)),
    ))
}

/// 派发一个分片任务：许可门控在任务内部等待（不阻塞轮询循环），任务
/// 完成后经 `report_tx` 回报。
/// Dispatch one segment job: the permit wait happens inside the task
/// (never blocks the poll loop); the outcome is reported over `report_tx`.
#[allow(clippy::too_many_arguments)]
fn spawn_segment_job(
    api: &StripchatApi,
    username: &str,
    seg: &HlsSegment,
    is_tail: bool,
    init_url: Option<String>,
    session_dir: &std::path::Path,
    init_cache: &InitCache,
    seg_permits: &Arc<Semaphore>,
    report_tx: &mpsc::UnboundedSender<SegReport>,
) {
    let job = PendingSeg {
        seq: seg.sequence,
        url: seg.url.clone(),
        init_url,
        duration_secs: seg.duration_secs,
        is_tail,
        session_dir: session_dir.to_path_buf(),
    };
    let api = api.clone();
    let username = username.to_string();
    let init_cache = Arc::clone(init_cache);
    let permits = Arc::clone(seg_permits);
    let report_tx = report_tx.clone();
    tokio::spawn(async move {
        // 许可在任务内等待：信号量饱和时任务排队，轮询循环不被拖住
        // The permit is awaited inside the task: a saturated semaphore
        // queues jobs without stalling the poll loop
        let Ok(_permit) = permits.acquire_owned().await else {
            return;
        };
        let report = segment_job(&api, &username, job, &init_cache).await;
        let _ = report_tx.send(report);
    });
}

/// 单个分片任务本体：带退避重试地下载 → 取 init（共享缓存，写盘时才
/// 取用，预热通常已就绪）→ 头拼接 → ffmpeg 转 TS → 按序号有序写入
/// m3u8 → 回报。尾条 404 在任务内继续退避重试（发布延迟 ~2s 通常落
/// 在重试窗口内），所有终判都在任务内完成，主循环只负责计数。
/// One segment job: download with backoff retries → fetch the init (shared
/// cache, pulled only at write time — the prefetch is usually warm by
/// then) → header concat → ffmpeg convert → sorted m3u8 insert → report.
/// A tail 404 keeps backing off inside the job (the ~2s publish lag
/// usually lands inside the retry window); every terminal verdict is
/// reached in-job — the main loop only counts.
async fn segment_job(
    api: &StripchatApi,
    username: &str,
    job: PendingSeg,
    init_cache: &InitCache,
) -> SegReport {
    // <=1000 字节的响应体不可能是有效分片（多为占位/残体页）：归一化为
    // 失败走统一重试路径。（init 分片本就很小，不走此路径，不受影响）
    // A body <=1000 bytes can't be a real segment (usually a stub or
    // truncated page): normalize into a failure on the unified retry path.
    // (init segments are legitimately tiny and don't take this path)
    let mut data: Option<Vec<u8>> = None;
    for attempt in 0..=RETRY_DELAYS_MS.len() as u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(
                RETRY_DELAYS_MS[(attempt - 1) as usize],
            ))
            .await;
        }
        match api.download_segment(&job.url).await {
            Ok(d) if d.len() > 1000 => {
                data = Some(d);
                break;
            }
            Ok(_) => {
                tracing::warn!(
                    "{}",
                    crate::tl!(
                        "recorder.segmentRetryTooSmall",
                        seq = job.seq,
                        username = username,
                        attempt = attempt + 1
                    )
                );
            }
            // 清单尾条 404 = 多半尚未发布完（发布延迟 ~2s 落在退避窗口
            // 内即可拿到）：按未就绪继续重试而非判逐出——用尽重试仍 404
            // 则落到下一条按逐出计数（此时它确已滑出/逐出，计数才真实）。
            // A 404 on the tail entry = usually not fully published yet (a
            // ~2s publish lag lands inside the backoff window): keep
            // retrying as not-ready instead of calling it evicted — if it
            // still 404s after every retry it falls into the arm below and
            // is counted as evicted (by then it truly slid out / was
            // evicted, so the count is honest).
            Err(AppError::HttpNotFound(_))
                if job.is_tail && (attempt as usize) < RETRY_DELAYS_MS.len() =>
            {
                tracing::debug!(
                    "{}",
                    crate::tl!(
                        "recorder.tailSegmentNotPublished",
                        username = username,
                        seq = job.seq
                    )
                );
            }
            // 404 终判 = 已被 CDN 边缘逐出（live 对象 TTL 极短；尾条重试
            // 耗尽同样落此——持久 404 就是真实缺失）：缺失计数、不重试。
            // Terminal 404 = already evicted at the CDN edge (live objects
            // have tiny TTLs; an exhausted tail lands here too — a persistent
            // 404 IS a real loss): counted, no retry.
            Err(AppError::HttpNotFound(_)) => {
                tracing::warn!(
                    "{}",
                    crate::tl!(
                        "recorder.segmentEvicted",
                        seq = job.seq,
                        username = username
                    )
                );
                return SegReport::Evicted;
            }
            Err(e) => {
                if attempt == 0 {
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "recorder.segmentDownloadFailed",
                            seq = job.seq,
                            error = e
                        )
                    );
                } else {
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "recorder.segmentRetryAttemptFailed",
                            seq = job.seq,
                            username = username,
                            attempt = attempt,
                            max = RETRY_DELAYS_MS.len(),
                            error = e
                        )
                    );
                }
            }
        }
    }
    let Some(data) = data else {
        tracing::error!(
            "{}",
            crate::tl!(
                "recorder.segmentPermanentFailed",
                seq = job.seq,
                username = username,
                max = RETRY_DELAYS_MS.len()
            )
        );
        return SegReport::DownloadFailed;
    };

    // 取 init：共享按 URL 去重的缓存——变更点已在入队时预热，这里几乎
    // 总能直接命中；取不到（init 真挂了）按终判失败计数。
    // Fetch the init: the shared per-URL cache — the change point was
    // warmed at enqueue so this almost always hits; a real init failure
    // counts as a terminal loss.
    let init_data = match &job.init_url {
        Some(url) => match cached_init(init_cache, api, url).await {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::error!(
                    "{}",
                    crate::tl!("recorder.initSegmentFailed", error = e)
                );
                return SegReport::DownloadFailed;
            }
        },
        None => None,
    };

    let fmp4: Vec<u8> = match init_data.as_deref() {
        Some(h) => {
            let mut v = Vec::with_capacity(h.len() + data.len());
            v.extend_from_slice(h);
            v.extend_from_slice(&data);
            v
        }
        None => data,
    };

    let ts_path = job
        .session_dir
        .join(format!("{}_segment{:06}.ts", username, job.seq));

    match convert_to_ts(fmp4, &ts_path).await {
        Ok(_) => {
            // 按 seq 有序插入而非末尾追加：并发任务完成顺序不定，有序插入
            // 保证条目总落在真实时间位置，m3u8 顺序不倒退
            // Insert in seq order instead of appending: concurrent jobs
            // finish out of order, and sorted insertion lands every entry at
            // its true chronological position — the m3u8 never regresses
            insert_to_m3u8_sorted(
                ts_path.parent().unwrap_or(&ts_path),
                &ts_path,
                job.seq,
                job.duration_secs,
            );
            SegReport::Written {
                dir: job.session_dir,
                duration_secs: job.duration_secs,
            }
        }
        Err(e) => {
            // ffmpeg 失败（本地问题，重试 CDN 无意义），直接终判
            // ffmpeg failure is local; retrying the CDN is pointless —
            // terminal
            tracing::error!(
                "{}",
                crate::tl!(
                    "recorder.ffmpegConvertFailed",
                    seq = job.seq,
                    error = e
                )
            );
            SegReport::DownloadFailed
        }
    }
}

/// 取/建某个 init URL 的共享 [`OnceCell`]（超出容量先清空再建——在飞
/// 任务持有自己的 Arc，不受影响）。
/// Get-or-create the shared [`OnceCell`] for an init URL (the map is
/// cleared past capacity — in-flight jobs hold their own Arc).
fn init_cell(cache: &InitCache, url: &str) -> Arc<OnceCell<Vec<u8>>> {
    let mut map = cache.lock();
    if map.len() >= INIT_CACHE_CAP && !map.contains_key(url) {
        map.clear();
    }
    map.entry(url.to_string()).or_default().clone()
}

/// 后台预热一个 init URL：未初始化才 spawn（已有在飞/已完成下载时
/// OnceCell 自身去重，多任务同 URL 只实下一次）。
/// Warm an init URL in the background: only spawned while uninitialized
/// (the OnceCell dedupes in-flight/completed downloads — many jobs on one
/// URL still trigger a single fetch).
fn prefetch_init(cache: &InitCache, api: &StripchatApi, url: &str) {
    let cell = init_cell(cache, url);
    if cell.initialized() {
        return;
    }
    let api = api.clone();
    let url_owned = url.to_string();
    tokio::spawn(async move {
        let _ = cell
            .get_or_try_init(|| async { api.download_segment(&url_owned).await })
            .await;
    });
}

/// 经共享缓存取 init 字节：同 URL 的并发调用共享同一次下载，失败不
/// 缓存（下一个访问者自动重试）。
/// Fetch init bytes via the shared cache: concurrent callers on the same
/// URL share one download; failures are not cached (the next accessor
/// retries).
async fn cached_init(
    cache: &InitCache,
    api: &StripchatApi,
    url: &str,
) -> Result<Vec<u8>> {
    let cell = init_cell(cache, url);
    cell.get_or_try_init(|| async { api.download_segment(url).await })
        .await
        .cloned()
}

/// 预取中的播放清单任务：(目标清单 URL, JoinHandle<清单文本>)——URL 一并
/// 记录是因为消费时清单可能已被刷新换 URL，旧快照必须作废。
/// In-flight playlist prefetch: (target playlist URL, JoinHandle<text>) — the
/// URL travels with it because the playlist may have been refreshed to a new
/// URL by consume time, in which case the stale snapshot must be discarded.
type PlaylistPrefetch = (String, tokio::task::JoinHandle<Result<String>>);

/// 清单窗口追踪状态（跨轮持久）/ Cross-round playlist-window tracking state
#[derive(Default)]
struct WindowTrack {
    /// 跨轮见过的最大文件名序号（无 MEDIA-SEQUENCE 时的回退检测）
    /// Max filename seq seen across rounds (fallback detection when the
    /// playlist lacks MEDIA-SEQUENCE)
    max_seq_seen: Option<u32>,
    /// 从清单内相邻序号学习到的文件名编号步长（回退路径用）
    /// Filename numbering stride learned from adjacent seqs (fallback path)
    seq_step: Option<u32>,
    /// 媒体序号覆盖端点：历次清单已覆盖条目区间 [ms, ms+count) 的最大右端。
    /// 新清单的 ms 超出该端点 = 有条目未见即滑出（权威缺口计数）
    /// Media-sequence coverage end: the max right edge of covered entry
    /// ranges [ms, ms+count) across playlists. A new playlist's ms beyond
    /// it = entries slid out unseen (the authoritative gap count)
    covered_end: Option<u64>,
}

/// 后台预取一次播放清单（与轮间等待并行）/ Spawn a background playlist
/// fetch (parallel to the inter-poll wait).
fn spawn_playlist_prefetch(api: &StripchatApi, url: &str) -> PlaylistPrefetch {
    let api = api.clone();
    let url_owned = url.to_string();
    let fetch_url = url_owned.clone();
    let handle = tokio::spawn(async move { api.fetch_playlist(&fetch_url).await });
    (url_owned, handle)
}

/// 把一串升序序号压缩成区间字符串，用于缺口日志。等差连续段合并：
/// +1 记作 `a-b`，其他步长记作 `a-b(+N)`（如 `448-452(+2)`），孤立的记 `a`。
/// Compress an ascending seq list into ranges for gap logs. Constant-stride
/// runs merge as `a-b` for +1 or `a-b(+N)` for stride N (e.g. `448-452(+2)`);
/// a lone seq renders as `a`.
fn fmt_seq_ranges(seqs: &[u32]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < seqs.len() {
        let start = seqs[i];
        // 本段步长由首个差值决定；后续差值相同则并入同一区间 /
        // the run's stride is fixed by the first delta; equal deltas join it
        let run_step = if i + 1 < seqs.len() {
            seqs[i + 1] - start
        } else {
            0
        };
        let mut end = start;
        while i + 1 < seqs.len() && seqs[i + 1] - end == run_step {
            i += 1;
            end = seqs[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if end == start {
            out.push_str(&start.to_string());
        } else if run_step == 1 {
            out.push_str(&format!("{start}-{end}"));
        } else {
            out.push_str(&format!("{start}-{end}(+{run_step})"));
        }
        i += 1;
    }
    out
}
