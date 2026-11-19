//! ECH 借用配置的 DNS 侧：公共 DoH 端点表（dns-json 与 RFC 8484
//! wire-format 两种格式，按端点声明）、HTTPS（type 65）记录查询、
//! `ech=` / `ipv4hint=` / `ipv6hint=` 解析与「源域 × 端点」借用矩阵。
//!
//! DNS side of the borrowed ECH config: the public DoH endpoint table
//! (both dns-json and RFC 8484 wire-format flavors, declared per endpoint),
//! HTTPS (type 65) record queries, `ech=` / `ipv4hint=` / `ipv6hint=`
//! parsing and the "source domain × endpoint" borrow matrix.

use crate::core::error::{AppError, Result};
use base64::Engine;
use reqwest::Client;
use rustls::client::EchConfig;
use rustls_pki_types::EchConfigListBytes;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// DoH 端点的请求格式。/ Request flavor of a DoH endpoint.
#[derive(Clone, Copy)]
enum DohFlavor {
    /// Google 风格 dns-json：`?name=&type=` + `Accept: application/dns-json`
    /// Google-style dns-json: `?name=&type=` + `Accept: application/dns-json`
    Json,
    /// RFC 8484 wire-format：`?dns=<base64url>` + `Accept: application/dns-message`
    /// RFC 8484 wire-format: `?dns=<base64url>` + `Accept: application/dns-message`
    Wire,
    /// 两种格式都收：先 JSON，传输失败再试 wire /
    /// Accepts both: JSON first, wire on transport failure
    Both,
}

/// 一个 DoH 端点：URL（已含正确路径）+ 支持的请求格式。
/// 实测（test/dohcheck 直连探测）：alidns 的 JSON 在 /resolve、wire 在
/// /dns-query；腾讯/360/ODVR 对 HTTPS 型查询只认 wire-format——JSON
/// 参数直接 400。
/// A DoH endpoint: URL (already carrying the correct path) + supported
/// request flavor. Measured (test/dohcheck direct probe): AliDNS serves
/// JSON at /resolve and wire at /dns-query; Tencent/360/ODVR only accept
/// wire-format for HTTPS-type queries — the JSON params get a 400.
struct DohEndpoint {
    url: &'static str,
    flavor: DohFlavor,
}

/// 查询 HTTPS 记录用的公共 DoH 端点，按序回退。
/// Cloudflare 端点在中国大陆不可达时由后续境内公共 DNS（阿里/腾讯/360）
/// 与 ODVR 兜底；配置了代理时这些请求走代理客户端，首项依旧可用。
/// Public DoH endpoints for HTTPS-record lookups, tried in order. The
/// Cloudflare endpoint is covered in mainland China by the following public
/// resolvers (AliDNS/Tencent/360) plus ODVR; when a proxy is configured the
/// queries ride the proxied client so the first entry remains usable.
const DOH_ENDPOINTS: &[DohEndpoint] = &[
    DohEndpoint {
        url: "https://cloudflare-dns.com/dns-query",
        flavor: DohFlavor::Both,
    },
    DohEndpoint {
        url: "https://dns.alidns.com/resolve",
        flavor: DohFlavor::Json,
    },
    DohEndpoint {
        url: "https://dns.alidns.com/dns-query",
        flavor: DohFlavor::Wire,
    },
    DohEndpoint {
        url: "https://doh.pub/dns-query",
        flavor: DohFlavor::Wire,
    },
    DohEndpoint {
        url: "https://sm2.doh.pub/dns-query",
        flavor: DohFlavor::Wire,
    },
    DohEndpoint {
        url: "https://doh.360.cn/dns-query",
        flavor: DohFlavor::Wire,
    },
    DohEndpoint {
        url: "https://odvr.nic.cz/doh",
        flavor: DohFlavor::Wire,
    },
];

/// ECH 配置的借用源域池（均为实测发布 `ech=` 的 Cloudflare zone，
/// public_name = cloudflare-ech.com；各 zone 的密钥份额可能不同但均被
/// CF 边缘接受）。不是按序回退——`lookup_borrowed` 并行取回全部可用
/// 候选，由 `race_borrowed` 按实测延迟选最优 hint 地址。
/// Pool of domains to borrow the ECH config from (all Cloudflare zones
/// confirmed publishing `ech=`, public_name = cloudflare-ech.com; each
/// zone's keyshare may differ but all are accepted by CF edges). NOT a
/// fallback chain — `lookup_borrowed` fetches every viable candidate in
/// parallel and `race_borrowed` picks the lowest-latency hint address.
const ECH_SOURCE_DOMAINS: &[&str] = &[
    // 以下域均实测（test/echsrc）发布 ech= + ipv4hint/ipv6hint，且 hint
    // IP 集合高度多样（104.18/104.20/104.21/172.66/172.67 多段任播对 +
    // 2606:4700::/3030~3037:: 多段 v6）——竞速探活时等于 40+ 条候选路由。
    // 顺序无关：源域并行查询、按探活延迟选优。
    // 实测排除（test/echspeed）：www.shopify.com 只发 hint 不发 ech=；
    // saas.sin.fan 的 HTTPS 应答 CNAME 到非 CF 的 singnetworkcdn——
    // 均不可借用，勿加回。
    // All domains below confirmed (test/echsrc) to publish ech= +
    // ipv4hint/ipv6hint with highly diverse hint IP sets (104.18/104.20/
    // 104.21/172.66/172.67 anycast pairs + 2606:4700::/3030~3037:: v6) —
    // that's 40+ candidate routes for the race probe. Order is irrelevant:
    // sources resolve in parallel and the lowest-latency probe wins.
    // Measured rejections (test/echspeed): www.shopify.com publishes
    // hints but no ech=; saas.sin.fan's HTTPS answer CNAMEs to the
    // non-CF singnetworkcdn — neither is borrowable; don't re-add.
    "cloudflare-ech.com",
    "crypto.cloudflare.com",
    "encryptedsni.com",
    "icook.hk",
    "cm.edu.kg",
    "godotengine.org",
    "www.britannica.com",
    "www.prometheus.io",
    "www.kyocera.com",
    "celestia.org",
    "lido.fi",
    "0xcaner.top",
    "ech.digital",
];

/// 单个 DoH 请求的超时 / Timeout for a single DoH request
const DOH_TIMEOUT: Duration = Duration::from_secs(5);
/// 借用配置正缓存下限（ECH 公钥轮换快，太短没意义）/
/// Borrowed-config cache floor (ECH keys rotate quickly; shorter buys little)
const BORROWED_TTL_MIN: Duration = Duration::from_secs(60);
/// 借用配置正缓存上限 / Borrowed-config cache cap
const BORROWED_TTL_MAX: Duration = Duration::from_secs(3600);
/// 记录未给 TTL 时的默认缓存时长；竞速胜组无 TTL 时亦用 /
/// Default TTL when the record gives none; also the race pick's fallback
pub(super) const BORROWED_TTL_FALLBACK: Duration = Duration::from_secs(300);
/// 记录未给 TTL 时的默认缓存时长 / Default TTL when the record gives none
const BORROWED_TTL_DEFAULT: Duration = BORROWED_TTL_FALLBACK;

/// 「首次借到配置」标记：置位前允许输出一次「借用配置就绪」提示，
/// 首次成功后置位，此后的 TTL 刷新全程静默——有问题时警告/错误日志
/// 依旧会暴露。/ "First successful borrow" flag: gates a one-shot
/// "borrowed config ready" notice; set after the first success so every
/// later TTL refresh stays silent — warn/error logs still surface
/// problems.
static BORROWED_LOGGED: AtomicBool = AtomicBool::new(false);

/// 标记借用配置已就绪日志输出完毕，返回此前是否已标记（false = 本次
/// 是首次就绪，调用方应输出就绪提示）。/ Marks the borrowed-config-ready
/// notice as emitted and returns whether it was already marked (false =
/// this is the first ready state, the caller should log it).
pub(super) fn mark_borrowed_logged() -> bool {
    BORROWED_LOGGED.swap(true, Ordering::Relaxed)
}

/// 从 HTTPS 记录提取出的可借用内容：ECH 配置 + 钉接地址 + TTL。
/// What an HTTPS record yields for borrowing: the ECH config, pin
/// addresses, and the TTL.
pub(super) struct RecordExtract {
    /// 借用的 ECH 配置 / Borrowed ECH config
    pub(super) ech: EchConfig,
    /// `ipv4hint=` / `ipv6hint=` 钉接地址（含 :443 端口）/
    /// Pin addresses from the `ipv4hint=` / `ipv6hint=` params (`:443`)
    pub(super) addrs: Vec<SocketAddr>,
    /// 建议缓存时长（经上下限钳制）/ Suggested cache TTL (clamped)
    pub(super) ttl: Duration,
    /// 记录来源的借用源域（竞速探活时作为 ECH 目标主机——本 zone 必然
    /// 可回源，不会 zone-miss）/ The source domain this record came from
    /// (used as the ECH target host when race-probing — its own zone is
    /// always routable, so no zone-miss)
    pub(super) source: String,
    /// 原始 ECHConfigList 字节：按密钥把候选分组——同密钥组的 hint 地址
    /// 可合并竞速（实测 12/13 个源域发布的是同一份 CF 全网共享配置）/
    /// Raw ECHConfigList bytes: groups candidates by key — hint addresses
    /// of the same key pool together for the race (12/13 sources measured
    /// publishing literally the same CF network-wide config)
    pub(super) ech_raw: Vec<u8>,
}

/// 并行解析**全部**借用源域：每个源域各自走端点链（首个成功端点即停），
/// 返回所有可用候选——由调用方竞速探活选出延迟最低的（ech, addrs）组。
/// 候选集为空 = 借用彻底失败。
/// Resolves **all** borrow source domains in parallel: each walks its own
/// endpoint chain (stops at the first endpoint that answers), returning every
/// viable candidate — the caller race-probes them for the lowest-latency
/// (ech, addrs) pair. An empty result means borrowing failed entirely.
pub(super) async fn lookup_borrowed(client: &Client) -> Vec<RecordExtract> {
    let mut set = tokio::task::JoinSet::new();
    for source in ECH_SOURCE_DOMAINS {
        let client = client.clone();
        set.spawn(async move { lookup_source(&client, source).await });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Some(rec)) => out.push(rec),
            // 该源所有端点传输失败或权威应答无 ech/hint → 跳过 /
            // All endpoints for this source failed or answered without
            // ech/hints — skip it
            Ok(None) => {}
            // join 失败（panic/abort）按该源不可用处理 / A failed join
            // (panic/abort) counts as that source being unavailable
            Err(_) => {}
        }
    }
    out
}

/// 解析单个借用源域：按端点顺序尝试，首个拿到完整 `ech=` + hint 的端点
/// 即返回；全失败返回 None。/ Resolves one borrow source domain: tries
/// endpoints in order, returning at the first that yields both `ech=` and
/// hints; None when all fail.
async fn lookup_source(client: &Client, source: &str) -> Option<RecordExtract> {
    for endpoint in DOH_ENDPOINTS {
        match query_endpoint(client, endpoint, source).await {
            Ok(Some(rec)) => return Some(rec),
            // 权威应答但无可用 ech/hint → 该源不可用，直接返回 /
            // Authoritative answer without usable ech/hints — this source
            // is unusable, done
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(
                    "{}",
                    crate::tl!(
                        "ech.dohQueryFailed",
                        endpoint = endpoint.url,
                        source = source,
                        error = e
                    )
                );
            }
        }
    }
    None
}

/// 一条 HTTPS（type 65）记录应答：TTL + 文本表示（`1 . alpn=… ech=…`）。
/// One HTTPS (type 65) record answer: TTL + presentation text.
struct RecordAnswer {
    ttl: u64,
    data: String,
}

/// 按端点声明的格式查询并提取借用内容：`Both` 先 JSON，传输级失败再试
/// wire-format；`Ok(Some)` = 完整可借用，`Ok(None)` = 权威应答但不可
/// 借用，`Err` = 传输/解析失败（换下个端点）。
/// Queries an endpoint per its declared flavor and extracts the borrowable
/// parts: `Both` tries JSON first and wire-format on transport failure;
/// `Ok(Some)` = fully borrowable, `Ok(None)` = answered fine but not
/// borrowable, `Err` = transport/parse failure (move to the next endpoint).
async fn query_endpoint(
    client: &Client,
    endpoint: &DohEndpoint,
    name: &str,
) -> Result<Option<RecordExtract>> {
    match endpoint.flavor {
        DohFlavor::Json => query_json(client, endpoint.url, name).await,
        DohFlavor::Wire => query_wire(client, endpoint.url, name).await,
        DohFlavor::Both => match query_json(client, endpoint.url, name).await {
            Err(_) => query_wire(client, endpoint.url, name).await,
            ok => ok,
        },
    }
}

/// 向单个 dns-json 端点查询域名的 HTTPS 记录并提取借用内容。
/// Queries one dns-json endpoint for the domain's HTTPS record and extracts
/// the borrowable parts.
async fn query_json(client: &Client, endpoint: &str, name: &str) -> Result<Option<RecordExtract>> {
    let records = fetch_https_records(client, endpoint, name).await?;
    extract_record(name, &records)
}

/// 向单个 wire-format 端点查询域名的 HTTPS 记录并提取借用内容。
/// Queries one wire-format endpoint for the domain's HTTPS record and
/// extracts the borrowable parts.
async fn query_wire(client: &Client, endpoint: &str, name: &str) -> Result<Option<RecordExtract>> {
    let records = fetch_wire_records(client, endpoint, name).await?;
    extract_wire_records(name, &records)
}

/// 向单个 dns-json 端点查询域名的 HTTPS 记录，只返回原始应答列表。
/// Queries one dns-json endpoint for the domain's HTTPS records and returns
/// the raw answer list.
async fn fetch_https_records(
    client: &Client,
    endpoint: &str,
    name: &str,
) -> Result<Vec<RecordAnswer>> {
    // reqwest 0.13 的 `.query` 需要单独 feature，改用 url crate 拼参数 /
    // reqwest 0.13's `.query` sits behind a feature — build the params via the url crate instead
    let url = reqwest::Url::parse_with_params(endpoint, &[("name", name), ("type", "HTTPS")])
        .map_err(|e| AppError::Other(format!("DoH URL 构建失败: {e}")))?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/dns-json")
        .timeout(DOH_TIMEOUT)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    // dns-json 应答的 Status 即 RCODE：非 0（如 4=Not Implemented）视为
    // 传输级失败，让调用方换格式/端点重试 / The dns-json Status field IS
    // the RCODE: a non-zero value (e.g. 4=Not Implemented) counts as a
    // transport failure so the caller can retry another flavor/endpoint
    if let Some(rcode) = body["Status"].as_i64()
        && rcode != 0
    {
        return Err(AppError::Other(format!("DoH Status={rcode}")));
    }

    let mut out = Vec::new();
    let empty = Vec::new();
    for ans in body["Answer"].as_array().unwrap_or(&empty) {
        // 65 = HTTPS 记录 / 65 = HTTPS record
        if ans["type"].as_u64() != Some(65) {
            continue;
        }
        let Some(data) = ans["data"].as_str() else {
            continue;
        };
        out.push(RecordAnswer {
            ttl: ans["TTL"].as_u64().unwrap_or(0),
            data: data.to_string(),
        });
    }
    Ok(out)
}

/// 从记录列表提取借用内容：需至少一条 ech= 且至少一个 IP hint，否则 None。
/// Extracts the borrowable parts from a record list: requires at least one
/// ech= and one IP hint, otherwise None.
fn extract_record(name: &str, records: &[RecordAnswer]) -> Result<Option<RecordExtract>> {
    // name 即借用源域，透传给 RecordExtract.source 供竞速探活当 ECH
    // 目标主机 / name IS the borrow source domain — forwarded to
    // RecordExtract.source as the ECH target for race probing
    let mut ttl_min: Option<u64> = None;
    let mut ech_raw: Option<Vec<u8>> = None;
    let mut addrs: Vec<SocketAddr> = Vec::new();
    for rec in records {
        if rec.ttl > 0 {
            ttl_min = Some(ttl_min.map_or(rec.ttl, |m| m.min(rec.ttl)));
        }
        if ech_raw.is_none()
            && let Some(b64) = extract_svcb_param(&rec.data, "ech=")
            && let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64)
            && !raw.is_empty()
        {
            ech_raw = Some(raw);
        }
        if let Some(hint) = extract_svcb_param(&rec.data, "ipv4hint=") {
            addrs.extend(
                hint.split(',')
                    .filter_map(|s| s.trim().parse::<IpAddr>().ok())
                    .map(|ip| SocketAddr::new(ip, 443)),
            );
        }
        if let Some(hint) = extract_svcb_param(&rec.data, "ipv6hint=") {
            addrs.extend(
                hint.split(',')
                    .filter_map(|s| s.trim().parse::<IpAddr>().ok())
                    .map(|ip| SocketAddr::new(ip, 443)),
            );
        }
    }

    build_extract(ech_raw, addrs, ttl_min, name)
}

/// 两种格式的共用收尾：ech 原文 + hint 地址齐全才构造
/// `EchConfig`（TTL 取记录最小值并钳制）。
/// Shared tail for both flavors: only builds the `EchConfig` when raw ech
/// bytes and at least one hint address exist (TTL = record minimum, clamped).
fn build_extract(
    ech_raw: Option<Vec<u8>>,
    addrs: Vec<SocketAddr>,
    ttl_min: Option<u64>,
    source: &str,
) -> Result<Option<RecordExtract>> {
    let (Some(raw), false) = (ech_raw, addrs.is_empty()) else {
        return Ok(None);
    };
    let ech = EchConfig::new(
        EchConfigListBytes::from(raw.clone()),
        rustls::crypto::aws_lc_rs::hpke::ALL_SUPPORTED_SUITES,
    )
    .map_err(|e| AppError::Other(format!("ECH 配置不兼容: {e}")))?;
    let ttl = ttl_min
        .map(Duration::from_secs)
        .unwrap_or(BORROWED_TTL_DEFAULT)
        .clamp(BORROWED_TTL_MIN, BORROWED_TTL_MAX);
    Ok(Some(RecordExtract {
        ech,
        addrs,
        ttl,
        source: source.to_string(),
        ech_raw: raw,
    }))
}

// ---- RFC 8484 wire-format 通道 / wire-format channel ----

/// 一条 wire-format HTTPS RR 中已解析出的借用素材。
/// Borrowable pieces already parsed out of one wire-format HTTPS RR.
struct WireAnswer {
    ttl: u64,
    ech: Option<Vec<u8>>,
    addrs: Vec<SocketAddr>,
}

/// DNS 查询报文里固定的 16 位 ID（DoH 走 TLS 无欺骗风险，固定值只用于
/// 校验应答确属本查询）。/ Fixed 16-bit query ID (DoH rides TLS so spoofing
/// is a non-issue — the constant only lets us verify the response belongs
/// to our query).
const WIRE_QUERY_ID: u16 = 0x5CA7;
/// DNS type HTTPS / DNS type HTTPS
const QTYPE_HTTPS: u16 = 65;
/// DNS class IN / DNS class IN
const QCLASS_IN: u16 = 1;
/// SVCB param key：ech / SVCB param key: ech
const SVC_KEY_ECH: u16 = 5;
/// SVCB param key：ipv4hint / SVCB param key: ipv4hint
const SVC_KEY_IPV4HINT: u16 = 4;
/// SVCB param key：ipv6hint / SVCB param key: ipv6hint
const SVC_KEY_IPV6HINT: u16 = 6;

/// 向单个 wire-format 端点查询域名的 HTTPS 记录：手工编码查询报文 →
/// `GET ?dns=<base64url>` → 解析应答里的全部 type-65 RR。
/// Queries one wire-format endpoint for the domain's HTTPS records:
/// hand-encode the query message → `GET ?dns=<base64url>` → parse every
/// type-65 RR in the answer.
async fn fetch_wire_records(
    client: &Client,
    endpoint: &str,
    name: &str,
) -> Result<Vec<WireAnswer>> {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(wire_query(name));
    let url = reqwest::Url::parse_with_params(endpoint, &[("dns", encoded.as_str())])
        .map_err(|e| AppError::Other(format!("DoH URL 构建失败: {e}")))?;
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/dns-message")
        .timeout(DOH_TIMEOUT)
        .send()
        .await?;
    let body = resp.bytes().await?;
    parse_dns_message(&body)
}

/// 编码最小 DNS 查询报文：header（RD 位置位、QD=1）+ 一个 type-65
/// question。/ Encodes a minimal DNS query message: header (RD bit set,
/// QD=1) + one type-65 question.
fn wire_query(name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(name.len() + 18);
    q.extend_from_slice(&WIRE_QUERY_ID.to_be_bytes());
    q.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD
    q.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    q.extend_from_slice(&[0u8; 6]); // AN/NS/AR = 0
    for label in name.split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0); // root
    q.extend_from_slice(&QTYPE_HTTPS.to_be_bytes());
    q.extend_from_slice(&QCLASS_IN.to_be_bytes());
    q
}

/// 顺序跳过一个 DNS 域名并返回其结束位置；压缩指针（0xC0）合法地终止
/// 域名，只跳 2 字节不展开。/ Sequentially skips a DNS name and returns the
/// position just past it; a compression pointer (0xC0) legally terminates
/// the name — skipped as 2 bytes without being followed.
fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let b = *msg.get(pos)?;
        if b == 0 {
            return Some(pos + 1);
        }
        if b & 0xC0 == 0xC0 {
            return (pos + 2 <= msg.len()).then_some(pos + 2);
        }
        if b & 0xC0 != 0 {
            return None; // 非法标签位模式 / illegal label-bit pattern
        }
        pos += 1 + b as usize;
        if pos > msg.len() {
            return None;
        }
    }
}

/// 解析 DNS 应答报文：校验 ID 回显与 QR/RCODE，跳过 question 区，收集
/// answer 区全部 type-65 RR（TTL + 解析后的 SVCB 借用素材）。
/// Parses a DNS response message: verifies the ID echo plus QR/RCODE, skips
/// the question section and collects every type-65 RR in the answer
/// section (TTL + already-parsed SVCB borrow pieces).
fn parse_dns_message(msg: &[u8]) -> Result<Vec<WireAnswer>> {
    let id = WIRE_QUERY_ID.to_be_bytes();
    if msg.len() < 12 || msg[0] != id[0] || msg[1] != id[1] || msg[2] & 0x80 == 0 {
        return Err(AppError::Other("非 DNS 应答 / not a DNS response".into()));
    }
    let rcode = msg[3] & 0x0F;
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let ancount = u16::from_be_bytes([msg[6], msg[7]]) as usize;
    // 怪癖宽容：doh.360.cn 实测返回 RCODE=5(REFUSED) 却仍带完整应答——
    // 有答案就解析，空答案的非零 RCODE（SERVFAIL/REFUSED）才算失败 /
    // Quirk tolerance: doh.360.cn was observed returning RCODE=5 (REFUSED)
    // while still carrying a full answer — parse whatever answers exist and
    // only treat a non-zero RCODE with an EMPTY answer section as failure
    if rcode != 0 && ancount == 0 {
        return Err(AppError::Other(format!("DNS RCODE={rcode}")));
    }
    let mut pos = 12;
    for _ in 0..qdcount {
        pos = skip_name(msg, pos)
            .and_then(|p| (p + 4 <= msg.len()).then_some(p + 4)) // + QTYPE/QCLASS
            .ok_or_else(|| AppError::Other("DNS question 截断 / truncated DNS question".into()))?;
    }
    let mut out = Vec::new();
    for _ in 0..ancount {
        let Some(rh) = skip_name(msg, pos) else { break };
        if rh + 10 > msg.len() {
            break;
        }
        let rtype = u16::from_be_bytes([msg[rh], msg[rh + 1]]);
        let ttl = u32::from_be_bytes(msg[rh + 4..rh + 8].try_into().unwrap()) as u64;
        let rdlen = u16::from_be_bytes([msg[rh + 8], msg[rh + 9]]) as usize;
        let rd_start = rh + 10;
        if rd_start + rdlen > msg.len() {
            break;
        }
        if rtype == QTYPE_HTTPS {
            let (ech, addrs) = parse_svcb_rdata(&msg[rd_start..rd_start + rdlen]);
            out.push(WireAnswer { ttl, ech, addrs });
        }
        pos = rd_start + rdlen;
    }
    Ok(out)
}

/// 解析一条 SVCB RDATA：priority(2) + target_name + svcparams 列表，
/// 只提取 ech(key 5) 与 ipv4hint/ipv6hint(key 4/6)。
/// Parses one SVCB RDATA: priority(2) + target_name + the svcparams list —
/// only ech (key 5) and ipv4hint/ipv6hint (keys 4/6) are extracted.
fn parse_svcb_rdata(rd: &[u8]) -> (Option<Vec<u8>>, Vec<SocketAddr>) {
    let mut ech = None;
    let mut addrs = Vec::new();
    let Some(mut pos) = skip_name(rd, 2) else {
        return (ech, addrs);
    };
    while pos + 4 <= rd.len() {
        let key = u16::from_be_bytes([rd[pos], rd[pos + 1]]);
        let len = u16::from_be_bytes([rd[pos + 2], rd[pos + 3]]) as usize;
        pos += 4;
        if pos + len > rd.len() {
            break;
        }
        let val = &rd[pos..pos + len];
        match key {
            SVC_KEY_IPV4HINT => {
                for c in val.as_chunks::<4>().0 {
                    addrs.push(SocketAddr::new(
                        IpAddr::V4(std::net::Ipv4Addr::from(*c)),
                        443,
                    ));
                }
            }
            SVC_KEY_IPV6HINT => {
                for c in val.as_chunks::<16>().0 {
                    addrs.push(SocketAddr::new(
                        IpAddr::V6(std::net::Ipv6Addr::from(*c)),
                        443,
                    ));
                }
            }
            SVC_KEY_ECH if ech.is_none() && !val.is_empty() => ech = Some(val.to_vec()),
            _ => {}
        }
        pos += len;
    }
    (ech, addrs)
}

/// wire 应答列表 → 借用内容：逻辑同 `extract_record`，只是数据已是
/// 解析好的二进制而非文本表示。/ Wire answer list → borrowable parts: same
/// logic as `extract_record`, but the data arrives already parsed as
/// binary rather than presentation text.
fn extract_wire_records(name: &str, records: &[WireAnswer]) -> Result<Option<RecordExtract>> {
    let mut ttl_min: Option<u64> = None;
    let mut ech_raw: Option<Vec<u8>> = None;
    let mut addrs: Vec<SocketAddr> = Vec::new();
    for rec in records {
        if rec.ttl > 0 {
            ttl_min = Some(ttl_min.map_or(rec.ttl, |m| m.min(rec.ttl)));
        }
        if ech_raw.is_none()
            && let Some(raw) = &rec.ech
        {
            ech_raw = Some(raw.clone());
        }
        addrs.extend(rec.addrs.iter().copied());
    }
    build_extract(ech_raw, addrs, ttl_min, name)
}

/// 从 HTTPS 记录的文本表示中提取某个 SvcParam 值（形如 `key="..."`，兼容
/// 不带引号的形式——dns-json 各家实现不一）。
/// Extracts one SvcParam value from an HTTPS record's presentation text
/// (`key="..."`; also tolerates the unquoted form — dns-json providers differ).
fn extract_svcb_param<'a>(data: &'a str, key: &str) -> Option<&'a str> {
    let pos = data.find(key)? + key.len();
    let rest = &data[pos..];
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some(&quoted[..end]);
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Some(&rest[..end]).filter(|s| !s.is_empty())
}
