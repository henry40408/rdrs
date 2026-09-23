use std::net::SocketAddr;

use axum::{
    Form, Json,
    extract::{ConnectInfo, Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::AppState;
use crate::auth::{hash_password, validate_password_strength, verify_password};
use crate::error::{AppError, AppResult};
use crate::middleware::flash::FlashRedirect;
use crate::middleware::{AuthUser, Bucket};
use crate::models::api_token;
use crate::models::session;
use crate::models::user;
use crate::models::user_settings;
use crate::models::{category, entry};
use crate::services::{KagiConfig, LinkdingConfig, audit};

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub id: i64,
    pub username: String,
    pub role: crate::models::Role,
    pub is_admin: bool,
    pub is_masquerading: bool,
    /// The real admin's id while masquerading.
    pub original_user_id: Option<i64>,
    pub created_at: String,
    pub session_created_at: String,
}

/// The current user plus session-derived `is_admin` / `is_masquerading`.
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
    /// Stored credentials exist but this `RDRS_SECRET` cannot decrypt them.
    pub credentials_unreadable: bool,
}

/// Read-only settings payload for the user-settings page.
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
    let stored = user_settings::get_save_services_config(
        &state.db,
        user_id,
        state.config.service_token_key(),
    )
    .await
    .unwrap_or_else(|_| {
        user_settings::StoredServices::Config(crate::services::save::SaveServicesConfig::default())
    });
    let credentials_unreadable = stored.is_undecryptable();
    let save_config = stored.or_default();

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
        credentials_unreadable,
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
    /// `"name"` or `"unread"`; lists are sent in name order and the client
    /// re-orders (it knows which row is active).
    pub sidebar_sort: &'static str,
    /// Client-side filter; the server still sends read rows.
    pub sidebar_hide_read: bool,
}

/// Per-page chrome data for every authenticated render.
#[derive(Default, Clone)]
pub struct ChromeData {
    pub theme: Option<String>,
    pub categories: Vec<SidebarCategoryDto>,
    pub total_unread: i64,
    pub total_summarized: i64,
    pub sidebar_prefs: user_settings::SidebarPrefs,
    /// See [`crate::services::CachedChrome::offline_keep`].
    pub offline_keep: i64,
    /// `Some` only while masquerading.
    pub original_user_is_admin: Option<bool>,
}

/// Chrome data for `user_id`, via the per-user sidebar cache. The
/// `original_user_id` lookup is session-specific and never cached.
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

    // Snapshot before any DB read so a concurrent bust blocks the publish.
    let generation = state.sidebar_cache.begin_read(user_id);

    if let Some(cached) = state.sidebar_cache.get(user_id) {
        return ChromeData {
            theme: cached.theme,
            categories: cached.categories,
            total_unread: cached.total_unread,
            total_summarized: cached.total_summarized,
            sidebar_prefs: cached.sidebar_prefs,
            offline_keep: cached.offline_keep,
            original_user_is_admin,
        };
    }

    // One row read covers theme and sidebar prefs.
    let settings = user_settings::find_by_user_id(&state.db, user_id)
        .await
        .unwrap_or(None);
    let theme = settings.as_ref().and_then(|s| s.theme.clone());
    let sidebar_prefs = settings
        .as_ref()
        .map_or_else(user_settings::SidebarPrefs::default, |s| {
            user_settings::sidebar_prefs_of(s)
        });
    let offline_keep = settings
        .as_ref()
        .map_or(user_settings::OFFLINE_KEEP_OFF, |s| s.offline_keep);
    let cats = category::list_by_user(&state.db, user_id)
        .await
        .unwrap_or_default();
    let unread_by_cat = entry::count_unread_by_category(&state.db, user_id)
        .await
        .unwrap_or_default();
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
        sidebar_prefs,
        offline_keep,
    };

    // Don't cache the empty state: it is the likeliest to go stale.
    if has_feeds || fresh.total_unread > 0 {
        state
            .sidebar_cache
            .insert_if_current(user_id, generation, fresh.clone());
    }

    ChromeData {
        theme: fresh.theme,
        categories: fresh.categories,
        total_unread: fresh.total_unread,
        total_summarized: fresh.total_summarized,
        sidebar_prefs: fresh.sidebar_prefs,
        offline_keep: fresh.offline_keep,
        original_user_is_admin,
    }
}

/// Sidebar payload, shared by the JSON API and the inline page embed.
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
        sidebar_sort: chrome.sidebar_prefs.sort,
        sidebar_hide_read: chrome.sidebar_prefs.hide_read,
    })
}

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

#[derive(Debug, Serialize)]
pub struct SidebarFeedDto {
    pub id: i64,
    pub title: String,
    pub unread_count: i64,
    /// Whether `/api/feeds/{id}/icon` exists; avoids a broken-image 404.
    pub has_icon: bool,
}

#[derive(Debug, Serialize)]
pub struct SidebarFeedsResponse {
    pub category_id: i64,
    pub feeds: Vec<SidebarFeedDto>,
}

/// `GET /api/sidebar/categories/{id}/feeds`. Kept out of `/api/sidebar`, which
/// is embedded in every page. Ownership: both queries are `user_id`-scoped.
pub async fn get_sidebar_category_feeds(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(category_id): Path<i64>,
) -> AppResult<Json<SidebarFeedsResponse>> {
    let user_id = auth_user.user.id;
    category::find_by_id_and_user(&state.db, category_id, user_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;

    let feeds = crate::models::feed::list_by_category(&state.db, category_id).await?;
    let unread = entry::count_unread_by_feed_in_category(&state.db, user_id, category_id).await?;
    let feed_ids: Vec<i64> = feeds.iter().map(|f| f.id).collect();
    let with_icon =
        crate::models::image::existing_ids(&state.db, crate::models::image::ENTITY_FEED, &feed_ids)
            .await?;

    Ok(Json(SidebarFeedsResponse {
        category_id,
        feeds: feeds
            .into_iter()
            .map(|f| SidebarFeedDto {
                unread_count: unread.get(&f.id).copied().unwrap_or(0),
                has_icon: with_icon.contains(&f.id),
                title: f.title.unwrap_or(f.url),
                id: f.id,
            })
            .collect(),
    }))
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

// Form-action handlers for the SSR /user-settings page; each returns a FlashRedirect.

#[derive(Debug, Deserialize)]
pub struct ChangePasswordForm {
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

pub async fn change_password_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    headers: HeaderMap,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    Form(req): Form<ChangePasswordForm>,
) -> impl IntoResponse {
    if req.new_password != req.confirm_password {
        return FlashRedirect::error("/user-settings", "New passwords do not match.");
    }

    // Rate-limit after the free mismatch check but before the costly Argon2
    // verify and strength estimate (~79ms worst case).
    let peer = connect.map(|Extension(ConnectInfo(addr))| addr.ip());
    let ip = state.config.client_ip(peer, &headers);
    if let Some(retry_after_secs) = state
        .login_rate_limiter
        .try_acquire(Bucket::PasswordChange, ip)
        .retry_after_secs()
    {
        tracing::warn!(event = "auth.rate_limited", %ip, bucket = ?Bucket::PasswordChange, endpoint = "POST /user-settings/password", "credential attempt rate limited");
        audit::login_rate_limited(
            "POST /user-settings/password",
            "password_change",
            &ip.to_string(),
        );
        return FlashRedirect::error(
            "/user-settings",
            format!("Too many attempts. Please try again in {retry_after_secs} seconds."),
        );
    }

    // zxcvbn feedback shown as-is. A rejection keeps its rate-limit reservation.
    if let Err(AppError::Validation(msg)) =
        validate_password_strength(&req.new_password, &[&auth_user.user.username])
    {
        let msg = if msg.ends_with('.') {
            msg
        } else {
            format!("{msg}.")
        };
        return FlashRedirect::error("/user-settings", msg);
    }

    if !verify_password(&req.current_password, &auth_user.user.password_hash) {
        return FlashRedirect::error("/user-settings", "Current password is incorrect.");
    }

    // Success must not consume the rate-limit budget.
    state.login_rate_limiter.release(Bucket::PasswordChange, ip);

    let Ok(new_hash) = hash_password(&req.new_password) else {
        return FlashRedirect::error("/user-settings", "Failed to hash password.");
    };
    let user_id = auth_user.user.id;

    let result: AppResult<()> = async {
        user::update_password(&state.db, user_id, &new_hash).await?;
        session::delete_user_sessions(&state.db, user_id).await?;
        audit::sessions_destroyed_bulk(user_id, "password_change", None);
        // API tokens bypass `session`, so revoke them too.
        api_token::delete_user_tokens(&state.db, user_id).await?;
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

/// Signs out other browser sessions; deliberately leaves API tokens alone.
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
        Ok(0) => {
            audit::sessions_destroyed_bulk(user_id, "revoke_others", Some(0));
            FlashRedirect::info(
                "/user-settings",
                "No other sessions were signed in — nothing to sign out.",
            )
        }
        Ok(count) => {
            audit::sessions_destroyed_bulk(user_id, "revoke_others", Some(count));
            FlashRedirect::success(
                "/user-settings",
                format!(
                    "Signed out {count} other session{}.",
                    if count == 1 { "" } else { "s" }
                ),
            )
        }
        Err(_) => FlashRedirect::error("/user-settings", "Failed to sign out other sessions."),
    }
}

/// Revoke one browser session (`user_id`-scoped). The current session is
/// refused server-side, not just hidden in the UI.
pub async fn revoke_session_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if auth_user.session.is_masquerading() {
        return FlashRedirect::error(
            "/user-settings",
            "Session management is unavailable while masquerading.",
        );
    }

    if id == auth_user.session.id {
        return FlashRedirect::error(
            "/user-settings",
            "That is the session you are using — sign out instead.",
        );
    }

    let user_id = auth_user.user.id;
    match session::delete_user_session_by_id(&state.db, id, user_id).await {
        Ok(0) => FlashRedirect::error("/user-settings", "That session is no longer active."),
        Ok(count) => {
            audit::sessions_destroyed_bulk(user_id, "revoke_session", Some(count));
            FlashRedirect::success("/user-settings", "Session revoked.")
        }
        Err(_) => FlashRedirect::error("/user-settings", "Failed to revoke session."),
    }
}

/// Revoke one `GReader` API token (`user_id`-scoped).
pub async fn revoke_api_token_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    if auth_user.session.is_masquerading() {
        return FlashRedirect::error(
            "/user-settings",
            "Session management is unavailable while masquerading.",
        );
    }

    match api_token::delete_token(&state.db, id, auth_user.user.id).await {
        Ok(()) => {
            audit::api_tokens_destroyed(auth_user.user.id, "revoke_token", None);
            FlashRedirect::success("/user-settings", "API token revoked.")
        }
        Err(_) => FlashRedirect::error("/user-settings", "Failed to revoke API token."),
    }
}

/// Revoke all of the user's `GReader` API tokens.
pub async fn revoke_all_api_tokens_form(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> impl IntoResponse {
    if auth_user.session.is_masquerading() {
        return FlashRedirect::error(
            "/user-settings",
            "Session management is unavailable while masquerading.",
        );
    }

    match api_token::delete_user_tokens(&state.db, auth_user.user.id).await {
        Ok(0) => {
            audit::api_tokens_destroyed(auth_user.user.id, "revoke_all", Some(0));
            FlashRedirect::info("/user-settings", "There were no API tokens to revoke.")
        }
        Ok(count) => {
            audit::api_tokens_destroyed(auth_user.user.id, "revoke_all", Some(count));
            FlashRedirect::success(
                "/user-settings",
                format!(
                    "Revoked {count} API token{}.",
                    if count == 1 { "" } else { "s" }
                ),
            )
        }
        Err(_) => FlashRedirect::error("/user-settings", "Failed to revoke API tokens."),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdatePreferencesForm {
    pub theme: String,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
    /// `"name"` or `"unread"`; absent means the default.
    pub sidebar_sort: Option<String>,
    /// Checkbox: presence means on.
    pub sidebar_hide_read: Option<String>,
    /// Entries kept offline, `0` for off; absent leaves the setting unchanged.
    pub offline_keep: Option<i64>,
    /// Checkbox: presence means on.
    pub pixel_tracking: Option<String>,
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
        _ => None,
    };
    let epp = req.entries_per_page;
    let retention_read_days = req.retention_read_days;
    let sidebar_sort = req
        .sidebar_sort
        .as_deref()
        .unwrap_or(user_settings::DEFAULT_SIDEBAR_SORT)
        .to_string();
    let sidebar_hide_read = req.sidebar_hide_read.is_some();
    let pixel_tracking = req.pixel_tracking.is_some();

    let result: AppResult<()> = async {
        user_settings::upsert(&state.db, user_id, epp).await?;
        user_settings::update_theme(&state.db, user_id, theme).await?;
        user_settings::update_retention_read_days(&state.db, user_id, retention_read_days).await?;
        user_settings::update_sidebar_prefs(&state.db, user_id, &sidebar_sort, sidebar_hide_read)
            .await?;
        if let Some(keep) = req.offline_keep {
            user_settings::update_offline_keep(&state.db, user_id, keep).await?;
        }
        user_settings::update_pixel_tracking(&state.db, user_id, pixel_tracking).await?;
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
        // `or_default`: re-submitting recovers from undecryptable credentials.
        let mut config = user_settings::get_save_services_config(
            &state.db,
            user_id,
            state.config.service_token_key(),
        )
        .await?
        .or_default();

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

        user_settings::update_save_services(
            &state.db,
            user_id,
            &config,
            state.config.service_token_key(),
        )
        .await?;
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
            // `or_default`: re-submitting recovers from undecryptable credentials.
            let mut config = user_settings::get_save_services_config(
                &state.db,
                user_id,
                state.config.service_token_key(),
            )
            .await?
            .or_default();
            config.kagi = None;
            user_settings::update_save_services(
                &state.db,
                user_id,
                &config,
                state.config.service_token_key(),
            )
            .await?;
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
        // `or_default`: re-submitting recovers from undecryptable credentials.
        let mut config = user_settings::get_save_services_config(
            &state.db,
            user_id,
            state.config.service_token_key(),
        )
        .await?
        .or_default();

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

        user_settings::update_save_services(
            &state.db,
            user_id,
            &config,
            state.config.service_token_key(),
        )
        .await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => FlashRedirect::success("/user-settings", "Kagi configuration updated."),
        Err(AppError::Validation(msg)) => FlashRedirect::error("/user-settings", msg),
        _ => FlashRedirect::error("/user-settings", "Failed to update Kagi configuration."),
    }
}
