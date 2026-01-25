use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::session;
use crate::models::user;
use crate::models::user_settings;
use crate::services::{KagiConfig, LinkdingConfig};
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

#[derive(Debug, Deserialize)]
pub struct UpdateLinkdingRequest {
    pub api_url: Option<String>,
    pub api_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateLinkdingResponse {
    pub configured: bool,
    pub api_url: Option<String>,
}

pub async fn update_linkding_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<UpdateLinkdingRequest>,
) -> AppResult<Json<UpdateLinkdingResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    // Get current config
    let mut config = user_settings::get_save_services_config(&conn, auth_user.user.id)?;

    // Update Linkding config
    // If both api_url and api_token are empty strings or None, clear the config
    let api_url = req.api_url.filter(|s| !s.is_empty());
    let api_token = req.api_token.filter(|s| !s.is_empty());

    if api_url.is_some() || api_token.is_some() {
        // Update or create config
        let current = config.linkding.unwrap_or(LinkdingConfig {
            api_url: String::new(),
            api_token: String::new(),
        });

        config.linkding = Some(LinkdingConfig {
            api_url: api_url.unwrap_or(current.api_url),
            api_token: api_token.unwrap_or(current.api_token),
        });
    } else {
        // Clear config if both are empty
        config.linkding = None;
    }

    // Save updated config
    user_settings::update_save_services(&conn, auth_user.user.id, &config)?;

    let configured = config
        .linkding
        .as_ref()
        .map(|c| c.is_configured())
        .unwrap_or(false);

    Ok(Json(UpdateLinkdingResponse {
        configured,
        api_url: config.linkding.map(|c| c.api_url),
    }))
}

#[derive(Debug, Serialize)]
pub struct GetLinkdingResponse {
    pub configured: bool,
    pub api_url: Option<String>,
}

pub async fn get_linkding_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<GetLinkdingResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let config = user_settings::get_save_services_config(&conn, auth_user.user.id)?;

    let configured = config
        .linkding
        .as_ref()
        .map(|c| c.is_configured())
        .unwrap_or(false);

    Ok(Json(GetLinkdingResponse {
        configured,
        // Return api_url but not api_token for security
        api_url: config.linkding.map(|c| c.api_url),
    }))
}

fn extract_kagi_session_token(session_link: &str) -> Result<String, AppError> {
    let url = Url::parse(session_link.trim())
        .map_err(|_| AppError::Validation("Invalid session link URL".to_string()))?;

    url.query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Validation("No token found in session link".to_string()))
}

#[derive(Debug, Deserialize)]
pub struct UpdateKagiRequest {
    pub session_link: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateKagiResponse {
    pub configured: bool,
    pub language: Option<String>,
}

pub async fn update_kagi_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<UpdateKagiRequest>,
) -> AppResult<Json<UpdateKagiResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    // Get current config
    let mut config = user_settings::get_save_services_config(&conn, auth_user.user.id)?;

    // Update Kagi config
    let has_language_field = req.language.is_some();
    let session_token = match req.session_link.filter(|s| !s.is_empty()) {
        Some(link) => Some(extract_kagi_session_token(&link)?),
        None => None,
    };
    let language = req.language.filter(|s| !s.is_empty());

    if session_token.is_some() || has_language_field {
        // Update or create config
        let current = config.kagi.unwrap_or(KagiConfig {
            session_token: String::new(),
            language: None,
        });

        config.kagi = Some(KagiConfig {
            session_token: session_token.unwrap_or(current.session_token),
            language: if has_language_field {
                language
            } else {
                current.language
            },
        });
    } else if session_token.is_none() && !has_language_field {
        // Clear config if both are empty/not provided
        config.kagi = None;
    }

    // Save updated config
    user_settings::update_save_services(&conn, auth_user.user.id, &config)?;

    let configured = config
        .kagi
        .as_ref()
        .map(|c| c.is_configured())
        .unwrap_or(false);

    Ok(Json(UpdateKagiResponse {
        configured,
        language: config.kagi.and_then(|c| c.language),
    }))
}

#[derive(Debug, Serialize)]
pub struct GetKagiResponse {
    pub configured: bool,
    pub language: Option<String>,
}

pub async fn get_kagi_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<GetKagiResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let config = user_settings::get_save_services_config(&conn, auth_user.user.id)?;

    let configured = config
        .kagi
        .as_ref()
        .map(|c| c.is_configured())
        .unwrap_or(false);

    Ok(Json(GetKagiResponse {
        configured,
        // Return language but not session_token for security
        language: config.kagi.and_then(|c| c.language),
    }))
}
