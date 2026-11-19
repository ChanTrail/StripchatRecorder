//! 录制会话生命周期子模块：启动、停止与文件轮转。
//! Recording-session lifecycle submodule: start, stop and file rotation.

use super::{HandoffGuard, RecorderManager, RecordingSession};
use crate::core::emitter::{Emitter, EmitterExt};
use crate::core::error::{AppError, Result};
use chrono::Local;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

impl RecorderManager {
    /// 启动录制（通用版本，接受任意 emitter）。
    /// 创建会话目录，启动异步录制循环，录制完成后自动合并分片并触发后处理。
    ///
    /// Start recording (generic version, accepts any emitter).
    /// Creates the session directory, starts the async recording loop,
    /// and automatically merges segments and triggers post-processing after completion.
    ///
    /// # 返回值 / Returns
    /// 录制会话目录路径 / Recording session directory path
    pub async fn start_recording_with_emitter(
        self: &Arc<Self>,
        username: &str,
        playlist_url: &str,
        emitter: Arc<dyn Emitter>,
    ) -> Result<String> {
        if self.is_recording(username) {
            return Err(AppError::AlreadyRecording(username.to_string()));
        }

        let settings = self.state.get_settings();
        // 录制不设并发上限：每路录制是网络 IO 密集、CPU 极低（转码是占空比 ~2% 的
        // `-c copy` 突发），实测对系统压力可忽略；并发压力统一由后处理 AIMD 承担。
        // Recordings are uncapped: each is network-IO-bound with ~2% duty-cycle
        // `-c copy` transcode bursts (measured negligible); concurrency pressure is
        // absorbed by the post-processing AIMD instead.

        // 分片输出目录：直接使用 output_dir，不再有独立的合并视频目录
        // Segment output directory: use output_dir directly; no separate merged-video directory
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let session_dir = PathBuf::from(&settings.output_dir)
            .join(username)
            .join(format!("{}_{}", username, timestamp));
        fs::create_dir_all(&session_dir)?;

        let (stop_tx, stop_rx) = mpsc::channel(1);

        let session = RecordingSession {
            dir_path: session_dir.clone(),
            started_at: chrono::Local::now(),
            stop_tx,
        };

        // 在同一把写锁内复核“未在录制”后再插入，防止同一主播被并发开录
        // 出现两条录制循环（进而触发两次后处理）。开头的检查只是快速路径。
        // 复核失败时不删除已创建的 session_dir：同一秒内的并发调用会得到同一路径，
        // 删除会破坏胜出者的目录；遗留的空目录由 segment_merge::startup_remove_empty_dirs 清理。
        //
        // Re-check "not recording" under the same write lock before inserting, so
        // concurrent starts for the same streamer can't spawn two recording loops
        // (and thus two post-processing runs). The check above is only a fast path.
        // On failure the created session_dir is NOT removed: concurrent calls within
        // the same second resolve to the same path, so deleting it would break the
        // winner's directory; leftover empty directories are cleaned up by
        // segment_merge::startup_remove_empty_dirs.
        {
            let mut sessions = self.sessions.write();
            if sessions.contains_key(username) {
                return Err(AppError::AlreadyRecording(username.to_string()));
            }
            sessions.insert(username.to_string(), session);
        }
        self.state
            .active_recording_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // 录制开始时立即创建 meta，写入 recording 状态
        // Create meta immediately when recording starts, with "recording" status
        {
            let started_at = chrono::Local::now().to_rfc3339();
            let meta = crate::recording::meta::VideoMeta {
                meta_version: crate::recording::meta::META_VERSION,
                status: "recording".to_string(),
                started_at,
                size_bytes: 0,
                video_duration_secs: None,
                video_resolution: None,
                pp_execution: None,
                segments_downloaded: None,
                segments_failed: None,
                video_path: None, // write_meta 会自动填入 / auto-filled by write_meta
                pp_progress: None,
            };
            crate::recording::meta::write_meta(&session_dir, &meta);
        }

        emitter.emit(
            "recording-started",
            &serde_json::json!({
                "username": username,
                "dir_path": session_dir.to_string_lossy()
            }),
        );

        let result_path = session_dir.to_string_lossy().to_string();
        let mut session_dir = session_dir;
        let manager = Arc::clone(self);
        let username = username.to_string();
        let playlist_url = playlist_url.to_string();

        tokio::spawn(async move {
            if let Err(e) = manager
                .recording_loop(
                    &username,
                    &playlist_url,
                    &mut session_dir,
                    stop_rx,
                    Arc::clone(&emitter),
                )
                .await
            {
                tracing::error!(
                    "{}",
                    crate::tl!("recorder.recordingError", username = username, error = e)
                );
            }

            let record_duration_secs = manager.sessions.read().get(&username).map(|s| {
                chrono::Local::now()
                    .signed_duration_since(s.started_at)
                    .num_seconds()
                    .max(0) as u64
            });

            // 先登记交接状态再移除会话，消除"会话已移除但后处理尚未 claim"的窗口，
            // 避免维护扫描在此期间把该目录误判为陈旧并重复触发后处理。
            // Register the handoff state before removing the session, closing the window where
            // the session is gone but post-processing hasn't claimed yet — otherwise the
            // maintenance scan could misjudge the directory as stale and re-trigger it.
            manager
                .waiting_merge_dirs
                .write()
                .insert(session_dir.clone());
            manager.sessions.write().remove(&username);
            manager
                .state
                .active_recording_count
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);

            // 录制结束后清理 segment_stats 缓存（以 session_dir 路径为 key）
            // Clean up segment_stats cache after recording ends (keyed by session_dir path)
            manager
                .segment_stats
                .write()
                .remove(&session_dir.to_string_lossy().to_string());

            let was_manual = manager.manually_stopping.write().remove(&username);
            if !was_manual {
                manager.naturally_stopped.write().insert(username.clone());
            }

            let session_dir_clone = session_dir.clone();
            let username_clone = username.clone();
            let state_clone = Arc::clone(&manager.state);
            let emitter_clone = Arc::clone(&emitter);

            emitter.emit(
                "recording-stopped",
                &serde_json::json!({
                    "username": username,
                    "session_dir": session_dir.to_string_lossy(),
                    "record_duration_secs": record_duration_secs,
                }),
            );

            // 录制结束后触发后处理流水线（完全按照用户配置执行，不做任何自动注入）。
            // 若用户流水线中无任何启用节点，则跳过后处理。
            //
            // After recording ends, trigger the post-processing pipeline as-is per user config.
            // If no enabled nodes exist in the user pipeline, skip post-processing entirely.
            tokio::task::spawn_blocking(move || {
                let user_pipeline = state_clone.get_pipeline();

                // 若流水线中没有任何启用节点，直接跳过后处理
                // If no enabled nodes, skip post-processing entirely
                if !user_pipeline.nodes.iter().any(|n| n.enabled) {
                    return;
                }

                let stem = session_dir_clone
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");
                let started_at = crate::recording::service::parse_timestamp_from_stem_pub(stem)
                    .unwrap_or_else(|| chrono::Local::now().to_rfc3339());

                // session_dir 作为流水线的初始输入和 meta 占位路径
                // session_dir acts as both pipeline initial input and meta placeholder path
                crate::recording::meta::ensure_meta(&session_dir_clone, &started_at);

                // session_dir 已在移除会话前登记到 waiting_merge_dirs（交接状态）
                // session_dir was already registered in waiting_merge_dirs (handoff) before the session was removed
                emitter_clone.emit(
                    "recording-pp-waiting",
                    &serde_json::json!({
                        "username": username_clone,
                        "session_dir": session_dir_clone.to_string_lossy(),
                        "video_path": session_dir_clone.to_string_lossy(),
                    }),
                );

                // 触发后处理流水线，initial_path = session_dir
                // Trigger post-processing pipeline; initial_path = session_dir
                crate::postprocess::service::run_postprocess_for_path(
                    &session_dir_clone,
                    &session_dir_clone,
                    &user_pipeline,
                    &emitter_clone,
                    &state_clone,
                );
            })
            .await
            .ok();

            // 无条件结束交接状态（覆盖流水线为空提前返回与 panic 的情况）
            // Unconditionally end the handoff state (covers empty-pipeline early return and panics)
            manager.waiting_merge_dirs.write().remove(&session_dir);
        });

        Ok(result_path)
    }

    /// 文件轮转：在 HLS 分片边界创建新会话目录，将旧目录提交给后处理流水线，
    /// 同时继续向新目录写入后续分片（不中断录制会话）。
    ///
    /// File rotation: create a new session directory at an HLS segment boundary,
    /// submit the old directory to the post-processing pipeline, and continue writing
    /// subsequent segments to the new directory (recording session uninterrupted).
    pub(super) fn rotate_session(
        self: &Arc<Self>,
        username: &str,
        session_dir: &mut PathBuf,
        emitter: &Arc<dyn Emitter>,
    ) -> Result<()> {
        let timestamp = Local::now().format("%Y%m%d_%H%M%S");
        let parent = session_dir
            .parent()
            .ok_or_else(|| AppError::Other("session_dir has no parent".to_string()))?
            .to_path_buf();

        // 生成不冲突的新目录名，格式与初始会话目录一致（username_timestamp）
        // Generate a conflict-free new directory name in the same format as the initial session dir
        let candidate = parent.join(format!("{}_{}", username, timestamp));
        let next_dir = if !candidate.exists() {
            fs::create_dir_all(&candidate)?;
            candidate
        } else {
            // 同一秒内触发多次轮转时加后缀避免冲突
            // Add a numeric suffix when multiple rotations happen within the same second
            let mut n = 2u64;
            loop {
                let c = parent.join(format!("{}_{}_{}", username, timestamp, n));
                if !c.exists() {
                    fs::create_dir_all(&c)?;
                    break c;
                }
                n += 1;
            }
        };

        // 在会话切换到新目录之前登记旧目录的交接状态（此时 session_dir 仍是旧目录），
        // 避免维护扫描在后处理 claim 之前把旧目录误判为陈旧。
        // Register the old directory's handoff state before the session switches to the new
        // directory (session_dir is still the old one here), so the maintenance scan can't
        // misjudge it as stale before post-processing claims it.
        self.waiting_merge_dirs.write().insert(session_dir.clone());

        // 更新 sessions 表中的 dir_path，并记录开始时间
        let now = chrono::Local::now();
        {
            let mut sessions = self.sessions.write();
            if let Some(session) = sessions.get_mut(username) {
                session.dir_path = next_dir.clone();
                session.started_at = now;
            }
        }

        // 创建新 meta（recording 状态）
        {
            let started_at = chrono::Local::now().to_rfc3339();
            let meta = crate::recording::meta::VideoMeta {
                meta_version: crate::recording::meta::META_VERSION,
                status: "recording".to_string(),
                started_at,
                size_bytes: 0,
                video_duration_secs: None,
                video_resolution: None,
                pp_execution: None,
                segments_downloaded: None,
                segments_failed: None,
                video_path: None,
                pp_progress: None,
            };
            crate::recording::meta::write_meta(&next_dir, &meta);
        }

        emitter.emit(
            "recording-started",
            &serde_json::json!({
                "username": username,
                "dir_path": next_dir.to_string_lossy(),
            }),
        );

        // 旧目录提交给后处理流水线（异步，不阻塞录制）
        let old_dir = std::mem::replace(session_dir, next_dir);
        let manager = Arc::clone(self);
        let username_owned = username.to_string();
        let emitter_owned = Arc::clone(emitter);

        tokio::task::spawn_blocking(move || {
            // 交接状态由守卫结束：正常返回、流水线为空提前返回与 panic 都会清理
            // The handoff state is ended by the guard: cleaned up on normal return,
            // empty-pipeline early return and panic alike
            let _handoff = HandoffGuard {
                manager: Arc::clone(&manager),
                dir: old_dir.clone(),
            };

            let user_pipeline = manager.state.get_pipeline();
            if !user_pipeline.nodes.iter().any(|n| n.enabled) {
                return;
            }

            let stem = old_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let started_at = crate::recording::service::parse_timestamp_from_stem_pub(stem)
                .unwrap_or_else(|| {
                    let local: chrono::DateTime<chrono::Local> = chrono::Utc::now().into();
                    local.to_rfc3339()
                });

            crate::recording::meta::ensure_meta(&old_dir, &started_at);

            // old_dir 已在切换会话目录前登记到 waiting_merge_dirs（交接状态）
            // old_dir was already registered in waiting_merge_dirs (handoff) before the session dir switched
            emitter_owned.emit(
                "recording-pp-waiting",
                &serde_json::json!({
                    "username": username_owned,
                    "session_dir": old_dir.to_string_lossy(),
                    "video_path": old_dir.to_string_lossy(),
                }),
            );

            crate::postprocess::service::run_postprocess_for_path(
                &old_dir,
                &old_dir,
                &user_pipeline,
                &emitter_owned,
                &manager.state,
            );
        });

        tracing::info!(
            "{}",
            crate::tl!(
                "recorder.started",
                username = username,
                dir = session_dir.display()
            )
        );
        Ok(())
    }

    /// 手动停止录制（标记为手动停止，防止自动重录）。
    /// Manually stop recording (marks as manually stopped to prevent auto-restart).
    pub async fn stop_recording(self: &Arc<Self>, username: &str) -> Result<()> {
        // 在 sessions 读锁内插入 manually_stopping：录制任务结束时先在 sessions 写锁内移除会话，
        // 之后才读取并清除 manually_stopping，因此标记只会在会话仍存在时写入，并必然被该会话的
        // 结束流程消费（不会残留到下一次录制）。锁顺序为 sessions → manually_stopping，其他地方
        // 没有反向嵌套。parking_lot 守卫是 !Send，不能跨 await 持有，所以先克隆 stop_tx 再 send。
        //
        // Insert into manually_stopping while holding the sessions read lock: when the recording
        // task ends it first removes the session under the sessions write lock and only then
        // reads and clears manually_stopping, so the flag is only written while the session
        // still exists and is guaranteed to be consumed by that session's shutdown (it can't
        // leak into the next recording). Lock order is sessions → manually_stopping, never
        // nested the other way. parking_lot guards are !Send and can't be held across await,
        // so clone stop_tx first and send afterwards.
        let stop_tx = {
            let sessions = self.sessions.read();
            let session = sessions
                .get(username)
                .ok_or_else(|| AppError::NotRecording(username.to_string()))?;
            self.manually_stopping.write().insert(username.to_string());
            session.stop_tx.clone()
        };
        let _ = stop_tx.send(()).await;
        Ok(())
    }

    /// 自动停止录制（不标记为手动停止，允许自动重录）。
    /// Automatically stop recording (not marked as manually stopped, allows auto-restart).
    pub async fn stop_recording_auto(self: &Arc<Self>, username: &str) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(username)
            .cloned()
            .ok_or_else(|| AppError::NotRecording(username.to_string()))?;
        let _ = session.stop_tx.send(()).await;
        Ok(())
    }
}
