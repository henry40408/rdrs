use axum::{Json, extract::State, http::StatusCode};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::{AuthUser, SESSION_COOKIE_NAME, build_session_cookie};
use crate::models::category;
use crate::models::session;
use crate::models::user::{self, Role};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> AppResult<(StatusCode, Json<RegisterResponse>)> {
    if req.username.is_empty() {
        return Err(AppError::Validation("Username is required".to_string()));
    }
    if req.password.len() < 6 {
        return Err(AppError::Validation(
            "Password must be at least 6 characters".to_string(),
        ));
    }

    let can_register = state.config.can_register(0); // We check count below
    let config = state.config.clone();
    let password_hash = hash_password(&req.password)?;

    let user_count = user::count(&state.db).await?;

    if !config.can_register(user_count) {
        return Err(AppError::RegistrationNotAllowed);
    }

    let role = if user_count == 0 {
        Role::Admin
    } else {
        Role::User
    };

    let user = user::create_user(&state.db, &req.username, &password_hash, role).await?;

    // Seed a default category so the account can add its first feed
    // without first creating a category. Matches the "Uncategorized"
    // convention used by OPML import and the GReader subscription API.
    category::create_category(&state.db, user.id, "Uncategorized").await?;

    // Suppress unused variable warning
    let _ = can_register;

    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> AppResult<(CookieJar, Json<LoginResponse>)> {
    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }
    let user = user::find_by_username(&state.db, &req.username)
        .await?
        .ok_or(AppError::InvalidCredentials)?;

    if !verify_password(&req.password, &user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    if user.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    let new_session = session::create_session(&state.db, user.id).await?;

    let cookie = build_session_cookie(
        &new_session.session_token,
        &state.config.secret,
        state.config.cookie_secure,
    );
    // Refresh the readable CSRF cookie to match the new session token: the
    // token the visitor was carrying was derived from their pre-login
    // (anonymous) session and no longer verifies.
    let csrf = crate::middleware::build_csrf_cookie(
        &new_session.session_token,
        &state.config.secret,
        state.config.cookie_secure,
    );

    Ok((
        jar.add(cookie).add(csrf),
        Json(LoginResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        }),
    ))
}

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub redirect_to: String,
    /// Whether the trusted forward-auth proxy identity header is present on
    /// this request (not whether the session itself was created via forward
    /// auth). The SPA uses it to explain that a local logout cannot end a
    /// proxy-managed session when no `auth_proxy_logout_url` is configured.
    pub via_forward_auth: bool,
    /// Whether an `auth_proxy_logout_url` is configured. When true, `redirect_to`
    /// is that URL (absolute `IdP` URL or a same-host path) and the client should
    /// navigate to it; when false, `redirect_to` is the `/login` fallback and no
    /// external logout endpoint exists to hand off to.
    pub logout_url_configured: bool,
}

/// Clears the local session and reports where the client should go next.
///
/// `redirect_to` is the configured `auth_proxy_logout_url`, or `/login` if
/// none is set. `via_forward_auth` reports whether the trusted proxy identity header is
/// present on this request. `logout_url_configured` explicitly indicates whether an
/// `auth_proxy_logout_url` is configured, so the client can decide whether to
/// navigate to `redirect_to`.
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<(CookieJar, Json<LogoutResponse>)> {
    let token = auth_user.session.session_token.clone();
    session::delete_session(&state.db, &token).await?;

    // Removal must match the Path=/ the cookie was set with, or the browser
    // keeps the (now-invalid) session_token cookie. Mirrors flash.rs. The
    // readable CSRF cookie is cleared alongside it; the next page load mints a
    // fresh anonymous pair, so a stale token cannot linger and 403 the re-login.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
    let csrf_removal = Cookie::build((crate::middleware::CSRF_COOKIE_NAME, ""))
        .path("/")
        .build();

    let logout_url_configured = state.config.auth_proxy_logout_url.is_some();
    let redirect_to = state
        .config
        .auth_proxy_logout_url
        .clone()
        .unwrap_or_else(|| "/login".to_string());

    Ok((
        jar.remove(removal).remove(csrf_removal),
        Json(LogoutResponse {
            redirect_to,
            via_forward_auth: auth_user.via_forward_auth,
            logout_url_configured,
        }),
    ))
}
