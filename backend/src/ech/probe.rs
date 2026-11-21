//! ECH 探活：对目标主机做裸 TLS+ECH 握手，并在同一连接上发一次最小 HTTP
//! 请求，验证「CF 边缘能为 inner SNI 完成握手且能回源」。
//!
//! 传输栈为 btls（BoringSSL）+ tokio-btls——与 wreq 数据腿同一 TLS
//! 家族，握手行为一致；探活不关心证书校验（只验证 ECH 接受与边缘回源，
//! 不做任何敏感传输），故关闭验证以免被系统/代理根证书差异误伤。
//!
//! ECH probing: a bare TLS+ECH handshake plus a minimal HTTP request on the
//! same connection, verifying "the CF edge can handshake for this inner SNI
//! and route to its origin".
//!
//! The transport is btls (BoringSSL) + tokio-btls — the same TLS family as
//! the wreq data legs, so handshake behavior matches. The probe does not
//! verify certificates (it only checks ECH acceptance and edge routability
//! and carries nothing sensitive), so verification is disabled to stay
//! immune to system/proxy root-store differences.

use btls::ssl::{Ssl, SslConnector, SslMethod, SslVerifyMode};
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_btls::SslStream;

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
	/// 服务端拒绝 ECH：`Some(bytes)` = 附了 retry_configs 的原始
	/// ECHConfigList（借来的密钥已轮换，可就地换新）；`None` = 未附
	/// （经 `ech_name_override` 判定为拒绝），只能经 DoH 重新解析。
	/// Server rejected ECH: `Some(bytes)` = the raw retry_configs
	/// ECHConfigList (the borrowed key rotated — can be swapped in place);
	/// `None` = none sent (identified as rejection via `ech_name_override`),
	/// so the borrowed config must be re-resolved via DoH.
	Rejected(Option<Vec<u8>>),
}

/// 构造一次探活握手用的 `Ssl`：ECH config + http/1.1 ALPN，证书不验证
/// （探活只关心 ECH 接受/边缘回源，真实数据腿的 wreq 客户端自带完整验证）。
/// Builds the `Ssl` for one probe handshake: ECH config + http/1.1 ALPN,
/// certificate verification off (the probe only cares about ECH
/// acceptance/edge routability; the real wreq data legs verify normally).
fn ech_ssl(ech_list: &[u8], host: &str) -> Option<Ssl> {
	let connector = SslConnector::bare_builder(SslMethod::tls()).ok()?.build();
	let mut cfg = connector.configure().ok()?;
	cfg.set_verify(SslVerifyMode::NONE);
	cfg.set_verify_hostname(false);
	// ALPN 只报 http/1.1：裸写 GET / 后只需解析一行状态码 /
	// offer only http/1.1: after a raw GET / just the status line needs parsing
	cfg.set_alpn_protos(b"\x08http/1.1").ok()?;
	let mut ssl = cfg.into_ssl(host).ok()?;
	// `SSL_set1_ech_config_list`：外层 SNI 变为 public_name，真实主机进入
	// 加密的 inner ClientHello / `SSL_set1_ech_config_list`: the outer SNI
	// becomes public_name while the true host goes into the encrypted inner
	// ClientHello
	ssl.set_ech_config_list(ech_list).ok()?;
	Some(ssl)
}

/// 单地址握手尝试的结果 / Outcome of one per-address handshake attempt
enum Shake {
	/// 握手完成且 `SSL_ech_accepted`——已建立可用连接 /
	/// handshake done and `SSL_ech_accepted` — usable connection
	Accepted(SslStream<TcpStream>),
	/// 服务端拒绝 ECH：retry_configs（有则携带）/
	/// server rejected ECH: retry_configs when provided
	Rejected(Option<Vec<u8>>),
	/// 其他失败（TCP/TLS/超时或握手完成但 ECH 未被接受）/
	/// anything else (TCP/TLS/timeout, or handshake done without ECH acceptance)
	Other,
}

/// 对单个地址做 TCP + TLS/ECH 握手分类。ECH 拒绝在 BoringSSL 里表现
/// 为握手错误 + `get_ech_retry_configs`/`get_ech_name_override` 留痕
/// （后者是服务器在拒绝时告知验备用名的字段）。
/// TCP + TLS/ECH handshake against one address, classified. An ECH
/// rejection surfaces in BoringSSL as a handshake error plus
/// `get_ech_retry_configs`/`get_ech_name_override` left on the SSL
/// object (the latter is the name the server tells the client to verify
/// against on rejection).
async fn shake(ech_list: &[u8], host: &str, addr: SocketAddr) -> Shake {
	let Some(ssl) = ech_ssl(ech_list, host) else {
		return Shake::Other;
	};
	tokio::time::timeout(PROBE_TIMEOUT, async {
		let tcp = TcpStream::connect(addr).await.ok()?;
		let mut stream = SslStream::new(ssl, tcp).ok()?;
		match Pin::new(&mut stream).connect().await {
			Ok(()) if stream.ssl().ech_accepted() => Some(Shake::Accepted(stream)),
			Ok(()) => Some(Shake::Other), // 握手完成但 ECH 未接受 / handshake done, ECH not accepted
			Err(_) => {
				let ssl = stream.ssl();
				if let Some(rc) = ssl.get_ech_retry_configs() {
					return Some(Shake::Rejected(Some(rc.to_vec())));
				}
				if ssl.get_ech_name_override().is_some() {
					return Some(Shake::Rejected(None));
				}
				Some(Shake::Other)
			}
		}
	})
	.await
	.unwrap_or(Some(Shake::Other))
	.unwrap_or(Shake::Other)
}

/// 探活：握手 + `ech_accepted` 断言 + 一次 `GET /` 回源检查（状态码 <500 才算
/// 边缘能路由；5xx 为 CF 边缘→源站失败，如 530 zone-miss）。不走 3xx 之后的
/// 路径——任何 HTTP 响应都证明边缘能路由该主机，重定向目标是业务层的事。
/// Probe: handshake + `ech_accepted` assertion + one `GET /` routability check
/// (status <500 means the edge can route; 5xx is a CF edge→origin failure
/// such as 530 zone-miss). Redirects are not followed — any HTTP response
/// proves the edge routes the host; where it leads is the app's business.
pub(super) async fn probe_ech(ech_list: &[u8], host: &str, addrs: &[SocketAddr]) -> ProbeOutcome {
	let mut rejected: Option<Option<Vec<u8>>> = None;
	for addr in addrs {
		match shake(ech_list, host, *addr).await {
			Shake::Accepted(mut stream) => return http_probe(&mut stream, host).await,
			// ECH 被拒：记下 retry_configs（若有），继续试下个地址 /
			// ECH rejected: record retry_configs (if any) and keep trying
			// the next address
			Shake::Rejected(rc) => rejected = Some(rc),
			Shake::Other => {}
		}
	}
	match rejected {
		Some(rc) => ProbeOutcome::Rejected(rc),
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
	ech_list: &[u8],
	addr: SocketAddr,
) -> Option<(Duration, usize)> {
	let ssl = ech_ssl(ech_list, RACE_PROBE_HOST)?;
	let t0 = Instant::now();
	tokio::time::timeout(RACE_TIMEOUT, async {
        let tcp = TcpStream::connect(addr).await?;
        let mut stream = SslStream::new(ssl, tcp)?;
        Pin::new(&mut stream)
            .connect()
            .await
            .map_err(std::io::Error::other)?;
        if !stream.ssl().ech_accepted() {
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
async fn http_probe(stream: &mut SslStream<TcpStream>, host: &str) -> ProbeOutcome {
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
