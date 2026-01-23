use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::middleware::AdminUser;
use crate::models::session;
use crate::models::user::{self, Role, User};
use crate::AppState;

pub async fn list_users(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> AppResult<Json<Vec<User>>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let users = user::list_all(&conn)?;
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

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let target = user::find_by_id(&conn, user_id)?.ok_or(AppError::UserNotFound)?;

    if let Some(role) = req.role {
        if target.role != role {
            user::update_role(&conn, user_id, role)?;
        }
    }

    if let Some(disabled) = req.disabled {
        if disabled && !target.is_disabled() {
            user::disable_user(&conn, user_id)?;
            session::delete_user_sessions(&conn, user_id)?;
        } else if !disabled && target.is_disabled() {
            user::enable_user(&conn, user_id)?;
        }
    }

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

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    user::delete_user(&conn, user_id)?;

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

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let target = user::find_by_id(&conn, target_user_id)?.ok_or(AppError::UserNotFound)?;

    if target.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    session::start_masquerade(&conn, &admin.session.session_token, target_user_id)?;

    Ok(StatusCode::OK)
}

pub async fn stop_masquerade(
    State(state): State<AppState>,
    admin: AdminUser,
) -> AppResult<StatusCode> {
    if !admin.session.is_masquerading() {
        return Err(AppError::NotMasquerading);
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    session::stop_masquerade(&conn, &admin.session.session_token)?;

    Ok(StatusCode::OK)
}
