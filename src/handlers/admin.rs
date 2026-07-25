use std::net::SocketAddr;

use axum::{
    Form,
    extract::{ConnectInfo, Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::AdminUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::api_token;
use crate::models::session;
use crate::models::user::{self, Role};
use crate::services::audit;
use crate::utils::http::request_user_agent;

pub async fn stop_masquerade(
    State(state): State<AppState>,
    admin: AdminUser,
) -> AppResult<StatusCode> {
    if !admin.session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    let session_token = admin.session.session_token.clone();
    // While masquerading, `admin.user` is the impersonated target and
    // `original_user_id` is the real admin — the one both acting here and
    // being restored to the session.
    let admin_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    session::stop_masquerade(&state.db, &session_token).await?;
    audit::masquerade_stopped(
        &state.config.secret,
        &session_token,
        admin_user_id,
        admin_user_id,
    );

    Ok(StatusCode::OK)
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
) -> impl IntoResponse {
    if admin.session.is_masquerading() {
        return FlashRedirect::error("/admin", "You are already masquerading as another user.");
    }

    let session_token = admin.session.session_token.clone();
    // Not masquerading yet (checked above), so `original_user_id` is `None`
    // and this is just the current admin — the actor about to start acting as
    // `target_user_id`.
    let actor_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let result: AppResult<()> = async {
        let target = user::find_by_id(&state.db, target_user_id)
            .await?
            .ok_or(AppError::UserNotFound)?;
        if target.is_disabled() {
            return Err(AppError::UserDisabled);
        }
        session::start_masquerade(&state.db, &session_token, target_user_id).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
            let ip = state.config.client_ip(peer, &headers).to_string();
            let user_agent = request_user_agent(&headers);
            audit::masquerade_started(
                &state.config.secret,
                &session_token,
                actor_user_id,
                target_user_id,
                &ip,
                &user_agent,
            );
            FlashRedirect::info("/", "You are now masquerading as another user.")
        }
        _ => FlashRedirect::error("/admin", "Failed to start masquerade."),
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
