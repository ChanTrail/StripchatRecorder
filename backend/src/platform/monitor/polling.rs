//! 轮询循环与单主播轮询子模块：监控主循环、并发轮询与自动录制触发。
//! Polling submodule: monitor loop, concurrent per-streamer polling and
//! auto-recording triggers.

use super::{
    POLL_NETWORK_RETRY_DELAYS, PREVIEW_MAX_ATTEMPTS_PER_DAY, PreviewRefresh, StatusMonitor,
    StreamerStatus, is_retryable_network_error, schedule_poll_interval,
};
use crate::config::app_state::StreamerData;
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::Result;
use crate::core::notifications::NotificationLevel;
use crate::platform::stripchat::{StreamInfo, StripchatApi};
use std::sync::Arc;
use tokio::sync::mpsc;

impl StatusMonitor {
    /// 查询主播直播状态；遇到网络类错误（见 [`is_retryable_network_error`]）时在本轮内按
    /// [`POLL_NETWORK_RETRY_DELAYS`] 短间隔重试，其他错误直接返回。
    ///
    /// Query a streamer's live status; on a network error (see [`is_retryable_network_error`])
    /// retry within this round after the short [`POLL_NETWORK_RETRY_DELAYS`]; other errors are
    /// returned as is.
    ///
    /// 不在这里顺带请求离线预览图（`fetch_offline_preview` 固定为 false）：它只在主播离线时
    /// 才会请求，而离线预览图不论在线与否都要刷新，由 `poll_streamer` 单独请求。
    /// The offline preview isn't requested here (`fetch_offline_preview` is always false): that
    /// path only fetches it while offline, but the offline preview must be refreshed regardless of
    /// online status, so `poll_streamer` requests it separately.
    async fn get_stream_info_with_retry(
        api: &StripchatApi,
        username: &str,
        fetch_playlist: bool,
        model_id: Option<i64>,
    ) -> Result<StreamInfo> {
        let mut attempt = 0usize;
        loop {
            match api
                .get_stream_info(username, fetch_playlist, model_id, false)
                .await
            {
                Err(e)
                    if attempt < POLL_NETWORK_RETRY_DELAYS.len()
                        && is_retryable_network_error(&e) =>
                {
                    let delay = POLL_NETWORK_RETRY_DELAYS[attempt];
                    attempt += 1;
                    tracing::info!(
                        "{}",
                        crate::tl!(
                            "monitor.pollRetrying",
                            username = username,
                            attempt = attempt,
                            max = POLL_NETWORK_RETRY_DELAYS.len(),
                            secs = delay.as_secs(),
                            // 带上底层原因（连接被拒/TLS 握手/DNS/连接被关闭等）
                            // Include the underlying cause (refused/TLS handshake/DNS/closed, …)
                            error = e.detailed()
                        )
                    );
                    tokio::time::sleep(delay).await;
                }
                result => return result,
            }
        }
    }

    /// 监控主循环：延迟 10 秒后轮询一次，然后按配置的间隔周期性轮询。
    /// Monitor main loop: wait 10 s then poll once, then poll periodically at the configured interval.
    pub(super) async fn monitor_loop(
        self: Arc<Self>,
        emitter: Arc<dyn Emitter>,
        mut restart_rx: mpsc::Receiver<()>,
    ) {
        // 延迟 10 秒等网络稳定后再发起首次轮询，避免启动瞬间网络不稳导致报错
        // Wait 10 s for network to stabilize before the first poll, avoiding errors on startup
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        self.poll_all_with_emitter(&emitter).await;

        loop {
            let poll_interval =
                tokio::time::Duration::from_secs(self.state.get_settings().poll_interval_secs);

            tokio::select! {
                _ = restart_rx.recv() => {
                    // poll_interval_secs 已变更，立即以新间隔重新开始计时（不立即轮询）
                    // poll_interval_secs changed; restart timer with new interval (no immediate poll)
                    tracing::info!("{}", crate::tl!("monitor.pollIntervalRestarted"));
                    continue;
                }
                _ = tokio::time::sleep(poll_interval) => {
                    self.poll_all_with_emitter(&emitter).await;
                }
            }
        }
    }

    /// 并发轮询所有追踪主播的状态（通用版本）。
    /// Concurrently poll the status of all tracked streamers (generic version).
    pub async fn poll_all_with_emitter(self: &Arc<Self>, emitter: &Arc<dyn Emitter>) {
        let settings = self.state.get_settings();
        let streamers = self.state.get_streamers();

        if streamers.is_empty() {
            return;
        }

        let api = match self.build_api(emitter) {
            Some(a) => Arc::new(a),
            None => return,
        };

        // schedule 自适应间隔的上限（固定 200 s），下限为用户设置的 base
        // Upper bound for schedule-adaptive interval (fixed 200 s); lower bound = user base
        const SCHEDULE_MAX_SECS: u64 = 200;
        let base_secs = settings.poll_interval_secs;
        let now = std::time::Instant::now();

        // 限制同时进行中的 API 请求数，避免主播过多时同时打出大量请求触发限流。
        // 5 路并发足以在正常延迟下及时完成一轮轮询，同时对 SC 服务器友好。
        //
        // Limit concurrent in-flight API requests to prevent rate-limiting when
        // there are many tracked streamers. 5 concurrent requests is enough to
        // finish a round quickly under normal latency while remaining polite to SC.
        const POLL_CONCURRENCY: usize = 5;
        let sem = Arc::new(tokio::sync::Semaphore::new(POLL_CONCURRENCY));

        let active_streamers: Vec<_> = streamers
            .into_iter()
            // 过滤已确认失效的主播（永久跳过）
            // Filter permanently-dead streamers
            .filter(|s| !self.dead_streamers.read().contains(&s.username))
            // Schedule 自适应间隔检查：
            // 若距上次轮询尚未到本次应有的间隔，且主播不在录制中，则跳过本轮。
            // 录制中的主播始终参与轮询（检测断流）。
            // 首次轮询（next_poll_at 中无记录）无条件执行。
            //
            // Schedule-adaptive interval check:
            // Skip if the per-streamer interval has not elapsed yet and the
            // streamer is not currently recording. Recording streamers always
            // participate (to detect stream drops). First-ever poll (no entry
            // in next_poll_at) always executes.
            .filter(|s| {
                if self.recorder.is_recording(&s.username) {
                    return true;
                }
                match self.next_poll_at.read().get(&s.username) {
                    Some(&next) => now >= next,
                    None => true, // 从未轮询过，立即执行
                }
            })
            .collect();

        // 每个任务直接返回本轮新确认失效的主播名，等待任务结束时收集。
        // 之前用容量 16 的 channel 收集、且要等所有任务结束后才读取：同一轮新失效的主播
        // 超过 16 个时，第 17 个任务会一直阻塞在 send 上，本轮永远结束不了，轮询整体停摆。
        // 改为从 JoinHandle 取返回值，没有容量上限，也不会互相等待。
        //
        // Each task returns the username newly confirmed dead this round, collected when the
        // task is awaited. Previously a capacity-16 channel was used and only read after all
        // tasks finished: with more than 16 newly-dead streamers in one round, the 17th task
        // blocked on send forever, the round never finished and polling stopped entirely.
        // Taking the value from the JoinHandle has no capacity limit and no mutual waiting.
        let tasks: Vec<_> = active_streamers
            .into_iter()
            .map(|streamer| {
                let api = Arc::clone(&api);
                let monitor = Arc::clone(self);
                let emitter = Arc::clone(emitter);
                let auto_record_global = settings.auto_record;
                let sem = Arc::clone(&sem);

                tokio::spawn(async move {
                    // 在发起 API 请求前获取信号量许可，限制并发数
                    // Acquire a semaphore permit before making the API request to cap concurrency
                    let _permit = sem.acquire().await;
                    monitor
                        .poll_streamer(
                            &api,
                            streamer,
                            &emitter,
                            auto_record_global,
                            base_secs,
                            SCHEDULE_MAX_SECS,
                        )
                        .await
                })
            })
            .collect();

        // 收集本轮所有新死亡主播，合并成一条通知
        // Collect all newly-dead streamers and emit a single merged notification
        let mut newly_dead: Vec<String> = Vec::new();
        for t in tasks {
            if let Ok(Some(username)) = t.await {
                newly_dead.push(username);
            }
        }

        if !newly_dead.is_empty() {
            newly_dead.sort();
            use std::collections::HashMap;
            let (message, key, args) = if newly_dead.len() == 1 {
                let mut a = HashMap::new();
                a.insert("username".to_string(), serde_json::json!(&newly_dead[0]));
                (
                    format!(
                        "Streamer {} cannot be found by username or internal ID (possibly renamed, deleted, or banned). Future polls will be skipped.",
                        newly_dead[0]
                    ),
                    "notifications.backend.streamerDeadOne",
                    a,
                )
            } else {
                let usernames = newly_dead.join(", ");
                let mut a = HashMap::new();
                a.insert("count".to_string(), serde_json::json!(newly_dead.len()));
                a.insert("usernames".to_string(), serde_json::json!(usernames));
                (
                    format!(
                        "{} streamers cannot be found (possibly renamed, deleted, or banned). Future polls will be skipped: {}",
                        newly_dead.len(),
                        newly_dead.join(", ")
                    ),
                    "notifications.backend.streamerDeadMany",
                    a,
                )
            };
            self.state.notification_store.emit_i18n_with_action(
                emitter,
                NotificationLevel::Warning,
                "streamer_dead",
                message,
                key,
                Some(args),
                Some(crate::core::notifications::NotificationAction {
                    action_type: "remove_streamers".to_string(),
                    targets: newly_dead,
                }),
            );
        }
    }

    /// 轮询单个主播的状态，更新缓存，并根据状态变化触发自动录制逻辑。
    /// 轮询完成后根据 schedule 计算下次可轮询时刻并写入 `next_poll_at`。
    /// 若该主播本轮被确认失效（首次），返回其用户名；否则返回 None。
    ///
    /// Poll a single streamer's status, update the cache, and trigger auto-recording logic.
    /// After polling, computes the next eligible poll time from the schedule and stores it
    /// in `next_poll_at`. Returns the username if newly confirmed dead; otherwise None.
    async fn poll_streamer(
        self: &Arc<Self>,
        api: &StripchatApi,
        streamer: StreamerData,
        emitter: &Arc<dyn Emitter>,
        auto_record_global: bool,
        base_secs: u64,
        max_secs: u64,
    ) -> Option<String> {
        let mut username = streamer.username.clone();

        let is_recording = self.recorder.is_recording(&username);
        let (was_online, was_recording) = self
            .statuses
            .read()
            .get(&username)
            .map(|s| (s.is_online, s.is_recording))
            .unwrap_or((false, false));

        if !self.statuses.read().contains_key(&username) {
            self.statuses
                .write()
                .entry(username.clone())
                .or_insert_with(|| StreamerStatus {
                    username: username.clone(),
                    is_online: false,
                    is_recording,
                    is_recordable: false,
                    status: String::new(),
                    thumbnail_url: None,
                    playlist_url: None,
                    playlist_resolution: 0,
                    playlist_prefers_higher: false,
                    playlist_codec: String::new(),
                });
        }

        // 仅当确实可能触发自动录制时才拉取 playlist URL：
        // 未在录制 + 该主播启用了自动录制 + 全局自动录制也开启。
        // 手动录制按钮不依赖此处的缓存 URL，点击时会即时拉取。
        //
        // Only fetch the playlist URL when auto-recording could actually trigger:
        // not currently recording + this streamer has auto-record on + global auto-record is on.
        // The manual-record button does not rely on this cached URL; it fetches on demand.
        let need_playlist = !is_recording && streamer.auto_record && auto_record_global;
        let info = match Self::get_stream_info_with_retry(
            api,
            &username,
            need_playlist,
            streamer.model_id,
        )
        .await
        {
            Ok(i) => i,
            Err(crate::core::error::AppError::UserNotFound(_)) => {
                // 用户名查不到，且 model_id 反查也失败（get_stream_info 已处理改名回退）
                // Username not found and model_id reverse-lookup also failed
                // (get_stream_info already handles rename fallback).
                let already_dead = self.dead_streamers.read().contains(&username);
                if !already_dead {
                    // 写入内存 dead set + 持久化到 streamers.json
                    // Add to in-memory dead set + persist to streamers.json
                    self.dead_streamers.write().insert(username.clone());
                    self.state.mark_streamer_dead(&username);
                    tracing::warn!(
                        "{}",
                        crate::tl!("monitor.streamerDead", username = username)
                    );
                    // 返回用户名，由 poll_all_with_emitter 统一合并通知
                    // Return username so poll_all_with_emitter can merge notifications
                    return Some(username);
                }
                return None;
            }
            Err(e) => {
                // 带上底层原因，便于判断失败类型 / Include the underlying cause to tell failure types apart
                tracing::error!(
                    "{}",
                    crate::tl!(
                        "monitor.pollFailed",
                        username = username,
                        error = e.detailed()
                    )
                );
                // 网络/API 错误时仍以 base_secs 重试，不拉长间隔
                // On network/API error, retry at base_secs — don't extend the interval
                let next = std::time::Instant::now() + std::time::Duration::from_secs(base_secs);
                self.next_poll_at.write().insert(username.clone(), next);
                return None;
            }
        };

        // 首次成功查询且此前尚无 model_id（升级前的旧数据）时，回填 model_id，
        // 便于日后改名反查有据可依。
        // On first successful lookup with no model_id yet (pre-upgrade data), backfill
        // it so future rename lookups have something to fall back on.
        if streamer.model_id.is_none()
            && let Some(mid) = info.model_id
        {
            self.state.backfill_model_id(&username, mid);
        }

        if let Some(ref new_username) = info.renamed_to
            && !is_recording
        {
            match self.state.rename_streamer(&username, new_username) {
                Ok(()) => {
                    tracing::info!(
                        "{}",
                        crate::tl!(
                            "monitor.streamerRenamed",
                            oldUsername = username,
                            newUsername = new_username
                        )
                    );
                    // 重新绑定 statuses 缓存的 key，避免旧 key 下的缓存永久残留。
                    //
                    // 注意：必须先将 remove 结果存到局部变量再做 insert，不能把两个
                    // write() 调用写在同一个 `if let` 的条件和 body 里——条件表达式
                    // 产生的临时值（这里是第一个 write() 的锁守卫）生命周期会延续到
                    // 整个 if let 语句结束，包括 body，届时 body 里的第二个 write()
                    // 会试图重新获取同一把已持有的锁，导致自死锁。
                    //
                    // Must store the `remove` result in a local first, then `insert` —
                    // can't have both `write()` calls inside the same `if let`'s
                    // condition and body: the temporary produced in the condition
                    // (the first `write()`'s lock guard) lives through the entire
                    // `if let` including the body, causing a self-deadlock when the
                    // body tries to acquire the same lock again.
                    let old_status = self.statuses.write().remove(&username);
                    if let Some(old_status) = old_status {
                        self.statuses
                            .write()
                            .insert(new_username.clone(), old_status);
                    }
                    emitter.emit(
                        "streamer-renamed",
                        &serde_json::json!({ "old_username": username, "new_username": new_username }),
                    );
                    username = new_username.clone();
                }
                Err(e) => {
                    // 新用户名已存在于追踪列表中，放弃改名，本轮按旧用户名继续处理。
                    // New username already tracked; abandon rename and keep old username for this round.
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "monitor.streamerRenameSkipped",
                            username = username,
                            error = e
                        )
                    );
                }
            }
        }

        // 离线预览图随轮询刷新，与在线状态无关：在线时前端展示实时截图，但离线预览图作为
        // 兜底图片也要保持最新（与原来的单独刷新任务一致）。该主播今天（本地日期）还没取到时
        // 请求一次 cam 接口，取到后写入缓存（未变化时不写盘）；取不到时保留原有缓存，下一轮
        // 再试，每天最多 PREVIEW_MAX_ATTEMPTS_PER_DAY 次。放在构建状态之前，离线时本轮就能用上
        // 新地址。仍持有轮询并发名额，不会突破 5 路并发上限。
        //
        // Refresh the offline preview as part of polling, regardless of online status: while
        // online the frontend shows the live snapshot, but the offline preview as the fallback
        // image must stay current too (same as the former separate refresh task). When this
        // streamer has no URL yet today (local date), request the cam endpoint once and cache the
        // result (nothing is written to disk when unchanged); on failure keep the existing cache
        // and retry next round, at most PREVIEW_MAX_ATTEMPTS_PER_DAY times a day. Done before
        // building the status so an offline streamer uses the new URL this round. Still holds the
        // poll concurrency slot, so the 5-way cap is respected.
        let today = chrono::Local::now().date_naive();
        let mut fresh_preview: Option<String> = None;
        if let Some(model_id) = info.model_id.or(streamer.model_id)
            && PreviewRefresh::needed(self.preview_refresh.read().get(&username), today)
        {
            fresh_preview = api.get_cam_preview_url(&username, model_id).await;
            let progress = self.preview_refresh.read().get(&username).copied();
            let progress = PreviewRefresh::record(progress, today, fresh_preview.is_some());
            self.preview_refresh
                .write()
                .insert(username.clone(), progress);
            match &fresh_preview {
                Some(url) => {
                    tracing::debug!(
                        "{}",
                        crate::tl!("monitor.previewRefreshed", username = username, url = url)
                    );
                    self.state
                        .set_cached_preview_url(&username, Some(url.clone()));
                }
                None => tracing::debug!(
                    "{}",
                    crate::tl!(
                        "monitor.previewRefreshFailed",
                        username = username,
                        attempt = progress.attempts,
                        max = PREVIEW_MAX_ATTEMPTS_PER_DAY
                    )
                ),
            }
        }

        let status = StreamerStatus {
            username: username.clone(),
            is_online: info.is_online,
            is_recording,
            // 直接用 API 返回的 is_recordable（is_live && public），不依赖是否拉取了 playlist URL。
            // 录制中时保留缓存值（正常情况下此时 API 仍返回 true；保留缓存仅作额外防护，
            // 避免录制过程中状态短暂抖动导致按钮被错误禁用）。
            //
            // Use is_recordable directly from the API response (is_live && public),
            // independent of whether a playlist URL was fetched.
            // While recording, preserve the cached value as an extra guard against
            // transient status flicker that could incorrectly disable the button.
            is_recordable: if is_recording {
                self.statuses
                    .read()
                    .get(&username)
                    .map(|s| s.is_recordable)
                    .unwrap_or(info.is_recordable)
            } else {
                info.is_recordable
            },
            status: info.status.clone(),
            // 离线时 API 不返回缩略图，用本轮刚刷新的离线预览图，没有时用 StreamerData 中
            // 持久化的缓存值兜底，保证前端始终有预览图可显示。
            // 在线时直接用 API 返回的直播缩略图，不使用缓存。
            //
            // When offline the API returns no thumbnail; use the offline preview refreshed this
            // round, falling back to the cached value from StreamerData, so the frontend always
            // has a preview. When online, use the live thumbnail from the API directly.
            thumbnail_url: if info.is_online {
                info.thumbnail_url.clone()
            } else {
                info.thumbnail_url
                    .clone()
                    .or_else(|| fresh_preview.clone())
                    .or_else(|| streamer.cached_preview_url.clone())
            },
            playlist_url: info.playlist_url.clone(),
            playlist_resolution: api.preferred_resolution(),
            playlist_prefers_higher: api.prefers_higher_resolution(),
            playlist_codec: api.preferred_codec().to_string(),
        };

        emitter.emit("status-update", &status);

        self.statuses.write().insert(username.clone(), status);

        let stream_no_longer_recordable = is_recording && !info.is_recordable;
        if stream_no_longer_recordable {
            tracing::info!(
                "{}",
                crate::tl!(
                    "monitor.streamNotRecordable",
                    username = username,
                    isOnline = info.is_online,
                    isRecordable = info.is_recordable,
                    status = info.status
                )
            );
            let _ = self.recorder.stop_recording_auto(&username).await;
        }

        let recording_dropped = was_recording && !is_recording && info.is_online;
        let just_came_online = info.is_online && !was_online;
        let naturally_stopped = self.recorder.naturally_stopped.write().remove(&username);
        let should_be_recording =
            info.is_recordable && !is_recording && streamer.auto_record && auto_record_global;
        if (just_came_online || recording_dropped || naturally_stopped || should_be_recording)
            && streamer.auto_record
            && auto_record_global
            && !is_recording
            && let Some(ref playlist_url) = info.playlist_url
            && {
                let settings = self.state.get_settings();
                // 分辨率与编码设置已通过 build_api 传入 api，此处校验缓存 playlist 是否仍与当前设置匹配
                // Resolution and codec were passed into api via build_api; verify the cached playlist still matches current settings
                self.statuses
                    .read()
                    .get(&username)
                    .is_none_or(|s| s.playlist_matches(&settings))
            }
        {
            tracing::info!(
                "{}",
                crate::tl!(
                    "monitor.autoStart",
                    username = username,
                    justOnline = just_came_online,
                    dropped = recording_dropped,
                    naturalStop = naturally_stopped,
                    shouldBe = should_be_recording
                )
            );
            let _ = self
                .recorder
                .start_recording_with_emitter(&username, playlist_url, Arc::clone(emitter))
                .await;
        }

        // 计算下次应轮询该主播的最早时刻，写入 next_poll_at。
        // 若当前处于录制中，始终用 base_secs（不拉长间隔，保持对断流的快速响应）。
        // 否则由 schedule 活跃度决定间隔（0%→200 s, 80%→base, >80%→base）。
        //
        // Compute and store the earliest next-poll time for this streamer.
        // Use base_secs when recording (fast stream-drop detection).
        // Otherwise, derive the interval from the schedule activity.
        {
            let interval_secs = if self.recorder.is_recording(&username) {
                base_secs
            } else {
                schedule_poll_interval(streamer.schedule.as_deref(), base_secs, max_secs)
            };
            let next = std::time::Instant::now() + std::time::Duration::from_secs(interval_secs);
            self.next_poll_at.write().insert(username.clone(), next);
        }

        None
    }
}
