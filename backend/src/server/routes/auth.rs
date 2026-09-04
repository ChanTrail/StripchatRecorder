//! 认证相关路由 handler / Authentication route handlers
//!
//! GET  /api/auth/status          — 查询密码是否已设置、当前是否已登录
//! POST /api/auth/init-password   — 首次设置管理员密码（仅未设置时可用）
//! POST /api/auth/login           — 登录，返回 session token
//! POST /api/auth/logout          — 登出，清除 token
//! POST /api/auth/renew           — 主动续期（将过期时间延长至 now + TOKEN_TTL）
//! POST /api/auth/change-password — 修改密码（需已登录）

use crate::server::{
    auth::{extract_ip, validate_password_strength, VerifyResult},
    error::{ApiError, ApiResult},
    router::ServerState,
};
use axum::{Json, body::Body, extract::State as AxumState, http::Request};
use serde::{Deserialize, Serialize};

// ─── 响应 / Response types ─────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AuthStatusResponse {
    pub password_set: bool,
    pub logged_in: bool,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
}

// ─── 请求体 / Request bodies ───────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PasswordBody {
    pub password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    pub old_password: String,
    pub new_password: String,
}

// ─── Handlers ──────────────────────────────────────────────────────────────

/// GET /api/auth/status
pub async fn auth_status(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<AuthStatusResponse> {
    Ok(Json(AuthStatusResponse {
        password_set: s.app_state.has_admin_password(),
        logged_in: s.token_store.is_logged_in(),
    }))
}

/// POST /api/auth/init-password
/// 首次设置密码（需通过密码强度校验）。密码已存在时拒绝。
pub async fn init_password(
    AxumState(s): AxumState<ServerState>,
    Json(body): Json<PasswordBody>,
) -> ApiResult<serde_json::Value> {
    if s.app_state.has_admin_password() {
        return Err(ApiError("管理员密码已设置".into()));
    }
    let password = body.password.trim().to_string();
    validate_password_strength(&password).map_err(|e| ApiError(e.to_string()))?;
    s.app_state.set_admin_password(&password).map_err(ApiError::from)?;
    s.token_store.mark_password_configured();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/auth/login
/// 验证密码，成功后生成并返回绑定 IP 的 session token。
/// 失败超过阈值后该 IP 被临时封禁。
pub async fn login(
    AxumState(s): AxumState<ServerState>,
    req: Request<Body>,
) -> ApiResult<LoginResponse> {
    let ip = extract_ip(&req);

    // IP 封禁检查 / IP lockout check
    if s.token_store.is_locked(ip) {
        let secs = s.token_store.lockout_remaining_secs(ip);
        return Err(ApiError(format!(
            "登录失败次数过多，IP 已被临时封禁，请 {} 秒后重试 / Too many failures, retry after {}s",
            secs, secs
        )));
    }

    // 提取请求体 / Extract body
    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 16)
        .await
        .map_err(|e| ApiError(e.to_string()))?;
    let body: PasswordBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| ApiError(e.to_string()))?;

    let password = body.password.clone();
    let app_state = std::sync::Arc::clone(&s.app_state);
    let ok = tokio::task::spawn_blocking(move || app_state.verify_admin_password(&password))
        .await
        .unwrap_or(false);

    if !ok {
        let locked = s.token_store.record_fail(ip);
        if locked {
            let secs = s.token_store.lockout_remaining_secs(ip);
            return Err(ApiError(format!(
                "密码错误次数过多，IP 已被封禁 {} 秒 / Too many failures, IP locked for {}s",
                secs, secs
            )));
        }
        return Err(ApiError("密码错误 / Wrong password".into()));
    }

    let token = s.token_store.create_session(ip);
    Ok(Json(LoginResponse { token }))
}

/// POST /api/auth/logout
pub async fn logout(
    AxumState(s): AxumState<ServerState>,
) -> ApiResult<serde_json::Value> {
    s.token_store.clear();
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/auth/renew
/// 主动续期，将 Token 过期时间延长至 now + TOKEN_TTL。
/// 需要在 Authorization 头中携带当前有效 token，且 IP 必须与登录时一致。
pub async fn renew(
    AxumState(s): AxumState<ServerState>,
    req: Request<Body>,
) -> ApiResult<serde_json::Value> {
    let ip = extract_ip(&req);
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError("缺少 Token / Missing token".into()))?;

    if s.token_store.renew(token, ip) {
        Ok(Json(serde_json::json!({ "ok": true })))
    } else {
        Err(ApiError("Token 无效或已过期 / Token invalid or expired".into()))
    }
}

/// POST /api/auth/change-password
/// 修改密码：验证旧密码后设置新密码，并使当前 session token 失效（需重新登录）。
/// Change password: verify old password, set new one, then invalidate current session.
pub async fn change_password(
    AxumState(s): AxumState<ServerState>,
    req: Request<Body>,
) -> ApiResult<serde_json::Value> {
    // 验证 Token（change-password 在 protected_routes 中，中间件已验证过，
    // 但仍需要提取 IP 做二次确认，防止中间人替换 body）
    let ip = extract_ip(&req);
    let token_str = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError("缺少 Token / Missing token".into()))?
        .to_string();

    // 确认 token 与 IP 仍匹配（中间件只做了 token 校验，这里再校验 IP）
    match s.token_store.verify(&token_str, ip) {
        VerifyResult::Ok => {}
        VerifyResult::IpMismatch => {
            return Err(ApiError("IP 与登录时不一致，请重新登录 / IP mismatch, please re-login".into()));
        }
        _ => return Err(ApiError("Token 无效 / Invalid token".into())),
    }

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 16)
        .await
        .map_err(|e| ApiError(e.to_string()))?;
    let body: ChangePasswordBody = serde_json::from_slice(&body_bytes)
        .map_err(|e| ApiError(e.to_string()))?;

    let old_pwd = body.old_password.clone();
    let new_pwd = body.new_password.trim().to_string();

    // 校验新密码强度 / Validate new password strength
    validate_password_strength(&new_pwd).map_err(|e| ApiError(e.to_string()))?;

    // 验证旧密码 / Verify old password
    let app_state = std::sync::Arc::clone(&s.app_state);
    let old_ok =
        tokio::task::spawn_blocking(move || app_state.verify_admin_password(&old_pwd))
            .await
            .unwrap_or(false);
    if !old_ok {
        return Err(ApiError("旧密码错误 / Wrong current password".into()));
    }

    // 设置新密码 / Set new password
    s.app_state
        .set_admin_password(&new_pwd)
        .map_err(ApiError::from)?;

    // 使当前 token 失效，强制重新登录 / Invalidate current token; force re-login
    s.token_store.clear();

    Ok(Json(serde_json::json!({ "ok": true, "relogin_required": true })))
}
