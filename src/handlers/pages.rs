use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::error::AppError;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::user_settings;
use crate::models::{category, feed};
use crate::AppState;

/// Escape a JSON string for safe embedding inside HTML `<script>` tags.
/// Replaces `</` with `<\/` to prevent `</script>` breakout attacks.
fn escape_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

/// Format a datetime as a human-readable relative time string.
/// Returns (relative_text, iso_datetime_for_tooltip).
pub fn format_relative_time(dt: Option<chrono::DateTime<chrono::Utc>>) -> (String, String) {
    match dt {
        None => ("Never".to_string(), String::new()),
        Some(dt) => {
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(dt);
            let seconds = duration.num_seconds();
            let relative = if seconds < 60 {
                "Just now".to_string()
            } else if seconds < 3600 {
                let mins = duration.num_minutes();
                format!("{} minute{} ago", mins, if mins == 1 { "" } else { "s" })
            } else if seconds < 86400 {
                let hours = duration.num_hours();
                format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
            } else if seconds < 2_592_000 {
                let days = duration.num_days();
                format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
            } else if seconds < 31_536_000 {
                let months = duration.num_days() / 30;
                format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
            } else {
                let years = duration.num_days() / 365;
                format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
            };
            (relative, dt.to_rfc3339())
        }
    }
}

/// Compute freshness CSS class and key from feed_updated_at and fetched_at.
pub fn compute_freshness(
    feed_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    fetched_at: Option<chrono::DateTime<chrono::Utc>>,
) -> (String, String) {
    let now = chrono::Utc::now();
    match feed_updated_at {
        Some(updated) => {
            let days = (now - updated).num_days();
            if days <= 30 {
                (String::new(), "fresh".to_string())
            } else if days <= 90 {
                ("feed-freshness-warning".to_string(), "warning".to_string())
            } else {
                ("feed-freshness-stale".to_string(), "stale".to_string())
            }
        }
        None => match fetched_at {
            Some(fetched) if (now - fetched).num_days() <= 30 => {
                ("muted".to_string(), "fresh".to_string())
            }
            Some(fetched) if (now - fetched).num_days() <= 90 => {
                ("feed-freshness-warning".to_string(), "warning".to_string())
            }
            _ => ("feed-freshness-stale".to_string(), "stale".to_string()),
        },
    }
}

#[derive(serde::Deserialize)]
pub struct StatisticsQuery {
    pub period: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Resolve the date range from period query params.
/// Returns (from_str, to_str, active_period) as ISO date strings for SQL.
pub fn resolve_statistics_period(query: &StatisticsQuery) -> (String, String, String) {
    let today = chrono::Utc::now().date_naive();
    let default_from = today - chrono::Duration::days(7);

    let period = query.period.as_deref().unwrap_or("7d");

    match period {
        "30d" => {
            let from = today - chrono::Duration::days(30);
            (
                from.to_string(),
                (today + chrono::Duration::days(1)).to_string(),
                "30d".to_string(),
            )
        }
        "90d" => {
            let from = today - chrono::Duration::days(90);
            (
                from.to_string(),
                (today + chrono::Duration::days(1)).to_string(),
                "90d".to_string(),
            )
        }
        "all" => (
            "1970-01-01".to_string(),
            (today + chrono::Duration::days(1)).to_string(),
            "all".to_string(),
        ),
        "custom" => {
            let from = query
                .from
                .as_deref()
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            let to = query
                .to
                .as_deref()
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

            match (from, to) {
                (Some(f), Some(t)) if f <= t => {
                    let max_to = f + chrono::Duration::days(365);
                    let clamped_to = if t > max_to { max_to } else { t };
                    (
                        f.to_string(),
                        (clamped_to + chrono::Duration::days(1)).to_string(),
                        "custom".to_string(),
                    )
                }
                _ => (
                    default_from.to_string(),
                    (today + chrono::Duration::days(1)).to_string(),
                    "7d".to_string(),
                ),
            }
        }
        _ => (
            default_from.to_string(),
            (today + chrono::Duration::days(1)).to_string(),
            "7d".to_string(),
        ),
    }
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    pub signup_enabled: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
}

impl IntoResponse for LoginTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn login_page(State(state): State<AppState>, flash: Flash) -> (Flash, LoginTemplate) {
    let signup_enabled = state
        .db
        .read_user(|c| crate::models::user::count(c).ok())
        .await
        .ok()
        .flatten()
        .map(|count| state.config.can_register(count))
        .unwrap_or(false);

    (
        flash.clone(),
        LoginTemplate {
            signup_enabled,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
        },
    )
}

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterTemplate {
    pub error: Option<String>,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
}

impl IntoResponse for RegisterTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn register_page(
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, RegisterTemplate) {
    let can_register = state
        .db
        .read_user(|c| crate::models::user::count(c).ok())
        .await
        .ok()
        .flatten()
        .map(|count| state.config.can_register(count))
        .unwrap_or(false);

    (
        flash.clone(),
        RegisterTemplate {
            error: if !can_register {
                Some("Registration is currently disabled".to_string())
            } else {
                None
            },
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
        },
    )
}

/// Serves the CSR shell for `/` (unread). The list itself is loaded by
/// `<rdrs-entries-page>` (mode `unread`) from `/reader/api/0/stream/contents`.
pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UnreadTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        UnreadTemplate {
            title: "Unread",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/admin`. The user list is loaded by
/// `<rdrs-admin-page>` from the existing `GET /api/admin/users` endpoint.
/// The page also calls `/api/me` to know which rows are the current admin
/// (and the original admin under masquerade) and disable destructive
/// actions for them.
pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AdminTemplate) {
    // Reuse the same shell helpers as other pages by adapting the admin
    // extractor into a PageAuthUser shape (sidebar/flash bootstrap don't
    // care which it is, only about user + session).
    let auth_user = PageAuthUser {
        user: admin.user,
        session: admin.session,
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        AdminTemplate {
            title: "Admin Panel",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves `/user-settings` rendered fully server-side. Account info,
/// GReader URLs, password / preferences / Linkding / Kagi forms targeting
/// `/user-settings/*` form-action endpoints, and a `<rdrs-passkeys>` mount
/// for the WebAuthn UI are all populated directly from `state.config` + DB.
pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let user_id = auth_user.user.id;

    // Load theme + entries_per_page + save_services in a single read.
    let (
        theme,
        entries_per_page,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
    ) = state
        .db
        .read_user(move |conn| {
            let theme = user_settings::get_theme(conn, user_id).unwrap_or(None);
            let entries_per_page = user_settings::get_entries_per_page(conn, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_config =
                user_settings::get_save_services_config(conn, user_id).unwrap_or_default();

            let linkding = save_config.linkding.as_ref();
            let linkding_configured = linkding.map(|c| c.is_configured()).unwrap_or(false);
            let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

            let kagi = save_config.kagi.as_ref();
            let kagi_configured = kagi.map(|c| c.is_configured()).unwrap_or(false);
            let kagi_language = kagi.and_then(|c| c.language.clone());

            Ok::<_, AppError>((
                theme,
                entries_per_page,
                linkding_configured,
                linkding_api_url,
                kagi_configured,
                kagi_language,
            ))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or((
            None,
            user_settings::DEFAULT_ENTRIES_PER_PAGE,
            false,
            String::new(),
            false,
            None,
        ));

    let public_base_url = state
        .config
        .public_base_url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{}", state.config.server_port));

    let role = auth_user.user.role.as_str().to_string();
    let created_at = auth_user
        .user
        .created_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let session_created_at = auth_user
        .session
        .created_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let username = auth_user.user.username.clone();

    (
        flash,
        UserSettingsTemplate {
            title: "User Settings",
            git_version: crate::GIT_VERSION,
            layout,
            username,
            role,
            created_at,
            session_created_at,
            public_base_url,
            theme,
            entries_per_page,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
        },
    )
}

// `categories_page` is now defined below as a CSR shell handler. The legacy
// `CategoriesTemplate` + SSR-rendered `templates/categories.html` were
// removed during the SSR-to-CSR migration (PR #170 follow-up).

/// Query parameters for `/feeds` and the `GET /api/feeds` JSON endpoint.
/// Used by the CSR `<rdrs-feeds-page>` to drive server-side filter / sort.
#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<String>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}

/// Serves the CSR shell for `/feeds`. The feed list itself is fetched by
/// `<rdrs-feeds-page>` from `GET /api/feeds`. CRUD (add / edit / delete /
/// import / export) goes through the existing GReader endpoints.
pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, FeedsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        FeedsTemplate {
            title: "Feeds",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/entries` (all). The list itself is loaded by
/// `<rdrs-entries-page>` (mode `all`) from `/reader/api/0/stream/contents`.
pub async fn entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, EntriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        EntriesTemplate {
            title: "Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Query parameters for the entry page redirect.
#[derive(serde::Deserialize, Default)]
pub struct EntryPageQuery {
    pub origin: Option<String>,
    pub feed: Option<i64>,
    pub category: Option<i64>,
    pub read_only: Option<String>,
    pub starred_only: Option<String>,
    pub has_summary: Option<String>,
}

/// Redirect `/entries/{id}` to the appropriate list page with `?entry={id}`.
pub async fn entry_page(
    _auth_user: PageAuthUser,
    Path(id): Path<i64>,
    Query(query): Query<EntryPageQuery>,
) -> Redirect {
    let origin = query.origin.as_deref().unwrap_or("");

    let base_url = match origin {
        "feed" => {
            if let Some(feed_id) = query.feed {
                format!("/feeds/{}/entries", feed_id)
            } else {
                "/entries".to_string()
            }
        }
        "category" => {
            if let Some(cat_id) = query.category {
                format!("/categories/{}/entries", cat_id)
            } else {
                "/entries".to_string()
            }
        }
        "read" => "/entries/read".to_string(),
        "starred" => "/entries/starred".to_string(),
        "summarized" => "/entries/summarized".to_string(),
        "entries" => "/entries".to_string(),
        "search" => "/search".to_string(),
        _ => "/".to_string(),
    };

    let redirect_url = format!("{}?entry={}", base_url, id);
    Redirect::to(&redirect_url)
}

/// Serves `/settings` rendered fully server-side. The read-only server
/// config table is populated directly from `state.config` via Askama —
/// no JS executes for this page apart from the shared chrome scripts.
pub async fn settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_agent_is_default = state.config.user_agent == crate::config::DEFAULT_USER_AGENT;

    (
        flash,
        SettingsTemplate {
            title: "Settings",
            git_version: crate::GIT_VERSION,
            layout,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            signup_enabled: state.config.signup_enabled,
            multi_user_enabled: state.config.multi_user_enabled,
        },
    )
}

/// Serves the CSR shell for `/entries/read`. Mode `read` in `<rdrs-entries-page>`.
pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, ReadEntriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        ReadEntriesTemplate {
            title: "Read Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/entries/starred`. Mode `starred` in `<rdrs-entries-page>`.
pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, StarredEntriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        StarredEntriesTemplate {
            title: "Starred Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/entries/summarized`. Mode `summarized` in `<rdrs-entries-page>`.
pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SummarizedEntriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        SummarizedEntriesTemplate {
            title: "Summarized Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/categories/{id}/entries`. Mode `category`
/// in `<rdrs-entries-page>` reads the category name from the inlined
/// sidebar bootstrap blob. The handler verifies ownership (404 otherwise).
pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, CategoryEntriesTemplate), AppError> {
    let user_id = auth_user.user.id;
    state
        .db
        .read_user(move |c| {
            category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
            Ok::<_, AppError>(())
        })
        .await??;

    let layout = build_app_layout(&state, &auth_user, &flash).await;

    Ok((
        flash,
        CategoryEntriesTemplate {
            title: "Category Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    ))
}

/// Serves the CSR shell for `/search`. The query input + URL state
/// are managed client-side by `<rdrs-entries-page>` (mode `search`).
pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SearchTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        SearchTemplate {
            title: "Search",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/feeds/{id}/entries`. Mode `feed` in
/// `<rdrs-entries-page>` resolves the stream-id, breadcrumb, and icon
/// asynchronously from `GET /api/feeds`. The handler verifies that
/// `id` belongs to the authenticated user (404 otherwise).
pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    flash: Flash,
) -> Result<(Flash, FeedEntriesTemplate), AppError> {
    let user_id = auth_user.user.id;
    state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            Ok::<_, AppError>(())
        })
        .await??;

    let layout = build_app_layout(&state, &auth_user, &flash).await;

    Ok((
        flash,
        FeedEntriesTemplate {
            title: "Feed Entries",
            git_version: crate::GIT_VERSION,
            layout,
        },
    ))
}

/// Shared layout fields embedded in every per-route logged-in
/// template. Templates reference these as `{{ layout.<field> }}`.
pub struct AppLayoutContext {
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_bootstrap_json: String,
    pub flash_bootstrap_json: String,
}

/// Build the shared layout context for a logged-in page response.
/// Loads the user's theme, the sidebar tree (escaped for inline
/// embedding), and the flash messages (also escaped).
pub async fn build_app_layout(
    state: &AppState,
    auth_user: &PageAuthUser,
    flash: &Flash,
) -> AppLayoutContext {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(state, auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    AppLayoutContext {
        theme,
        git_version: crate::GIT_VERSION,
        sidebar_bootstrap_json,
        flash_bootstrap_json,
    }
}

/// Per-route template for `/settings`. Renders the full server-config
/// table directly via Askama from fields populated out of `state.config`.
/// The shared chrome (sidebar, flash bootstrap, theme) lives in `layout`.
///
/// `git_version` is duplicated here (in addition to `layout.git_version`)
/// because `base.html` references the bare `{{ git_version }}` outside of
/// the blocks owned by `app_layout.html`. Task 4 will move that chrome
/// into `app_layout.html` and let the duplication go away.
#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/user-settings`. Renders the full page server-side
/// (account info, GReader URLs, password / preferences / linkding / kagi
/// forms, and a `<rdrs-passkeys>` mount). Form actions target the
/// `/user-settings/*` form-action handlers added in PR-4 Task 1.
#[derive(Template)]
#[template(path = "user_settings.html")]
pub struct UserSettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub session_created_at: String,
    pub public_base_url: String,
    pub theme: Option<String>,
    pub entries_per_page: i64,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: Option<String>,
}

impl IntoResponse for UserSettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/admin`.
#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for AdminTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/statistics`.
#[derive(Template)]
#[template(path = "statistics.html")]
pub struct StatisticsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for StatisticsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/categories`.
#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for CategoriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/feeds`.
#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for FeedsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/` (unread).
#[derive(Template)]
#[template(path = "unread.html")]
pub struct UnreadTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for UnreadTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/entries`.
#[derive(Template)]
#[template(path = "entries.html")]
pub struct EntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for EntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/entries/read`.
#[derive(Template)]
#[template(path = "read_entries.html")]
pub struct ReadEntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for ReadEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/entries/starred`.
#[derive(Template)]
#[template(path = "starred_entries.html")]
pub struct StarredEntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for StarredEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/entries/summarized`.
#[derive(Template)]
#[template(path = "summarized_entries.html")]
pub struct SummarizedEntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for SummarizedEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/feeds/{id}/entries`.
#[derive(Template)]
#[template(path = "feed_entries.html")]
pub struct FeedEntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for FeedEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/categories/{id}/entries`.
#[derive(Template)]
#[template(path = "category_entries.html")]
pub struct CategoryEntriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for CategoryEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/search`.
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for SearchTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Serialize the sidebar payload for inline embedding in the shell. Escapes
/// `</` to prevent `</script>` breakout.
async fn sidebar_bootstrap_json(state: &AppState, auth_user: &PageAuthUser) -> String {
    let payload =
        crate::handlers::user::build_sidebar_response(state, &auth_user.user, &auth_user.session)
            .await
            .ok();
    let json = match &payload {
        Some(p) => serde_json::to_string(p).unwrap_or_else(|_| "[]".to_string()),
        None => "null".to_string(),
    };
    escape_json_for_script(&json)
}

/// Serialize the pending flash messages for inline embedding in the shell.
fn flash_bootstrap_json(messages: &[FlashMessage]) -> String {
    let json = serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string());
    escape_json_for_script(&json)
}

/// Serves the CSR shell for `/statistics`. The actual stats data is fetched
/// by `<rdrs-statistics-page>` from `GET /api/statistics`.
pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, StatisticsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        StatisticsTemplate {
            title: "Statistics",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/categories`. The category list is fetched by
/// `<rdrs-categories-page>` from the existing GReader endpoints
/// (`/reader/api/0/tag/list` + `/reader/api/0/subscription/list`).
pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        CategoriesTemplate {
            title: "Categories",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}
