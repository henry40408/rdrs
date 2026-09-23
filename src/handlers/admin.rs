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
use crate::models::category;
use crate::models::session;
use crate::models::user::{self, Role};
use crate::models::user_invite;
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
    // While masquerading, `original_user_id` is the real admin.
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

/// Session and CSRF cookies reissued after a token rotation. Both must be
/// rebuilt: the CSRF token derives from the session token.
fn rotated_cookies(state: &AppState, new_token: &str) -> CookieJar {
    let secret = &state.config.secret;
    let secure = state.config.cookie_secure;
    CookieJar::new()
        .add(build_session_cookie(new_token, secret, secure))
        .add(build_csrf_cookie(new_token, secret, secure))
}

// Form-action POST endpoints for the SSR /admin page; each returns a FlashRedirect.

/// Redirect unless the session re-authenticated recently (OWASP: re-verify
/// before sensitive account changes). A redirect, not an extractor, so it works
/// without JS. Exempt: forward-auth sessions, and `stop_masquerade` (it would
/// ask for the impersonated user's password).
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

/// `POST /admin/reauth` — no-JS twin of `POST /api/session/reauth`; shares the
/// `PasswordChange` rate-limit budget so it cannot be used to brute-force.
pub async fn reauth_form(
    State(state): State<AppState>,
    admin: AdminUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Form(req): Form<AdminReauthForm>,
) -> impl IntoResponse {
    if admin.via_forward_auth {
        // The proxy re-asserts identity on every request; nothing to check.
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

    // Verify the real admin's password, not the impersonated user's.
    let actor_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let Ok(Some(actor)) = user::find_by_id(&state.db, actor_id).await else {
        return FlashRedirect::error("/admin", "Password confirmation failed.");
    };

    if !crate::auth::verify_password(&req.password, &actor.password_hash) {
        return FlashRedirect::error("/admin", "Incorrect password.");
    }
    // Success must not consume the rate-limit budget.
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

/// Mint an invite link for `user_id`. Only an HMAC is stored, so the raw token
/// is unrecoverable after this returns.
async fn issue_invite(
    state: &AppState,
    user_id: i64,
    actor_user_id: i64,
    reason: &str,
) -> AppResult<String> {
    let token = session::generate_token();
    let invite = user_invite::issue(&state.db, &state.config.secret, user_id, &token).await?;

    audit::invite_issued(
        &state.config.secret,
        &token,
        user_id,
        actor_user_id,
        reason,
        invite.expires_at,
    );

    Ok(format!("/invite/{token}"))
}

/// Absolute URL when `RDRS_PUBLIC_BASE_URL` is set, else the bare path.
fn invite_url(state: &AppState, path: &str) -> String {
    match &state.config.public_base_url {
        Some(base) => format!("{}{}", base.trim_end_matches('/'), path),
        None => path.to_string(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateUserForm {
    pub username: String,
    pub role: Role,
}

/// `POST /admin/users` — create an account and issue its first invite. The
/// account is unusable until the invite is redeemed.
pub async fn create_user_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Form(req): Form<CreateUserForm>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let username = req.username.trim().to_string();
    if username.is_empty() {
        return FlashRedirect::error("/admin", "Username is required.");
    }

    let actor_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let result: AppResult<String> = async {
        let user_count = user::count(&state.db).await?;
        if !state.config.can_create_account(user_count) {
            return Err(AppError::RegistrationNotAllowed);
        }

        // `"!"` never parses as a PHC string, so no password verifies.
        let created = user::create_user(&state.db, &username, "!", req.role).await?;
        audit::account_created(
            created.id,
            actor_id,
            username.chars().count(),
            req.role.as_str(),
        );

        category::create_category(&state.db, created.id, "Uncategorized").await?;

        issue_invite(&state, created.id, actor_id, "account_created").await
    }
    .await;

    match result {
        // Bare URL only: `/admin` detects it via `pages::extract_invite_link`.
        Ok(path) => FlashRedirect::success("/admin", invite_url(&state, &path)),
        Err(AppError::UsernameExists) => {
            FlashRedirect::error("/admin", "That username is already taken.")
        }
        Err(AppError::RegistrationNotAllowed) => FlashRedirect::error(
            "/admin",
            "This instance is single-user. Set RDRS_MULTI_USER_ENABLED=true to add accounts.",
        ),
        _ => FlashRedirect::error("/admin", "Failed to create the account."),
    }
}

/// `POST /admin/users/{id}/invite` — issue a fresh link, revoking any current
/// one. Also the password-reset path; the old password works until redeemed.
pub async fn reissue_invite_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let actor_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let result: AppResult<String> = async {
        let target = user::find_by_id(&state.db, user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        let reason = if target.password_hash == "!" {
            "account_created"
        } else {
            "password_reset"
        };
        issue_invite(&state, user_id, actor_id, reason).await
    }
    .await;

    match result {
        Ok(path) => FlashRedirect::success("/admin", invite_url(&state, &path)),
        _ => FlashRedirect::error("/admin", "Failed to issue a link."),
    }
}

/// `POST /admin/users/{id}/invite/revoke` — cancel an outstanding link.
pub async fn revoke_invite_form(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
) -> impl IntoResponse {
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect;
    }

    let actor_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    match user_invite::revoke_for_user(&state.db, user_id).await {
        Ok(0) => FlashRedirect::error("/admin", "There is no outstanding link for that account."),
        Ok(_) => {
            audit::invite_revoked(user_id, actor_id);
            FlashRedirect::success("/admin", "Link revoked.")
        }
        Err(_) => FlashRedirect::error("/admin", "Failed to revoke the link."),
    }
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
            // API tokens bypass `session`, so revoke them too.
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
    if let Some(redirect) = require_recent_authentication(&admin) {
        return redirect.into_response();
    }

    if admin.session.is_masquerading() {
        return FlashRedirect::error("/admin", "You are already masquerading as another user.")
            .into_response();
    }

    let session_token = admin.session.session_token.clone();
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
