//! Mouflon 解密密钥的读取、增删与 Worker 同步（`AppState` 方法，操作
//! `data.mouflon_keys`）。
//! Mouflon decryption-key access, mutation and Worker sync (`AppState`
//! methods over `data.mouflon_keys`).

use super::{AppState, MouflonKeysStore};
use crate::core::error::{AppError, Result};
use crate::ech::EchClients;
use crate::platform::stripchat::net::{format_send_error, https_host};
use crate::recording::hls::{MouflonKeys, encode_keystream_hex, parse_keystream_hex};
use serde::Deserialize;
use std::collections::HashMap;

impl AppState {
    /// 获取全部 Mouflon 解密密钥材料（pdkey 与 keystream，不含时间戳等元数据，供录制/转发使用）。
    /// Get all Mouflon decryption key material (pdkeys and keystreams, without timestamps or other
    /// metadata, for recording/relay use).
    pub fn get_mouflon_keys(&self) -> MouflonKeys {
        self.data.read().mouflon_keys.to_keys()
    }

    /// 获取完整的 Mouflon 密钥存储（含时间戳），供前端展示。
    /// Get the full Mouflon key store (including timestamps), for frontend display.
    pub fn get_mouflon_keys_store(&self) -> MouflonKeysStore {
        self.data.read().mouflon_keys.clone()
    }

    /// 添加或更新一个 pkey 的密钥：`pdkey` 与 `keystream`（十六进制）至少给一个，给了哪个就更新
    /// 哪个，另一个保持不变。更新 manual_updated_at 并保存。pkey 为空、两者都没给、或 keystream
    /// 不是合法十六进制（至少 16 字节）时返回错误，不修改任何数据。
    ///
    /// Add or update the key for a pkey: at least one of `pdkey` and `keystream` (hex) must be
    /// given; whichever is given is updated and the other is left untouched. Updates
    /// manual_updated_at and saves. Returns an error without changing anything when the pkey is
    /// empty, neither is given, or the keystream is not valid hex (at least 16 bytes).
    pub fn add_mouflon_key(
        &self,
        pkey: &str,
        pdkey: Option<&str>,
        keystream: Option<&str>,
    ) -> Result<()> {
        let pkey = pkey.trim();
        let pdkey = pdkey.map(str::trim).filter(|s| !s.is_empty());
        let keystream = keystream.map(str::trim).filter(|s| !s.is_empty());
        if pkey.is_empty() || (pdkey.is_none() && keystream.is_none()) {
            return Err(AppError::Other(
                "pkey is required, together with a pdkey or a keystream".to_string(),
            ));
        }
        let keystream_hex = match keystream {
            Some(text) => {
                let bytes = parse_keystream_hex(text)
                    .map_err(|e| AppError::Other(format!("Invalid keystream: {}", e)))?;
                Some(encode_keystream_hex(&bytes))
            }
            None => None,
        };

        let mut data = self.data.write();
        if let Some(pdkey) = pdkey {
            data.mouflon_keys
                .keys
                .insert(pkey.to_string(), pdkey.to_string());
        }
        if let Some(hex) = keystream_hex {
            data.mouflon_keys.keystreams.insert(pkey.to_string(), hex);
        }
        data.mouflon_keys.manual_updated_at = Some(chrono::Local::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 删除指定 pkey 的全部 Mouflon 密钥（pdkey 与 keystream），更新 manual_updated_at 并保存。
    /// Remove all Mouflon keys (pdkey and keystream) of the given pkey, update manual_updated_at, and save.
    pub fn remove_mouflon_key(&self, pkey: &str) -> Result<()> {
        let mut data = self.data.write();
        data.mouflon_keys.keys.remove(pkey);
        data.mouflon_keys.keystreams.remove(pkey);
        data.mouflon_keys.manual_updated_at = Some(chrono::Local::now().to_rfc3339());
        drop(data);
        self.save()
    }

    /// 从 Cloudflare Worker 同步 Mouflon 密钥（pdkey 表与 keystream 表）。
    ///
    /// Worker 返回 `{ "pkey2pdkey": {..}, "pkey2keystream": {..}, "updated_at": "<hash>" }`，
    /// `updated_at` 是两份密钥表内容拼接后的 SHA-256 哈希，任一表变化它就变化。这里把它当作
    /// 不透明字符串，与本地保存的 `synced_hash` 比对，不再按时间戳判断：
    ///   - 相同 → 数据源没变，只补回本地缺失的条目（例如手动删除的）；没有补任何条目返回 false
    ///   - 不同 → 补入本地没有的条目，记录新的 `synced_hash` 并更新 `auto_synced_at`，返回 true
    ///
    /// 本地已有的条目不会被覆盖（手动设置的优先）。为兼容旧版 Worker，`pkey2pdkey` 也接受旧
    /// 字段名 `keys`；无效的 keystream（不是十六进制或太短）会被忽略。
    ///
    /// Sync Mouflon keys (the pdkey table and the keystream table) from the Cloudflare Worker.
    ///
    /// The Worker returns `{ "pkey2pdkey": {..}, "pkey2keystream": {..}, "updated_at": "<hash>" }`
    /// where `updated_at` is the SHA-256 hash of both tables' contents concatenated, changing
    /// whenever either table changes. It is treated as an opaque string and compared with the
    /// locally stored `synced_hash` instead of judging by timestamp:
    ///   - Equal → the source is unchanged; only entries missing locally (e.g. manually deleted
    ///     ones) are restored; returns false when nothing was restored
    ///   - Different → entries missing locally are added, the new `synced_hash` is recorded and
    ///     `auto_synced_at` updated; returns true
    ///
    /// Entries already present locally are never overwritten (manual settings win). For older
    /// Workers, `pkey2pdkey` also accepts the former field name `keys`; invalid keystreams (not
    /// hex, or too short) are ignored.
    pub async fn sync_mouflon_keys_from_worker(
        &self,
        worker_url: &str,
        auth_token: Option<&str>,
    ) -> Result<bool> {
        #[derive(Deserialize)]
        struct WorkerResponse {
            #[serde(default, alias = "keys")]
            pkey2pdkey: HashMap<String, String>,
            #[serde(default)]
            pkey2keystream: HashMap<String, String>,
            updated_at: String,
        }

        // 同步客户端：ECH 开启且 worker 为 https 域名时先走 ECH 通道
        // （ech_client_for 自负责借用配置解析/构建/探活，无通道返回 None
        // 直接用 base）；ECH 腿传输失败或 5xx → 反馈缓存并回退普通连接
        // 重试一次；4xx 不重试（如授权失败，换通道结果一样）。
        // Sync client: with ECH enabled and an https worker host, try the
        // ECH channel first (ech_client_for resolves/builds/probes the
        // channel itself and yields None when unavailable, in which case
        // the base client is used straight away); a transport failure or
        // 5xx on the ECH leg feeds the ECH cache and retries once on the
        // plain client; 4xx doesn't retry (e.g. a bad token fails the
        // same way over plain).
        let base = mouflon_sync_client(None)?;
        let mut client = base.clone();
        let mut ech_ctx: Option<(EchClients, String)> = None;
        if self.get_settings().ech_enabled
            && let Some(host) = https_host(worker_url)
        {
            let ech = EchClients::new(base.clone(), |t| mouflon_sync_client(Some(t)));
            if let Some(c) = ech.ech_client_for(&host).await {
                tracing::debug!(
                    "{}",
                    crate::tl!("scheduler.mouflonSyncEch", host = host)
                );
                client = c;
                ech_ctx = Some((ech, host));
            }
        }

        let mut result = send_worker_request(&client, worker_url, auth_token).await;
        if let Some((ech, host)) = &ech_ctx {
            let demote = match &result {
                Err(_) => true,
                Ok(r) => r.status().is_server_error(),
            };
            if demote {
                match &result {
                    Ok(_) => ech.note_server_error(host).await,
                    Err(_) => ech.evict(host).await,
                }
                tracing::info!(
                    "{}",
                    crate::tl!("scheduler.mouflonSyncEchFallback", host = host)
                );
                result = send_worker_request(&base, worker_url, auth_token).await;
            }
        }

        let resp = result.map_err(|e| {
            AppError::Other(format!("Worker 请求失败: {}", format_send_error(&e)))
        })?;

        if !resp.status().is_success() {
            return Err(AppError::Other(format!(
                "Worker 返回错误状态: {}",
                resp.status()
            )));
        }

        let body: WorkerResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("Worker 响应解析失败: {}", e)))?;

        // updated_at 是内容哈希，只比较是否相等，不当作时间解析
        // updated_at is a content hash: compare for equality only, never parse it as a time
        let hash_changed =
            self.data.read().mouflon_keys.synced_hash.as_deref() != Some(body.updated_at.as_str());

        // keystream 只收合法的十六进制，统一成小写；数据源变化时才提示被忽略的条目，
        // 避免每次同步都重复刷同样的警告
        // Only accept valid hex keystreams, normalized to lowercase; ignored entries are reported
        // only when the source changed, so the same warning is not repeated on every sync
        let mut keystreams: HashMap<String, String> = HashMap::new();
        for (pkey, text) in body.pkey2keystream {
            match parse_keystream_hex(&text) {
                Ok(bytes) => {
                    keystreams.insert(pkey, encode_keystream_hex(&bytes));
                }
                Err(reason) if hash_changed => {
                    tracing::warn!(
                        "{}",
                        crate::tl!(
                            "scheduler.mouflonKeystreamInvalid",
                            pkey = pkey,
                            reason = reason
                        )
                    );
                }
                Err(_) => {}
            }
        }

        // 只补入本地没有的条目，已有的（含手动设置的）保持不变
        // Only add entries missing locally; existing ones (including manual settings) stay as they are
        let mut data = self.data.write();
        let mut added = 0usize;
        for (pkey, pdkey) in body.pkey2pdkey {
            if let std::collections::hash_map::Entry::Vacant(e) = data.mouflon_keys.keys.entry(pkey)
            {
                e.insert(pdkey);
                added += 1;
            }
        }
        for (pkey, keystream) in keystreams {
            if let std::collections::hash_map::Entry::Vacant(e) =
                data.mouflon_keys.keystreams.entry(pkey)
            {
                e.insert(keystream);
                added += 1;
            }
        }

        if hash_changed {
            data.mouflon_keys.synced_hash = Some(body.updated_at);
            data.mouflon_keys.auto_synced_at = Some(chrono::Local::now().to_rfc3339());
        } else if added == 0 {
            // 数据源没变，本地也没有缺失的条目
            // Source unchanged and nothing missing locally
            return Ok(false);
        }
        drop(data);
        self.save()?;
        Ok(true)
    }
}

/// 构建密钥同步用的 HTTP 客户端：`ech` 为 Some 时套用借用 ECH 栈并把目标
/// 主机钉到 CF 任播 IP（与 API/CDN 客户端同一套 `crate::ech::apply_ech`）；
/// 否则为普通客户端。两端共 15s 超时，与原实现一致。
/// Build the HTTP client for key sync: when `ech` is Some the borrowed ECH
/// stack applies and the target host pins to CF anycast IPs (the same
/// `crate::ech::apply_ech` as the API/CDN clients); otherwise a plain
/// client. Both flavors share the original 15s timeout.
fn mouflon_sync_client(ech: Option<&crate::ech::EchTarget>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15));
    if let Some(t) = ech {
        builder = crate::ech::apply_ech(builder, t);
    }
    builder.build().map_err(|e| AppError::Other(e.to_string()))
}

/// 发送一次密钥同步请求（可带 Bearer 授权头），原始 `reqwest` 结果留给
/// 调用方区分传输错误与 HTTP 状态。
/// Sends one key-sync request (optional Bearer Authorization header); the
/// raw `reqwest` result is left for the caller to tell transport errors
/// apart from HTTP statuses.
async fn send_worker_request(
    client: &reqwest::Client,
    worker_url: &str,
    auth_token: Option<&str>,
) -> std::result::Result<reqwest::Response, reqwest::Error> {
    let mut req = client.get(worker_url);
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    req.send().await
}
