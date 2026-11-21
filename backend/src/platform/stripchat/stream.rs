//! 直播状态查询与 HLS 播放列表获取（v1/v2 broadcasts、_auto.m3u8 竞速、
//! Mouflon 参数匹配）。
//! Live-status queries and HLS playlist retrieval (v1/v2 broadcasts,
//! _auto.m3u8 racing, Mouflon parameter matching).

use super::{BROADCAST_GONE_DESCRIPTIONS, CDN_TLDS, StreamInfo, StripchatApi, is_api_error_body};
use crate::core::error::{AppError, Result};

impl StripchatApi {
	/// 通过主播内部 ID 查询直播状态，使用 `/api/front/v2/broadcasts/{model_id}`。
	///
	/// 响应结构与 v1/broadcasts/{username} 相同（`item` 对象），同时包含
	/// `item.username` 字段——若该字段与调用方记录的 username 不一致，
	/// 即可直接得知主播已改名，无需额外的 cam 接口反查。
	///
	/// 主播不存在时，与 v1 相同，精确匹配
	///   `{"title":"An error occurred","description":"Entity \"Model\" not found"}`
	/// 或（账号已被删除时）
	///   `{"title":"An error occurred","description":"model already deleted"}`
	/// 才返回 `UserNotFound`；其他非 2xx（429、5xx 等）视为网络/API 错误，
	/// 不会误判为主播失效。
	///
	/// Query live status by internal model ID via `/api/front/v2/broadcasts/{model_id}`.
	///
	/// The response structure is identical to v1/broadcasts/{username} (an `item`
	/// object), but also includes `item.username` — if that field differs from the
	/// caller's recorded username, we know the streamer has been renamed directly,
	/// with no extra cam-endpoint round-trip needed.
	///
	/// When the streamer no longer exists, the v2 endpoint returns the same exact
	/// error body as v1: `{"title":"An error occurred","description":"Entity \"Model\" not found"}`,
	/// or `{"title":"An error occurred","description":"model already deleted"}` when the
	/// account has been deleted. Only those precise bodies map to `UserNotFound`; any
	/// other non-2xx (429, 5xx, network issues, etc.) is treated as a transient error
	/// and does NOT mark the streamer as dead.
	async fn fetch_stream_info_by_model_id(
		&self,
		model_id: i64,
		known_username: &str,
		fetch_playlist: bool,
		fetch_offline_preview: bool,
	) -> Result<StreamInfo> {
		let path = format!("/api/front/v2/broadcasts/{}", model_id);
		let url = self.api_url(&format!("https://stripchat.com{}", path));

		let resp = self.api_get(&url, &path).await?;

		let status = resp.status();
		let bytes = resp.bytes().await?;

		if !status.is_success() {
			// 与 v1 相同：仅精确匹配已知的"用户不存在"body 才返回 UserNotFound，
			// 其他非 2xx（429、5xx、网络错误等）均视为网络/API 错误，不误判为失效。
			// Same as v1: only the exact known "entity not found" body maps to
			// UserNotFound; any other non-2xx (429, 5xx, network issues, etc.) is
			// treated as a transient network/API error, not a dead-streamer signal.
			if is_api_error_body(&bytes, BROADCAST_GONE_DESCRIPTIONS) {
				return Err(AppError::UserNotFound(format!(
					"Model {} not found (id={})",
					known_username, model_id
				)));
			}
			return Err(AppError::Other(format!(
				"API return {} (id={})",
				status.as_u16(),
				model_id
			)));
		}

		let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
			AppError::Other(format!("API return {} (id={})", status.as_u16(), model_id))
		})?;

		let item = &json["item"];

		// 检测改名：v2 响应中 item.username 是当前真实用户名
		// Detect rename: item.username in the v2 response is the current real username
		let current_username = item["username"].as_str().unwrap_or(known_username);
		let renamed_to = if current_username.to_lowercase() != known_username.to_lowercase() {
			tracing::info!(
				"{}",
				crate::tl!(
					"stripchat.streamerRenamed",
					username = known_username,
					newUsername = current_username,
					modelId = model_id
				)
			);
			Some(current_username.to_string())
		} else {
			None
		};

		// 用当前实际用户名（可能已改名）进行后续解析，保持日志和缓存的一致性
		// Use the current real username (possibly renamed) for subsequent parsing
		let username = renamed_to.as_deref().unwrap_or(known_username);

		let is_live = item["isLive"].as_bool().unwrap_or(false);
		let status_text = item["status"].as_str().unwrap_or("unknown");

		let status = if is_live && status_text == "groupShow" {
			match self.get_group_show_detail(username, model_id).await {
				Some(ref detail) if detail.starts_with("groupShow:") => {
					match detail.strip_prefix("groupShow:").unwrap_or("") {
						"ticket" => "票务秀".to_string(),
						"perMinute" => "计时秀".to_string(),
						_ => "群组秀".to_string(),
					}
				}
				Some(ref mode) => match mode.as_str() {
					"private" => "私密秀".to_string(),
					"p2pVoice" | "p2p" => "P2P".to_string(),
					"virtualPrivate" => "虚拟私密".to_string(),
					_ => "群组秀".to_string(),
				},
				None => "群组秀".to_string(),
			}
		} else {
			match status_text {
				"public" => "公开秀".to_string(),
				"private" => "私密秀".to_string(),
				"virtualPrivate" => "虚拟私密".to_string(),
				"p2p" | "p2pVoice" => "P2P".to_string(),
				"idle" => "等待".to_string(),
				"off" => "离线".to_string(),
				_ => status_text.to_string(),
			}
		};

		let thumbnail_url = if is_live {
			let snapshot_ts = item["snapshotTimestamp"]
				.as_i64()
				.or_else(|| {
					item["snapshotTimestamp"]
						.as_str()
						.and_then(|s| s.parse().ok())
				})
				.unwrap_or(0);
			let stream_name = item["streamName"].as_str().unwrap_or("");
			if snapshot_ts > 0 && !stream_name.is_empty() {
				Some(format!(
					"https://img.doppiocdn.net/thumbs/{}/{}",
					snapshot_ts, stream_name
				))
			} else {
				item["previewUrl"].as_str().map(|s| s.to_string())
			}
		} else {
			// 离线时：仅当 fetch_offline_preview=true 时才请求 cam 接口，否则直接返回 None，
			// 由调用方使用缓存值（状态轮询通过 get_cam_preview_url 单独刷新离线预览图）。
			// Offline: only call the cam endpoint when fetch_offline_preview=true; otherwise
			// return None and the caller uses the cached value (status polling refreshes the
			// offline preview separately via get_cam_preview_url).
			if fetch_offline_preview {
				self.get_cam_preview_url(username, model_id).await
			} else {
				None
			}
		};

		let is_recordable = is_live && status_text == "public";

		let playlist_url = if is_recordable && fetch_playlist {
			self.get_playlist_url(username, model_id).await.ok()
		} else {
			None
		};

		Ok(StreamInfo {
			is_online: is_live,
			is_recordable,
			status,
			thumbnail_url,
			playlist_url,
			model_id: Some(model_id),
			renamed_to,
		})
	}

	/// 查询主播直播状态。
	///
	/// - 若有 `known_model_id`（绝大多数情况），直接用 v2/broadcasts/{id} 查询：
	///   - 响应中 `item.username` 与记录不同时，直接得知改名，无需额外请求。
	///   - 仅已知的"主播不存在/已删除"错误响应体映射为 `UserNotFound`，
	///     触发上层死亡标记流程；其他非 2xx 视为网络/API 错误。
	/// - 无 `known_model_id`（仅兼容升级前未回填的旧数据）时，
	///   回退到 v1/broadcasts/{username} 查询；查询成功时返回的 `model_id`
	///   由调用方（`poll_streamer`）通过 `backfill_model_id` 回填，
	///   下次轮询即可走 v2 路径。
	///
	/// Query a streamer's live status.
	///
	/// - When `known_model_id` is present (the common case), queries via
	///   `v2/broadcasts/{id}` directly:
	///   - If `item.username` in the response differs from the recorded username,
	///     a rename is detected immediately with no extra round-trip.
	///   - Only the known "streamer not found / already deleted" error bodies map
	///     to `UserNotFound`, triggering the dead-streamer marking flow upstream;
	///     any other non-2xx is treated as a network/API error.
	/// - When `known_model_id` is absent (only for pre-upgrade data that hasn't
	///   been backfilled), falls back to `v1/broadcasts/{username}`; the `model_id`
	///   returned on success is backfilled by the caller (`poll_streamer`) via
	///   `backfill_model_id`, so the v2 path is used from the next poll onward.
	pub async fn get_stream_info(
		&self,
		username: &str,
		fetch_playlist: bool,
		known_model_id: Option<i64>,
		fetch_offline_preview: bool,
	) -> Result<StreamInfo> {
		if let Some(model_id) = known_model_id {
			// 主路径：直接用 model_id 走 v2 接口
			// Primary path: query v2 directly by model_id
			self.fetch_stream_info_by_model_id(
				model_id,
				username,
				fetch_playlist,
				fetch_offline_preview,
			)
			.await
		} else {
			// 兼容路径：旧数据尚无 model_id，回退到 v1 by-username
			// Compat path: old data with no model_id, fall back to v1 by-username
			self.fetch_stream_info_by_username(username, fetch_playlist, fetch_offline_preview)
				.await
		}
	}

	/// `get_stream_info` 的兼容回退实现：按用户名查询 v1/broadcasts/{username}。
	/// 仅用于升级前未回填 model_id 的旧数据，一旦成功查询后 `poll_streamer` 会
	/// 立即通过 `backfill_model_id` 补全 model_id，下次轮询即走 v2 主路径。
	///
	/// Compat fallback behind `get_stream_info`: queries `v1/broadcasts/{username}`.
	/// Only used for pre-upgrade entries without a model_id. Once a successful query
	/// returns, `poll_streamer` immediately backfills the model_id via
	/// `backfill_model_id`, so the v2 primary path is used from the next poll onward.
	async fn fetch_stream_info_by_username(
		&self,
		username: &str,
		fetch_playlist: bool,
		fetch_offline_preview: bool,
	) -> Result<StreamInfo> {
		let path = format!("/api/front/v1/broadcasts/{}", username);
		let url = self.api_url(&format!("https://stripchat.com{}", path));

		let resp = self.api_get(&url, &path).await?;

		let json = Self::parse_broadcast_response(resp, username).await?;

		let item = &json["item"];

		let is_live = item["isLive"].as_bool().unwrap_or(false);
		let status_text = item["status"].as_str().unwrap_or("unknown");
		let model_id = item["modelId"].as_i64();

		// groupShow 时，二次请求 cam 接口获取具体秀类型（ticket / perMinute / private / p2pVoice 等）
		// For groupShow, make a secondary request to cam endpoint to get the specific show type
		let status = if is_live && status_text == "groupShow" {
			match model_id {
				Some(mid) => match self.get_group_show_detail(username, mid).await {
					Some(ref detail) if detail.starts_with("groupShow:") => {
						match detail.strip_prefix("groupShow:").unwrap_or("") {
							"ticket" => "票务秀".to_string(),
							"perMinute" => "计时秀".to_string(),
							_ => "群组秀".to_string(),
						}
					}
					Some(ref mode) => match mode.as_str() {
						"private" => "私密秀".to_string(),
						"p2pVoice" | "p2p" => "P2P".to_string(),
						"virtualPrivate" => "虚拟私密".to_string(),
						_ => "群组秀".to_string(),
					},
					None => "群组秀".to_string(),
				},
				None => "群组秀".to_string(),
			}
		} else {
			match status_text {
				"public" => "公开秀".to_string(),
				"private" => "私密秀".to_string(),
				"virtualPrivate" => "虚拟私密".to_string(),
				"p2p" | "p2pVoice" => "P2P".to_string(),
				"idle" => "等待".to_string(),
				"off" => "离线".to_string(),
				_ => status_text.to_string(),
			}
		};

		let thumbnail_url = if is_live {
			let snapshot_ts = item["snapshotTimestamp"]
				.as_i64()
				.or_else(|| {
					item["snapshotTimestamp"]
						.as_str()
						.and_then(|s| s.parse().ok())
				})
				.unwrap_or(0);
			let stream_name = item["streamName"].as_str().unwrap_or("");
			if snapshot_ts > 0 && !stream_name.is_empty() {
				Some(format!(
					"https://img.doppiocdn.net/thumbs/{}/{}",
					snapshot_ts, stream_name
				))
			} else {
				item["previewUrl"].as_str().map(|s| s.to_string())
			}
		} else {
			// v1/broadcasts 离线数据不含 previewUrl，仅当 fetch_offline_preview=true 时才请求 cam 接口
			// v1/broadcasts offline data has no previewUrl; only call cam endpoint when fetch_offline_preview=true
			if fetch_offline_preview {
				match model_id {
					Some(mid) => self.get_cam_preview_url(username, mid).await,
					None => None,
				}
			} else {
				None
			}
		};

		let is_recordable = is_live && status_text == "public";

		let playlist_url = if is_recordable && fetch_playlist {
			if let Some(mid) = model_id {
				self.get_playlist_url(username, mid).await.ok()
			} else {
				None
			}
		} else {
			None
		};

		Ok(StreamInfo {
			is_online: is_live,
			is_recordable,
			status,
			thumbnail_url,
			playlist_url,
			model_id,
			renamed_to: None,
		})
	}

	/// 对所有 CDN TLD 竞速请求 `_auto.m3u8` master playlist，返回最先成功的响应文本。
	/// `codec` 非空时 URL 末尾附加 `?preferredVideoCodec={codec}`。
	///
	/// Race all CDN TLDs for the `_auto.m3u8` master playlist and return the first successful response text.
	/// When `codec` is set, `?preferredVideoCodec={codec}` is appended to the URL.
	async fn fetch_auto_playlist(&self, model_id: i64, codec: Option<&str>) -> Result<String> {
		let mut tasks = tokio::task::JoinSet::new();

		for &tld in CDN_TLDS {
			// 使用固定路径模板：edge-hls.{tld}/hls/{model_id}/master/{model_id}_auto.m3u8
			let mut url = format!(
				"https://edge-hls.{}/hls/{}/master/{}_auto.m3u8",
				tld, model_id, model_id
			);
			if let Some(codec) = codec {
				url.push_str("?preferredVideoCodec=");
				url.push_str(codec);
			}
			let cdn_client = self.cdn_client.clone();
			let referer = self.referer_base();
			tasks.spawn(async move {
				let resp = cdn_client.get(&url).header("Referer", referer).send().await;
				(tld, url, resp)
			});
		}

		let mut errors: Vec<(String, String)> = Vec::new();

		while let Some(join_result) = tasks.join_next().await {
			let (tld, url, result) = match join_result {
				Ok(r) => r,
				Err(_) => continue,
			};
			match result {
				Ok(resp) if resp.status().is_success() => {
					tasks.abort_all();
					tracing::debug!("{}", crate::tl!("stripchat.autoM3u8TldSuccess", tld = tld));
					return Ok(resp.text().await?);
				}
				Ok(resp) => {
					errors.push((url, format!("HTTP {}", resp.status())));
				}
				Err(e) => {
					errors.push((url, e.to_string()));
				}
			}
		}

		for (url, err) in &errors {
			tracing::error!(
				"{}",
				crate::tl!("stripchat.autoM3u8Failed", url = url, error = err)
			);
		}
		Err(AppError::Other(format!(
			"All CDN TLDs failed for model {} _auto.m3u8",
			model_id
		)))
	}

	/// 获取主播的 HLS 播放列表 URL。
	/// 直接对所有 CDN TLD 竞速请求 `{model_id}_auto.m3u8`，按配置选择流。
	/// 首选编码按向下回退链尝试（AV1→H265→h264）：仅当服务端没给所求编码的
	/// 变体（CODECS 声明不含该编码族）时降一级重拉，末位兜底接受任何结果；
	/// 拉取失败是纯粹的拉取失败，直接报错不降级。若 playlist 包含 Mouflon 加密
	/// 参数，则按用户配置
	/// 的 Mouflon Keys 顺序逐一比对，取第一个匹配的 pkey 对应的 psch 拼入 URL。
	///
	/// Get the HLS playlist URL for a streamer.
	/// Races all CDN TLDs for `{model_id}_auto.m3u8` and selects a stream using the configured resolution preference.
	/// The preferred codec is tried on a downward fallback chain (AV1→H265→h264): only when the
	/// server doesn't hand back variants of the requested codec (the CODECS declaration lacks that
	/// family) does the chain step down and retry; the last step accepts whatever it gets. A fetch
	/// failure is a plain fetch failure and propagates without stepping down. If the playlist
	/// contains Mouflon encryption parameters, iterates through the
	/// user-configured Mouflon Keys in order and uses the first matching pkey's psch in the URL.
	pub async fn get_playlist_url(&self, username: &str, model_id: i64) -> Result<String> {
		// VIDEO_CODECS 按升序排列（h264/H265/AV1），回退链 = 首选 + 其下所有编码
		// VIDEO_CODECS is ordered ascending (h264/H265/AV1); the chain is the
		// preferred codec plus every codec below it
		let mut chain: Vec<Option<&'static str>> = Vec::new();
		if let Some(preferred) = self.preferred_codec {
			chain.push(Some(preferred));
			if let Some(pos) = super::VIDEO_CODECS.iter().position(|c| *c == preferred) {
				chain.extend(super::VIDEO_CODECS[..pos].iter().copied().rev().map(Some));
			}
		} else {
			chain.push(None);
		}

		// 逐档拉取并校验编码：拉取失败是纯粹的拉取失败，直接报错不降级；
		// 只有"拉到了但没有所求编码"才降一级。链末无条件接受（h264 兜底位）。
		// Fetch and verify each chain entry: a fetch failure is a plain fetch
		// failure — propagate it without stepping down. Only "fetched but the
		// codec family is absent" steps down. The last entry accepts
		// unconditionally (h264 floor).
		let mut playlist_text: Option<String> = None;
		for (idx, codec) in chain.iter().enumerate() {
			let text = self.fetch_auto_playlist(model_id, *codec).await?;
			if idx + 1 == chain.len()
				|| codec
					.is_none_or(|c| crate::recording::hls::master_playlist_offers_codec(&text, c))
			{
				playlist_text = Some(text);
				break;
			}
			tracing::debug!(
				"{}",
				crate::tl!(
					"stripchat.codecFallback",
					username = username,
					codec = codec.unwrap_or("")
				)
			);
		}
		let playlist_text = playlist_text.expect("chain is non-empty");

		let parsed = crate::recording::hls::parse_master_playlist(
			&playlist_text,
			self.preferred_resolution,
			self.prefers_higher_resolution,
		);

		let (url, mouflon_pairs) =
			parsed.ok_or_else(|| AppError::StreamOffline(username.to_string()))?;

		// 若存在 Mouflon 加密参数，则遍历用户配置的 keys，取第一个匹配的
		// If Mouflon encryption parameters exist, iterate user-configured keys and use the first match
		let final_url = if mouflon_pairs.is_empty() {
			url
		} else {
			// 按 mouflon_pairs 顺序遍历，找到第一个在用户 keys 中存在的 pkey
			// Iterate mouflon_pairs in order, find the first pkey present in user keys
			let matched = mouflon_pairs
				.iter()
				.find(|(_, pkey)| self.mouflon_keys.contains(pkey.as_str()));

			match matched {
				Some((psch, pkey)) => {
					let sep = if url.contains('?') { "&" } else { "?" };
					format!("{}{}psch={}&pkey={}", url, sep, psch, pkey)
				}
				None => {
					// 没有匹配的 key，回退到第一个 pair（无解密密钥）
					// No matching key found, fall back to the first pair (no decryption key)
					let (psch, pkey) = &mouflon_pairs[0];
					let sep = if url.contains('?') { "&" } else { "?" };
					format!("{}{}psch={}&pkey={}", url, sep, psch, pkey)
				}
			}
		};

		tracing::info!("{}", crate::tl!("stripchat.usingUrl", url = final_url));

		Ok(final_url)
	}
}
