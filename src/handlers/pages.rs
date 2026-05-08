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
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Unread - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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

/// Serves the CSR shell for `/user-settings`. Account info, preferences,
/// passkeys, and integrations are all loaded by `<rdrs-user-settings-page>`
/// from existing JSON endpoints (`/api/me`, `/api/user-settings`,
/// `/api/passkeys`, `/api/user/settings/{linkding,kagi,theme}`).
pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        UserSettingsTemplate {
            title: "User Settings",
            git_version: crate::GIT_VERSION,
            layout,
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
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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

/// Serves the CSR shell for `/settings`. Read-only server config is
/// loaded by `<rdrs-settings-page>` from `GET /api/server-config`.
pub async fn settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    (
        flash,
        SettingsTemplate {
            title: "Settings",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/entries/read`. Mode `read` in `<rdrs-entries-page>`.
pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Read Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    )
}

/// Serves the CSR shell for `/entries/starred`. Mode `starred` in `<rdrs-entries-page>`.
pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Starred Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    )
}

/// Serves the CSR shell for `/entries/summarized`. Mode `summarized` in `<rdrs-entries-page>`.
pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Summarized Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
) -> Result<(Flash, AppShellTemplate), AppError> {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| {
            category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
            Ok::<_, AppError>(user_settings::get_theme(c, user_id).unwrap_or(None))
        })
        .await??;

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    Ok((
        flash,
        AppShellTemplate {
            title: "Category Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    ))
}

/// Serves the CSR shell for `/search`. The query input + URL state
/// are managed client-side by `<rdrs-entries-page>` (mode `search`).
pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AppShellTemplate) {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Search - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
) -> Result<(Flash, AppShellTemplate), AppError> {
    let user_id = auth_user.user.id;
    let theme = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            Ok::<_, AppError>(user_settings::get_theme(c, user_id).unwrap_or(None))
        })
        .await??;

    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    Ok((
        flash,
        AppShellTemplate {
            title: "Feed Entries - RDRS",
            element_tag: "rdrs-entries-page",
            script_path: "/static/js/pages/entries.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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

/// Shared CSR shell template. Each migrated page returns this with the
/// element_tag and script_path of its page module.
///
/// Bootstrap fields carry the minimum per-user JSON the CSR chrome needs to
/// paint without a network round trip:
/// - `sidebar_bootstrap_json`: the `/api/sidebar` payload
/// - `flash_bootstrap_json`: pending flash messages (consumed via the `Flash`
///   extractor; the response also clears the cookie)
///
/// The page's own data (statistics rows, category list, etc.) is still
/// fetched after mount.
#[derive(Template)]
#[template(path = "app_shell.html")]
pub struct AppShellTemplate {
    pub title: &'static str,
    pub element_tag: &'static str,
    pub script_path: &'static str,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_bootstrap_json: String,
    pub flash_bootstrap_json: String,
}

impl IntoResponse for AppShellTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/settings`. Extends `app_layout.html` with the
/// settings page's element tag + script path. The shared chrome (sidebar,
/// flash bootstrap, theme) lives in `layout`.
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
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/user-settings`.
#[derive(Template)]
#[template(path = "user_settings.html")]
pub struct UserSettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
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
