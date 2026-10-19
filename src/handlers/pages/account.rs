use askama::Template;
use axum::extract::State;

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::api_token;
use crate::models::session;
use crate::models::user_settings;

/// `GET /user-settings`.
pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let user_id = auth_user.user.id;

    let (
        theme,
        entries_per_page,
        retention_read_days,
        offline_keep,
        sidebar_prefs,
        pixel_tracking_enabled,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
        credentials_unreadable,
    ) = {
        let theme = user_settings::get_theme(&state.db, user_id)
            .await
            .unwrap_or(None);
        let entries_per_page = user_settings::get_entries_per_page(&state.db, user_id)
            .await
            .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
        let retention_read_days = user_settings::get_retention_read_days(&state.db, user_id)
            .await
            .unwrap_or(0);
        let offline_keep = user_settings::get_offline_keep(&state.db, user_id)
            .await
            .unwrap_or(user_settings::OFFLINE_KEEP_OFF);
        let sidebar_prefs = user_settings::get_sidebar_prefs(&state.db, user_id)
            .await
            .unwrap_or_default();
        let pixel_tracking_enabled =
            user_settings::get_pixel_tracking_enabled_at(&state.db, user_id)
                .await
                .unwrap_or(None)
                .is_some();
        let save_config = user_settings::get_save_services_config(
            &state.db,
            user_id,
            state.config.service_token_key(),
        )
        .await
        .unwrap_or_else(|_| {
            user_settings::StoredServices::Config(
                crate::services::save::SaveServicesConfig::default(),
            )
        });
        let credentials_unreadable = save_config.is_undecryptable();
        let save_config = save_config.or_default();

        let linkding = save_config.linkding.as_ref();
        let linkding_configured =
            linkding.is_some_and(crate::services::save::linkding::LinkdingConfig::is_configured);
        let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

        let kagi = save_config.kagi.as_ref();
        let kagi_configured =
            kagi.is_some_and(crate::services::summarize::kagi::KagiConfig::is_configured);
        let kagi_language = kagi.and_then(|c| c.language.clone());

        (
            theme,
            entries_per_page,
            retention_read_days,
            offline_keep,
            sidebar_prefs,
            pixel_tracking_enabled,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
            credentials_unreadable,
        )
    };

    let public_base_url = state
        .config
        .public_base_url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{}", state.config.server_bind.port()));

    let role = auth_user.user.role.as_str().to_string();
    let created_at = auth_user
        .user
        .created_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let created_at_iso = auth_user.user.created_at.to_rfc3339();
    let session_created_at = auth_user
        .session
        .created_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let session_created_at_iso = auth_user.session.created_at.to_rfc3339();
    let username = auth_user.user.username.clone();

    let is_masquerading = auth_user.session.is_masquerading();
    let sessions: Vec<SessionRow> = if is_masquerading {
        Vec::new()
    } else {
        session::list_user_sessions(&state.db, user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.is_expired())
            .map(|s| SessionRow {
                id: s.id,
                created_at: s.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                created_at_iso: s.created_at.to_rfc3339(),
                expires_at: s.expires_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                expires_at_iso: s.expires_at.to_rfc3339(),
                is_current: s.id == auth_user.session.id,
                user_agent: s.user_agent.clone(),
                ip_address: s.ip_address.clone(),
                last_seen: s.last_seen_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                last_seen_iso: s.last_seen_at.to_rfc3339(),
            })
            .collect()
    };

    // Hidden while masquerading, like `sessions`: never expose the target's tokens.
    let api_tokens: Vec<ApiTokenRow> = if is_masquerading {
        Vec::new()
    } else {
        api_token::list_user_tokens(&state.db, user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| !t.is_expired())
            .map(|t| ApiTokenRow {
                id: t.id,
                label: t.label.clone(),
                created_at: t.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                created_at_iso: t.created_at.to_rfc3339(),
                last_seen: t.last_seen_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                last_seen_iso: t.last_seen_at.to_rfc3339(),
                expires_at: t.expires_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                expires_at_iso: t.expires_at.to_rfc3339(),
                user_agent: t.user_agent.clone(),
                ip_address: t.ip_address.clone(),
            })
            .collect()
    };

    (
        flash,
        UserSettingsTemplate {
            title: "Settings",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            username,
            role,
            created_at,
            created_at_iso,
            session_created_at,
            session_created_at_iso,
            sessions,
            show_sessions: !is_masquerading,
            api_tokens,
            public_base_url,
            theme,
            entries_per_page,
            retention_read_days,
            entries_per_page_suggestions: user_settings::ENTRIES_PER_PAGE_SUGGESTIONS,
            retention_read_days_suggestions: user_settings::RETENTION_READ_DAYS_SUGGESTIONS,
            offline_keep_suggestions: user_settings::OFFLINE_KEEP_SUGGESTIONS,
            offline_keep,
            sidebar_sort: sidebar_prefs.sort,
            sidebar_hide_read: sidebar_prefs.hide_read,
            pixel_tracking_enabled,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
            credentials_unreadable,
        },
    )
}

/// "Active Sessions" card. `id` is safe to expose (revoke re-checks ownership);
/// the session token must never be — it is a bearer credential.
pub struct SessionRow {
    pub id: i64,
    pub created_at: String,
    pub created_at_iso: String,
    pub expires_at: String,
    pub expires_at_iso: String,
    pub is_current: bool,
    pub user_agent: String,
    pub ip_address: String,
    pub last_seen: String,
    pub last_seen_iso: String,
}

/// "`GReader` API Tokens" row. `id` is safe to expose: revoke is `user_id`-scoped.
pub struct ApiTokenRow {
    pub id: i64,
    pub label: String,
    pub created_at: String,
    pub created_at_iso: String,
    pub last_seen: String,
    pub last_seen_iso: String,
    pub expires_at: String,
    pub expires_at_iso: String,
    pub user_agent: String,
    pub ip_address: String,
}

/// Per-route template for `/user-settings`.
#[derive(Template)]
#[template(path = "user_settings.html")]
pub struct UserSettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub created_at_iso: String,
    pub session_created_at: String,
    pub session_created_at_iso: String,
    pub sessions: Vec<SessionRow>,
    /// Hidden while masquerading so the admin can't revoke the target's sessions.
    pub show_sessions: bool,
    /// Unexpired `GReader` tokens; empty while masquerading.
    pub api_tokens: Vec<ApiTokenRow>,
    pub public_base_url: String,
    pub theme: Option<String>,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
    /// `<datalist>` suggestions, sourced from the model's constants.
    pub entries_per_page_suggestions: &'static [i64],
    pub retention_read_days_suggestions: &'static [i64],
    pub offline_keep_suggestions: &'static [i64],
    /// Entries kept offline, or [`user_settings::OFFLINE_KEEP_OFF`].
    pub offline_keep: i64,
    /// `"name"` or `"unread"`.
    pub sidebar_sort: &'static str,
    pub sidebar_hide_read: bool,
    pub pixel_tracking_enabled: bool,
    /// See [`super::SetupTemplate::password_min_length`].
    pub password_min_length: usize,
    pub password_max_length: usize,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: Option<String>,
    /// Stored credentials can't be decrypted with the current `RDRS_SECRET`;
    /// warn rather than show "not configured" and invite an overwrite.
    pub credentials_unreadable: bool,
}
