use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Extension, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use time::Duration;

use crate::AppState;
use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::{
    AuthUser, Bucket, CSRF_COOKIE_NAME_HOST, SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST,
    build_session_cookie,
};
use crate::models::category;
use crate::models::session;
use crate::models::user::{self, Role};
use crate::services::audit;
use crate::utils::http::request_user_agent;

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
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
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

    // Reserve an attempt before any DB query or password hashing. Never
    // released: a successful registration is exactly the abuse this limiter
    // exists to slow down (an attacker scripting account creation), so
    // unlike login there is no "correct credential" outcome that should hand
    // the budget back.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if !state.login_rate_limiter.try_acquire(Bucket::Register, ip) {
        tracing::warn!(%ip, bucket = ?Bucket::Register, endpoint = "POST /api/register", "credential attempt rate limited");
        audit::login_rate_limited("POST /api/register", "register", &ip.to_string());
        return Err(AppError::TooManyRequests);
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
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<LoginRequest>,
) -> AppResult<(CookieJar, Json<LoginResponse>)> {
    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }

    // Reserve an attempt before the username lookup or password verification
    // — enforcing the limit any later would still let an attacker choose how
    // much Argon2 work the server does per guess.
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    let user_agent = request_user_agent(&headers);
    if !state.login_rate_limiter.try_acquire(Bucket::Login, ip) {
        tracing::warn!(%ip, bucket = ?Bucket::Login, endpoint = "POST /api/session", "credential attempt rate limited");
        audit::login_rate_limited("POST /api/session", "login", &ip.to_string());
        return Err(AppError::TooManyRequests);
    }

    let Some(user) = user::find_by_username(&state.db, &req.username).await? else {
        audit::login_failed(
            req.username.len(),
            "unknown_user",
            &ip.to_string(),
            &user_agent,
        );
        return Err(AppError::InvalidCredentials);
    };

    if !verify_password(&req.password, &user.password_hash) {
        audit::login_failed(
            req.username.len(),
            "bad_password",
            &ip.to_string(),
            &user_agent,
        );
        return Err(AppError::InvalidCredentials);
    }

    // The password was correct: hand the reservation back so a legitimate
    // user is never locked out by their own successful logins. Done before
    // the disabled-account check below so a correct password never leaks
    // information via a rate-limit side channel either.
    state.login_rate_limiter.release(Bucket::Login, ip);

    if user.is_disabled() {
        audit::login_failed(req.username.len(), "disabled", &ip.to_string(), &user_agent);
        return Err(AppError::UserDisabled);
    }

    let ip = ip.to_string();
    let new_session = session::create_session(&state.db, user.id, &user_agent, &ip).await?;
    audit::session_created(
        &state.config.secret,
        &new_session.session_token,
        user.id,
        "password",
        &ip,
        &user_agent,
    );

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

/// Ask the browser to discard this origin's residue on logout.
///
/// Deliberately **omits `"cookies"`**: logout already emits explicit removal
/// cookies, and `Clear-Site-Data` processing is asynchronous relative to JS —
/// including `"cookies"` would race the `flash` cookie that
/// `rdrs-flash.js:42` writes after this response lands, swallowing the
/// "You have been logged out." notice. `"cache"` and `"storage"` have no such
/// conflict, and `"storage"` is the real win here: it clears the sidebar
/// mirror `rdrs-sidebar.js:61` keeps in `sessionStorage`, which otherwise
/// leaks the previous user's feed titles and unread counts on a shared
/// machine. `"executionContexts"` is omitted too — it would force a reload
/// that fights the client's own redirect.
const LOGOUT_CLEAR_SITE_DATA: &str = "\"cache\", \"storage\"";

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
) -> AppResult<(
    [(HeaderName, HeaderValue); 1],
    CookieJar,
    Json<LogoutResponse>,
)> {
    let token = auth_user.session.session_token.clone();
    session::delete_session(&state.db, &token).await?;
    audit::session_destroyed(&state.config.secret, &token, auth_user.user.id, "logout");

    // Removal must match the Path=/ the cookie was set with, or the browser
    // keeps the (now-invalid) session_token cookie. Mirrors flash.rs. The
    // readable CSRF cookie is cleared alongside it; the next page load mints a
    // fresh anonymous pair, so a stale token cannot linger and 403 the re-login.
    //
    // Four removal cookies, not two: the session may currently be carried
    // under either the unprefixed or the __Host--prefixed name (see
    // `middleware::auth::session_token_from_jar`), and a leftover cookie
    // under whichever name is *not* in active use this deployment — e.g. from
    // before an upgrade, or from before an operator flipped
    // `RDRS_COOKIE_SECURE` — must not survive logout.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
    let csrf_removal = Cookie::build((crate::middleware::CSRF_COOKIE_NAME, ""))
        .path("/")
        .build();

    // The __Host- removal cookies carry Secure and Path=/ unconditionally —
    // regardless of the current `cookie_secure` setting — because a browser
    // silently discards a __Host- cookie that lacks Secure. A non-Secure
    // removal would therefore be a no-op, and the stale __Host- cookie would
    // survive logout. They are `jar.add()`-ed rather than `jar.remove()`-d:
    // `remove()` only emits a removal Set-Cookie when this *request's* Cookie
    // header already carried that exact name, which would silently skip the
    // __Host- pair whenever the current request happened to authenticate via
    // the unprefixed cookie (or vice versa).
    let host_removal = Cookie::build((SESSION_COOKIE_NAME_HOST, ""))
        .path("/")
        .secure(true)
        .max_age(Duration::ZERO)
        .build();
    let host_csrf_removal = Cookie::build((CSRF_COOKIE_NAME_HOST, ""))
        .path("/")
        .secure(true)
        .max_age(Duration::ZERO)
        .build();

    let logout_url_configured = state.config.auth_proxy_logout_url.is_some();
    let redirect_to = state
        .config
        .auth_proxy_logout_url
        .clone()
        .unwrap_or_else(|| "/login".to_string());

    Ok((
        [(
            HeaderName::from_static("clear-site-data"),
            HeaderValue::from_static(LOGOUT_CLEAR_SITE_DATA),
        )],
        jar.remove(removal)
            .remove(csrf_removal)
            .add(host_removal)
            .add(host_csrf_removal),
        Json(LogoutResponse {
            redirect_to,
            via_forward_auth: auth_user.via_forward_auth,
            logout_url_configured,
        }),
    ))
}
