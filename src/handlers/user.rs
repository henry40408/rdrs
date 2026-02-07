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

    if !verify_password(&req.current_password, &auth_user.user.password_hash) {
        return Err(AppError::InvalidCredentials);
    }

    let new_hash = hash_password(&req.new_password)?;
    let user_id = auth_user.user.id;

    state
        .db
        .user(move |conn| {
            user::update_password(conn, user_id, &new_hash)?;
            // Delete all sessions for the user to force re-login
            session::delete_user_sessions(conn, user_id)?;
            Ok::<_, AppError>(())
        })
        .await??;

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
    let user_id = auth_user.user.id;
    let epp = req.entries_per_page;

    let settings = state
        .db
        .user(move |conn| user_settings::upsert(conn, user_id, epp))
        .await??;

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
    let user_id = auth_user.user.id;

    let (configured, api_url) = state
        .db
        .user(move |conn| {
            // Get current config
            let mut config = user_settings::get_save_services_config(conn, user_id)?;

            // Update Linkding config
            let api_url = req.api_url.filter(|s| !s.is_empty());
            let api_token = req.api_token.filter(|s| !s.is_empty());

            if api_url.is_some() || api_token.is_some() {
                let current = config.linkding.unwrap_or(LinkdingConfig {
                    api_url: String::new(),
                    api_token: String::new(),
                });

                config.linkding = Some(LinkdingConfig {
                    api_url: api_url.unwrap_or(current.api_url),
                    api_token: api_token.unwrap_or(current.api_token),
                });
            } else {
                config.linkding = None;
            }

            user_settings::update_save_services(conn, user_id, &config)?;

            let configured = config
                .linkding
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);
            let url = config.linkding.map(|c| c.api_url);

            Ok::<_, AppError>((configured, url))
        })
        .await??;

    Ok(Json(UpdateLinkdingResponse {
        configured,
        api_url,
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
    let user_id = auth_user.user.id;

    let (configured, api_url) = state
        .db
        .user(move |conn| {
            let config = user_settings::get_save_services_config(conn, user_id)?;

            let configured = config
                .linkding
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);

            Ok::<_, AppError>((configured, config.linkding.map(|c| c.api_url)))
        })
        .await??;

    Ok(Json(GetLinkdingResponse {
        configured,
        api_url,
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
    let has_language_field = req.language.is_some();
    let session_token = match req.session_link.filter(|s| !s.is_empty()) {
        Some(link) => Some(extract_kagi_session_token(&link)?),
        None => None,
    };
    let language = req.language.filter(|s| !s.is_empty());

    let user_id = auth_user.user.id;
    let (configured, lang) = state
        .db
        .user(move |conn| {
            let mut config = user_settings::get_save_services_config(conn, user_id)?;

            if session_token.is_some() || has_language_field {
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
                config.kagi = None;
            }

            user_settings::update_save_services(conn, user_id, &config)?;

            let configured = config
                .kagi
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);
            let lang = config.kagi.and_then(|c| c.language);

            Ok::<_, AppError>((configured, lang))
        })
        .await??;

    Ok(Json(UpdateKagiResponse {
        configured,
        language: lang,
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
    let user_id = auth_user.user.id;

    let (configured, language) = state
        .db
        .user(move |conn| {
            let config = user_settings::get_save_services_config(conn, user_id)?;

            let configured = config
                .kagi
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);

            Ok::<_, AppError>((configured, config.kagi.and_then(|c| c.language)))
        })
        .await??;

    Ok(Json(GetKagiResponse {
        configured,
        language,
    }))
}

#[derive(Debug, Deserialize)]
pub struct UpdateThemeRequest {
    pub theme: Option<String>, // "dark", "light", or null/missing for system
}

#[derive(Debug, Serialize)]
pub struct GetThemeResponse {
    pub theme: Option<String>,
}

pub async fn get_theme(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<GetThemeResponse>> {
    let user_id = auth_user.user.id;

    let theme = state
        .db
        .user(move |conn| user_settings::get_theme(conn, user_id))
        .await??;

    Ok(Json(GetThemeResponse { theme }))
}

pub async fn update_theme(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<UpdateThemeRequest>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;

    // Validate theme value
    if let Some(ref theme) = req.theme {
        if theme != "dark" && theme != "light" {
            return Err(AppError::Validation(
                "Theme must be 'dark', 'light', or null".to_string(),
            ));
        }
    }

    state
        .db
        .user(move |conn| user_settings::update_theme(conn, user_id, req.theme))
        .await??;

    Ok(StatusCode::OK)
}
