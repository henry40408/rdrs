use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::flash::FlashRedirect;
use crate::middleware::{AdminUser, Bucket, build_csrf_cookie, build_session_cookie};
use crate::models::api_token;
use crate::models::session;
use crate::models::user::{self, Role};
use crate::services::audit;
use crate::utils::http::request_user_agent;

pub async fn stop_masquerade(
    State(state): State<AppState>,
    admin: AdminUser,
) -> AppResult<impl IntoResponse> {
    if !admin.session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    let session_token = admin.session.session_token.clone();
    // While masquerading, `admin.user` is the impersonated target and
    // `original_user_id` is the real admin — the one both acting here and
    // being restored to the session.
    let admin_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let new_token = session::stop_masquerade(&state.db, &session_token).await?;
    audit::masquerade_stopped(
        &state.config.secret,
        &session_token,
        &new_token,
        admin_user_id,
        admin_user_id,
    );

    Ok((rotated_cookies(&state, &new_token), StatusCode::OK))
}

/// The pair of cookies a session-token rotation has to reissue, as a jar the
/// handler can return alongside its own response.
///
/// Both are rebuilt from `new_token`: the session cookie carries the token
/// itself (signed), and the CSRF cookie carries a value *derived* from it
/// (`secret::derive_csrf`), so reissuing only the first would leave the client
/// holding a CSRF token that no longer matches the session and every
/// subsequent state-changing request would fail `csrf_guard`.
///
/// `slide_session_cookie` leaves a response alone once it carries a
/// `Set-Cookie` for these purposes under either the plain or `__Host-` name,
/// so the cookies added here reach the browser unmodified.
fn rotated_cookies(state: &AppState, new_token: &str) -> CookieJar {
    let secret = &state.config.secret;
    let secure = state.config.cookie_secure;
    CookieJar::new()
        .add(build_session_cookie(new_token, secret, secure))
        .add(build_csrf_cookie(new_token, secret, secure))
}

// ============================================================================
// Form-action POST endpoints for the SSR /admin page (PR-5 T1).
// Each accepts application/x-www-form-urlencoded bodies and returns a
// FlashRedirect response (303 See Other + flash cookie + Location).
// Existing JSON endpoints above remain until PR-5 T2 removes them.
// ============================================================================

/// Refuse an account-changing action unless the session proved its password
/// recently, returning the redirect to answer with when it did not.
///
/// OWASP's Authentication Cheat Sheet asks for current credentials before
/// sensitive account changes, precisely so that a stolen session or a CSRF
/// that slips a guard cannot quietly hand out admin, disable an account, or
/// delete one. rdrs already had the mechanism —
/// `middleware::auth::RecentlyAuthenticated` and its `REAUTH_WINDOW_MINUTES`
/// window — but only passkey enrolment used it; the admin panel, which is
/// strictly more powerful, did not.
///
/// This is a plain function rather than an extractor because the SSR admin
/// page has no JavaScript to catch a 403 and re-prompt (the way `passkey.js`
/// does): the answer has to be a redirect the browser can simply follow, and
/// `/admin` renders its own confirmation form when the window has lapsed.
///
/// Two deliberate exemptions:
/// - **Forward-auth sessions**, for the reason given on `AdminUser`'s
///   `via_forward_auth`: their password is not rdrs's to check.
/// - **`stop_masquerade`**, which never calls this. While masquerading, the
///   password that would be asked for belongs to the *impersonated* user, so
///   the check could not be satisfied and the admin would be stranded inside
///   the masquerade. Ending one is also a de-escalation, not an escalation.
fn require_recent_authentication(admin: &AdminUser) -> Option<FlashRedirect> {
    if admin.via_forward_auth || admin.session.authenticated_recently(chrono::Utc::now()) {
        return None;
    }

    Some(FlashRedirect::error(
        "/admin",
        "Confirm your password before changing accounts, then try again.",
    ))
}

#[derive(Debug, Deserialize)]
pub struct AdminReauthForm {
    pub password: String,
}

/// `POST /admin/reauth` — re-open the confirmation window from the admin page.
///
/// The JSON `POST /api/session/reauth` does the same job for `passkey.js`;
/// this is its form-encoded twin, so the admin panel keeps working with
/// JavaScript switched off. Like that endpoint it creates nothing and rotates
/// nothing — the only thing that changes is `last_authenticated_at` — and it
/// draws on the same `PasswordChange` budget, so it cannot be used to
/// brute-force a password that the change-password form throttles.
pub async fn reauth_form(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Form(req): Form<AdminReauthForm>,
) -> impl IntoResponse {
    if admin.via_forward_auth {
        // Nothing to re-check; the proxy re-asserts this identity on every
        // request. Answering "confirmed" keeps a stray submission coherent
        // rather than demanding a password the account may not have.
        return FlashRedirect::success("/admin", "Confirmed.");
    }

    if state.config.disable_local_auth {
        return FlashRedirect::error("/admin", "Password confirmation is disabled.");
    }

    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::PasswordChange, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::PasswordChange, endpoint = "POST /admin/reauth", "credential attempt rate limited");
        audit::login_rate_limited("POST /admin/reauth", "reauth", &ip.to_string());
        return FlashRedirect::error(
            "/admin",
            format!("Too many attempts. Please try again in {retry_after_secs} seconds."),
        );
    }

    // While masquerading, `admin.user` is the impersonated account — the wrong
    // password to ask for. The real admin is `original_user_id`, and their
    // hash is what has to verify.
    let actor_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let Ok(Some(actor)) = user::find_by_id(&state.db, actor_id).await else {
        return FlashRedirect::error("/admin", "Password confirmation failed.");
    };

    if !crate::auth::verify_password(&req.password, &actor.password_hash) {
        return FlashRedirect::error("/admin", "Incorrect password.");
    }
    // Correct password: hand the reservation back, matching every other
    // credential path — confirming legitimately must not eat into the budget.
    state.login_rate_limiter.release(Bucket::PasswordChange, ip);

    if session::mark_authenticated(&state.db, admin.session.id)
        .await
        .is_err()
    {
        return FlashRedirect::error("/admin", "Password confirmation failed.");
    }
    audit::session_reauthenticated(
        &state.config.secret,
        &admin.session.session_token,
        actor_id,
        "password",
    );

    FlashRedirect::success("/admin", "Confirmed. You can now change accounts.")
}

#[derive(Debug, Deserialize)]
pub struct UpdateRoleForm {
    pub role: Role,
}

pub async fn update_role_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
    Form(req): Form<UpdateRoleForm>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    if user_id == original_admin_id {
        return FlashRedirect::error("/admin", "You cannot modify your own role.");
    }

    let role = req.role;
    let result: AppResult<()> = async {
        let target = user::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        if target.role != role {
            user::update_role(&state.db, user_id, role).await?;
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => FlashRedirect::success("/admin", format!("Role updated to {}.", role.as_str())),
        _ => FlashRedirect::error("/admin", "Failed to update role."),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateStatusForm {
    pub disabled: bool,
}

pub async fn update_status_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
    Form(req): Form<UpdateStatusForm>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    if user_id == original_admin_id {
        return FlashRedirect::error("/admin", "You cannot modify your own status.");
    }

    let disabled = req.disabled;
    let result: AppResult<()> = async {
        let target = user::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        if disabled && !target.is_disabled() {
            user::disable_user(&state.db, user_id).await?;
            session::delete_user_sessions(&state.db, user_id).await?;
            audit::sessions_destroyed_bulk(user_id, "admin_disable", None);
            // Disabling an account must also cut off any GReader client still
            // holding an API token — otherwise a disabled user's RSS app keeps
            // syncing indefinitely, since its token never touches `session`.
            api_token::delete_user_tokens(&state.db, user_id).await?;
        } else if !disabled && target.is_disabled() {
            user::enable_user(&state.db, user_id).await?;
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            let msg = if disabled {
                "User disabled."
            } else {
                "User enabled."
            };
            FlashRedirect::success("/admin", msg)
        }
        _ => FlashRedirect::error("/admin", "Failed to update user status."),
    }
}

pub async fn start_masquerade_form(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Path(target_user_id): Path<i64>,
) -> Response {
    // Taking over another account is the most powerful thing on this page, so
    // it is guarded like the rest. Note the asymmetry with `stop_masquerade`,
    // which is deliberately not guarded — see `require_recent_authentication`.
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect.into_response();
    }

    if admin.session.is_masquerading() {
        return FlashRedirect::error("/admin", "You are already masquerading as another user.")
            .into_response();
    }

    let session_token = admin.session.session_token.clone();
    // Not masquerading yet (checked above), so `original_user_id` is `None`
    // and this is just the current admin — the actor about to start acting as
    // `target_user_id`.
    let actor_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let result: AppResult<String> = async {
        let target = user::find_by_id(&state.db, target_user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        if target.is_disabled() {
            return Err(AppError::UserDisabled);
        }
        session::start_masquerade(&state.db, &session_token, target_user_id).await
    }
    .await;

    match result {
        Ok(new_token) => {
            let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
            let ip = state.config.client_ip(peer, &headers).to_string();
            let user_agent = request_user_agent(&headers);
            audit::masquerade_started(
                &state.config.secret,
                &session_token,
                &new_token,
                actor_user_id,
                target_user_id,
                &ip,
                &user_agent,
            );
            (
                rotated_cookies(&state, &new_token),
                FlashRedirect::info("/", "You are now masquerading as another user."),
            )
                .into_response()
        }
        _ => FlashRedirect::error("/admin", "Failed to start masquerade.").into_response(),
    }
}

pub async fn delete_user_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    if user_id == original_admin_id {
        return FlashRedirect::error("/admin", "You cannot delete your own account.");
    }

    let result = user::delete_user(&state.db, user_id).await;

    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/admin", "User deleted.")
        }
        _ => FlashRedirect::error("/admin", "Failed to delete user."),
    }
}
