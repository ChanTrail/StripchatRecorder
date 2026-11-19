//! HTTP 客户端构建与 URL/错误处理工具（`stripchat` 内部共享）。
//! HTTP client construction and URL/error utilities (shared within `stripchat`).

use crate::core::error::{AppError, Result};
use reqwest::Client;

/// 模拟浏览器的 User-Agent / Browser-mimicking User-Agent
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36";

/// 从 URL 提取 ECH 适用的 https 域名（ECH 只对 TLS 生效；`domain()` 自动排除
/// IP 字面量——ECH 对 IP 主机没有意义）。
/// Extracts the https domain the ECH applies to from a URL (ECH only applies
/// to TLS; `domain()` already excludes IP literals — ECH is meaningless for
/// IP hosts).
pub(crate) fn https_host(url: &str) -> Option<String> {
    let u = reqwest::Url::parse(url).ok()?;
    if u.scheme() != "https" {
        return None;
    }
    u.domain().map(|s| s.to_string())
}

/// 构建用于 CDN 分片下载的 HTTP 客户端（支持代理，启用 TCP keepalive）。
/// `ech` 为 Some 时用借用的 ECH 栈并把该主机钉到 CF 任播 IP。
/// Build an HTTP client for CDN segment downloads (supports proxy, enables
/// TCP keepalive). When `ech` is Some the client uses the borrowed ECH stack
/// and pins the host to the CF anycast IPs.
pub(super) fn build_client(
    proxy_url: Option<&str>,
    ech: Option<&crate::ech::EchTarget>,
) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .connection_verbose(false);

    if let Some(proxy) = proxy_url
        && !proxy.is_empty()
    {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
    } else {
        builder = builder.no_proxy();
    }
    if let Some(t) = ech {
        builder = apply_ech(builder, t);
    }

    Ok(builder.build()?)
}

/// 格式化 reqwest send 错误，带上完整 source 原因链：`top → source → ...`。
/// `e.to_string()` 只给顶层 "error sending request"，看不出是 DNS 失败、
/// 连接重置、超时还是代理拒绝。
/// Format a reqwest send error with its full source chain:
/// `top → source → ...`. `e.to_string()` only gives the top-level
/// "error sending request", hiding whether it was DNS failure, connection
/// reset, timeout, or proxy refusal.
pub(crate) fn format_send_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(" → ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// 应用借用式 ECH：统一委托给 `crate::ech::apply_ech`（预配置 TLS 栈 +
/// 钉接 CF 任播 IP + www↔apex 配对钉接）。
/// Applies borrowed ECH: delegates to `crate::ech::apply_ech` (preconfigured
/// TLS stack + CF anycast pins + the www↔apex paired pin).
fn apply_ech(
    builder: reqwest::ClientBuilder,
    t: &crate::ech::EchTarget,
) -> reqwest::ClientBuilder {
    crate::ech::apply_ech(builder, t)
}

/// API 客户端 HTTP/2 PING 的发送间隔（连接空闲时也发送）。
/// Interval between HTTP/2 PINGs on API client connections (also sent while idle).
const API_H2_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
/// 等待 HTTP/2 PING 回应的超时，超时即关闭该连接。
/// Timeout waiting for an HTTP/2 PING ack; the connection is closed when it expires.
const API_H2_PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 构建用于 API 请求的 HTTP 客户端（支持代理，ALPN 自动协商→HTTP/2，附加请求优先级头）。
///
/// HTTP 版本跟随 ALPN 协商结果（reqwest 默认 `HttpVersionPref::All`）：wire 上
/// 提供 `h2, http/1.1`，CF 边缘总是选 h2 → 实际走 HTTP/2；不用
/// `http2_prior_knowledge` 强制——先验 h2 会在 ALPN 未协商出 h2 时（如 MITM
/// 剥掉 ALPN、仅 h1 的隧道）仍发 h2 preface 直接协议错误，自动协商则能正确回落
/// h1，且与浏览器行为一致（浏览器从不做 TLS 上的 h2 先验）。
///
/// 状态轮询跨轮复用这个客户端，HTTP/2 连接会在两轮之间空闲着；如果对端或中间网络悄悄断掉了
/// 空闲连接，下一轮同时进行的请求（共用这一条连接）会一起失败在它上面。因此开启：
/// - TCP keepalive（15 秒，与 CDN 客户端一致）
/// - HTTP/2 PING：每 [`API_H2_PING_INTERVAL`] 发送一次（空闲时也发），
///   [`API_H2_PING_TIMEOUT`] 内没有回应就关闭连接，连接池随之丢弃它，下一轮直接新建连接。
///   经代理时 PING 走端到端隧道，能发现对端已失效的连接。
///
/// Build an HTTP client for API requests (supports proxy, ALPN auto-negotiation → HTTP/2,
/// request priority header).
///
/// The HTTP version follows the negotiated ALPN (reqwest's default `HttpVersionPref::All`):
/// the wire offers `h2, http/1.1`, CF edges always pick h2 → it still speaks HTTP/2.
/// `http2_prior_knowledge` is deliberately NOT used — prior-knowledge h2 sends the h2
/// preface even when ALPN didn't yield h2 (e.g. a MITM stripping ALPN or an h1-only
/// tunnel) and breaks the connection outright, while auto-negotiation falls back to
/// h1 correctly and matches what browsers do (browsers never do h2 prior knowledge
/// over TLS).
///
/// Status polling reuses this client across rounds, so its HTTP/2 connection sits idle between
/// rounds; if the peer or something in between silently drops the idle connection, the next
/// round's concurrent requests (which share that one connection) all fail on it. So this enables:
/// - TCP keepalive (15 s, same as the CDN client)
/// - HTTP/2 PING every [`API_H2_PING_INTERVAL`] (also while idle); without an ack within
///   [`API_H2_PING_TIMEOUT`] the connection is closed and dropped from the pool, so the next round
///   opens a new one. Through a proxy the PING travels end to end inside the tunnel, so it detects
///   connections the far end has dropped.
///
/// 注意：`Priority: u=0, i` 头必须放**每个请求的 .header()** 上，不能用
/// `default_headers`——实测（test/ech 最小化复现）default_headers 改变头在
/// wire 上的顺序，CF WAF 按浏览器指纹校验头序会把 /api/front/v2/* 判成
/// 机器人返回 403。
/// NOTE: the `Priority: u=0, i` header must be set per request via
/// `.header()` — NOT via `default_headers`. Verified in the minimal ECH
/// repro (test/): default_headers changes the wire order of headers, and CF
/// WAF's browser-fingerprint header-order check then 403s /api/front/v2/*.
pub(super) fn build_api_client(
    proxy_url: Option<&str>,
    ech: Option<&crate::ech::EchTarget>,
) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        // HTTP 版本跟随 ALPN 协商（默认 All）：wire 提供 h2,http/1.1，CF 总选 h2 → HTTP/2；
        // ALPN 未出 h2 时自动回落 h1，不用先验 h2 硬闯 / HTTP version follows the
        // negotiated ALPN (default All): wire offers h2,http/1.1, CF always picks h2
        // → HTTP/2; falls back to h1 when h2 isn't negotiated — no prior-knowledge h2.
        .http2_keep_alive_interval(API_H2_PING_INTERVAL)
        .http2_keep_alive_timeout(API_H2_PING_TIMEOUT)
        .http2_keep_alive_while_idle(true);
    if let Some(t) = ech {
        builder = apply_ech(builder, t);
    }

    if let Some(proxy) = proxy_url
        && !proxy.is_empty()
    {
        builder =
            builder.proxy(reqwest::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
        return Ok(builder.build()?);
    }
    builder = builder.no_proxy();
    Ok(builder.build()?)
}
