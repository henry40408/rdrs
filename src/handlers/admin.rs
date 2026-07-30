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
use crate::middleware::{AdminUser, build_csrf_cookie, build_session_cookie};
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
