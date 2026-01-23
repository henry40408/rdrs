use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;

use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::user;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<ChangePasswordRequest>,
) -> AppResult<StatusCode> {
    if req.new_password.len() < 6 {
        return Err(AppError::Validation(
            "New password must be at least 6 characters".to_string(),
        ));
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    if !verify_password(&req.current_password, &auth_user.user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    let new_hash = hash_password(&req.new_password)?;
    user::update_password(&conn, auth_user.user.id, &new_hash)?;

    Ok(StatusCode::OK)
}

pub async fn get_current_user(auth_user: AuthUser) -> Json<crate::models::User> {
    Json(auth_user.user)
}
