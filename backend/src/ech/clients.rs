//! 按主机名缓存的 ECH 客户端集合：借用配置槽位、逐主机探活与
//! 负缓存回退（fail-open）。
//!
//! Per-hostname cache of ECH clients: the borrowed-config slot, per-host
//! probing and negative-cache fallback (fail-open).

use super::doh;
use super::probe::{ProbeOutcome, probe_addr_speed, probe_ech};
use crate::core::error::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use wreq::Client;

/// 借用配置整体解析失败后的冷却时长 / Cooldown after the borrowed-config lookup fails entirely
const BORROWED_FAIL_TTL: Duration = Duration::from_secs(300);
/// 借用后台刷新失败、且槽内有旧配置续用时，到下次重试的间隔 /
/// Interval before the next refresh attempt after a failed background
/// refresh while a stale config kept serving
const BORROWED_REFRESH_RETRY: Duration = Duration::from_secs(60);
/// 单个主机探活/构建失败的负缓存时长 / Negative-cache TTL for per-host probe/build failures
const ENTRY_NEG_TTL: Duration = Duration::from_secs(300);
/// 探活通过的主机条目寿命：与借用配置的 TTL 解耦——旧实现条目随借用
/// 配置每分钟到期重建+重新探活（一次真实握手），在坏链路上是周期性
/// 抖动源。条目本就随连接错误驱逐，固定长寿即可。
/// Lifetime of a probe-passing host entry: decoupled from the borrowed
/// config's TTL — entries used to die and re-probe (a real handshake)
/// every time the borrow expired, a periodic churn on bad links. Entries
/// are already evicted on connect errors, so a fixed lifetime is fine.
const ENTRY_TTL: Duration = Duration::from_secs(600);
/// ECH 被拒且服务端未附 retry_configs（密钥已轮换、只能走 DoH 重解析）时的
/// 短冷却——给 DNS 记录留出轮换时间，同时防止每个主机都重打一遍 DoH。
/// Short cooldown for "rejected without retry_configs" (key rotated, DoH
/// re-resolve is the only way out) — gives the DNS record time to rotate
/// while keeping every host from re-running DoH.
const ROTATED_STALE_TTL: Duration = Duration::from_secs(60);

/// 单个 ECH 客户端的连接目标：`resolve_to_addrs(host, addrs)` + 待注入的
/// ECHConfigList。/ Connection target for one ECH client:
/// `resolve_to_addrs(host, addrs)` plus the ECHConfigList to inject.
#[derive(Clone)]
pub struct EchTarget {
    /// 目标主机名（TLS ServerName / inner SNI / Host 头，同时是 DNS 覆盖键）
    /// Target hostname (TLS ServerName / inner SNI / Host header — also the
    /// resolve-override key)
    pub host: String,
    /// 钉到的 CF 边缘地址（含端口，通常 :443）/ Pinned CF edge addresses (with port, usually :443)
    pub addrs: Vec<SocketAddr>,
    /// 借用的 ECHConfigList 原始字节（握手前经 `SSL_set1_ech_config_list`
    /// 注入）/ Borrowed ECHConfigList raw bytes (injected via
    /// `SSL_set1_ech_config_list` before the handshake)
    pub ech_list: Vec<u8>,
}

/// 借来的 ECH 配置：一个源域的 ECHConfigList + CF 任播 IP + 过期时刻。
/// 对同一 TTL 周期内的所有目标主机复用。
/// The borrowed ECH config: one source domain's ECHConfigList + CF anycast
/// IPs + expiry. Reused for every target host within its TTL.
#[derive(Clone)]
struct Borrowed {
    ech_list: Vec<u8>,
    addrs: Vec<SocketAddr>,
    expires: Instant,
}

/// 借用配置槽位：空 / 就绪 / 失败冷却中。
/// Borrowed-config slot: empty / ready / failing with cooldown.
enum BorrowSlot {
    Empty,
    Ready(Borrowed),
    Failed(Instant),
}

/// 主机条目缓存：`client: Some` = 探活通过的 ECH 客户端；`None` = 不可用
/// （负缓存），访问该主机回退基础客户端。
/// Host entry cache: `client: Some` = probe-passing ECH client; `None` =
/// unavailable (negative cache — the base client is used).
struct Entry {
    client: Option<Client>,
    expires: Instant,
}

/// 按目标重建同类客户端的构建器类型 /
/// Builder type that rebuilds a same-flavored client for a target
type ClientBuilder = Arc<dyn Fn(&EchTarget) -> Result<Client> + Send + Sync>;

/// 由借用配置构建 TLS 选项：Chrome149 指纹档的 `TlsOptions` + 注入
/// `ech_config_list`（内层 ClientHello 因此也是完整 Chrome 指纹）。
/// Builds the TLS options from the borrowed config: the Chrome149
/// emulation `TlsOptions` plus the injected `ech_config_list` (so the
/// inner ClientHello is a full Chrome fingerprint too).
fn ech_tls_options(ech_list: &[u8]) -> wreq::tls::TlsOptions {
    let mut opts = wreq::IntoEmulation::into_emulation(wreq_util::Emulation::Chrome149)
        .tls_options
        .unwrap_or_default();
    opts.ech_config_list = Some(ech_list.to_vec());
    opts
}

/// 应用借用式 ECH 到 wreq 客户端构建器：注入带 ECHConfigList 的 TLS 选项
/// （`tls_options` 整组替换——须在所有 `emulation()` 之后调用，替换项本身
/// 已是 Chrome 指纹档）+ 把目标主机钉到 CF 任播 IP。同时把 www↔apex
/// 对应主机钉到同一组 IP——裸域与 www 互跳 301 很常见，缺了配对钉接
/// 会让重定向跳出 ECH 覆盖范围、回落到（可能被污染的）DNS。
/// Applies borrowed ECH to a wreq client builder: injects the TLS options
/// carrying the ECHConfigList (`tls_options` replaces the whole group —
/// must be called after all `emulation()` calls; the replacement itself
/// is already the Chrome profile) and pins the target host to the CF
/// anycast IPs. The www↔apex counterpart is pinned to the same IPs —
/// apex↔www 301s are common, and without the paired pin a redirect would
/// escape ECH coverage and fall back to (possibly poisoned) DNS.
pub fn apply_ech(mut builder: wreq::ClientBuilder, t: &EchTarget) -> wreq::ClientBuilder {
    builder = builder
        .tls_options(ech_tls_options(&t.ech_list))
        .resolve_to_addrs(t.host.clone(), t.addrs.iter().copied());
    if let Some(alt) = www_counterpart(&t.host) {
        builder = builder.resolve_to_addrs(alt, t.addrs.iter().copied());
    }
    builder
}

/// 返回 host 的 www↔apex 对应名：`foo.bar` ↔ `www.foo.bar`；更深的子域
/// （如 sub.foo.bar）返回 None——`www.sub.foo.bar` 通常不存在。
/// Returns the www↔apex counterpart of a host: `foo.bar` ↔ `www.foo.bar`;
/// deeper subdomains (e.g. sub.foo.bar) yield None — `www.sub.foo.bar`
/// usually doesn't exist.
fn www_counterpart(host: &str) -> Option<String> {
    const WWW: &str = "www.";
    if let Some(rest) = host.strip_prefix(WWW) {
        (!rest.is_empty()).then(|| rest.to_string())
    } else if host.split('.').count() == 2 {
        Some(format!("{WWW}{host}"))
    } else {
        None
    }
}

/// 进程级共享的借用配置槽位：每个 `StripchatApi::new` 都会新建一份
/// EchClients（监控、录制器、浏览页各持一份）——实例级槽位让每个新
/// 客户端都重打一遍 DoH+竞速（~10-20s），同一进程内出现多次重复的
/// 首次竞速。借用配置本就是全局可复用的（同一 public_name、同组钉接
/// 地址），改为进程级 OnceLock 共享：整个进程只有首次借用走内联
/// 竞速，任何实例可触发后台刷新，所有实例共用结果。
/// Process-global borrowed-config slot: every `StripchatApi::new` creates
/// its own EchClients (monitor, recorder, browse page each hold one) —
/// instance-level slots made every new client re-run DoH+race (~10-20s),
/// so a single process saw repeated inline first races. The borrowed
/// config is globally reusable anyway (same public_name, same pin
/// addrs), so it's a process-level OnceLock shared slot: only the very
/// first borrow races inline; any instance may kick a background
/// refresh, and every instance shares the result.
fn borrowed_slot() -> &'static Mutex<BorrowSlot> {
    static SLOT: std::sync::OnceLock<Mutex<BorrowSlot>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(BorrowSlot::Empty))
}

/// 进程级按主机驱逐计数：403/连接错误每驱逐一次 +1，`resolve_slow` 重建
/// 时把钉接地址左旋该计数——同列表同序重建只会让新连接落回刚才失败
/// 的那条边缘（403 原样复发），轮换后首选另一条任播 IP，才是真正的
/// 边缘重掷。跨实例共享（与 `borrowed_slot` 同理）：relay worker、监控
/// 等各自持有独立条目缓存，但任一处驱逐后，其它实例下次重建也换边。
/// Process-global per-host eviction counter: +1 per 403/connect-error
/// eviction; `resolve_slow` left-rotates the pinned addrs by this count on
/// rebuild — rebuilding with the same addr order lands the fresh conn on
/// the same failing edge (the 403 recurs), while rotation prefers another
/// anycast IP, a real edge re-roll. Shared across instances (like
/// `borrowed_slot`): relay workers, the monitor etc. keep separate entry
/// caches, but a strike recorded anywhere rotates everyone's next rebuild.
fn edge_strikes() -> &'static Mutex<HashMap<String, usize>> {
    static MAP: std::sync::OnceLock<Mutex<HashMap<String, usize>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 进程级「借用刷新在飞」标志：多个 EchClients 实例/并发 client_for
/// 同时发现借用过期时只允许一个后台刷新 / Process-global "borrowed
/// refresh in flight" flag — only one background refresh may run even
/// when many clients notice the expiry at once
static BORROWED_REFRESHING: AtomicBool = AtomicBool::new(false);

/// 刷新在飞标志的 drop 守卫：刷新任务 panic 也照样复位，避免标志
/// 卡死导致旧配置无限续用 / Drop guard for the in-flight flag: resets
/// even if the refresh task panics, so the flag can never get stuck
struct RefreshFlagGuard;
impl Drop for RefreshFlagGuard {
    fn drop(&mut self) {
        BORROWED_REFRESHING.store(false, Ordering::SeqCst);
    }
}

/// 按主机名缓存的 ECH 客户端集合。
///
/// API 与 CDN 各持一份（两者基础客户端的代理/超时/H2 参数不同）。`base`
/// 一身三用：回退目标、DoH 传输、以及借用配置失败时的兜底。
///
/// Per-hostname cache of ECH clients.
///
/// The API and CDN sides each hold one (their base clients differ in
/// proxy/timeout/H2 settings). `base` serves as the fallback client when
/// ECH is unavailable and doubles as the DoH transport for borrowed-config
/// lookups.
#[derive(Clone)]
pub struct EchClients {
    /// 基础客户端：回退目标（无 ECH / 负缓存时的兜底），兼作 DoH 传输 /
    /// Base client: fallback target (when ECH is unavailable or
    /// negative-cached), doubling as the DoH transport
    base: Client,
    /// 以给定目标重建同类客户端（闭包内捕获了代理等参数）/
    /// Rebuilds a same-flavored client for the given target (the closure
    /// captures proxy and other parameters)
    build: ClientBuilder,
    /// host -> 条目 / host -> entry
    /// 借用槽为进程级共享（`borrowed_slot()`），不入字段 /
    /// The borrowed slot is process-global (`borrowed_slot()`), not a field
    map: Arc<Mutex<HashMap<String, Entry>>>,
    /// 同主机解析在飞去重表：host → 完成广播（leader spawn 的后台任务写完
    /// 缓存后广播；跟随者订阅后读回缓存）。跨主机不互斥——修复旧实现
    /// 「全局锁贯穿解析全程」导致多主机并发 resolve 全部串行的问题。
    /// Per-host in-flight resolve dedup: host → completion broadcast (the
    /// leader's detached task writes the cache then broadcasts; followers
    /// subscribe and read the cache back). No cross-host mutual exclusion —
    /// fixes the old "global lock held across the entire resolve" that
    /// serialized every concurrent multi-host resolve.
    inflight: Arc<Mutex<HashMap<String, tokio::sync::broadcast::Sender<()>>>>,
}

impl EchClients {
    /// 创建集合：`base` 为回退客户端并兼作 DoH 传输，`build` 为按目标重建的函数。
    /// Create the set: `base` is the fallback client and doubles as the DoH
    /// transport; `build` rebuilds a client for a target.
    pub fn new(
        base: Client,
        build: impl Fn(&EchTarget) -> Result<Client> + Send + Sync + 'static,
    ) -> Self {
        Self {
            base,
            build: Arc::new(build),
            map: Arc::new(Mutex::new(HashMap::new())),
            inflight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 返回访问该主机应使用的客户端。
    ///
    /// 命中缓存直接用；未命中先确保借用配置（Cloudflare 源域），再按目标
    /// 主机构建 ECH 客户端并探活；任何失败都回退 `base`（fail-open）。互斥
    /// 锁贯穿全程，顺带对同一主机的并发解析/构建去重。
    ///
    /// Returns the client to use for this host.
    ///
    /// Cache hit → stored entry; miss → ensure the borrowed config (from a
    /// Cloudflare source domain), build a per-host ECH client, and probe it;
    /// any failure falls back to `base` (fail-open). The mutex spans the whole
    /// sequence, deduplicating concurrent resolves/builds for one host.
    pub async fn client_for(&self, host: &str) -> Client {
        self.resolve(host)
            .await
            .unwrap_or_else(|| self.base.clone())
    }

    /// 仅在该主机确实持有探活通过的 ECH 客户端时返回 `Some`；负缓存、借用
    /// 失败、探活未过等一切回退场景都返回 `None`。未命中时的解析/构建/
    /// 探活流程与 [`client_for`] 相同，只是调用方拿不到 `base` 兜底——用于
    /// 「无 ECH 通道就不该发起请求」的场合（如 CDN 兜底竞速里的 ECH 候选：
    /// 用 base 重发一遍已失败的直连只是浪费，还会污染错误清单）。
    /// Returns `Some` only when the host actually holds a probe-passed ECH
    /// client; every fallback case (negative cache, borrow failure, failed
    /// probe) returns `None`. On a miss it runs the same
    /// resolve/build/probe flow as [`client_for`], minus the `base`
    /// fallback — for callers that shouldn't issue a request at all when no
    /// ECH channel exists (e.g. the CDN fallback race's ECH candidate:
    /// re-sending the already-failed direct request over `base` is wasted
    /// work and pollutes the error list).
    pub async fn ech_client_for(&self, host: &str) -> Option<Client> {
        self.resolve(host).await
    }

    /// [`client_for`] 的解析本体：返回该主机条目内的客户端——`Some` 为
    /// 探活通过的 ECH 客户端，`None` 为负缓存/不可用（由上层决定回退
    /// 还是放弃）。快路径只读缓存；未命中走「同主机广播去重 + 跨主机
    /// 并行」——leader 把真实解析 spawn 成不随调用方取消的后台任务，
    /// 完成后广播，大家统一「等广播→读缓存」。旧实现全局锁贯穿解析
    /// 全程，多主机并发 resolve 被整体串行（状态页逐主机 ECH 探测/多
    /// TLD ECH 竞速腿会排队几十秒）。
    /// The resolution core of [`client_for`]: returns the host entry's
    /// client — `Some` for a probe-passed ECH client, `None` for
    /// negative-cache/unavailable (the caller decides between falling back
    /// and giving up). The fast path only reads the cache; on a miss it
    /// runs "same-host broadcast dedup + cross-host parallelism" — the
    /// leader spawns a detached task doing the real resolve and
    /// broadcasting on completion; everyone then takes the uniform
    /// "await broadcast → read cache" path. The old version held the
    /// global lock across the whole resolve, serializing concurrent
    /// multi-host resolves (the status page's per-host ECH probes / the
    /// multi-TLD ECH race legs queued for tens of seconds).
    async fn resolve(&self, host: &str) -> Option<Client> {
        let host = host.to_ascii_lowercase();
        {
            let map = self.map.lock().await;
            if let Some(e) = map.get(&host)
                && e.expires > Instant::now()
            {
                return e.client.clone();
            }
        }
        let mut rx = {
            let mut inf = self.inflight.lock().await;
            if let Some(tx) = inf.get(&host) {
                tx.subscribe()
            } else {
                let (tx, _) = tokio::sync::broadcast::channel::<()>(1);
                // leader 自己的接收端在 spawn 前创建——保证广播送达 /
                // the leader's own receiver is created before spawn —
                // guaranteed delivery
                let rx = tx.subscribe();
                inf.insert(host.clone(), tx.clone());
                let this = self.clone();
                let h = host.clone();
                tokio::spawn(async move {
                    // 等锁期间可能已有别的解析写入（含负缓存），复查再干 /
                    // another resolve may have written the entry (even a
                    // negative one) while we queued — re-check first
                    let cached: Option<Option<Client>> = {
                        let map = this.map.lock().await;
                        map.get(&h)
                            .filter(|e| e.expires > Instant::now())
                            .map(|e| e.client.clone())
                    };
                    let r = match cached {
                        Some(c) => c,
                        None => this.resolve_slow(&h).await,
                    };
                    this.inflight.lock().await.remove(&h);
                    let _ = tx.send(());
                    r
                });
                rx
            }
        };
        // leader 的 sender 掉落（任务 panic）→ recv 返回 Err → 照常读缓存；
        // 无条目则 None 回退 / if the leader's sender drops (task panic),
        // recv errs — the cache read below still runs and a miss falls
        // back to None
        let _ = rx.recv().await;
        let map = self.map.lock().await;
        map.get(&host).and_then(|e| e.client.clone())
    }

    /// 真实的解析/构建/探活流程（resolve 的后台任务体）：确保借用配置 →
    /// 构建目标 TLS → 逐地址探活 → 写缓存条目（正/负）。
    /// The real resolve/build/probe flow (resolve's detached task body):
    /// ensure borrowed config → build target TLS → probe addrs → write the
    /// cache entry (positive or negative).
    async fn resolve_slow(&self, host: &str) -> Option<Client> {
        let Some(borrowed) = self.ensure_borrowed().await else {
            self.map.lock().await.insert(
                host.to_string(),
                Entry {
                    client: None,
                    expires: Instant::now() + ENTRY_NEG_TTL,
                },
            );
            return None;
        };

        let mut target = EchTarget {
            host: host.to_string(),
            addrs: borrowed.addrs.clone(),
            ech_list: borrowed.ech_list.clone(),
        };
        // 驱逐后重建：按打击计数轮换钉接地址顺序，新连接与探活都首选另
        // 一条任播 IP——否则同序重连只会落回刚才 403/失败的那条边缘。
        // 借用刷新后地址表变了也无妨，取模轮换只是换个起点。
        // Post-eviction rebuild: rotate the pinned addr order by the strike
        // count so both the probe and the fresh conn prefer another anycast
        // IP — same-order reconnects would land right back on the edge that
        // just 403'd/failed. A changed addr list after a borrow refresh is
        // harmless: the modulo rotation simply picks a different start.
        let strikes = *edge_strikes().lock().await.get(host).unwrap_or(&0);
        let addr_count = target.addrs.len();
        if strikes > 0 && addr_count > 1 {
            target.addrs.rotate_left(strikes % addr_count);
        }
        // 最多两轮：ECH 被拒且服务端附了 retry_configs（密钥轮换）时，就地
        // 换密钥重建客户端再探一次 / Up to two rounds: when ECH is rejected
        // with server retry_configs (key rotation), swap keys in place,
        // rebuild the client and probe once more
        let mut entry_client = None;
        for attempt in 0..2 {
            let client = match (self.build)(&target) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "{}",
                        crate::tl!("ech.clientBuildFailed", host = host, error = e)
                    );
                    break;
                }
            };
            match probe_ech(&target.ech_list, host, &target.addrs).await {
                ProbeOutcome::Good => {
                    entry_client = Some(client);
                    break;
                }
                ProbeOutcome::Rejected(Some(retry_list)) if attempt == 0 => {
                    // 规范恢复路径：服务端把新配置直接塞进拒绝里，就地更新
                    // 借用槽并换新 ECHConfigList 重试，无需再打 DoH /
                    // The spec's recovery path: the server embeds fresh
                    // configs in the rejection — update the slot in place
                    // and retry with the new ECHConfigList, no extra DoH
                    self.apply_retry_configs(&borrowed, &retry_list).await;
                    target.ech_list = retry_list;
                    continue;
                }
                ProbeOutcome::Rejected(None) => {
                    // 密钥轮换但服务端没给新配置：借用配置标为短冷却，让下次
                    // client_for 经 DoH 重解析 / Key rotated without retry
                    // configs: mark the borrowed config stale for a short
                    // cooldown so the next client_for re-resolves via DoH
                    self.expire_borrowed(ROTATED_STALE_TTL).await;
                    tracing::info!("{}", crate::tl!("ech.probeFailed", host = host));
                    break;
                }
                _ => {
                    // 连接/TLS/回源失败：目标不在 ECH 边缘或边缘无法回源该
                    // 主机（zone-miss → 530）→ 按无 ECH 通道负缓存 /
                    // Connect/TLS/origin failure: not an ECH edge, or the edge
                    // cannot route this host (zone-miss → 530) —
                    // negative-cache as no-ECH
                    tracing::info!("{}", crate::tl!("ech.probeFailed", host = host));
                    break;
                }
            }
        }
        // 探活通过的条目走固定长寿 ENTRY_TTL，与借用 TTL 解耦——借用
        // 到期只触发后台刷新，不该带着每分钟的条目重建+重探活；条目随
        // 连接错误驱逐，密钥轮换经 Rejected/驱逐路径自愈。失败条目走
        // 固定负缓存 / Passing entries get the fixed ENTRY_TTL, decoupled
        // from the borrow TTL — borrow expiry only kicks a background
        // refresh and shouldn't drag per-minute entry rebuilds+re-probes;
        // entries evict on connect errors and key rotation self-heals via
        // the Rejected/eviction path. Failed entries use the fixed
        // negative TTL.
        let expires = if entry_client.is_some() {
            Instant::now() + ENTRY_TTL
        } else {
            Instant::now() + ENTRY_NEG_TTL
        };
        self.map.lock().await.insert(
            host.to_string(),
            Entry {
                client: entry_client.clone(),
                expires,
            },
        );
        entry_client
    }

    /// 确保借用配置就绪：TTL 内直接复用；**过期但配置仍在 → stale-while-
    /// revalidate，立刻返回旧配置并后台刷新**（DoH+竞速在坏链路上要
    /// 10-20s，旧实现持全局槽位锁内联重借，每个 TTL 周期冻结全部
    /// client_for——日志里的周期性分片停滞正源于此）；首次借用或冷却
    /// 过后才走内联解析（持槽位锁顺带对并发首次借用去重）。
    /// Ensures the borrowed config is ready: reused within TTL; **expired
    /// but present → stale-while-revalidate — return the stale config
    /// immediately and refresh in the background** (DoH+race takes 10-20s
    /// on bad links; the old inline re-borrow held the global slot lock
    /// and froze every client_for each TTL cycle — the periodic segment
    /// stalls in the logs came from exactly that). Only a first borrow or
    /// a cooled-down failure resolves inline (the slot lock also dedupes
    /// concurrent first borrows).
    async fn ensure_borrowed(&self) -> Option<Borrowed> {
        {
            let slot = borrowed_slot().lock().await;
            match &*slot {
                BorrowSlot::Ready(b) if b.expires > Instant::now() => {
                    return Some(b.clone());
                }
                // 密钥轮换冷却：无任何可服务配置 / rotation cooldown: nothing to serve
                BorrowSlot::Failed(until) if *until > Instant::now() => return None,
                // 过期旧配置：立即续用并踢后台刷新；旧配置即便密钥已轮换，
                // per-host 探活也能用 retry_configs 就地换新
                // Stale-but-present config: keep serving it and kick a
                // background refresh; even if its key rotated, per-host
                // probing heals in place via retry_configs
                BorrowSlot::Ready(b) => {
                    let b = b.clone();
                    Self::spawn_borrowed_refresh(self.base.clone());
                    return Some(b);
                }
                _ => {}
            }
        }

        // 无可服务配置（首次借用/失败冷却过后）：持锁内联解析。
        // 等锁期间可能已有其他实例/后台刷新写入了就绪配置——先复查。
        // Nothing to serve (first borrow / post-cooldown): resolve inline
        // under the lock. Another instance or the background refresh may
        // have populated the slot while we waited — re-check first.
        let mut slot = borrowed_slot().lock().await;
        match &*slot {
            BorrowSlot::Ready(b) if b.expires > Instant::now() => return Some(b.clone()),
            BorrowSlot::Failed(until) if *until > Instant::now() => return None,
            _ => {}
        }
        let Some(b) = resolve_borrowed(&self.base).await else {
            tracing::warn!(
                "{}",
                crate::tl!(
                    "ech.borrowedResolveFailed",
                    secs = BORROWED_FAIL_TTL.as_secs()
                )
            );
            *slot = BorrowSlot::Failed(Instant::now() + BORROWED_FAIL_TTL);
            return None;
        };
        *slot = BorrowSlot::Ready(b.clone());
        Some(b)
    }

    /// 后台刷新借用配置（SWR 的 R）：在飞标志去重，结果写回槽位。
    /// 刷新失败且槽内有旧配置 → 旧配置短期续约（下轮再试）；没有任何
    /// 配置才进入失败冷却 / Refreshes the borrowed config in the
    /// background (the R of SWR): deduped by the in-flight flag, writes
    /// the result back to the slot. On failure with a stale config still
    /// present → renew it briefly (retry next cycle); only enters the
    /// failure cooldown when nothing was ever borrowed
    fn spawn_borrowed_refresh(doh: Client) {
        if BORROWED_REFRESHING.swap(true, Ordering::SeqCst) {
            return;
        }
        tokio::spawn(async move {
            let _guard = RefreshFlagGuard;
            match resolve_borrowed(&doh).await {
                Some(b) => {
                    *borrowed_slot().lock().await = BorrowSlot::Ready(b);
                }
                None => {
                    let mut slot = borrowed_slot().lock().await;
                    match &*slot {
                        BorrowSlot::Ready(old) => {
                            *slot = BorrowSlot::Ready(Borrowed {
                                expires: Instant::now() + BORROWED_REFRESH_RETRY,
                                ..old.clone()
                            });
                        }
                        _ => {
                            *slot = BorrowSlot::Failed(Instant::now() + BORROWED_FAIL_TTL);
                        }
                    }
                }
            }
        });
    }

    /// ECH 拒绝附带的 retry_configs（已是序列化的 ECHConfigList）：就地
    /// 更新借用槽——密钥轮换的规范恢复路径，不打 DoH。
    /// The server retry_configs attached to an ECH rejection (already a
    /// serialized ECHConfigList): updates the borrowed slot in place —
    /// the spec's key-rotation recovery path, no DoH needed.
    async fn apply_retry_configs(&self, old: &Borrowed, retry_list: &[u8]) {
        let new = Borrowed {
            ech_list: retry_list.to_vec(),
            addrs: old.addrs.clone(),
            expires: old.expires,
        };
        tracing::info!("{}", crate::tl!("ech.keysRotated"));
        *borrowed_slot().lock().await = BorrowSlot::Ready(new);
    }

    /// 借用配置被判失效（密钥轮换且未附新配置）：置短期冷却让请求走
    /// 回退通道，同时踢后台刷新——刷新一旦成功会提前把槽位换回
    /// Ready，ECH 通道不用干等满冷却 / Marks the borrowed config stale
    /// (key rotated with no retry configs): a short cooldown sends
    /// requests down the fallback path while a background refresh runs —
    /// as soon as it succeeds the slot flips back to Ready, so the ECH
    /// channel doesn't idle out the full cooldown
    async fn expire_borrowed(&self, ttl: Duration) {
        *borrowed_slot().lock().await = BorrowSlot::Failed(Instant::now() + ttl);
        Self::spawn_borrowed_refresh(self.base.clone());
    }

    /// 该主机的 ECH 客户端出现连接层错误（TLS 失败、ECH 密钥轮换被拒等）：
    /// 驱逐条目，下次 client_for 重新探活——若密钥已轮换，探活路径会用
    /// retry_configs 自动换新。仅 ECH 条目可驱逐，负缓存/缺失不受影响。
    /// A connect-level error on the host's ECH client (TLS failure, ECH
    /// key-rotation rejection, etc.): evicts the entry so the next
    /// client_for re-probes — on rotation the probe path swaps in
    /// retry_configs automatically. Only ECH entries can be evicted;
    /// negative-cache/missing entries are untouched.
    pub async fn evict(&self, host: &str) {
        let host = host.to_ascii_lowercase();
        let mut map = self.map.lock().await;
        if matches!(map.get(&host), Some(e) if e.client.is_some()) {
            map.remove(&host);
            // 记一次打击：重建时轮换钉接地址，别落回同一条边缘 /
            // record a strike so the rebuild rotates pinned addrs off the
            // same edge
            *edge_strikes().lock().await.entry(host).or_default() += 1;
        }
    }

    /// 该主机的 ECH 请求返回了服务端错误（5xx，如 CF 530 zone-miss——边缘
    /// 能解密但无法回源）：按无 ECH 通道负缓存，TTL 内不再为该主机启用 ECH。
    /// An ECH request to the host returned a server error (5xx, e.g. CF 530
    /// zone-miss — the edge decrypts but cannot route): negative-caches the
    /// host as no-ECH; ECH stays off for this host within the TTL.
    pub async fn note_server_error(&self, host: &str) {
        let host = host.to_ascii_lowercase();
        let mut map = self.map.lock().await;
        if matches!(map.get(&host), Some(e) if e.client.is_some()) {
            tracing::info!("{}", crate::tl!("ech.edgeErrorDemoted", host = host));
            map.insert(
                host,
                Entry {
                    client: None,
                    expires: Instant::now() + ENTRY_NEG_TTL,
                },
            );
        }
    }

    /// 该主机的请求返回 403（CF WAF 拦截）。实测该拦截按「连接所落的
    /// CF 边缘节点」标记——同一出口下不同连接落不同边缘、命中率各异，
    /// 而 ECH 长连接落上坏边缘会一直 403 到重建。因此驱逐该主机条目：
    /// 下次 client_for 重新探活并新建连接（重掷边缘抽签），而不是降级
    /// 回普通路径（普通路径的 403 率反而更高）。仅在该主机确实持有
    /// ECH 客户端时动作；普通客户端路径的 403 不触发任何状态变化。
    /// A request to the host returned 403 (CF WAF block). Measurements
    /// show the block is keyed on the CF edge node the connection landed
    /// on — conns from the same egress land on different edges with
    /// different hit rates, and an ECH long-lived conn on a bad edge
    /// keeps 403ing until rebuilt. So the entry is evicted plus a strike
    /// recorded: the next client_for rebuild rotates the pinned addr order
    /// so the fresh conn prefers another anycast IP (a real edge re-roll —
    /// same-order rebuilds just landed back on the same 403ing edge),
    /// rather than demoting to the plain path (which blocks even more).
    /// Only acts when the host actually holds an ECH client; 403s on the
    /// plain path change nothing.
    pub async fn note_forbidden(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let mut map = self.map.lock().await;
        if matches!(map.get(&host), Some(e) if e.client.is_some()) {
            tracing::warn!("{}", crate::tl!("ech.forbiddenOverEch", host = host));
            map.remove(&host);
            *edge_strikes().lock().await.entry(host).or_default() += 1;
            true
        } else {
            false
        }
    }
}

/// 竞速后保留的钉接地址数（按实测速度排序）/
/// Pin addresses kept after the race (sorted by measured speed)
const RACE_KEEP_ADDRS: usize = 8;
/// 体数据达到该字节数即按真实吞吐计分，否则降级为握手延迟样本 /
/// Bodies reaching this size are scored by real throughput, otherwise the
/// sample degrades to handshake latency
const RACE_SPEED_MIN_BYTES: usize = 64 * 1024;

/// 竞速评分：Speed 层（读到足够体数据，按 B/s 排序）永远优于 Latency
/// 层（只拿到握手延迟，按 RTT 排序）——握手快不等于下载快，实测过
/// 532ms 握手的路径吞吐 <35KB/s。/ Race score: the Speed tier (enough
/// body read → ranked by B/s) always outranks the Latency tier (handshake
/// only → ranked by RTT) — a fast handshake proved not to mean a fast
/// download (a 532ms-handshake path measured <35KB/s).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RaceScore {
    /// 仅握手延迟（毫秒，小者优）/ Handshake latency only (ms, lower wins)
    Latency(u128),
    /// 实测吞吐（字节/秒，大者优）/ Measured throughput (bytes/s, higher wins)
    Speed(u128),
}

impl RaceScore {
    /// 由 (耗时, 字节数) 计算评分 / Scores one (elapsed, bytes) sample
    fn of(elapsed: Duration, bytes: usize) -> Self {
        if bytes >= RACE_SPEED_MIN_BYTES {
            RaceScore::Speed(bytes as u128 * 1000 / elapsed.as_millis().max(1))
        } else {
            RaceScore::Latency(elapsed.as_millis())
        }
    }
    /// 排序键：Speed 降序优先、Latency 升序次优 /
    /// Sort key: Speed descending first, Latency ascending second
    fn sort_key(&self) -> (std::cmp::Reverse<u8>, std::cmp::Reverse<u128>, u128) {
        match self {
            RaceScore::Speed(bps) => (std::cmp::Reverse(1), std::cmp::Reverse(*bps), 0),
            RaceScore::Latency(ms) => (std::cmp::Reverse(0), std::cmp::Reverse(0), *ms),
        }
    }
}

/// 竞速样本的日志展示值：Speed 档 `{MB/s}-{ms}ms`（速度-全程耗时），
/// Latency 档 `{ms}ms`（无吞吐可报时单列耗时）。/ Display value of a
/// race sample: Speed tier `{MB/s}-{ms}ms` (throughput-total time),
/// Latency tier `{ms}ms` (total time alone when no throughput is known).
fn race_score_text(score: RaceScore, elapsed: Duration) -> String {
    match score {
        RaceScore::Speed(bps) => {
            format!(
                "{:.2}MB/s-{}ms",
                bps as f64 / 1048576.0,
                elapsed.as_millis()
            )
        }
        RaceScore::Latency(ms) => format!("{ms}ms"),
    }
}

/// 借用配置竞速结果：胜出的密钥组（ech + 按速度排好的地址池 + TTL +
/// 展示用标签）。/ Race outcome: the winning key group (ech + speed-ranked
/// addr pool + TTL + display label).
struct RacePick {
    ech_list: Vec<u8>,
    addrs: Vec<SocketAddr>,
    ttl: Duration,
    /// 日志用：胜出地址来自的源域 / For logging: the source domain of the
    /// winning address
    source: String,
    /// 日志用：胜出地址 / For logging: the winning address
    best_addr: SocketAddr,
    /// 日志用：胜出评分（吞吐 B/s 或延迟 ms）/ For logging: the winning
    /// score (throughput B/s or latency ms)
    best: RaceScore,
    /// 日志用：胜出样本全程耗时（连接+握手+读体）/ For logging: the
    /// winning sample's total probe time (connect + handshake + body read)
    elapsed: Duration,
}

/// 完整解析一份借用配置：DoH 多源域查询 → 密钥分组地址池竞速 → Borrowed。
/// 供首次内联借用与后台刷新共用；不写槽位，由调用方决定落盘时机。
/// Resolves a full borrowed config: multi-source DoH lookup → key-group
/// addr-pool race → Borrowed. Shared by the inline first borrow and the
/// background refresh; does not touch the slot — the caller decides when
/// to write.
async fn resolve_borrowed(doh_client: &Client) -> Option<Borrowed> {
    let cands = doh::lookup_borrowed(doh_client).await;
    if cands.is_empty() {
        return None;
    }

    // 候选竞速：按 ECH 密钥分组后对全池 hint 地址并发测速（真实吞吐），
    // 胜组地址池按评分排序截取；全灭则退回首个候选——与旧「按序取首」
    // 语义一致，per-host 探活仍会兜底 / Candidate race: group by ECH
    // key, then speed-probe the pooled hint addresses concurrently
    // (real throughput); the winning group's pool is score-sorted and
    // truncated. If every probe dies, fall back to the first candidate —
    // same as the old first-in-order semantics, and per-host probing
    // still guards
    let (ech_cfg, addrs, ttl) = match race_borrowed(&cands).await {
        Some(p) => {
            tracing::info!(
                "{}",
                crate::tl!(
                    "ech.borrowedPicked",
                    source = p.source,
                    addr = p.best_addr.to_string(),
                    score = race_score_text(p.best, p.elapsed)
                )
            );
            (p.ech_list, p.addrs, p.ttl)
        }
        None => {
            let rec = cands.into_iter().next().expect("non-empty");
            (rec.ech_raw, rec.addrs, rec.ttl)
        }
    };
    // 就绪提示仅在首次借到配置时输出；此后 TTL 刷新静默，
    // 失败路径仍由警告日志暴露 / The ready notice is logged
    // only on the first successful borrow; later TTL refreshes
    // stay silent while failures still surface via warnings
    if !doh::mark_borrowed_logged() {
        tracing::info!("{}", crate::tl!("ech.borrowedReady"));
    }
    Some(Borrowed {
        ech_list: ech_cfg,
        addrs,
        expires: Instant::now() + ttl,
    })
}

/// 多候选借用配置竞速：先按 ECH 密钥把候选分组（同密钥组的 hint 地址
/// 可互相混用——实测绝大多数源域发布的是 CF 全网共享的同一份配置），
/// 对全池地址并发 `probe_addr_speed`（统一打 speed.cloudflare.com 的
/// 测速对象，实测吞吐而非仅握手延迟），取评分最高的地址所在密钥组，
/// 其地址池按评分排序后截断保留 [`RACE_KEEP_ADDRS`] 个。
/// 全部探活失败返回 None（调用方退回首个候选——与旧「按序取首」语义
/// 一致，per-host 探活仍会兜底）。
/// Race across borrowed-config candidates: group by ECH key first (hint
/// addresses within one key group are interchangeable — most sources
/// measured publishing literally the same CF network-wide config), then
/// run `probe_addr_speed` on the pooled addresses (all hitting the same
/// speed.cloudflare.com object — real throughput, not just handshake
/// RTT). The winning key group is whichever produced the best-scoring
/// address; its pooled addresses are sorted by score and truncated to
/// [`RACE_KEEP_ADDRS`]. None when every probe fails (the caller falls
/// back to the first candidate — same as the old first-in-order
/// semantics, and per-host probing still guards).
async fn race_borrowed(cands: &[doh::RecordExtract]) -> Option<RacePick> {
    use std::collections::HashSet;

    // 1) 对全部 (候选, hint地址) 并发测速；同一地址只探一次 /
    // 1) Speed-probe every (candidate, hint addr) concurrently; each
    // unique address only once
    let mut set = tokio::task::JoinSet::new();
    let mut seen: HashSet<SocketAddr> = HashSet::new();
    for (i, c) in cands.iter().enumerate() {
        for addr in &c.addrs {
            if !seen.insert(*addr) {
                continue;
            }
            let ech_list = c.ech_raw.clone();
            let addr = *addr;
            set.spawn(
                async move { probe_addr_speed(&ech_list, addr).await.map(|s| (i, addr, s)) },
            );
        }
    }
    // 2) 收集全部样本（首个通过即返回会退化成握手竞速——速度层要等读体，
    // 收集完排序更准）/ 2) Collect ALL samples (returning at the first
    // success would degrade to a handshake race — the Speed tier needs the
    // body read, so rank after collection)
    let mut samples: Vec<(usize, SocketAddr, Duration, usize)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(Some((i, a, (el, n)))) = joined {
            samples.push((i, a, el, n));
        }
    }
    if samples.is_empty() {
        return None;
    }
    // 3) 全局最优地址 → 其密钥组（同 ech_raw 的候选可互换 hint 地址）/
    // 3) Global best address → its key group (same-ech_raw candidates can
    // interchange hint addresses)
    samples.sort_by_key(|x| RaceScore::of(x.2, x.3).sort_key());
    // 逐样本列出实测值（按评分排序，最快在前）；探活失败的地址不在
    // 样本里、无法展示。样本逐条较啰嗦，降为 debug——info 只保留
    // borrowedPicked 胜出记录 / List every sample in score order
    // (fastest first); failed probes never became samples and can't be
    // shown. Per-sample lines are noisy — demoted to debug; info keeps
    // only the borrowedPicked winner record
    for (i, a, el, n) in &samples {
        tracing::debug!(
            "{}",
            crate::tl!(
                "ech.borrowedRaceSample",
                source = cands[*i].source,
                addr = a.to_string(),
                score = race_score_text(RaceScore::of(*el, *n), *el)
            )
        );
    }
    let (win_i, win_addr, win_el, win_bytes) = samples[0];
    let key = &cands[win_i].ech_raw;

    // 4) 胜组地址池 = 组内全部候选的 hint 并集，按已测样本排序、未测排尾 /
    // 4) Winning group's addr pool = union of its candidates' hints, sorted
    // by measured sample, unprobed last
    let mut pool: Vec<(SocketAddr, RaceScore)> = Vec::new();
    let mut pool_seen: HashSet<SocketAddr> = HashSet::new();
    for c in cands.iter().filter(|c| c.ech_raw == *key) {
        for a in &c.addrs {
            if pool_seen.insert(*a) {
                let sc = samples
                    .iter()
                    .find(|(_, sa, _, _)| sa == a)
                    .map(|(_, _, el, n)| RaceScore::of(*el, *n))
                    .unwrap_or(RaceScore::Latency(u128::MAX));
                pool.push((*a, sc));
            }
        }
    }
    pool.sort_by_key(|(_, s)| s.sort_key());
    let mut addrs: Vec<SocketAddr> = pool.iter().map(|(a, _)| *a).collect();
    addrs.truncate(RACE_KEEP_ADDRS);
    // 胜出地址置首（双保险：排序后本来就该在首）/ Winner first (belt &
    // braces — sorting should already put it there)
    if let Some(p) = addrs.iter().position(|a| *a == win_addr) {
        addrs.swap(0, p);
    }
    let ttl = cands
        .iter()
        .filter(|c| c.ech_raw == *key)
        .map(|c| c.ttl)
        .min()
        .unwrap_or(doh::BORROWED_TTL_FALLBACK);
    Some(RacePick {
        ech_list: cands[win_i].ech_raw.clone(),
        addrs,
        ttl,
        source: cands[win_i].source.clone(),
        best_addr: win_addr,
        best: RaceScore::of(win_el, win_bytes),
        elapsed: win_el,
    })
}
