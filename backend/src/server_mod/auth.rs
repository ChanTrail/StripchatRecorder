//! 管理员认证模块 / Admin Authentication Module
//!
//! 功能：
//! - Token 存储：绑定登录 IP，带过期时间（默认 8 小时），每次有效请求自动续期
//! - IP 风控：同一 IP 登录失败超过阈值后临时封禁
//! - 密码强度校验：至少 6 位，包含字母、数字和特殊字符
//! - Axum 中间件：验证 Bearer Token，setup 阶段自动放行
//!
//! Features:
//! - Token storage: bound to login IP, with expiry (default 8h), auto-renewed on each valid request
//! - IP rate limiting: temporarily block IPs that exceed failed login attempts
//! - Password strength validation: min 6 chars, must include letters, digits, and special chars
//! - Axum middleware: validates Bearer Token; auto-allows during setup phase

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

// ─── 常量 / Constants ──────────────────────────────────────────────────────

/// Token 有效期（8 小时）/ Token expiry (8 hours)
pub const TOKEN_TTL: Duration = Duration::from_secs(8 * 3600);
/// 每次有效请求自动续期时长（1 小时）/ Auto-renew duration on each valid request (1 hour)
pub const TOKEN_RENEW: Duration = Duration::from_secs(3600);
/// 连续失败登录次数上限 / Max consecutive failed login attempts before lockout
pub const MAX_FAIL: u32 = 5;
/// 封禁时长（15 分钟）/ Lockout duration (15 minutes)
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(15 * 60);

// ─── 数据结构 / Data structures ─────────────────────────────────────────────

struct SessionEntry {
    /// Token 字符串 / Token string
    token: String,
    /// 绑定的登录 IP / Bound login IP address
    bound_ip: IpAddr,
    /// 过期时刻 / Expiry instant
    expires_at: Instant,
}

struct FailEntry {
    /// 连续失败次数 / Consecutive failure count
    count: u32,
    /// 封禁解除时刻（None = 未封禁）/ Lockout lift instant (None = not locked)
    locked_until: Option<Instant>,
}

struct TokenStoreInner {
    session: Option<SessionEntry>,
    password_configured: bool,
    /// IP → 登录失败记录 / IP → login failure record
    fail_map: HashMap<IpAddr, FailEntry>,
}

/// Token 存储（单用户）/ Token store (single-user)
#[derive(Clone)]
pub struct TokenStore(Arc<RwLock<TokenStoreInner>>);

impl Default for TokenStore {
    fn default() -> Self {
        Self::new(false)
    }
}

impl TokenStore {
    pub fn new(password_already_set: bool) -> Self {
        Self(Arc::new(RwLock::new(TokenStoreInner {
            session: None,
            password_configured: password_already_set,
            fail_map: HashMap::new(),
        })))
    }

    /// 登录成功：生成 Token，绑定 IP，重置该 IP 的失败计数。
    /// Login success: generate token, bind IP, reset failure count for this IP.
    pub fn create_session(&self, ip: IpAddr) -> String {
        let token = new_token();
        let mut inner = self.0.write();
        inner.session = Some(SessionEntry {
            token: token.clone(),
            bound_ip: ip,
            expires_at: Instant::now() + TOKEN_TTL,
        });
        inner.fail_map.remove(&ip);
        token
    }

    /// 验证 Token：检查匹配、IP 绑定、是否过期。
    /// 验证成功时自动续期 TOKEN_RENEW。
    ///
    /// Verify token: checks match, IP binding, and expiry.
    /// Auto-renews by TOKEN_RENEW on success.
    pub fn verify(&self, token: &str, ip: IpAddr) -> VerifyResult {
        let mut inner = self.0.write();
        let session = match &mut inner.session {
            Some(s) => s,
            None => return VerifyResult::Invalid,
        };
        if session.token != token {
            return VerifyResult::Invalid;
        }
        if Instant::now() > session.expires_at {
            inner.session = None;
            return VerifyResult::Expired;
        }
        // IP 不匹配时拒绝（可能是 token 泄漏）
        // Reject on IP mismatch (possible token leak)
        if session.bound_ip != ip {
            return VerifyResult::IpMismatch;
        }
        // 自动续期 / Auto-renew
        let new_exp = Instant::now() + TOKEN_RENEW;
        if new_exp > session.expires_at {
            session.expires_at = new_exp;
        }
        VerifyResult::Ok
    }

    /// 强制续期：将过期时间延长至 now + TOKEN_TTL（前端主动调用 /api/auth/renew 时）。
    /// Force-renew: extend expiry to now + TOKEN_TTL (called by frontend via /api/auth/renew).
    pub fn renew(&self, token: &str, ip: IpAddr) -> bool {
        let mut inner = self.0.write();
        if let Some(s) = &mut inner.session
            && s.token == token && s.bound_ip == ip && Instant::now() <= s.expires_at
        {
            s.expires_at = Instant::now() + TOKEN_TTL;
            return true;
        }
        false
    }

    /// 登出：清除当前 session。
    pub fn clear(&self) {
        self.0.write().session = None;
    }

    pub fn is_logged_in(&self) -> bool {
        let inner = self.0.read();
        inner.session.as_ref().is_some_and(|s| Instant::now() <= s.expires_at)
    }

    pub fn mark_password_configured(&self) {
        self.0.write().password_configured = true;
    }

    pub fn has_password(&self) -> bool {
        self.0.read().password_configured
    }

    // ── IP 风控 / IP rate limiting ────────────────────────────────────────

    /// 记录一次登录失败。返回该 IP 是否已被封禁。
    /// Record a login failure. Returns whether this IP is now locked.
    pub fn record_fail(&self, ip: IpAddr) -> bool {
        let mut inner = self.0.write();
        let entry = inner.fail_map.entry(ip).or_insert(FailEntry {
            count: 0,
            locked_until: None,
        });
        entry.count += 1;
        if entry.count >= MAX_FAIL {
            entry.locked_until = Some(Instant::now() + LOCKOUT_DURATION);
        }
        entry.locked_until.is_some_and(|t| Instant::now() < t)
    }

    /// 检查 IP 是否正在封禁中。封禁已过期时自动清除记录。
    /// Check whether an IP is currently locked. Auto-clears expired lockouts.
    pub fn is_locked(&self, ip: IpAddr) -> bool {
        let mut inner = self.0.write();
        if let Some(entry) = inner.fail_map.get_mut(&ip)
            && let Some(until) = entry.locked_until
        {
            if Instant::now() < until {
                return true;
            }
            // 封禁已过期，清除 / Lockout expired; clear
            inner.fail_map.remove(&ip);
        }
        false
    }

    /// 返回 IP 封禁剩余秒数（0 = 未封禁或已过期）。
    /// Returns remaining lockout seconds for an IP (0 = not locked or expired).
    pub fn lockout_remaining_secs(&self, ip: IpAddr) -> u64 {
        let inner = self.0.read();
        if let Some(entry) = inner.fail_map.get(&ip)
            && let Some(until) = entry.locked_until
        {
            let now = Instant::now();
            if now < until {
                return (until - now).as_secs();
            }
        }
        0
    }
}

/// Token 验证结果 / Token verification result
pub enum VerifyResult {
    Ok,
    Invalid,
    Expired,
    IpMismatch,
}

// ─── 工具函数 / Utilities ──────────────────────────────────────────────────

/// 生成 32 字节随机 Token（十六进制字符串）。
fn new_token() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 从请求中提取客户端 IP（优先 X-Forwarded-For，其次 ConnectInfo）。
/// Extract client IP from request (X-Forwarded-For first, then ConnectInfo).
pub fn extract_ip(req: &Request<Body>) -> IpAddr {
    // 优先信任反向代理的 X-Forwarded-For 首个地址
    // Prefer first address from X-Forwarded-For (reverse proxy)
    if let Some(fwd) = req.headers().get("x-forwarded-for")
        && let Ok(s) = fwd.to_str()
        && let Some(first) = s.split(',').next()
        && let Ok(ip) = first.trim().parse::<IpAddr>()
    {
        return ip;
    }
    // 其次用 ConnectInfo / Fall back to ConnectInfo
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

/// 密码强度校验：至少 6 位，包含字母、数字、特殊字符。
/// Password strength check: min 6 chars, must contain letters, digits, and special chars.
pub fn validate_password_strength(password: &str) -> Result<(), &'static str> {
    if password.len() < 6 {
        return Err("密码长度不能少于 6 位 / Password must be at least 6 characters");
    }
    let has_letter = password.chars().any(|c| c.is_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());
    if !has_letter {
        return Err("密码必须包含字母 / Password must contain at least one letter");
    }
    if !has_digit {
        return Err("密码必须包含数字 / Password must contain at least one digit");
    }
    if !has_special {
        return Err("密码必须包含特殊字符 / Password must contain at least one special character");
    }
    Ok(())
}

// ─── Axum 中间件 / Axum middleware ─────────────────────────────────────────

pub async fn auth_middleware(
    State(store): State<TokenStore>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();

    if !path.starts_with("/api/") {
        return next.run(req).await;
    }
    if (path.starts_with("/api/auth/") && path != "/api/auth/change-password") || path == "/api/events" {
        return next.run(req).await;
    }
    // locale 路由公开，未登录时也需要加载语言包 / Locale routes are public
    if path.starts_with("/api/locale/") || path == "/api/locales" {
        return next.run(req).await;
    }
    if !store.has_password() {
        return next.run(req).await;
    }

    let ip = extract_ip(&req);
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match token {
        Some(t) => match store.verify(t, ip) {
            VerifyResult::Ok => next.run(req).await,
            VerifyResult::Expired => {
                (StatusCode::UNAUTHORIZED, "Token expired").into_response()
            }
            VerifyResult::IpMismatch => {
                (StatusCode::UNAUTHORIZED, "IP mismatch").into_response()
            }
            VerifyResult::Invalid => {
                (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
            }
        },
        None => (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
    }
}
