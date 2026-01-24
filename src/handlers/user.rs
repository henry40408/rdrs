use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::session;
use crate::models::user;
use crate::models::user_settings;
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

    // Delete all sessions for the user to force re-login
    session::delete_user_sessions(&conn, auth_user.user.id)?;

    Ok(StatusCode::OK)
}

pub async fn get_current_user(auth_user: AuthUser) -> Json<crate::models::User> {
    Json(auth_user.user)
}

#[derive(Debug, Deserialize)]
pub struct UpdateSettingsRequest {
    pub entries_per_page: i64,
}

#[derive(Debug, Serialize)]
pub struct UpdateSettingsResponse {
    pub entries_per_page: i64,
}

pub async fn update_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<UpdateSettingsRequest>,
) -> AppResult<Json<UpdateSettingsResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let settings = user_settings::upsert(&conn, auth_user.user.id, req.entries_per_page)?;

    Ok(Json(UpdateSettingsResponse {
        entries_per_page: settings.entries_per_page,
    }))
}
