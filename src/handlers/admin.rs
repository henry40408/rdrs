use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Form, Json,
};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::middleware::flash::FlashRedirect;
use crate::middleware::AdminUser;
use crate::models::session;
use crate::models::user::{self, Role, User};
use crate::AppState;

pub async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Vec<User>>> {
    let users = state.db.read_user(user::list_all).await??;
    Ok(Json(users))
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub role: Option<Role>,
    pub disabled: Option<bool>,
}

pub async fn update_user(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> AppResult<StatusCode> {
    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    if user_id == original_admin_id {
        return Err(AppError::CannotModifySelf);
    }

    state
        .db
        .user(move |conn| {
            let target = user::find_by_id(conn, user_id)?.ok_or(AppError::UserNotFound)?;

            if let Some(role) = req.role {
                if target.role != role {
                    user::update_role(conn, user_id, role)?;
                }
            }

            if let Some(disabled) = req.disabled {
                if disabled && !target.is_disabled() {
                    user::disable_user(conn, user_id)?;
                    session::delete_user_sessions(conn, user_id)?;
                } else if !disabled && target.is_disabled() {
                    user::enable_user(conn, user_id)?;
                }
            }

            Ok::<_, AppError>(())
        })
        .await??;

    Ok(StatusCode::OK)
}

pub async fn delete_user(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(user_id): Path<i64>,
) -> AppResult<StatusCode> {
    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    if user_id == original_admin_id {
        return Err(AppError::CannotModifySelf);
    }

    state
        .db
        .user(move |conn| user::delete_user(conn, user_id))
        .await??;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn start_masquerade(
    State(state): State<AppState>,
    admin: AdminUser,
    Path(target_user_id): Path<i64>,
) -> AppResult<StatusCode> {
    if admin.session.is_masquerading() {
        return Err(AppError::AlreadyMasquerading);
    }

    let session_token = admin.session.session_token.clone();
    state
        .db
        .user(move |conn| {
            let target = user::find_by_id(conn, target_user_id)?.ok_or(AppError::UserNotFound)?;

            if target.is_disabled() {
                return Err(AppError::UserDisabled);
            }

            session::start_masquerade(conn, &session_token, target_user_id)?;

            Ok::<_, AppError>(())
        })
        .await??;

    Ok(StatusCode::OK)
}

pub async fn stop_masquerade(
    State(state): State<AppState>,
    admin: AdminUser,
) -> AppResult<StatusCode> {
    if !admin.session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    let session_token = admin.session.session_token.clone();
    state
        .db
        .user(move |conn| session::stop_masquerade(conn, &session_token))
        .await??;

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
    let result = state
        .db
        .user(move |conn| {
            let target = user::find_by_id(conn, user_id)?.ok_or(AppError::UserNotFound)?;
            if target.role != role {
                user::update_role(conn, user_id, role)?;
            }
            Ok::<_, AppError>(())
        })
        .await;

    match result {
        Ok(Ok(())) => {
            FlashRedirect::success("/admin", format!("Role updated to {}.", role.as_str()))
        }
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
    let result = state
        .db
        .user(move |conn| {
            let target = user::find_by_id(conn, user_id)?.ok_or(AppError::UserNotFound)?;
            if disabled && !target.is_disabled() {
                user::disable_user(conn, user_id)?;
                session::delete_user_sessions(conn, user_id)?;
            } else if !disabled && target.is_disabled() {
                user::enable_user(conn, user_id)?;
            }
            Ok::<_, AppError>(())
        })
        .await;

    match result {
        Ok(Ok(())) => {
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
    Path(target_user_id): Path<i64>,
) -> impl IntoResponse {
    if admin.session.is_masquerading() {
        return FlashRedirect::error("/admin", "You are already masquerading as another user.");
    }

    let session_token = admin.session.session_token.clone();
    let result = state
        .db
        .user(move |conn| {
            let target = user::find_by_id(conn, target_user_id)?.ok_or(AppError::UserNotFound)?;
            if target.is_disabled() {
                return Err(AppError::UserDisabled);
            }
            session::start_masquerade(conn, &session_token, target_user_id)?;
            Ok::<_, AppError>(())
        })
        .await;

    match result {
        Ok(Ok(())) => FlashRedirect::info("/", "You are now masquerading as another user."),
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

    let result = state
        .db
        .user(move |conn| user::delete_user(conn, user_id))
        .await;

    match result {
        Ok(Ok(())) => FlashRedirect::success("/admin", "User deleted."),
        _ => FlashRedirect::error("/admin", "Failed to delete user."),
    }
}
