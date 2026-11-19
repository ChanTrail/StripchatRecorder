//! Stripchat 前端 API 请求通道与各端点（cam、user-ids、浏览列表）。
//! Stripchat frontend API plumbing and endpoints (cam, user-ids, browse list).

use super::net::https_host;
use super::{BROADCAST_GONE_DESCRIPTIONS, StripchatApi, is_api_error_body};
use crate::core::error::{AppError, Result};
use reqwest::{Client, Response};

impl StripchatApi {
    /// 返回访问该 URL 应使用的 API 客户端：ECH 开启时按 URL 主机名解析并
    /// 缓存 ECH 客户端，否则直接返回基础客户端。
    /// The API client to use for this URL: with ECH enabled the per-host ECH
    /// client is resolved and cached; otherwise the base client.
    async fn api_client_for(&self, url: &str) -> Client {
        match (&self.api_ech, https_host(url)) {
            (Some(ech), Some(host)) => ech.client_for(&host).await,
            _ => self.api_client.clone(),
        }
    }

    /// 将一次经 ECH 通道的请求结果反馈给 ECH 缓存：
    /// - 响应 5xx（CF 边缘→源站失败，如 530 zone-miss）→ 该主机降级回退普通连接
    /// - 连接层错误（TLS 失败、ECH 密钥轮换被拒等）→ 驱逐条目以便下次重建
    ///
    /// 仅当该主机确实持有 ECH 客户端时才动作；`ech` 为 None（ECH 关闭）或
    /// 普通客户端路径的异常不触发任何状态变化。
    ///
    /// Feeds a request result on the ECH path back into the ECH cache:
    /// - a 5xx response (CF edge→origin failure, e.g. 530 zone-miss) demotes
    ///   the host to the plain client
    /// - connect-level errors (TLS failure, ECH key-rotation rejection) evict
    ///   the entry so the next lookup rebuilds it
    ///
    /// No-ops unless the host actually holds an ECH client; `ech` None (ECH
    /// disabled) or plain-client anomalies change nothing.
    ///
    /// 返回 true = 这是一次「ECH 通道上的 403」且条目已被驱逐（调用方
    /// 可换条新连接重掷边缘抽签重试）。
    /// Returns true = this was a "403 over ECH" and the entry was evicted
    /// (the caller may retry on a fresh connection for a new edge lottery).
    async fn note_ech_result(
        ech: &Option<crate::ech::EchClients>,
        url: &str,
        result: &std::result::Result<Response, reqwest::Error>,
    ) -> bool {
        let (Some(ech), Some(host)) = (ech, https_host(url)) else {
            return false;
        };
        match result {
            Ok(resp) if resp.status().is_server_error() => {
                ech.note_server_error(&host).await;
                false
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::FORBIDDEN => {
                // 403 = CF WAF 拦截：若该主机持有 ECH 客户端，说明 ECH 也
                // 被拦——驱逐并报告，供调用方换新连接重试 / 403 = CF WAF
                // block: if the host holds an ECH client the block happened
                // over ECH — evict and report so the caller can retry on a
                // fresh connection
                ech.note_forbidden(&host).await
            }
            Err(e) if e.is_connect() => {
                ech.evict(&host).await;
                false
            }
            _ => false,
        }
    }

    /// 发送一个经 API 客户端的 GET 请求（统一附带 `Priority` / `Referer`
    /// 头，`path` 即 Referer 所用的 API 路径）：
    /// - 5xx / 连接错误走 ECH 结果反馈（负缓存降级 / 驱逐重建）
    /// - 403 且该主机持有 ECH 客户端：连接落上了被标记的 CF 边缘——反馈
    ///   会驱逐条目，随后换新连接重掷边缘抽签**重试一次**（普通路径的 403
    ///   不重试——换不换连接都一样会被拦）
    ///
    /// Sends a GET through the API client (`Priority` / `Referer` attached
    /// uniformly; `path` is the API path used as Referer):
    /// - 5xx / connect errors feed the ECH cache (negative-cache demotion /
    ///   eviction for rebuild)
    /// - on 403 while the host holds an ECH client: the conn landed on a
    ///   flagged CF edge — the feedback evicts the entry, then the request
    ///   is **retried once** on a fresh connection (a new edge lottery;
    ///   plain-path 403s don't retry — a new conn changes nothing)
    pub(super) async fn api_get(&self, url: &str, path: &str) -> Result<Response> {
        let referer = self.referer_for_path(path);
        for attempt in 0..2 {
            let result = self
                .api_client_for(url)
                .await
                .get(url)
                .header("Priority", "u=0, i")
                .header("Referer", &referer)
                .send()
                .await;
            let evicted_on_403 = Self::note_ech_result(&self.api_ech, url, &result).await;
            if !(attempt == 0 && evicted_on_403) {
                return result.map_err(AppError::from);
            }
        }
        unreachable!("两轮内必然返回 / returns within two rounds")
    }

    /// 解析 v1/broadcasts/{username} 响应，统一处理"用户不存在"判定。
    ///
    /// 主播不存在时，Stripchat 固定返回：
    ///   `{"title":"An error occurred","description":"Entity \"Model\" not found"}`
    /// 主播账号已被删除时返回：
    ///   `{"title":"An error occurred","description":"model already deleted"}`
    /// 两者同样视为 `UserNotFound`；其他任何非 2xx（包括 404、429、5xx 等）
    /// 均视为网络/API 错误，不会被误判为"用户不存在"。
    ///
    /// Parse the v1/broadcasts/{username} response, uniformly handling "user not
    /// found" detection.
    ///
    /// When a streamer does not exist, Stripchat always returns exactly:
    ///   `{"title":"An error occurred","description":"Entity \"Model\" not found"}`
    /// When the streamer's account has been deleted, it returns:
    ///   `{"title":"An error occurred","description":"model already deleted"}`
    /// Both map to `UserNotFound`; any other non-2xx response (404, 429, 5xx, etc.)
    /// is treated as a network/API error and is NOT misclassified as "user not found".
    pub(super) async fn parse_broadcast_response(
        resp: Response,
        username: &str,
    ) -> Result<serde_json::Value> {
        let status = resp.status();
        let bytes = resp.bytes().await?;

        // 先检查 HTTP 状态：非 2xx 时，仅精确匹配已知的"用户不存在"body，
        // 否则一律视为网络/API 错误。
        // Check HTTP status first: for non-2xx, only the exact known "user not
        // found" body maps to UserNotFound; everything else is a network/API error.
        if !status.is_success() {
            // 精确匹配已知的"用户不存在/已删除"响应形状
            // Exact match for the known "user not found / deleted" response shapes
            if is_api_error_body(&bytes, BROADCAST_GONE_DESCRIPTIONS) {
                return Err(AppError::UserNotFound(format!(
                    "Model {} not found",
                    username
                )));
            }
            return Err(AppError::Other(format!(
                "API return {} ({})",
                status.as_u16(),
                username
            )));
        }

        // 2xx：正常解析 JSON
        // 2xx: parse JSON normally
        let json: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                return Err(AppError::Other(format!(
                    "API return {} ({})",
                    status.as_u16(),
                    username
                )));
            }
        };

        Ok(json)
    }

    /// 查询主播在 groupShow 状态时的具体秀类型，通过 v2/models/{model_id}/cam 接口获取。
    /// 返回 show.mode，以及 ticket/perMinute 子类型（仅 groupShow mode 时有值）。
    ///
    /// Query the specific show type when a streamer is in groupShow status,
    /// via the v2/models/{model_id}/cam endpoint.
    /// Returns the show.mode and ticket/perMinute subtype (only present in groupShow mode).
    pub(super) async fn get_group_show_detail(
        &self,
        username: &str,
        model_id: i64,
    ) -> Option<String> {
        let json = self.fetch_cam_json(username, model_id).await?;
        let show = &json["cam"]["show"];
        if show.is_null() || !show.is_object() {
            return None;
        }
        let mode = show["mode"].as_str()?;
        if mode == "groupShow" {
            let subtype = show["details"]["groupShow"]["type"]
                .as_str()
                .unwrap_or("")
                .to_string();
            return Some(format!("groupShow:{}", subtype));
        }
        Some(mode.to_string())
    }

    /// 从 v2/models/{model_id}/cam 接口获取主播的离线预览图 URL（previewUrl），与在线状态无关。
    /// 状态轮询用它每天为每个主播刷新 `cached_preview_url`；`get_stream_info` 在
    /// `fetch_offline_preview=true` 且主播离线时也会调用。请求失败或没有该字段时返回 None。
    ///
    /// Fetch the offline preview image URL (previewUrl) from the v2/models/{model_id}/cam
    /// endpoint, regardless of online status. Status polling uses it to refresh each streamer's
    /// `cached_preview_url` daily; `get_stream_info` also calls it when
    /// `fetch_offline_preview=true` and the streamer is offline. Returns None when the request
    /// fails or the field is absent.
    pub async fn get_cam_preview_url(&self, username: &str, model_id: i64) -> Option<String> {
        let json = self.fetch_cam_json(username, model_id).await?;
        json["user"]["user"]["previewUrl"]
            .as_str()
            .map(|s| s.to_string())
    }

    /// 请求 v2/models/{model_id}/cam 接口，返回解析后的 JSON。
    /// 供 get_group_show_detail 和 get_cam_preview_url 共用，避免重复请求逻辑。
    ///
    /// 端点已由 Stripchat 从按用户名查询（v2/models/username/{username}/cam）改为
    /// 按内部 ID 查询（v2/models/{model_id}/cam）——`username` 参数仅用于日志和
    /// Referer 头，不再出现在请求路径中。
    ///
    /// Fetch and parse the v2/models/{model_id}/cam endpoint JSON.
    /// Shared by get_group_show_detail and get_cam_preview_url to avoid duplicate request logic.
    ///
    /// Stripchat changed this endpoint from username-based lookup
    /// (v2/models/username/{username}/cam) to internal-ID-based lookup
    /// (v2/models/{model_id}/cam) — the `username` parameter is only used for logging
    /// and the Referer header, no longer appearing in the request path.
    async fn fetch_cam_json(&self, _username: &str, model_id: i64) -> Option<serde_json::Value> {
        let path = format!("/api/front/v2/models/{}/cam", model_id);
        let url = self.api_url(&format!("https://stripchat.com{}", path));
        let resp = self.api_get(&url, &path).await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json().await.ok()
    }

    /// 通过缓存的 model_id 反查主播当前的真实用户名（用于改名检测）。
    ///
    /// 当 v1/broadcasts/{旧用户名} 查询不到该主播时，说明用户名可能已变更（Stripchat
    /// 允许主播随时修改显示用户名，但内部 model_id 保持不变）。用之前缓存的
    /// model_id 请求 v2/models/{model_id}/cam，若返回数据中的 `user.user.username`
    /// 与旧用户名不同，即可确认是改名而非账号被删除/封禁，返回新用户名。
    ///
    /// Look up a streamer's current real username via a cached model_id (for rename
    /// detection).
    ///
    /// When v1/broadcasts/{old_username} can't find the streamer, the username may have
    /// changed (Stripchat lets streamers change their display username at any time,
    /// but the internal model_id stays fixed). Using the previously cached model_id to
    /// query v2/models/{model_id}/cam, if the returned `user.user.username` differs
    /// from the old username, this confirms a rename (rather than the account being
    /// deleted/banned) and returns the new username.
    pub async fn lookup_username_by_model_id(&self, model_id: i64) -> Option<String> {
        let url = self.api_url(&format!(
            "https://stripchat.com/api/front/v2/models/{}/cam",
            model_id
        ));
        let path = format!("/api/front/v2/models/{}/cam", model_id);
        let resp = self.api_get(&url, &path).await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        json["user"]["user"]["username"]
            .as_str()
            .map(|s| s.to_string())
    }

    /// 通过 `/api/front/users/user-ids/{username}` 获取主播的内部 ID。
    ///
    /// 专用于添加主播时的用户名校验，返回值直接是数字 ID，
    /// 避免从嵌套 JSON 中查找 `modelId` 字段。
    ///
    /// 主播不存在时 Stripchat 返回（username 为数字时形如 `User 259975465 not found`）：
    ///   `{"title":"An error occurred","description":"User {username} not found"}`
    /// 仅该响应映射为 `UserNotFound`（description 中的用户名不区分大小写比较）；
    /// 其他非 2xx 均视为网络/API 错误。
    ///
    /// Fetch a streamer's internal ID via `/api/front/users/user-ids/{username}`.
    ///
    /// Intended for username validation on add; the response is a plain `{"id": N}`
    /// with no nesting, so there's no need to dig for a `modelId` field.
    ///
    /// Stripchat returns the following when the streamer does not exist
    /// (e.g. `User 259975465 not found` for a numeric username):
    ///   `{"title":"An error occurred","description":"User {username} not found"}`
    /// Only that precise body maps to `UserNotFound` (the username in the
    /// description is compared case-insensitively); any other non-2xx is treated
    /// as a network/API error.
    async fn fetch_model_id_by_username(&self, username: &str) -> Result<i64> {
        let path = format!("/api/front/users/user-ids/{}", username);
        let url = self.api_url(&format!("https://stripchat.com{}", path));

        let resp = self.api_get(&url, &path).await?;

        let status = resp.status();
        let bytes = resp.bytes().await?;

        if !status.is_success() {
            // 匹配"用户不存在"响应：description 为 "User {username} not found"（不区分大小写）
            // Match "user not found": description is "User {username} not found" (case-insensitive)
            let expected_desc = format!("User {} not found", username);
            if is_api_error_body(&bytes, &[expected_desc.as_str()]) {
                return Err(AppError::UserNotFound(format!(
                    "User {} not found",
                    username
                )));
            }
            return Err(AppError::Other(format!(
                "API return {} ({})",
                status.as_u16(),
                username
            )));
        }

        let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            AppError::Other(format!("API return {} ({})", status.as_u16(), username))
        })?;

        json["id"].as_i64().ok_or_else(|| {
            AppError::Other(format!(
                "missing id field in user-ids response ({})",
                username
            ))
        })
    }

    /// 验证主播用户名是否存在，仅发一次轻量请求，不解析直播状态。
    /// 专用于添加主播时的用户名校验，成功时返回该主播的 model_id，
    /// 供添加流程持久化，用于日后改名反查。
    ///
    /// Verify whether a streamer username exists with a single lightweight request,
    /// without parsing any live status. Intended for username validation on add.
    /// On success, returns the streamer's model_id for the add flow to persist,
    /// enabling future rename lookups.
    pub async fn verify_user_exists(&self, username: &str) -> Result<i64> {
        self.fetch_model_id_by_username(username).await
    }

    /// 官方分类浏览列表：`GET /api/front/models`（v1 平铺分页接口，
    /// `v2/models` 是首页分块推荐流、不适合无限滚动）。返回精简后的
    /// `{models:[…], total, filtered}`，模型项仅保留卡片渲染所需字段。
    ///
    /// Official category browse list: `GET /api/front/models` (the v1 flat
    /// paginated endpoint — `v2/models` returns the homepage blocks feed and
    /// is unsuitable for infinite scroll). Returns a slimmed
    /// `{models:[…], total, filtered}` keeping only card-render fields.
    ///
    /// ## 参数 / Parameters
    /// - `primary_tag`：SC 一级分类 `girls|couples|men|trans`（非法值回退 girls）
    /// - `tag`：二级标签 slug（如 `ageYoung`、`ethnicityAsian`、`tagLanguageChinese`），映射到
    ///   `filterGroupTags=[["slug"]]`；None/空串不加过滤
    /// - `sort_by`：`stripRanking|trending`（非法值回退 stripRanking；
    ///   注意上游对未知 sortBy 直接返回空列表，因此这里做白名单钳制）
    /// - `offset`/`limit`：分页；limit 钳制 1..=80（上游超过 ~80 返回空）
    ///
    /// - `primary_tag`: SC top-level category `girls|couples|men|trans`
    ///   (invalid values fall back to girls)
    /// - `tag`: second-level tag slug (e.g. `ageYoung`, `ethnicityAsian`,
    ///   `tagLanguageChinese`),
    ///   mapped to `filterGroupTags=[["slug"]]`; None/empty disables filtering
    /// - `sort_by`: `stripRanking|trending` (invalid falls back to
    ///   stripRanking — upstream returns an EMPTY list for unknown sortBy
    ///   values, so it is allowlist-clamped here)
    /// - `offset`/`limit`: pagination; limit clamped to 1..=80 (upstream
    ///   returns empty beyond ~80)
    pub async fn list_models(
        &self,
        primary_tag: &str,
        tag: Option<&str>,
        sort_by: &str,
        offset: u32,
        limit: u32,
    ) -> Result<serde_json::Value> {
        // 实测合法值仅 girls/couples/men/trans（「男生」是 men，guys 会 400）
        // Verified values are only girls/couples/men/trans ("guys" returns 400)
        let primary_tag = match primary_tag {
            "couples" | "men" | "trans" => primary_tag,
            _ => "girls",
        };
        let sort_by = match sort_by {
            "trending" => "trending",
            _ => "stripRanking",
        };
        let limit = limit.clamp(1, 80);

        let mut path = format!(
            "/api/front/models?limit={}&offset={}&primaryTag={}&sortBy={}",
            limit, offset, primary_tag, sort_by
        );
        if let Some(t) = tag.filter(|t| !t.is_empty()) {
            // 仅放行字母数字 slug，防注入 / Allow only alphanumeric slugs (injection-safe)
            if t.chars().all(|c| c.is_ascii_alphanumeric()) {
                path.push_str(&format!("&filterGroupTags=%5B%5B%22{}%22%5D%5D", t));
            }
        }

        let url = self.api_url(&format!("https://stripchat.com{}", path));
        let resp = self.api_get(&url, &path).await?;
        let json: serde_json::Value = resp.json().await?;

        let models: Vec<serde_json::Value> = json["models"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|m| {
                        // 预览图优先实时快照（thumbs/{snapshot_ts}/{stream_name}，
                        // 与 get_stream_info 同一构造），保证卡片显示直播画面；
                        // 缺字段时回退 previewUrlThumbSmall（静态封面，常为旧图）
                        // Prefer the live snapshot (thumbs/{snapshot_ts}/
                        // {stream_name}, same as get_stream_info) so cards show
                        // the live frame; fall back to previewUrlThumbSmall
                        // (a static cover, often stale)
                        let snap_ts = m["snapshotTimestamp"]
                            .as_i64()
                            .or_else(|| {
                                m["snapshotTimestamp"].as_str().and_then(|s| s.parse().ok())
                            })
                            .unwrap_or(0);
                        let stream_name = m["streamName"].as_str().unwrap_or("");
                        let preview = if snap_ts > 0 && !stream_name.is_empty() {
                            serde_json::json!(format!(
                                "https://img.doppiocdn.net/thumbs/{snap_ts}/{stream_name}"
                            ))
                        } else {
                            m["previewUrlThumbSmall"].clone()
                        };
                        serde_json::json!({
                            "username": m["username"],
                            "model_id": m["id"],
                            "status": m["status"],
                            "viewers": m["viewersCount"],
                            "country": m["country"],
                            "is_hd": m["isHd"],
                            "is_new": m["isNew"],
                            "is_vr": m["isVr"],
                            "is_mobile": m["isMobile"],
                            "is_non_nude": m["isNonNude"],
                            "preview": preview,
                            "avatar": m["avatarUrl"],
                            "gender_group": m["genderGroup"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(serde_json::json!({
            "models": models,
            "total": json["totalCount"],
            "filtered": json["filteredCount"],
        }))
    }

    /// 服务状态页探测：经普通（非 ECH）API 客户端 GET url——镜像改写仍
    /// 生效，但不解析 ECH，与 ECH 腿并列构成非 ECH 基线（对齐分片下载里
    /// 普通腿与 ECH 腿同时竞速的形态）。
    /// Status-page probe: a single timed GET via the plain (non-ECH) API
    /// client — the mirror rewrite still applies, but ECH is not resolved,
    /// forming the non-ECH baseline next to the ECH leg (matching the
    /// segment download's plain + ECH racing legs).
    pub(crate) async fn probe_api_direct(&self, url: &str) -> Result<super::ProbeResult> {
        let url = self.api_url(url);
        let t0 = std::time::Instant::now();
        let resp = self
            .api_client
            .get(&url)
            .header("Referer", self.referer_base())
            .timeout(super::PROBE_TIMEOUT)
            .send()
            .await?;
        Ok(super::ProbeResult {
            status: resp.status().as_u16(),
            elapsed: t0.elapsed(),
            url,
            headers: resp.headers().clone(),
        })
    }

    /// 服务状态页探测：经 API ECH 通道 GET url——镜像改写与生产 `api_get`
    /// 一致（ECH 腿与直连腿测同一条"链接"）。整条腿绝对只走 ECH：
    /// `ech_client_for` 拿不到探活通过的通道就返回 None（记
    /// no-ech-channel 失败腿），**绝不退回普通客户端**；「解析通道 +
    /// 发请求」共享一个探测预算，超出即按「ECH 不通」的超时失败返回——
    /// 状态页把该腿记为失败点（等效超时），而不会拖垮整条目本轮缺席。
    /// Status-page probe: a timed GET via the API ECH channel — the mirror
    /// rewrite matches production `api_get` (both legs measure the same
    /// link). The leg is strictly ECH-only: `ech_client_for` yields None
    /// when no probe-passed channel exists (a no-ech-channel failure leg)
    /// and **never falls back to the plain client**; "resolve channel +
    /// request" share one probe budget — past it the leg returns a
    /// timeout-class failure (the page plots a failure point, equivalent
    /// to a timeout) instead of dragging the whole item past the
    /// collection deadline.
    pub(crate) async fn probe_api_ech(&self, url: &str) -> Result<Option<super::ProbeResult>> {
        let url = self.api_url(url);
        let (Some(ech), Some(host)) = (&self.api_ech, https_host(&url)) else {
            return Ok(None);
        };
        tokio::time::timeout(super::PROBE_TIMEOUT, async {
            let Some(client) = ech.ech_client_for(&host).await else {
                return Ok(None);
            };
            let t0 = std::time::Instant::now();
            let resp = client
                .get(&url)
                .header("Referer", self.referer_base())
                .send()
                .await?;
            Ok(Some(super::ProbeResult {
                status: resp.status().as_u16(),
                elapsed: t0.elapsed(),
                url,
                headers: resp.headers().clone(),
            }))
        })
        .await
        .map_err(|_| AppError::Other("ECH 探测超时 / ECH probe timed out".into()))?
    }
}
