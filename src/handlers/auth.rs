use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Extension, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use time::Duration;

use crate::AppState;
use crate::auth::{
    hash_password, validate_password_strength, verify_dummy_password, verify_password,
};
use crate::error::{AppError, AppResult};
use crate::middleware::{
    AuthUser, Bucket, CSRF_COOKIE_NAME_HOST, SESSION_COOKIE_NAME, SESSION_COOKIE_NAME_HOST,
    build_session_cookie,
    flash::{FlashRedirect, SetFlash},
};
use crate::models::category;
use crate::models::session;
use crate::models::user::{self, Role};
use crate::services::audit;
use crate::utils::http::request_user_agent;

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct SetupResponse {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

/// `POST /api/setup` — create the instance's first (admin) account.
///
/// The only anonymous account-creating endpoint; refuses once any account
/// exists (`Config::can_setup`). Later accounts go through `handlers::invite`.
pub async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<SetupRequest>,
) -> AppResult<(StatusCode, SetFlash, Json<SetupResponse>)> {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let user = perform_setup(&state, &headers, peer, &req, "POST /api/setup").await?;
    // Flash cookies are signed, so only the server can mint one.
    Ok((
        StatusCode::CREATED,
        SetFlash::success("Account created. Please sign in."),
        Json(user),
    ))
}

/// Shared by [`setup`] and [`setup_form`]; `endpoint` only labels logs.
async fn perform_setup(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    req: &SetupRequest,
    endpoint: &'static str,
) -> AppResult<SetupResponse> {
    if req.username.is_empty() {
        return Err(AppError::Validation("Username is required".to_string()));
    }

    // Reserve before any DB query or hashing. Never released: unlike login,
    // there is no "correct credential" outcome to refund.
    let ip = state.config.client_ip(peer, headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::AccountSetup, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::AccountSetup, endpoint, "credential attempt rate limited");
        audit::login_rate_limited(endpoint, "setup", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    // Before the expensive work, so a closed endpoint costs one count.
    let config = state.config.clone();
    let user_count = user::count(&state.db).await?;
    if !config.can_setup(user_count) {
        return Err(AppError::RegistrationNotAllowed);
    }

    // Behind the limiter: zxcvbn's worst case (~79ms) rivals Argon2.
    validate_password_strength(&req.password, &[&req.username])?;

    let password_hash = hash_password(&req.password)?;

    let user = user::create_user(&state.db, &req.username, &password_hash, Role::Admin).await?;

    // Same default as OPML import and the GReader subscription API.
    category::create_category(&state.db, user.id, "Uncategorized").await?;

    audit::account_created(
        user.id,
        user.id,
        user.username.chars().count(),
        user.role.as_str(),
    );

    Ok(SetupResponse {
        id: user.id,
        username: user.username,
        role: user.role,
    })
}

/// No-JS fallback for the first-run form; `setup.js` uses the JSON endpoint.
#[derive(Debug, Deserialize)]
pub struct SetupForm {
    pub username: String,
    pub password: String,
    #[serde(rename = "confirm-password")]
    pub confirm_password: String,
}

pub async fn setup_form(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    axum::Form(form): axum::Form<SetupForm>,
) -> Response {
    let render_error = |message: String| {
        crate::handlers::pages::SetupTemplate {
            error: Some(message),
            flash_messages: Vec::new(),
            git_version: crate::GIT_VERSION,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
            csrf_token: crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
        }
        .into_response()
    };

    if form.password != form.confirm_password {
        return render_error("Passwords do not match".to_string());
    }

    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let req = SetupRequest {
        username: form.username,
        password: form.password,
    };
    match perform_setup(&state, &headers, peer, &req, "POST /setup").await {
        Ok(_) => {
            FlashRedirect::success("/login", "Account created. Please sign in.").into_response()
        }
        Err(AppError::Validation(msg)) => render_error(msg),
        Err(AppError::TooManyRequests { retry_after_secs }) => render_error(format!(
            "Too many attempts. Please try again in {retry_after_secs} seconds."
        )),
        Err(AppError::UsernameExists) => render_error("Username already exists".to_string()),
        Err(_) => render_error("Could not create the account".to_string()),
    }
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
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let (jar, resp) = perform_login(&state, jar, &headers, peer, &req, "POST /api/session").await?;
    Ok((jar, Json(resp)))
}

/// Shared by [`login`] and [`login_form`]; `endpoint` only labels logs.
async fn perform_login(
    state: &AppState,
    jar: CookieJar,
    headers: &HeaderMap,
    peer: Option<std::net::IpAddr>,
    req: &LoginRequest,
    endpoint: &'static str,
) -> AppResult<(CookieJar, LoginResponse)> {
    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }

    // Reserve before lookup/verify, so guesses can't force Argon2 work.
    let ip = state.config.client_ip(peer, headers);
    let user_agent = request_user_agent(headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::Login, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::Login, endpoint, "credential attempt rate limited");
        audit::login_rate_limited(endpoint, "login", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    // Per-account budget too: the per-IP one is useless against rotating IPs.
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire_account(Bucket::Login, &req.username)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::Login, subject = "account", endpoint, "credential attempt rate limited");
        audit::login_rate_limited(endpoint, "login_account", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    let Some(user) = user::find_by_username(&state.db, &req.username).await? else {
        // Equalize timing with "wrong password" to avoid user enumeration.
        verify_dummy_password(&req.password);
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

    // Correct password: refund both reservations, before the disabled check
    // so it can't leak via a rate-limit side channel.
    state.login_rate_limiter.release(Bucket::Login, ip);
    state
        .login_rate_limiter
        .release_account(Bucket::Login, &req.username);

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
    // The pre-login CSRF token no longer verifies against the new session.
    let csrf = crate::middleware::build_csrf_cookie(
        &new_session.session_token,
        &state.config.secret,
        state.config.cookie_secure,
    );

    Ok((
        jar.add(cookie).add(csrf),
        LoginResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        },
    ))
}

/// `POST /login` body: the credentials plus the page to land on afterwards.
#[derive(Debug, Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub next: Option<String>,
}

/// `POST /login` — no-JS form variant of [`login`]; failures re-render the
/// form (200) with the same generic message as the JSON endpoint.
pub async fn login_form(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    axum::Form(form): axum::Form<LoginForm>,
) -> Response {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    // Read before `jar` is moved; a failed attempt re-renders the form.
    let csrf_token = crate::middleware::csrf_token_from_jar(&jar, &state.config.secret);
    let next = crate::handlers::pages::login_next(form.next.as_deref());
    let req = LoginRequest {
        username: form.username,
        password: form.password,
    };
    match perform_login(&state, jar, &headers, peer, &req, "POST /login").await {
        Ok((jar, _)) => {
            let location = next.as_deref().unwrap_or("/");
            (jar, Redirect::to(location)).into_response()
        }
        Err(e) => {
            let setup_available = user::count(&state.db)
                .await
                .is_ok_and(|count| state.config.can_setup(count));
            crate::handlers::pages::LoginTemplate {
                setup_available,
                flash_messages: Vec::new(),
                git_version: crate::GIT_VERSION,
                local_auth_enabled: !state.config.disable_local_auth,
                csrf_token,
                error: Some(login_error_message(&e)),
                next,
            }
            .into_response()
        }
    }
}

/// Deliberately uninformative; must match `AppError`'s JSON wording.
fn login_error_message(err: &AppError) -> String {
    match err {
        AppError::TooManyRequests { retry_after_secs } => {
            format!("Too many attempts. Please try again in {retry_after_secs} seconds.")
        }
        AppError::Forbidden => "Password sign-in is disabled on this instance.".to_string(),
        AppError::UserDisabled => "This account is disabled.".to_string(),
        _ => "Invalid credentials".to_string(),
    }
}

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub redirect_to: String,
    /// Whether the trusted forward-auth identity header is on this request.
    pub via_forward_auth: bool,
    /// Whether `redirect_to` is the `auth_proxy_logout_url` (else `/login`).
    pub logout_url_configured: bool,
}

/// `Clear-Site-Data` on logout. `"storage"` clears the sidebar mirror in
/// `sessionStorage`. Omits `"cookies"` (would race the flash cookie) and
/// `"executionContexts"` (would force a reload fighting the redirect).
const LOGOUT_CLEAR_SITE_DATA: &str = "\"cache\", \"storage\"";

#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    /// Absent for a forward-auth session, which has no rdrs password to give.
    #[serde(default)]
    pub password: String,
}

/// Re-prove credentials, restarting the
/// [`crate::middleware::RecentlyAuthenticated`] window. Only updates
/// `last_authenticated_at`.
///
/// Shares the `PasswordChange` rate-limit budget so a hijacked session can
/// neither brute-force the password here nor sidestep that limit.
pub async fn reauthenticate(
    State(state): State<AppState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<ReauthRequest>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;
    let token = auth_user.session.session_token.clone();

    // The proxy re-asserts forward-auth identity per request; nothing to check.
    if auth_user.via_forward_auth {
        session::mark_authenticated(&state.db, auth_user.session.id).await?;
        audit::session_reauthenticated(&state.config.secret, &token, user_id, "forward_auth");
        return Ok(StatusCode::NO_CONTENT);
    }

    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }

    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::PasswordChange, ip)
        .retry_after_secs()
    {
        audit::login_rate_limited("POST /api/session/reauth", "reauth", &ip.to_string());
        return Err(AppError::TooManyRequests { retry_after_secs });
    }

    if !verify_password(&req.password, &auth_user.user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }
    // Correct password: refund the reservation.
    state.login_rate_limiter.release(Bucket::PasswordChange, ip);

    session::mark_authenticated(&state.db, auth_user.session.id).await?;
    audit::session_reauthenticated(&state.config.secret, &token, user_id, "password");

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /logout` — no-JS form variant of [`logout`] (forms cannot send DELETE).
pub async fn logout_form(
    State(state): State<AppState>,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    request_headers: HeaderMap,
    jar: CookieJar,
    auth_user: Result<AuthUser, AppError>,
) -> AppResult<Response> {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let (headers, jar, body) =
        destroy_session(&state, jar, auth_user, peer, &request_headers).await?;

    let flash = logged_out_flash(&body);

    Ok((headers, jar, flash, Redirect::to(&body.redirect_to)).into_response())
}

/// Clears the local session and reports where the client should go next.
pub async fn logout(
    State(state): State<AppState>,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    request_headers: HeaderMap,
    jar: CookieJar,
    auth_user: Result<AuthUser, AppError>,
) -> AppResult<(
    [(HeaderName, HeaderValue); 1],
    CookieJar,
    SetFlash,
    Json<LogoutResponse>,
)> {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let (headers, jar, body) =
        destroy_session(&state, jar, auth_user, peer, &request_headers).await?;
    // Flash cookies are signed, so only the server can mint one.
    let flash = logged_out_flash(&body);
    Ok((headers, jar, flash, Json(body)))
}

/// Under forward-auth with no logout URL, the proxy signs the reader straight
/// back in, so warn instead of claiming success.
fn logged_out_flash(body: &LogoutResponse) -> SetFlash {
    if body.via_forward_auth && !body.logout_url_configured {
        SetFlash::warning(
            "You are signed in via your reverse proxy. To end your session, log out at your proxy or SSO provider.",
        )
    } else {
        SetFlash::info("You have been logged out.")
    }
}

/// Destroy the session and clear every cookie it could be carried under.
/// An already-expired session is not an error: the cookies and storage still
/// need clearing.
async fn destroy_session(
    state: &AppState,
    jar: CookieJar,
    auth_user: Result<AuthUser, AppError>,
    peer: Option<std::net::IpAddr>,
    request_headers: &HeaderMap,
) -> AppResult<([(HeaderName, HeaderValue); 1], CookieJar, LogoutResponse)> {
    let via_forward_auth = match auth_user {
        Ok(auth_user) => {
            let token = auth_user.session.session_token.clone();
            session::delete_session(&state.db, &token).await?;
            audit::session_destroyed(&state.config.secret, &token, auth_user.user.id, "logout");
            auth_user.via_forward_auth
        }
        // Nothing to destroy; the proxy header still decides the banner.
        Err(AppError::Unauthorized) => crate::middleware::forward_auth::forward_auth_identity(
            &state.config,
            peer,
            request_headers,
        )
        .is_some(),
        Err(e) => return Err(e),
    };

    // Path=/ must match the original cookie. Both the unprefixed and __Host-
    // names are cleared, since either may hold a leftover session.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
    let csrf_removal = Cookie::build((crate::middleware::CSRF_COOKIE_NAME, ""))
        .path("/")
        .build();

    // __Host- removals always need Secure or the browser ignores them, and are
    // `add()`-ed because `remove()` only fires if the request carried that name.
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
        LogoutResponse {
            redirect_to,
            via_forward_auth,
            logout_url_configured,
        },
    ))
}
