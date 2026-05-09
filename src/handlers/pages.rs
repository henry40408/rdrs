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

/// Serves `/admin` rendered fully server-side. The user table is rendered
/// directly from the DB via Askama, with self-detection in the handler so
/// the template can hide destructive actions on rows that match either the
/// effective admin or the original admin (under masquerade). Each row's
/// action buttons are `<form>` elements posting to the `/admin/users/{id}/*`
/// form-action endpoints added in PR-5 T1.
pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AdminTemplate) {
    let auth_user = PageAuthUser {
        user: admin.user.clone(),
        session: admin.session.clone(),
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let effective_admin_id = admin.user.id;

    let users = state
        .db
        .read_user(crate::models::user::list_all)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
        .into_iter()
        .map(|u| {
            let disabled = u.is_disabled();
            AdminUserView {
                id: u.id,
                username: u.username,
                role: u.role.as_str().to_string(),
                disabled,
                created_at: u.created_at.format("%Y-%m-%d").to_string(),
                is_self: u.id == effective_admin_id || u.id == original_admin_id,
            }
        })
        .collect();

    (
        flash,
        AdminTemplate {
            title: "Admin Panel",
            git_version: crate::GIT_VERSION,
            layout,
            users,
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

/// One row of the SSR `/admin` user table.
pub struct AdminUserView {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    pub is_self: bool,
}

/// Per-route template for `/admin`.
#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub users: Vec<AdminUserView>,
}

impl IntoResponse for AdminTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// One day in the daily-read chart, with pre-computed bar height + label.
pub struct DailyReadView {
    pub date: String,
    pub count: i64,
    pub height_percent: f64,
    pub short_label: String,
}

/// One row in the "Entries by Category" list, with pre-computed bar width.
pub struct CategoryStatsView {
    pub name: String,
    pub count: i64,
    pub width_percent: f64,
}

/// One row in the "Top Feeds" list, with pre-computed bar width.
pub struct FeedStatsView {
    pub title: String,
    pub count: i64,
    pub width_percent: f64,
}

/// Site-wide stats block shown to non-masquerading admins.
pub struct AdminStatsView {
    pub total_users: i64,
    pub total_feeds: i64,
    pub total_entries: i64,
    pub read_rate_fmt: String,
}

/// Per-route template for `/statistics`.
#[derive(Template)]
#[template(path = "statistics.html")]
pub struct StatisticsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub active_period: String,
    pub custom_from: String,
    pub custom_to: String,
    pub total_entries: i64,
    pub read_entries: i64,
    pub unread_entries: i64,
    pub starred_entries: i64,
    pub summaries: i64,
    pub read_rate_fmt: String,
    pub daily_max: i64,
    pub daily_read_counts: Vec<DailyReadView>,
    pub categories: Vec<CategoryStatsView>,
    pub top_feeds: Vec<FeedStatsView>,
    pub admin: Option<AdminStatsView>,
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

/// Serves `/statistics` rendered fully server-side. Period buttons are
/// plain `<a href="?period=...">`; the custom-date range is a native GET
/// form. All chart bar heights / widths are pre-computed in the handler
/// so the template stays free of expressions.
pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<StatisticsQuery>,
) -> (Flash, StatisticsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };
    let show_admin_stats = is_admin && !is_masquerading;

    let (from, to, active_period) = resolve_statistics_period(&query);
    let chart_from = if active_period == "all" {
        let today = chrono::Utc::now().date_naive();
        (today - chrono::Duration::days(90)).to_string()
    } else {
        from.clone()
    };

    let user_id = auth_user.user.id;
    let from_c = from.clone();
    let to_c = to.clone();
    let chart_from_c = chart_from.clone();

    let (overview, daily, cats, feeds, admin_counts, admin_entry_stats) = state
        .db
        .read_user(move |c| {
            let overview =
                crate::models::statistics::get_personal_overview(c, user_id, &from_c, &to_c)
                    .unwrap_or_default();
            let daily =
                crate::models::statistics::get_daily_read_counts(c, user_id, &chart_from_c, &to_c)
                    .unwrap_or_default();
            let cats =
                crate::models::statistics::get_entries_by_category(c, user_id, &from_c, &to_c)
                    .unwrap_or_default();
            let feeds = crate::models::statistics::get_top_feeds(c, user_id, &from_c, &to_c, 10)
                .unwrap_or_default();
            let admin_counts = if show_admin_stats {
                crate::models::statistics::get_admin_counts(c).ok()
            } else {
                None
            };
            let admin_entry_stats = if show_admin_stats {
                crate::models::statistics::get_admin_entry_stats(c, &from_c, &to_c).ok()
            } else {
                None
            };
            Ok::<_, AppError>((
                overview,
                daily,
                cats,
                feeds,
                admin_counts,
                admin_entry_stats,
            ))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    let (custom_from, custom_to) = if active_period == "custom" {
        (
            query.from.clone().unwrap_or_default(),
            query.to.clone().unwrap_or_default(),
        )
    } else {
        (String::new(), String::new())
    };

    let daily_max = daily.iter().map(|d| d.count).max().unwrap_or(0);
    let cat_max = cats.iter().map(|c| c.count).max().unwrap_or(0);
    let feed_max = feeds.iter().map(|f| f.count).max().unwrap_or(0);

    let daily_read_counts = daily
        .into_iter()
        .map(|d| {
            let date_str = d.date.format("%Y-%m-%d").to_string();
            let short_label = if date_str.len() >= 10 {
                format!("{}/{}", &date_str[5..7], &date_str[8..10])
            } else {
                date_str.clone()
            };
            let height_percent = if daily_max > 0 {
                (d.count as f64 * 100.0) / daily_max as f64
            } else {
                0.0
            };
            DailyReadView {
                date: date_str,
                count: d.count,
                height_percent,
                short_label,
            }
        })
        .collect();

    let categories = cats
        .into_iter()
        .map(|c| CategoryStatsView {
            count: c.count,
            width_percent: if cat_max > 0 {
                (c.count as f64 * 100.0) / cat_max as f64
            } else {
                0.0
            },
            name: c.name,
        })
        .collect();

    let top_feeds = feeds
        .into_iter()
        .map(|f| FeedStatsView {
            count: f.count,
            width_percent: if feed_max > 0 {
                (f.count as f64 * 100.0) / feed_max as f64
            } else {
                0.0
            },
            title: f.title,
        })
        .collect();

    let admin = match (admin_counts, admin_entry_stats) {
        (Some(c), Some(e)) => Some(AdminStatsView {
            total_users: c.total_users,
            total_feeds: c.total_feeds,
            total_entries: e.total_entries,
            read_rate_fmt: format!("{:.1}", e.read_rate()),
        }),
        _ => None,
    };

    (
        flash,
        StatisticsTemplate {
            title: "Statistics",
            git_version: crate::GIT_VERSION,
            layout,
            active_period,
            custom_from,
            custom_to,
            total_entries: overview.total_entries,
            read_entries: overview.read_entries,
            unread_entries: overview.unread_entries(),
            starred_entries: overview.starred_entries,
            summaries: overview.summaries,
            read_rate_fmt: format!("{:.1}", overview.read_rate()),
            daily_max,
            daily_read_counts,
            categories,
            top_feeds,
            admin,
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
