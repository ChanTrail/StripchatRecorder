//! ECH（Encrypted Client Hello）支持——"借用式"实现。
//! ECH (Encrypted Client Hello) support — the "borrowed config" variant.
//!
//! 与浏览器的标准模式（用目标自己的 HTTPS 记录）不同，本实现参照
//! DoH-ECH-Demo 的思路：
//!
//! - 从固定的 Cloudflare ECH 源域（`cloudflare-ech.com` 等）查 HTTPS 记录，
//!   借来 ECHConfigList（public_name = `cloudflare-ech.com`，CF 全网共享
//!   同一份密钥对）和 `ipv4hint`（CF 任播 IP）。
//! - 该配置对所有 Cloudflare 接入的目标主机通用：TLS ServerName 仍为目标
//!   主机，真实 SNI 随 inner ClientHello 公钥加密，链路上只能看到
//!   `cloudflare-ech.com`；连接经 `resolve_to_addrs` 钉到 CF IP，顺带免疫
//!   目标域名的 DNS 污染（无需再解析目标 A 记录）。
//! - 主机不在支持 ECH 的边缘（非 Cloudflare 等）时握手必败——构建后先发
//!   一次轻量探活，失败按负缓存标记为无 ECH 通道：`client_for` 调用方回退
//!   普通客户端，`ech_client_for` 调用方按无通道处理（机会性增强，不会
//!   更差）。
//!
//! 按职责拆分为以下子模块（父模块只做文档与统一再导出，外部 API 路径不变）：
//!
//! - [`doh`]：公共 DoH（dns-json）端点、HTTPS（type 65）记录查询、
//!   `ech=` / `ipv?hint=` 文本解析与「源域 × 端点」借用矩阵
//! - [`clients`]：按主机名缓存的 ECH 客户端集合——借用配置槽位、逐主机
//!   探活与负缓存回退（fail-open），对外提供 [`EchClients`] / [`EchTarget`]
//! - [`probe`]：裸 TLS+ECH 握手探活，验证目标是否位于 ECH 边缘
//!
//! Unlike the standard browser model (each target's own HTTPS record), this
//! follows the DoH-ECH-Demo approach:
//!
//! - Resolve the HTTPS record of a fixed Cloudflare ECH source domain
//!   (`cloudflare-ech.com` etc.) to borrow its ECHConfigList
//!   (public_name = `cloudflare-ech.com` — the same keypair network-wide)
//!   plus `ipv4hint` (Cloudflare anycast IPs).
//! - That config works for every Cloudflare-fronted target: the TLS
//!   ServerName stays the target host, the real SNI travels inside the
//!   encrypted inner ClientHello, and on-path observers only see
//!   `cloudflare-ech.com`. Connections are pinned to the CF IP via
//!   `resolve_to_addrs`, which also makes target-domain DNS poisoning
//!   irrelevant (no target A lookup is needed at all).
//! - Hosts not behind an ECH-capable edge always fail the handshake — so a
//!   lightweight probe runs after build, and failures are negative-cached
//!   as no-ECH: `client_for` callers fall back to the plain client while
//!   `ech_client_for` callers see no channel (opportunistic, never worse).
//!
//! Split into submodules by concern (the parent only documents and
//! re-exports; external API paths are unchanged):
//!
//! - [`doh`]: public DoH (dns-json) endpoints, HTTPS (type 65) record
//!   queries, `ech=` / `ipv?hint=` presentation parsing and the
//!   "source domain × endpoint" borrow matrix
//! - [`clients`]: per-hostname cache of ECH clients — borrowed-config slot,
//!   per-host probing and negative-cache fallback (fail-open); exposes
//!   [`EchClients`] / [`EchTarget`]
//! - [`probe`]: bare TLS+ECH handshake probe verifying the target sits on an
//!   ECH-capable edge

// ─── 子模块 / Submodules ─────────────────────────────────────────────────────

/// 按主机名缓存的 ECH 客户端集合 / Per-hostname cache of ECH clients
mod clients;
/// DoH 端点与 HTTPS 记录解析 / DoH endpoints and HTTPS record parsing
mod doh;
/// 裸 TLS+ECH 握手探活 / Bare TLS+ECH handshake probe
mod probe;

// ─── 统一再导出（外部 API 路径保持 crate::ech::* 不变）───────────────────────
// ─── Unified re-exports (external API paths stay `crate::ech::*`) ───────────

pub use clients::{EchClients, EchTarget, apply_ech};
