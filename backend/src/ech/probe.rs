//! ECH 探活：对目标主机做裸 TLS+ECH 握手，并在同一连接上发一次最小 HTTP
//! 请求，验证「CF 边缘能为 inner SNI 完成握手且能回源」。
//!
//! ECH probing: a bare TLS+ECH handshake plus a minimal HTTP request on the
//! same connection, verifying "the CF edge can handshake for this inner SNI
//! and route to its origin".

use rustls::client::EchStatus;
use rustls::internal::msgs::handshake::EchConfigPayload;
use rustls_pki_types::ServerName;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// ECH 客户端建好后单次探活步骤的超时 / Timeout for a single probe step after build
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 探活结果。/ Probe outcome.
pub(super) enum ProbeOutcome {
    /// ECH 被接受且边缘能回源该主机 / ECH accepted and the edge can route the host
    Good,
    /// 连接、TLS 或回源失败——目标按非 ECH 主机处理（负缓存回退）。
    /// 注意：CF 边缘对任意 inner SNI 都会接受共享 ECH 密钥的 offer，因此
    /// 「握手成功」不代表「边缘能回源该主机」——zone-miss 主机会在请求层
    /// 返回 HTTP 530，探活必须看过回源结果才算通过。
    /// Connect, TLS, or origin failure — treat the target as non-ECH
    /// (negative-cache fallback). Note: the CF edge accepts the shared ECH
    /// key for ANY inner SNI, so a "successful handshake" does NOT mean the
    /// host can be routed — zone-miss hosts return HTTP 530 at request time;
    /// the probe must see a real origin response to pass.
    Bad,
    /// 服务端拒绝 ECH：`Some(payloads)` = 附了 retry_configs（借来的密钥
    /// 已轮换，可就地换新）；`None` = 未附，只能经 DoH 重新解析。
    /// Server rejected ECH: `Some(payloads)` = retry_configs attached (the
    /// borrowed key rotated — can be swapped in place); `None` = none sent,
    /// the borrowed config must be re-resolved via DoH.
    Rejected(Option<Vec<EchConfigPayload>>),
}

/// 探活：握手 + `ech_status` 断言 + 一次 `GET /` 回源检查（状态码 <500 才算
/// 边缘能路由；5xx 为 CF 边缘→源站失败，如 530 zone-miss）。不走 3xx 之后的
/// 路径——任何 HTTP 响应都证明边缘能路由该主机，重定向目标是业务层的事。
/// Probe: handshake + `ech_status` assertion + one `GET /` routability check
/// (status <500 means the edge can route; 5xx is a CF edge→origin failure
/// such as 530 zone-miss). Redirects are not followed — any HTTP response
/// proves the edge routes the host; where it leads is the app's business.
pub(super) async fn probe_ech(
    tls: &rustls::ClientConfig,
    host: &str,
    addrs: &[SocketAddr],
) -> ProbeOutcome {
    let Ok(name) = ServerName::try_from(host.to_string()) else {
        return ProbeOutcome::Bad;
    };
    // 探活强制 HTTP/1.1：裸写 GET / 后只需解析一行状态码 / Force HTTP/1.1
    // for the probe: after a raw GET / only the status line needs parsing
    let mut h1 = tls.clone();
    h1.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(Arc::new(h1));
    let mut rejected: Option<Option<Vec<EchConfigPayload>>> = None;
    for addr in addrs {
        let result = tokio::time::timeout(PROBE_TIMEOUT, async {
            let stream = TcpStream::connect(addr).await?;
            connector.connect(name.clone(), stream).await
        })
        .await;
        match result {
            Ok(Ok(mut stream)) => {
                // 严格断言 Accepted——rustls 在未接受时本应硬失败，显式检查
                // 防御未来行为变化 / Assert Accepted strictly — rustls already
                // hard-fails on non-acceptance; the explicit check guards
                // against future behavior changes
                if stream.get_ref().1.ech_status() != EchStatus::Accepted {
                    return ProbeOutcome::Bad;
                }
                return http_probe(&mut stream, host).await;
            }
            Ok(Err(e)) => {
                // ECH 被拒：记下 retry_configs（若有），其余错误继续试下个地址 /
                // ECH rejected: record retry_configs (if any); other errors
                // just try the next address
                if let Some(payloads) = ech_rejection_configs(&e) {
                    rejected = Some(payloads);
                }
            }
            Err(_) => {}
        }
    }
    match rejected {
        Some(payloads) => ProbeOutcome::Rejected(payloads),
        None => ProbeOutcome::Bad,
    }
}

/// 竞速测速目标：CF 自带测速端点。任何 CF 边缘都能按 inner SNI 路由它
/// （无 zone-miss），且 `__down?bytes=N` 返回统一大小对象——所有候选
/// 地址在同一主机同一对象上比吞吐，度量公平。
/// Race speed target: CF's own speed endpoint. Every CF edge can route it
/// by inner SNI (no zone-miss), and `__down?bytes=N` serves a uniform-size
/// object — every candidate addr races throughput on the SAME host/object,
/// keeping the metric fair.
const RACE_PROBE_HOST: &str = "speed.cloudflare.com";
/// 竞速读取的体字节上限（到量即收尾计时）/ Body cap for the race read
/// (timing stops once this much arrives)
const RACE_BODY_CAP: usize = 256 * 1024;
/// 竞速单地址全程上限（握手+读体）/ Overall deadline for one race probe
/// (handshake + body read)
const RACE_TIMEOUT: Duration = Duration::from_secs(8);

/// 对**单个**地址做限速 ECH 测速：TCP 连接 + ECH 握手 +
/// `GET /__down?bytes=1MB`，读体至 [`RACE_BODY_CAP`]（或 EOF/超时），
/// 返回 `(全程耗时, 读到的总字节数)`。字节数够大时调用方按真实吞吐
/// 计分；只读到小体（如端点 404 的错误页）则降级为握手延迟样本。
/// Speed probe of a **single** address: TCP connect + ECH handshake +
/// `GET /__down?bytes=1MB`, reads the body up to [`RACE_BODY_CAP`] (or
/// EOF/timeout), returning `(total elapsed, total bytes read)`. Enough
/// bytes → a real throughput sample for scoring; a small body (e.g. a 404
/// error page) degrades to a handshake-latency sample.
pub(super) async fn probe_addr_speed(
    tls: &rustls::ClientConfig,
    addr: SocketAddr,
) -> Option<(Duration, usize)> {
    let name = ServerName::try_from(RACE_PROBE_HOST.to_string()).ok()?;
    // 与 probe_ech 一致强制 HTTP/1.1 / Force HTTP/1.1 like probe_ech
    let mut h1 = tls.clone();
    h1.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = TlsConnector::from(Arc::new(h1));
    let t0 = Instant::now();
    tokio::time::timeout(RACE_TIMEOUT, async {
        let stream = TcpStream::connect(addr).await?;
        let mut stream = connector.connect(name.clone(), stream).await?;
        if stream.get_ref().1.ech_status() != EchStatus::Accepted {
            return Err(std::io::Error::other("ECH not accepted"));
        }
        let req = format!(
            "GET /__down?bytes=1048576 HTTP/1.1\r\nHost: {RACE_PROBE_HOST}\r\nUser-Agent: curl/8.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await?;
        // 读到够量或 EOF——响应头一并计入（<1KB 对 256KB 目标可忽略）/
        // Read until cap or EOF — response headers counted too (<1KB is
        // negligible against the 256KB cap)
        let mut buf = [0u8; 16 * 1024];
        let mut got = 0usize;
        while got < RACE_BODY_CAP {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        std::io::Result::Ok((t0.elapsed(), got))
    })
    .await
    .ok()
    .and_then(|r| r.ok())
}

/// 在已建立的 ECH 连接上发最小 `GET /`：状态码 <500 判 `Good`（边缘能回源），
/// ≥500 或读写/解析失败判 `Bad`。/ Sends a minimal `GET /` on the established
/// ECH connection: a status <500 means `Good` (the edge routes), ≥500 or any
/// IO/parse failure means `Bad`.
async fn http_probe(stream: &mut TlsStream<TcpStream>, host: &str) -> ProbeOutcome {
    let req = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: curl/8.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    let mut buf = [0u8; 512];
    let io = async {
        stream.write_all(req.as_bytes()).await?;
        stream.read(&mut buf).await
    };
    let Ok(Ok(n)) = tokio::time::timeout(PROBE_TIMEOUT, io).await else {
        return ProbeOutcome::Bad;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let Some(line) = head.lines().next() else {
        return ProbeOutcome::Bad;
    };
    // 解析 "HTTP/1.1 NNN" / parse "HTTP/1.1 NNN"
    let code = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    match code {
        Some(c) if c < 500 => ProbeOutcome::Good,
        _ => ProbeOutcome::Bad,
    }
}

/// 从握手 io 错误中提取 ECH 拒绝信息（tokio-rustls 把 rustls 错误包成
/// `InvalidData` 内层错误）。返回 `Some(payloads)` 表示确为 ECH 拒绝，
/// `payloads` 即服务端附带的 retry_configs（可为 None）。
/// Extracts the ECH rejection detail from a handshake io error (tokio-rustls
/// wraps the rustls error as an `InvalidData` inner). `Some(payloads)` means
/// it really was an ECH rejection; `payloads` are the server's retry_configs
/// (possibly None).
fn ech_rejection_configs(e: &std::io::Error) -> Option<Option<Vec<EchConfigPayload>>> {
    match e.get_ref()?.downcast_ref::<rustls::Error>()? {
        rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::ServerRejectedEncryptedClientHello(payloads),
        ) => Some(payloads.clone()),
        _ => None,
    }
}
