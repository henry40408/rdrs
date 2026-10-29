use askama::Template;
use axum::extract::State;

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::db::Db;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::api_token;
use crate::models::session;
use crate::models::user_settings::{self, UserSettings};
use crate::services::save::linkding::LinkdingConfig;
use crate::services::summarize::kagi::KagiConfig;
use crate::utils::datetime::format_display;

/// What the page shows of the "save" and "summarize" services. The stored
/// credentials themselves are never shown.
struct ServiceSettings {
    linkding_configured: bool,
    linkding_api_url: String,
    kagi_configured: bool,
    kagi_language: Option<String>,
    credentials_unreadable: bool,
}

fn service_settings(state: &AppState, settings: &UserSettings) -> ServiceSettings {
    let stored = settings.get_save_services_config(state.config.service_token_key());
    let credentials_unreadable = stored.is_undecryptable();
    let config = stored.or_default();

    let linkding = config.linkding.as_ref();
    let kagi = config.kagi.as_ref();
    ServiceSettings {
        linkding_configured: linkding.is_some_and(LinkdingConfig::is_configured),
        linkding_api_url: linkding.map(|c| c.api_url.clone()).unwrap_or_default(),
        kagi_configured: kagi.is_some_and(KagiConfig::is_configured),
        kagi_language: kagi.and_then(|c| c.language.clone()),
        credentials_unreadable,
    }
}

async fn session_rows(db: &Db, user_id: i64, current_session_id: i64) -> Vec<SessionRow> {
    session::list_user_sessions(db, user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.is_expired())
        .map(|s| SessionRow {
            id: s.id,
            created_at: format_display(s.created_at),
            created_at_iso: s.created_at.to_rfc3339(),
            expires_at: format_display(s.expires_at),
            expires_at_iso: s.expires_at.to_rfc3339(),
            is_current: s.id == current_session_id,
            user_agent: s.user_agent.clone(),
            ip_address: s.ip_address.clone(),
            last_seen: format_display(s.last_seen_at),
            last_seen_iso: s.last_seen_at.to_rfc3339(),
        })
        .collect()
}

async fn api_token_rows(db: &Db, user_id: i64) -> Vec<ApiTokenRow> {
    api_token::list_user_tokens(db, user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !t.is_expired())
        .map(|t| ApiTokenRow {
            id: t.id,
            label: t.label.clone(),
            created_at: format_display(t.created_at),
            created_at_iso: t.created_at.to_rfc3339(),
            last_seen: format_display(t.last_seen_at),
            last_seen_iso: t.last_seen_at.to_rfc3339(),
            expires_at: format_display(t.expires_at),
            expires_at_iso: t.expires_at.to_rfc3339(),
            user_agent: t.user_agent.clone(),
            ip_address: t.ip_address.clone(),
        })
        .collect()
}

/// `GET /user-settings`.
pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    // One row read covers every preference and the service credentials; a
    // failed read degrades to the defaults.
    let settings = user_settings::load(&state.db, user_id)
        .await
        .unwrap_or_else(|_| UserSettings::defaults(user_id));
    let sidebar_prefs = user_settings::sidebar_prefs_of(&settings);
    let services = service_settings(&state, &settings);

    let public_base_url = state
        .config
        .public_base_url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{}", state.config.server_bind.port()));

    // Both lists are hidden while masquerading: never expose the target's
    // sessions or tokens.
    let is_masquerading = auth_user.session.is_masquerading();
    let (sessions, api_tokens) = if is_masquerading {
        (Vec::new(), Vec::new())
    } else {
        (
            session_rows(&state.db, user_id, auth_user.session.id).await,
            api_token_rows(&state.db, user_id).await,
        )
    };

    (
        flash,
        UserSettingsTemplate {
            title: "Settings",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            username: auth_user.user.username.clone(),
            role: auth_user.user.role.as_str().to_string(),
            created_at: format_display(auth_user.user.created_at),
            created_at_iso: auth_user.user.created_at.to_rfc3339(),
            session_created_at: format_display(auth_user.session.created_at),
            session_created_at_iso: auth_user.session.created_at.to_rfc3339(),
            sessions,
            show_sessions: !is_masquerading,
            api_tokens,
            public_base_url,
            theme: settings.theme,
            entries_per_page: settings.entries_per_page,
            retention_read_days: settings.retention_read_days,
            entries_per_page_suggestions: user_settings::ENTRIES_PER_PAGE_SUGGESTIONS,
            retention_read_days_suggestions: user_settings::RETENTION_READ_DAYS_SUGGESTIONS,
            offline_keep_suggestions: user_settings::OFFLINE_KEEP_SUGGESTIONS,
            offline_keep: settings.offline_keep,
            sidebar_sort: sidebar_prefs.sort,
            sidebar_hide_read: sidebar_prefs.hide_read,
            pixel_tracking_enabled: settings.pixel_tracking_enabled_at.is_some(),
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
            linkding_configured: services.linkding_configured,
            linkding_api_url: services.linkding_api_url,
            kagi_configured: services.kagi_configured,
            kagi_language: services.kagi_language,
            credentials_unreadable: services.credentials_unreadable,
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
