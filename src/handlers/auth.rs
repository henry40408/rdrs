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

/// `POST /api/setup` — create the instance's first account.
///
/// The only anonymous account-creating endpoint rdrs has, and it exists solely
/// because a fresh install has no admin to create one. It refuses outright as
/// soon as any account exists (`Config::can_setup`), which is what keeps it from
/// being the self-service registration this replaced: with zero accounts there
/// is no username to enumerate. Every later account is created by an admin and
/// activated through `handlers::invite`.
///
/// The account is an admin, because someone has to be.
pub async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<SetupRequest>,
) -> AppResult<(StatusCode, Json<SetupResponse>)> {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let user = perform_setup(&state, &headers, peer, &req, "POST /api/setup").await?;
    Ok((StatusCode::CREATED, Json(user)))
}

/// The first-account creation itself, shared by the JSON endpoint above and the
/// native form POST in [`setup_form`]. `endpoint` only labels the rate-limit
/// warnings and audit records.
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

    // Reserve an attempt before any DB query, strength estimation or password
    // hashing. Never released: scripted account creation is exactly the abuse
    // this limiter exists to slow down, so unlike login there is no "correct
    // credential" outcome that should hand the budget back.
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

    // Checked before the expensive work below, so a closed setup endpoint
    // costs a single indexed count rather than an estimate plus a hash.
    let config = state.config.clone();
    let user_count = user::count(&state.db).await?;
    if !config.can_setup(user_count) {
        return Err(AppError::RegistrationNotAllowed);
    }

    // Behind the limiter, not in front of it: zxcvbn costs ~86µs on a typical
    // password but ~79ms on a 128-character worst case (measured in release),
    // which is Argon2 territory. Validating first would let anyone choose how
    // much CPU each rejected attempt costs. The username is handed to the
    // estimator so a password built out of it is scored for what it is.
    validate_password_strength(&req.password, &[&req.username])?;

    let password_hash = hash_password(&req.password)?;

    let user = user::create_user(&state.db, &req.username, &password_hash, Role::Admin).await?;

    // Seed a default category so the account can add its first feed
    // without first creating a category. Matches the "Uncategorized"
    // convention used by OPML import and the GReader subscription API.
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

/// The first-run form, posted natively. Mirrors [`login_form`]: `setup.js`
/// still intercepts the submit and uses the JSON endpoint, and this is what a
/// browser without JavaScript falls back to. The `confirm_password` match is
/// checked here because it is a property of the *form*, not of the API.
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

/// The password sign-in itself, shared by the JSON endpoint above and the
/// native form POST in [`login_form`]. `endpoint` only labels the rate-limit
/// warnings and audit records, so each caller stays distinguishable in the log.
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

    // Reserve an attempt before the username lookup or password verification
    // — enforcing the limit any later would still let an attacker choose how
    // much Argon2 work the server does per guess.
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

    // Second dimension: the account being aimed at, regardless of where the
    // attempt came from. The per-IP budget above is worthless against a spray
    // that rotates addresses, since each one arrives with a full budget. Charged
    // before the lookup, for the same reason as the IP check.
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
        // Spend a verification against a hash nothing matches before
        // answering. Returning here directly would make "no such account"
        // measurably faster than "wrong password" — the generic error message
        // below says nothing, but the clock would.
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

    // The password was correct: hand both reservations back so a legitimate
    // user is never locked out by their own successful logins. Done before
    // the disabled-account check below so a correct password never leaks
    // information via a rate-limit side channel either.
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
        LoginResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        },
    ))
}

/// `POST /login` — the same sign-in as [`login`], driven by a native form submit
/// rather than `fetch`.
///
/// This is what makes `/login` work with JavaScript disabled: the form has a real
/// `action`/`method`, so a browser that never ran `login.js` posts here instead
/// of issuing a `GET` that would put the password in the address bar. `login.js`
/// still calls `preventDefault()` and takes the JSON route.
///
/// Failures re-render the login page with the message inline (200, not the error
/// status) — the visitor needs the form back, and the generic wording is
/// unchanged, so this reveals nothing the JSON endpoint doesn't.
pub async fn login_form(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    axum::Form(req): axum::Form<LoginRequest>,
) -> Response {
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    // Read before `jar` is handed over: a failed attempt re-renders the form,
    // and the session it belongs to is the same anonymous one either way.
    let csrf_token = crate::middleware::csrf_token_from_jar(&jar, &state.config.secret);
    match perform_login(&state, jar, &headers, peer, &req, "POST /login").await {
        Ok((jar, _)) => (jar, Redirect::to("/")).into_response(),
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
            }
            .into_response()
        }
    }
}

/// The one-line, deliberately uninformative rendering of a failed sign-in.
/// Mirrors what `AppError`'s JSON body would have said, so the two paths cannot
/// drift into telling a visitor different things about the same failure.
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
/// cookies, and `Clear-Site-Data` processing is asynchronous relative to JS, so
/// including it would race the `flash` cookie `rdrs-flash.js` writes after this
/// response lands and swallow the logout notice. `"storage"` is the real win —
/// it clears the sidebar mirror in `sessionStorage`, which otherwise leaks the
/// previous user's feed titles and unread counts on a shared machine.
/// `"executionContexts"` is omitted too: it would force a reload that fights the
/// client's own redirect.
const LOGOUT_CLEAR_SITE_DATA: &str = "\"cache\", \"storage\"";

#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    /// Absent for a forward-auth session, which has no rdrs password to give.
    #[serde(default)]
    pub password: String,
}

/// Re-prove the current session's credentials, restarting the window
/// [`crate::middleware::RecentlyAuthenticated`] enforces.
///
/// Creates nothing and rotates nothing: the session is already valid, and only
/// `last_authenticated_at` changes. That keeps this endpoint uninteresting to an
/// attacker who already holds the session — it grants no new access, only
/// re-opens a window they still have to spend on an audited operation.
///
/// Shares the `PasswordChange` rate-limit budget rather than taking its own, for
/// the reason that bucket exists: an unthrottled Argon2 verify lets a hijacked
/// session brute-force the account's real password. Sharing also stops this
/// endpoint from being used to sidestep the limit on "change password".
pub async fn reauthenticate(
    State(state): State<AppState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<ReauthRequest>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;
    let token = auth_user.session.session_token.clone();

    // A forward-auth session's identity is re-asserted by the proxy on every
    // request, so there is nothing to re-check — and the account may hold no
    // usable password at all. It never sees a `ReauthenticationRequired`; this
    // arm exists so a client that calls here anyway gets a coherent answer
    // rather than a password check it can never pass.
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
    // Correct password: hand the reservation back, so re-authenticating
    // legitimately never eats into the budget — same rationale as login and
    // change-password.
    state.login_rate_limiter.release(Bucket::PasswordChange, ip);

    session::mark_authenticated(&state.db, auth_user.session.id).await?;
    audit::session_reauthenticated(&state.config.secret, &token, user_id, "password");

    Ok(StatusCode::NO_CONTENT)
}

/// Clears the local session and reports where the client should go next.
///
/// `redirect_to` is the configured `auth_proxy_logout_url`, or `/login`.
/// `via_forward_auth` reports whether the trusted proxy identity header is
/// present, and `logout_url_configured` lets the client decide whether to
/// navigate to `redirect_to` at all.
/// `POST /logout` — the same thing as a native form submit.
///
/// Sign-out used to be reachable only through a `fetch` DELETE from a
/// `href="#"` link, and a form cannot send DELETE — so with scripting off there
/// was no way to end a session at all, which on a shared machine is not a
/// cosmetic gap.
pub async fn logout_form(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<Response> {
    let (headers, jar, body) = destroy_session(&state, jar, auth_user).await?;

    // Forward-auth with no logout URL configured: the proxy re-injects the
    // identity on the next request, so bouncing to /login would silently sign
    // the reader back in. Say so rather than pretending, matching what the
    // scripted path flashes.
    let flash = if body.via_forward_auth && !body.logout_url_configured {
        SetFlash::warning(
            "You are signed in via your reverse proxy. To end your session, log out at your proxy or SSO provider.",
        )
    } else {
        SetFlash::info("You have been logged out.")
    };

    Ok((headers, jar, flash, Redirect::to(&body.redirect_to)).into_response())
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<(
    [(HeaderName, HeaderValue); 1],
    CookieJar,
    Json<LogoutResponse>,
)> {
    let (headers, jar, body) = destroy_session(&state, jar, auth_user).await?;
    Ok((headers, jar, Json(body)))
}

/// Shared core: destroy the session, clear every cookie it could be carried
/// under, and work out where the caller should land. Both the JSON and the form
/// endpoint go through this so a change to cookie removal cannot apply to only
/// one of them.
async fn destroy_session(
    state: &AppState,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<([(HeaderName, HeaderValue); 1], CookieJar, LogoutResponse)> {
    let token = auth_user.session.session_token.clone();
    session::delete_session(&state.db, &token).await?;
    audit::session_destroyed(&state.config.secret, &token, auth_user.user.id, "logout");

    // Removal must match the Path=/ the cookie was set with, or the browser keeps
    // the now-invalid session_token cookie. The readable CSRF cookie is cleared
    // alongside it; the next page load mints a fresh anonymous pair, so a stale
    // token cannot linger and 403 the re-login.
    //
    // Four removal cookies, not two: the session may be carried under either the
    // unprefixed or the __Host- name, and a leftover under whichever name is not
    // in active use — from before an upgrade, or before an operator flipped
    // `RDRS_COOKIE_SECURE` — must not survive logout.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();
    let csrf_removal = Cookie::build((crate::middleware::CSRF_COOKIE_NAME, ""))
        .path("/")
        .build();

    // The __Host- removals carry Secure and Path=/ unconditionally, regardless of
    // the current setting, because a browser silently discards a __Host- cookie
    // that lacks Secure and the removal would be a no-op. They are `jar.add()`-ed
    // rather than `jar.remove()`-d: `remove()` only emits a removal when this
    // *request's* Cookie header already carried that exact name, which would skip
    // the __Host- pair whenever the request authenticated via the unprefixed one.
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
            via_forward_auth: auth_user.via_forward_auth,
            logout_url_configured,
        },
    ))
}
