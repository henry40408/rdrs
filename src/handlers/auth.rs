use axum::{extract::State, http::StatusCode, Json};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use time::Duration;

use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::{AuthUser, SESSION_COOKIE_NAME};
use crate::models::session;
use crate::models::user::{self, Role};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub id: i64,
    pub username: String,
    pub role: Role,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> AppResult<(StatusCode, Json<RegisterResponse>)> {
    if req.username.is_empty() {
        return Err(AppError::Validation("Username is required".to_string()));
    }
    if req.password.len() < 6 {
        return Err(AppError::Validation(
            "Password must be at least 6 characters".to_string(),
        ));
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let user_count = user::count(&conn)?;

    if !state.config.can_register(user_count) {
        return Err(AppError::RegistrationNotAllowed);
    }

    let role = if user_count == 0 {
        Role::Admin
    } else {
        Role::User
    };

    let password_hash = hash_password(&req.password)?;
    let user = user::create_user(&conn, &req.username, &password_hash, role)?;

    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        }),
    ))
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
    Json(req): Json<LoginRequest>,
) -> AppResult<(CookieJar, Json<LoginResponse>)> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let user = user::find_by_username(&conn, &req.username)?.ok_or(AppError::InvalidCredentials)?;

    if !verify_password(&req.password, &user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    if user.is_disabled() {
        return Err(AppError::UserDisabled);
    }

    let new_session = session::create_session(&conn, user.id)?;

    let cookie = Cookie::build((SESSION_COOKIE_NAME, new_session.session_token))
        .path("/")
        .http_only(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(Duration::days(session::SESSION_EXPIRY_DAYS))
        .build();

    Ok((
        jar.add(cookie),
        Json(LoginResponse {
            id: user.id,
            username: user.username,
            role: user.role,
        }),
    ))
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<CookieJar> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    session::delete_session(&conn, &auth_user.session.session_token)?;

    Ok(jar.remove(SESSION_COOKIE_NAME))
}
