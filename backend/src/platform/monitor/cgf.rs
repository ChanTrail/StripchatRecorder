//! CamGirlFinder 排期助手子模块：在线规律获取与自适应轮询间隔计算。
//! CamGirlFinder schedule helpers: online-schedule fetching and the
//! schedule-adaptive poll interval calculation.

/// 从 camgirlfinder.net 获取指定 StripChat 主播的历史在线规律（schedule）。
///
/// 接口：`GET https://api.camgirlfinder.net/models/sc/{username}`
///
/// 返回的 `schedule` 字段是 7×48 的 float 矩阵（UTC 时区）：
/// - 第一维 0–6：星期（0 = 周日）
/// - 第二维 0–47：每天的 48 个 30 分钟时段
/// - 值域 [0.0, 1.0]：过去 28 天内该时段有在线记录的频率
///
/// 网络失败或用户在 CGF 不存在时静默返回 `None`，不影响正常录制流程。
///
/// Fetches the historical online schedule for a StripChat streamer from
/// camgirlfinder.net. Returns `None` on network failure or if the account
/// is not found in CGF — caller should treat this as "no schedule data".
pub async fn cgf_fetch_schedule(username: &str, proxy: Option<&str>) -> Option<Vec<Vec<f32>>> {
    // 用户名已经过 Stripchat API 验证，只含字母数字和下划线，无需额外 URL 编码
    // Username was already validated by Stripchat API and only contains
    // alphanumeric characters and underscores — no URL encoding needed.
    let url = format!("https://api.camgirlfinder.net/models/sc/{}", username);

    let mut builder = reqwest::Client::builder()
        .user_agent("StripchatRecorder/1.0")
        .timeout(std::time::Duration::from_secs(20));

    if let Some(proxy_url) = proxy.filter(|s| !s.is_empty()) {
        match reqwest::Proxy::all(proxy_url) {
            Ok(p) => {
                builder = builder.proxy(p);
            }
            Err(e) => {
                tracing::warn!("{}", crate::tl!("monitor.cgfProxyInvalid", error = e));
            }
        }
    }

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "{}",
                crate::tl!("monitor.cgfClientFailed", username = username, error = e)
            );
            return None;
        }
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "{}",
                crate::tl!("monitor.cgfRequestFailed", username = username, error = e)
            );
            return None;
        }
    };

    if !resp.status().is_success() {
        tracing::debug!(
            "{}",
            crate::tl!(
                "monitor.cgfHttpError",
                status = resp.status().as_u16(),
                username = username
            )
        );
        return None;
    }

    // 只取 schedule 字段，避免反序列化整个响应体
    // Only extract the schedule field to avoid deserializing the full response body
    #[derive(serde::Deserialize)]
    struct CgfProfile {
        schedule: Option<Vec<Vec<f32>>>,
    }

    let profile: CgfProfile = match resp.json().await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "{}",
                crate::tl!("monitor.cgfParseFailed", username = username, error = e)
            );
            return None;
        }
    };

    let raw = match profile.schedule {
        Some(s) => s,
        None => {
            tracing::debug!(
                "{}",
                crate::tl!("monitor.cgfScheduleNull", username = username)
            );
            return None;
        }
    };

    // 必须恰好 7 行（7 天）；每行截断或补零到 48 个时段以容错边界情况
    // Must be exactly 7 rows (days); each row is truncated or zero-padded to 48 slots
    if raw.len() != 7 {
        return None;
    }
    let normalized: Vec<Vec<f32>> = raw
        .into_iter()
        .map(|mut row| {
            row.resize(48, 0.0); // 不足48补零；超出48截断
            row.truncate(48);
            row
        })
        .collect();
    Some(normalized)
}

/// 根据 schedule 矩阵和当前本地时间，计算该主播本轮使用的自适应轮询间隔。
///
/// ## 算法
///
/// 取当前本地时间的星期 + 30 分钟时段对应的活跃度值 `a ∈ [0.0, 1.0]`：
///
/// - `a ≥ 0.8`：活跃度足够高，直接使用用户配置的 `base_secs`，不做节流。
/// - `a ∈ [0.0, 0.8)`：通过平方根曲线将活跃度映射到 `[max_secs, base_secs]`：
///
/// ```text
/// interval = max_secs - (max_secs - base_secs) × √(a / 0.8)
/// ```
///
/// 平方根曲线使低活跃段间隔更激进地拉长，靠近 0.8 时平滑收敛到 `base_secs`。
///
/// | 活跃度 | 间隔（base=60s, max=200s）|
/// |--------|--------------------------|
/// | 0.00   | 200 s                    |
/// | 0.05   | 165 s                    |
/// | 0.20   | 130 s                    |
/// | 0.40   | 101 s                    |
/// | 0.60   |  79 s                    |
/// | 0.79   |  61 s                    |
/// | ≥ 0.80 | base_secs（用户设置）     |
///
/// 无 schedule 数据时返回 `base_secs`（不节流）。
///
/// ## Parameters
/// - `schedule` – 7×48 活跃度矩阵，`None` 表示尚未获取。
/// - `base_secs` – 用户配置的轮询间隔（秒）。
/// - `max_secs`  – 低活跃时段允许的最大间隔（秒），调用方固定传入 200（`SCHEDULE_MAX_SECS`）。
///
/// Computes the adaptive poll interval for a streamer based on its schedule matrix
/// and the current local time (weekday + 30-minute bucket). `max_secs` is the longest
/// interval allowed in low-activity buckets; the caller always passes 200
/// (`SCHEDULE_MAX_SECS`). See inline comments for the formula.
pub fn schedule_poll_interval(schedule: Option<&[Vec<f32>]>, base_secs: u64, max_secs: u64) -> u64 {
    use chrono::{Datelike as _, Timelike as _};

    let sched = match schedule {
        Some(s) => s,
        None => return base_secs, // 无数据：不节流
    };

    let now = chrono::Local::now();
    // chrono weekday: Mon=0…Sun=6;  CGF convention: Sun=0…Sat=6
    let cgf_day = now.weekday().num_days_from_sunday() as usize;
    let bucket = (now.hour() * 2 + now.minute() / 30) as usize;
    let activity = sched
        .get(cgf_day)
        .and_then(|row| row.get(bucket))
        .copied()
        .unwrap_or(1.0);

    // 活跃度 ≥ 0.8 直接用用户设置值，不节流
    if activity >= 0.8 {
        return base_secs;
    }

    // 平方根曲线映射：√(a / 0.8) ∈ [0, 1)
    let t = (activity / 0.8).sqrt();
    // interval = max - (max - base) × t，随 t 增大从 max 收敛到 base
    let interval = max_secs as f64 - (max_secs as f64 - base_secs as f64) * t as f64;
    interval.round() as u64
}
