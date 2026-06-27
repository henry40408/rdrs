use axum::{Json, extract::State, http::StatusCode};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use time::Duration;

use crate::AppState;
use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::{AuthUser, SESSION_COOKIE_NAME};
use crate::models::category;
use crate::models::session;
use crate::models::user::{self, Role};

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

    let can_register = state.config.can_register(0); // We check count inside closure
    let config = state.config.clone();
    let password_hash = hash_password(&req.password)?;

    let user = state
        .db
        .user(move |conn| {
            let user_count = user::count(conn)?;

            if !config.can_register(user_count) {
                return Err(AppError::RegistrationNotAllowed);
            }

            let role = if user_count == 0 {
                Role::Admin
            } else {
                Role::User
            };

            let user = user::create_user(conn, &req.username, &password_hash, role)?;

            // Seed a default category so the account can add its first feed
            // without first creating a category. Matches the "Uncategorized"
            // convention used by OPML import and the GReader subscription API.
            category::create_category(conn, user.id, "Uncategorized")?;

            Ok::<_, AppError>(user)
        })
        .await??;

    // Suppress unused variable warning
    let _ = can_register;

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
    if state.config.disable_local_auth {
        return Err(AppError::Forbidden);
    }
    let (user, new_session) = state
        .db
        .user(move |conn| {
            let user =
                user::find_by_username(conn, &req.username)?.ok_or(AppError::InvalidCredentials)?;

            if !verify_password(&req.password, &user.password_hash) {
                return Err(AppError::InvalidCredentials);
            }

            if user.is_disabled() {
                return Err(AppError::UserDisabled);
            }

            let new_session = session::create_session(conn, user.id)?;
            Ok::<_, AppError>((user, new_session))
        })
        .await??;

    let cookie = Cookie::build((SESSION_COOKIE_NAME, new_session.session_token))
        .path("/")
        .http_only(true)
        .same_site(axum_extra::extract::cookie::SameSite::Lax)
        .max_age(Duration::days(session::SESSION_ABSOLUTE_MAX_DAYS))
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

#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    pub redirect_to: String,
}

pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    auth_user: AuthUser,
) -> AppResult<(CookieJar, Json<LogoutResponse>)> {
    let token = auth_user.session.session_token.clone();
    state
        .db
        .user(move |conn| session::delete_session(conn, &token))
        .await??;

    // Removal must match the Path=/ the cookie was set with, or the browser
    // keeps the (now-invalid) session_token cookie. Mirrors flash.rs.
    let removal = Cookie::build((SESSION_COOKIE_NAME, "")).path("/").build();

    let redirect_to = state
        .config
        .auth_proxy_logout_url
        .clone()
        .unwrap_or_else(|| "/login".to_string());

    Ok((jar.remove(removal), Json(LogoutResponse { redirect_to })))
}
