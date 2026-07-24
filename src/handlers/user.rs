use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::AppState;
use crate::auth::{hash_password, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::middleware::flash::FlashRedirect;
use crate::models::session;
use crate::models::user;
use crate::models::user_settings;
use crate::models::{category, entry};
use crate::services::{KagiConfig, LinkdingConfig};

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
/// (`is_admin`, `is_masquerading`) used by CSR pages to decide what UI to show.
pub async fn get_me(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<MeResponse>> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        match auth_user.session.original_user_id {
            Some(original_id) => {
                let original = user::find_by_id(&state.db, original_id).await?;
                original.is_some_and(|u| u.is_admin())
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

    let entries_per_page = user_settings::get_entries_per_page(&state.db, user_id)
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
    let theme = user_settings::get_theme(&state.db, user_id)
        .await
        .unwrap_or(None);
    let save_config = user_settings::get_save_services_config(&state.db, user_id)
        .await
        .unwrap_or_default();

    let linkding = save_config.linkding.as_ref();
    let linkding_configured =
        linkding.is_some_and(super::super::services::save::linkding::LinkdingConfig::is_configured);
    let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

    let kagi = save_config.kagi.as_ref();
    let kagi_configured =
        kagi.is_some_and(super::super::services::summarize::kagi::KagiConfig::is_configured);
    let kagi_language = kagi.and_then(|c| c.language.clone()).unwrap_or_default();

    let response = UserSettingsResponse {
        entries_per_page,
        theme,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
    };

    Ok(Json(response))
}

#[derive(Debug, Clone, Serialize)]
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
    pub total_summarized: i64,
    pub via_forward_auth: bool,
}

/// Raw chrome data needed for every authenticated page render: theme,
/// sidebar categories with unread counts, and (when masquerading) the
/// admin flag of the original session user. Bundled in one struct so all
/// of it can be fetched in a single `read_user` closure (one channel
/// round-trip, one actor turn) instead of 2-3 sequential awaits.
#[derive(Default, Clone)]
pub struct ChromeData {
    pub theme: Option<String>,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
    pub total_summarized: i64,
    /// Only set when `original_user_id` is passed (i.e. session is
    /// masquerading). `None` outside the masquerade path.
    pub original_user_is_admin: Option<bool>,
}

/// Fetch all per-page chrome data for `user_id`. Backed by an in-memory
/// per-user cache (`state.sidebar_cache`): cache hits return without a
/// single DB call on the hot path. Cache misses fetch theme +
/// categories + unread counts in one `read_user` closure, populate the
/// cache, and return.
///
/// `original_user_id` (only `Some` when the session is masquerading) is
/// never cached — it depends on the *session*, not on `user_id` — so a
/// masquerading request adds one extra `read_user` lookup.
pub async fn read_chrome_data(
    state: &AppState,
    user_id: i64,
    original_user_id: Option<i64>,
) -> ChromeData {
    let original_user_is_admin = match original_user_id {
        Some(id) => user::find_by_id(&state.db, id)
            .await
            .ok()
            .map(|u| u.is_some_and(|u| u.is_admin())),
        None => None,
    };

    if let Some(cached) = state.sidebar_cache.get(user_id) {
        return ChromeData {
            theme: cached.theme,
            categories: cached.categories,
            total_unread: cached.total_unread,
            total_summarized: cached.total_summarized,
            original_user_is_admin,
        };
    }

    let theme = user_settings::get_theme(&state.db, user_id)
        .await
        .unwrap_or(None);
    let cats = category::list_by_user(&state.db, user_id)
        .await
        .unwrap_or_default();
    let unread_by_cat = entry::count_unread_by_category(&state.db, user_id)
        .await
        .unwrap_or_default();
    // Total unread is the sum of the per-category map already fetched —
    // avoids a second full scan via count_unread_by_user.
    let total_unread: i64 = unread_by_cat.values().sum();
    let total_summarized = crate::models::entry_summary::count_completed(&state.db, user_id)
        .await
        .unwrap_or(0);
    let has_feeds = crate::models::feed::count_by_user(&state.db, user_id)
        .await
        .unwrap_or(0)
        > 0;
    let categories: Vec<SidebarCategoryDto> = cats
        .into_iter()
        .map(|c| SidebarCategoryDto {
            id: c.id,
            name: c.name,
            unread_count: *unread_by_cat.get(&c.id).unwrap_or(&0),
        })
        .collect();
    let fresh = crate::services::CachedChrome {
        theme,
        categories,
        total_unread,
        total_summarized,
    };

    // Skip caching the "no content yet" state — an account with no feeds and no
    // unread (e.g. a brand-new account whose only category is the auto-seeded
    // empty "Uncategorized"). Such accounts pay a trivial extra query per page
    // load until they add their first feed; the cache populates normally after
    // that. The benefit: the empty state is the most likely-to-go-stale entry —
    // anything added via a path that bypasses our handler bust hooks (e.g. E2E
    // tests that seed straight into SQLite) would otherwise be hidden behind a
    // stale cache for up to the 60 s TTL.
    if has_feeds || fresh.total_unread > 0 {
        state.sidebar_cache.insert(user_id, fresh.clone());
    }

    ChromeData {
        theme: fresh.theme,
        categories: fresh.categories,
        total_unread: fresh.total_unread,
        total_summarized: fresh.total_summarized,
        original_user_is_admin,
    }
}

/// Build the sidebar payload for the given authenticated session. Used by
/// both the JSON API and the shell handler (which embeds it inline so the
/// CSR sidebar paints without a network round trip).
pub async fn build_sidebar_response(
    state: &AppState,
    user: &crate::models::User,
    session: &crate::models::session::Session,
    via_forward_auth: bool,
) -> AppResult<SidebarResponse> {
    let is_masquerading = session.is_masquerading();
    let chrome = read_chrome_data(
        state,
        user.id,
        if is_masquerading {
            session.original_user_id
        } else {
            None
        },
    )
    .await;

    let is_admin = if is_masquerading {
        chrome.original_user_is_admin.unwrap_or(false)
    } else {
        user.is_admin()
    };

    Ok(SidebarResponse {
        username: user.username.clone(),
        is_admin,
        is_masquerading,
        categories: chrome.categories,
        total_unread: chrome.total_unread,
        total_summarized: chrome.total_summarized,
        via_forward_auth,
    })
}

/// Returns sidebar data: user identity, masquerade/admin flags, categories
/// with unread counts, and total unread.
pub async fn get_sidebar(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<SidebarResponse>> {
    let payload = build_sidebar_response(
        &state,
        &auth_user.user,
        &auth_user.session,
        auth_user.via_forward_auth,
    )
    .await?;
    Ok(Json(payload))
}

pub async fn get_current_user(auth_user: AuthUser) -> Json<crate::models::User> {
    Json(auth_user.user)
}

fn extract_kagi_session_token(session_link: &str) -> Result<String, AppError> {
    let url = Url::parse(session_link.trim())
        .map_err(|_e| AppError::Validation("Invalid session link URL".to_string()))?;

    url.query_pairs()
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::Validation("No token found in session link".to_string()))
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

    let theme = user_settings::get_theme(&state.db, user_id).await?;

    Ok(Json(GetThemeResponse { theme }))
}

pub async fn update_theme(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<UpdateThemeRequest>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;

    // Validate theme value
    if let Some(ref theme) = req.theme
        && theme != "dark"
        && theme != "light"
    {
        return Err(AppError::Validation(
            "Theme must be 'dark', 'light', or null".to_string(),
        ));
    }

    user_settings::update_theme(&state.db, user_id, req.theme).await?;

    state.sidebar_cache.bust(user_id);
    Ok(StatusCode::OK)
}

// ============================================================================
// Form-action handlers for the SSR /user-settings page (PR-4 Task 1).
// Each accepts application/x-www-form-urlencoded bodies and returns a
// FlashRedirect response (303 See Other + flash cookie + Location).
// The existing JSON PUT endpoints continue to work alongside these until
// PR-4 Task 3 deletes them.
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ChangePasswordForm {
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

pub async fn change_password_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<ChangePasswordForm>,
) -> impl IntoResponse {
    if req.new_password != req.confirm_password {
        return FlashRedirect::error("/user-settings", "New passwords do not match.");
    }

    if req.new_password.len() < 6 {
        return FlashRedirect::error(
            "/user-settings",
            "New password must be at least 6 characters.",
        );
    }

    if !verify_password(&req.current_password, &auth_user.user.password_hash) {
        return FlashRedirect::error("/user-settings", "Current password is incorrect.");
    }

    let Ok(new_hash) = hash_password(&req.new_password) else {
        return FlashRedirect::error("/user-settings", "Failed to hash password.");
    };
    let user_id = auth_user.user.id;

    let result: AppResult<()> = async {
        user::update_password(&state.db, user_id, &new_hash).await?;
        // Delete all sessions for the user to force re-login
        session::delete_user_sessions(&state.db, user_id).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => FlashRedirect::success(
            "/login",
            "Password changed successfully. Please login with your new password.",
        ),
        _ => FlashRedirect::error("/user-settings", "Failed to update password."),
    }
}

pub async fn revoke_other_sessions_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> impl IntoResponse {
    if auth_user.session.is_masquerading() {
        return FlashRedirect::error(
            "/user-settings",
            "Session management is unavailable while masquerading.",
        );
    }

    let user_id = auth_user.user.id;
    match session::delete_user_sessions_except(&state.db, user_id, &auth_user.session.session_token)
        .await
    {
        Ok(()) => FlashRedirect::success("/user-settings", "Signed out all other sessions."),
        Err(_) => FlashRedirect::error("/user-settings", "Failed to sign out other sessions."),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdatePreferencesForm {
    pub theme: String,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
}

pub async fn update_preferences_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<UpdatePreferencesForm>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let theme = match req.theme.as_str() {
        "light" => Some("light".to_string()),
        "dark" => Some("dark".to_string()),
        // "system" or any other value -> store None
        _ => None,
    };
    let epp = req.entries_per_page;
    let retention_read_days = req.retention_read_days;

    let result: AppResult<()> = async {
        user_settings::upsert(&state.db, user_id, epp).await?;
        user_settings::update_theme(&state.db, user_id, theme).await?;
        user_settings::update_retention_read_days(&state.db, user_id, retention_read_days).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            state.sidebar_cache.bust(user_id);
            FlashRedirect::success("/user-settings", "Preferences updated.")
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/user-settings", msg),
        _ => FlashRedirect::error("/user-settings", "Failed to update preferences."),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateLinkdingForm {
    pub api_url: Option<String>,
    pub api_token: Option<String>,
    #[serde(rename = "_clear")]
    pub clear: Option<String>,
}

pub async fn update_linkding_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<UpdateLinkdingForm>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let clear = req.clear.is_some();

    let result: AppResult<()> = async {
        let mut config = user_settings::get_save_services_config(&state.db, user_id).await?;

        if clear {
            config.linkding = None;
        } else {
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
        }

        user_settings::update_save_services(&state.db, user_id, &config).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            let msg = if clear {
                "Linkding configuration cleared."
            } else {
                "Linkding configuration updated."
            };
            FlashRedirect::success("/user-settings", msg)
        }
        Err(AppError::Validation(msg)) => FlashRedirect::error("/user-settings", msg),
        _ => FlashRedirect::error("/user-settings", "Failed to update Linkding configuration."),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateKagiForm {
    pub session_link: Option<String>,
    pub language: Option<String>,
    #[serde(rename = "_clear")]
    pub clear: Option<String>,
}

pub async fn update_kagi_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Form(req): Form<UpdateKagiForm>,
) -> impl IntoResponse {
    let user_id = auth_user.user.id;
    let clear = req.clear.is_some();

    if clear {
        let result: AppResult<()> = async {
            let mut config = user_settings::get_save_services_config(&state.db, user_id).await?;
            config.kagi = None;
            user_settings::update_save_services(&state.db, user_id, &config).await?;
            Ok(())
        }
        .await;

        return match result {
            Ok(()) => FlashRedirect::success("/user-settings", "Kagi configuration cleared."),
            _ => FlashRedirect::error("/user-settings", "Failed to clear Kagi configuration."),
        };
    }

    let has_language_field = req.language.is_some();
    let session_token = match req.session_link.filter(|s| !s.is_empty()) {
        Some(link) => match extract_kagi_session_token(&link) {
            Ok(token) => Some(token),
            Err(AppError::Validation(msg)) => {
                return FlashRedirect::error("/user-settings", msg);
            }
            Err(_) => {
                return FlashRedirect::error("/user-settings", "Invalid Kagi session link.");
            }
        },
        None => None,
    };
    let language = req.language.filter(|s| !s.is_empty());

    let result: AppResult<()> = async {
        let mut config = user_settings::get_save_services_config(&state.db, user_id).await?;

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

        user_settings::update_save_services(&state.db, user_id, &config).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => FlashRedirect::success("/user-settings", "Kagi configuration updated."),
        Err(AppError::Validation(msg)) => FlashRedirect::error("/user-settings", msg),
        _ => FlashRedirect::error("/user-settings", "Failed to update Kagi configuration."),
    }
}
