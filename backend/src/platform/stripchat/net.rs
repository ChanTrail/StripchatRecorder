//! HTTP 客户端构建与 URL/错误处理工具（`stripchat` 内部共享）。
//!
//! 全链路走 wreq（BoringSSL，Chrome 指纹）：直连腿与 ECH 腿同栈——ECH 腿
//! 只是额外注入了 `TlsOptions.ech_config_list`（vendored wreq 私有补丁）
//! 与边缘钉接地址，TLS/H2/头部指纹与直连一致。
//!
//! HTTP client construction plus URL/error utilities (shared within
//! `stripchat`).
//!
//! Everything runs on wreq (BoringSSL, Chrome fingerprint): direct and ECH
//! legs share the stack — an ECH leg merely injects
//! `TlsOptions.ech_config_list` (the vendored-wreq private patch) plus
//! pinned edge addresses, keeping TLS/H2/header fingerprints identical to
//! direct traffic.

use crate::core::error::{AppError, Result};

/// 模拟浏览器的 User-Agent——与 [`EMULATION`] 的 Chrome 版本保持一致，
/// UA 与 TLS 指纹不匹配本身就是 bot 信号。
/// Browser-mimicking User-Agent — kept on the same Chrome version as
/// [`EMULATION`]; a UA/TLS-fingerprint mismatch is itself a bot signal.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// 直连腿的浏览器模拟档：TLS(JA3/JA4) + H2(Akamai) + 头部序指纹。
/// Browser emulation profile for direct legs: TLS (JA3/JA4) + H2
/// (Akamai) + header-order fingerprints.
const EMULATION: wreq_util::Profile = wreq_util::Emulation::Chrome149;

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

/// 构建用于 CDN 分片下载的 HTTP 客户端（wreq + Chrome 指纹）。
/// `ech` 为 Some 时套用借用 ECH 栈并把目标主机钉到 CF 任播 IP——供
/// [`crate::ech::EchClients`] 的按目标重建闭包调用。
/// Build the HTTP client for CDN segment downloads (wreq + Chrome
/// fingerprint). With `ech` Some the borrowed ECH stack applies and the
/// target host pins to CF anycast IPs — called by
/// [`crate::ech::EchClients`]' per-target rebuild closure.
pub(super) fn build_client(
    proxy_url: Option<&str>,
    ech: Option<&crate::ech::EchTarget>,
) -> Result<wreq::Client> {
    let mut builder = wreq::Client::builder()
        .emulation(EMULATION)
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .connection_verbose(false);

    if let Some(proxy) = proxy_url
        && !proxy.is_empty()
    {
        builder =
            builder.proxy(wreq::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
    } else {
        builder = builder.no_proxy();
    }
    // ECH 的 tls_options 整组替换，必须在 emulation() 之后 /
    // ECH's tls_options replace the whole group — must come after emulation()
    if let Some(t) = ech {
        builder = crate::ech::apply_ech(builder, t);
    }

    builder.build().map_err(|e| AppError::Other(e.to_string()))
}

/// API 客户端 HTTP/2 PING 的发送间隔（连接空闲时也发送）。
/// Interval between HTTP/2 PINGs on API client connections (also sent while idle).
const API_H2_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);
/// 等待 HTTP/2 PING 回应的超时，超时即关闭该连接。
/// Timeout waiting for an HTTP/2 PING ack; the connection is closed when it expires.
const API_H2_PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 构建用于 API 请求的 HTTP 客户端（wreq + Chrome 指纹）；`ech` 语义同
/// [`build_client`]。
///
/// HTTP 版本跟随 ALPN 协商：浏览器档的 ALPN 提供 h2,http/1.1，CF 边缘总选
/// h2 → 实际 HTTP/2；ALPN 未出 h2 时自动回落 h1（浏览器行为同理，从不做
/// TLS 上的 h2 先验）。
///
/// 状态轮询跨轮复用这个客户端，HTTP/2 连接会在两轮之间空闲着；若对端或中间
/// 网络悄悄断掉空闲连接，下一轮共用连接的请求会一起失败。经
/// [`wreq::Http2Options`] 开启 HTTP/2 PING（与 Chrome 的 H2 指纹参数独立——
/// PING 帧是运行时行为，不进入握手指纹）。
///
/// Build the HTTP client for API requests (wreq + Chrome fingerprint);
/// `ech` semantics match [`build_client`].
///
/// The HTTP version follows ALPN negotiation: the browser profile offers
/// h2,http/1.1 and CF edges always pick h2 → HTTP/2; falls back to h1 when
/// h2 isn't negotiated (what browsers do — no prior-knowledge h2 over TLS).
///
/// Status polling reuses this client across rounds so its H2 connection idles
/// between rounds; if the peer or something in between silently drops it, the
/// next round's requests sharing that connection all fail. HTTP/2 PINGs via
/// [`wreq::Http2Options`] detect dead idle conns (a runtime behavior, not part
/// of the handshake fingerprint).
pub(super) fn build_api_client(
    proxy_url: Option<&str>,
    ech: Option<&crate::ech::EchTarget>,
) -> Result<wreq::Client> {
    // 在模拟档的完整 H2 指纹上只补 keep-alive 参数：`.http2_options()` 是
    // 整组替换，不能拿来叠默认值——会抹掉 Chrome 的 H2 指纹。PING 帧是
    // 运行时行为，不进握手指纹。
    // Patch only the keep-alive fields on the profile's full H2
    // fingerprint: `.http2_options()` replaces the whole group — layering
    // builder defaults over it would erase the Chrome H2 fingerprint.
    // PING frames are runtime behavior, not part of the handshake fingerprint.
    let mut emulation = wreq::IntoEmulation::into_emulation(EMULATION);
    if let Some(h2) = &mut emulation.http2_options {
        h2.keep_alive_interval = Some(API_H2_PING_INTERVAL);
        h2.keep_alive_timeout = API_H2_PING_TIMEOUT;
        h2.keep_alive_while_idle = true;
    }
    let mut builder = wreq::Client::builder()
        .emulation(emulation)
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .tcp_keepalive(std::time::Duration::from_secs(15));
    if let Some(proxy) = proxy_url
        && !proxy.is_empty()
    {
        builder =
            builder.proxy(wreq::Proxy::all(proxy).map_err(|e| AppError::Other(e.to_string()))?);
    } else {
        builder = builder.no_proxy();
    }
    if let Some(t) = ech {
        builder = crate::ech::apply_ech(builder, t);
    }
    builder.build().map_err(|e| AppError::Other(e.to_string()))
}

/// 格式化 HTTP 发送错误，带上完整 source 原因链：`top → source → ...`。
/// `e.to_string()` 只给顶层 "error sending request"，看不出是 DNS 失败、
/// 连接重置、超时还是代理拒绝。对 `Error` 泛型——reqwest 与 wreq 共用
/// 同一实现。
/// Format an HTTP send error with its full source chain:
/// `top → source → ...`. `e.to_string()` only gives the top-level
/// "error sending request", hiding whether it was DNS failure, connection
/// reset, timeout, or proxy refusal. Generic over `Error` — reqwest and
/// wreq share the same implementation.
pub(crate) fn format_send_error(e: &(impl std::error::Error + ?Sized)) -> String {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(" → ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}
