use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::session;
use crate::models::user;
use crate::models::user_settings;
use crate::models::{category, entry};
use crate::services::{KagiConfig, LinkdingConfig};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub id: i64,
    pub username: String,
    pub role: crate::models::Role,
    pub is_admin: bool,
    pub is_masquerading: bool,
    /// The original admin's user id when masquerading (otherwise `None`).
    /// CSR pages use this to disable destructive actions on both the
    /// currently-impersonated user and the underlying admin.
    pub original_user_id: Option<i64>,
    pub created_at: String,
    pub session_created_at: String,
}

/// Returns the current user augmented with session-derived flags
/// (is_admin, is_masquerading) used by CSR pages to decide what UI to show.
pub async fn get_me(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<MeResponse>> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        match auth_user.session.original_user_id {
            Some(original_id) => {
                let original = state
                    .db
                    .read_user(move |conn| user::find_by_id(conn, original_id))
                    .await??;
                original.map(|u| u.is_admin()).unwrap_or(false)
            }
            None => false,
        }
    } else {
        auth_user.user.is_admin()
    };

    Ok(Json(MeResponse {
        id: auth_user.user.id,
        username: auth_user.user.username,
        role: auth_user.user.role,
        is_admin,
        is_masquerading,
        original_user_id: auth_user.session.original_user_id,
        created_at: auth_user
            .user
            .created_at
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        session_created_at: auth_user
            .session
            .created_at
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    }))
}

#[derive(Debug, Serialize)]
pub struct UserSettingsResponse {
    pub entries_per_page: i64,
    pub theme: Option<String>,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: String,
}

/// Bundled settings payload for the CSR user-settings page (theme,
/// entries-per-page, integration status). Mutations still flow through
/// the per-resource PUT endpoints; this is read-only.
pub async fn get_user_settings(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<UserSettingsResponse>> {
    let user_id = auth_user.user.id;

    let response = state
        .db
        .read_user(move |conn| {
            let entries_per_page = user_settings::get_entries_per_page(conn, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let theme = user_settings::get_theme(conn, user_id).unwrap_or(None);
            let save_config =
                user_settings::get_save_services_config(conn, user_id).unwrap_or_default();

            let linkding = save_config.linkding.as_ref();
            let linkding_configured = linkding.map(|c| c.is_configured()).unwrap_or(false);
            let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

            let kagi = save_config.kagi.as_ref();
            let kagi_configured = kagi.map(|c| c.is_configured()).unwrap_or(false);
            let kagi_language = kagi.and_then(|c| c.language.clone()).unwrap_or_default();

            Ok::<_, AppError>(UserSettingsResponse {
                entries_per_page,
                theme,
                linkding_configured,
                linkding_api_url,
                kagi_configured,
                kagi_language,
            })
        })
        .await??;

    Ok(Json(response))
}

#[derive(Debug, Serialize)]
pub struct ServerConfigResponse {
    pub git_version: &'static str,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
}

/// Read-only server configuration for the CSR settings page. These values
/// are configured via environment variables — there is no mutation API.
pub async fn get_server_config(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<ServerConfigResponse>> {
    // Auth required so configuration isn't exposed publicly.
    let _ = auth_user;
    let user_agent_is_default = state.config.user_agent == crate::config::DEFAULT_USER_AGENT;

    Ok(Json(ServerConfigResponse {
        git_version: crate::GIT_VERSION,
        user_agent: state.config.user_agent.clone(),
        user_agent_is_default,
        signup_enabled: state.config.signup_enabled,
        multi_user_enabled: state.config.multi_user_enabled,
    }))
}

#[derive(Debug, Serialize)]
pub struct SidebarCategoryDto {
    pub id: i64,
    pub name: String,
    pub unread_count: i64,
}

#[derive(Debug, Serialize)]
pub struct SidebarResponse {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
}

/// Build the sidebar payload for the given authenticated session. Used by
/// both the JSON API and the shell handler (which embeds it inline so the
/// CSR sidebar paints without a network round trip).
pub async fn build_sidebar_response(
    state: &AppState,
    user: &crate::models::User,
    session: &crate::models::session::Session,
) -> AppResult<SidebarResponse> {
    let is_masquerading = session.is_masquerading();
    let is_admin = if is_masquerading {
        match session.original_user_id {
            Some(original_id) => state
                .db
                .read_user(move |conn| user::find_by_id(conn, original_id))
                .await??
                .map(|u| u.is_admin())
                .unwrap_or(false),
            None => false,
        }
    } else {
        user.is_admin()
    };

    let user_id = user.id;
    let (categories, total_unread) = state
        .db
        .read_user(move |conn| {
            let cats = category::list_by_user(conn, user_id).unwrap_or_default();
            let unread_by_cat = entry::count_unread_by_category(conn, user_id).unwrap_or_default();
            let total_unread = entry::count_unread_by_user(conn, user_id).unwrap_or(0);

            let dtos: Vec<SidebarCategoryDto> = cats
                .into_iter()
                .map(|c| SidebarCategoryDto {
                    id: c.id,
                    name: c.name,
                    unread_count: *unread_by_cat.get(&c.id).unwrap_or(&0),
                })
                .collect();

            Ok::<_, AppError>((dtos, total_unread))
        })
        .await??;

    Ok(SidebarResponse {
        username: user.username.clone(),
        is_admin,
        is_masquerading,
        categories,
        total_unread,
    })
}

/// Returns sidebar data: user identity, masquerade/admin flags, categories
/// with unread counts, and total unread.
pub async fn get_sidebar(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<SidebarResponse>> {
    let payload = build_sidebar_response(&state, &auth_user.user, &auth_user.session).await?;
    Ok(Json(payload))
}

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
        .read_user(move |conn| {
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
        .read_user(move |conn| {
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
        .read_user(move |conn| user_settings::get_theme(conn, user_id))
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
