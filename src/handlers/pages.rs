use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::error::AppError;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::entry_summary;
use crate::models::user_settings;
use crate::models::{category, entry, feed};
use crate::services::sanitize_html;
use crate::AppState;

/// A category link for SSR sidebar rendering.
pub struct SidebarCategory {
    pub id: i64,
    pub name: String,
    pub unread_count: i64,
}

/// Fetch sidebar categories with unread counts for a user.
fn fetch_sidebar_data(conn: &rusqlite::Connection, user_id: i64) -> (Vec<SidebarCategory>, i64) {
    let cats = category::list_by_user(conn, user_id).unwrap_or_default();
    let unread_by_cat = entry::count_unread_by_category(conn, user_id).unwrap_or_default();
    let total_unread = entry::count_unread_by_user(conn, user_id).unwrap_or(0);

    let sidebar_cats = cats
        .into_iter()
        .map(|cat| {
            let unread_count = *unread_by_cat.get(&cat.id).unwrap_or(&0);
            SidebarCategory {
                id: cat.id,
                name: cat.name,
                unread_count,
            }
        })
        .collect();

    (sidebar_cats, total_unread)
}

/// Entry data for SSR embedding as JSON.
/// Field names match what the JS `_transformItem()` expects.
#[derive(serde::Serialize)]
struct SsrEntry {
    id: i64,
    feed_id: i64,
    category_id: i64,
    category_name: String,
    feed_title: String,
    feed_url: String,
    feed_has_icon: bool,
    title: String,
    link: Option<String>,
    content: String,
    author: String,
    published_at: Option<String>,
    read_at: Option<String>,
    starred_at: Option<String>,
    summary_status: Option<String>,
}

/// SSR data for the entry list component, embedded as JSON.
#[derive(serde::Serialize)]
struct SsrEntryListData {
    entries: Vec<SsrEntry>,
    continuation: Option<String>,
}

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

/// Entry data for SSR HTML rendering in Askama templates.
pub struct SsrEntryView {
    pub id: i64,
    pub feed_id: i64,
    pub category_id: i64,
    pub category_name: String,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub title: String,
    pub link: Option<String>,
    pub is_read: bool,
    pub is_starred: bool,
    pub summary_status: Option<String>,
    pub published_date: String,
    pub published_datetime: String,
}

fn ssr_entries_to_views(entries: &[SsrEntry]) -> Vec<SsrEntryView> {
    entries
        .iter()
        .map(|e| {
            let (published_date, published_datetime) = e
                .published_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| {
                    (
                        dt.format("%b %-d, %Y").to_string(),
                        dt.format("%Y-%m-%d %H:%M:%S").to_string(),
                    )
                })
                .unwrap_or_default();

            SsrEntryView {
                id: e.id,
                feed_id: e.feed_id,
                category_id: e.category_id,
                category_name: e.category_name.clone(),
                feed_title: if e.feed_title.is_empty() {
                    e.feed_url.clone()
                } else {
                    e.feed_title.clone()
                },
                feed_has_icon: e.feed_has_icon,
                title: if e.title.is_empty() {
                    "Untitled".to_string()
                } else {
                    e.title.clone()
                },
                link: e.link.clone(),
                is_read: e.read_at.is_some(),
                is_starred: e.starred_at.is_some(),
                summary_status: e.summary_status.clone(),
                published_date,
                published_datetime,
            }
        })
        .collect()
}

/// SSR data for the reading pane (entry detail view).
/// Embedded as JSON for JS hydration + fields for Askama HTML rendering.
#[derive(serde::Serialize)]
pub struct SsrReadingPaneEntry {
    pub id: i64,
    pub title: String,
    pub link: Option<String>,
    pub content: String,
    pub author: String,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub feed_id: i64,
    pub category_id: i64,
    pub category_name: String,
    pub published_at: Option<String>,
    pub read_at: Option<String>,
    pub starred_at: Option<String>,
    pub summary_status: Option<String>,
    /// Pre-formatted date for display
    #[serde(skip)]
    pub published_date: String,
}

/// Query parameter for pages that support `?entry=<id>` to SSR the reading pane.
#[derive(serde::Deserialize, Default)]
pub struct EntryQuery {
    pub entry: Option<i64>,
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

/// Fetch and sanitize an entry for SSR reading pane rendering.
fn fetch_reading_pane_entry(
    conn: &rusqlite::Connection,
    user_id: i64,
    entry_id: i64,
    secret: &[u8],
    proxy_base_url: Option<&str>,
) -> Option<SsrReadingPaneEntry> {
    let ewf = entry::find_by_id_with_feed(conn, entry_id).ok()??;

    // Verify the entry belongs to this user's feeds
    let cat = category::find_by_id(conn, ewf.category_id).ok()??;
    if cat.user_id != user_id {
        return None;
    }

    let e = &ewf.entry;
    let link = e.link.as_deref().unwrap_or("");
    let base_url = if link.is_empty() { None } else { Some(link) };
    let referrer = ewf.custom_referrer.as_deref();

    let content = if let Some(c) = e.content.as_deref() {
        sanitize_html(c, secret, base_url, referrer, proxy_base_url)
    } else {
        let fallback = e.summary.as_deref().unwrap_or("");
        sanitize_html(fallback, secret, base_url, referrer, proxy_base_url)
    };

    // Summary status
    let summary_status = entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])
        .ok()
        .and_then(|m| m.get(&entry_id).map(|s| s.as_str().to_string()));

    let published_date = e
        .published_at
        .map(|dt| dt.format("%b %-d, %Y %H:%M").to_string())
        .unwrap_or_default();

    Some(SsrReadingPaneEntry {
        id: e.id,
        title: e.title.clone().unwrap_or_else(|| "Untitled".to_string()),
        link: e.link.clone(),
        content,
        author: e.author.clone().unwrap_or_default(),
        feed_title: ewf.feed_title.unwrap_or_else(|| ewf.feed_url.clone()),
        feed_has_icon: ewf.feed_has_icon,
        feed_id: e.feed_id,
        category_id: ewf.category_id,
        category_name: ewf.category_name,
        published_at: e.published_at.map(|dt| dt.to_rfc3339()),
        read_at: e.read_at.map(|dt| dt.to_rfc3339()),
        starred_at: e.starred_at.map(|dt| dt.to_rfc3339()),
        summary_status,
        published_date,
    })
}

/// Convert EntryWithFeed + summary statuses to SSR entries.
fn entries_to_ssr(
    entries: Vec<entry::EntryWithFeed>,
    summary_statuses: &std::collections::HashMap<i64, entry_summary::SummaryStatus>,
) -> Vec<SsrEntry> {
    entries
        .into_iter()
        .map(|e| {
            let status = summary_statuses
                .get(&e.entry.id)
                .map(|s| s.as_str().to_string());
            SsrEntry {
                id: e.entry.id,
                feed_id: e.entry.feed_id,
                category_id: e.category_id,
                category_name: e.category_name,
                feed_title: e.feed_title.unwrap_or_default(),
                feed_url: e.feed_url,
                feed_has_icon: e.feed_has_icon,
                title: e.entry.title.unwrap_or_default(),
                link: e.entry.link,
                content: String::new(), // Don't include content in list view
                author: e.entry.author.unwrap_or_default(),
                published_at: e.entry.published_at.map(|dt| dt.to_rfc3339()),
                read_at: e.entry.read_at.map(|dt| dt.to_rfc3339()),
                starred_at: e.entry.starred_at.map(|dt| dt.to_rfc3339()),
                summary_status: status,
            }
        })
        .collect()
}

/// SSR result containing JSON for JS hydration and views for HTML rendering.
struct SsrEntryResult {
    json: String,
    views: Vec<SsrEntryView>,
    has_continuation: bool,
}

/// Fetch first page of entries for SSR.
fn fetch_entries_for_ssr(
    conn: &rusqlite::Connection,
    user_id: i64,
    filter: &entry::EntryFilter,
    limit: i64,
) -> SsrEntryResult {
    fetch_entries_for_ssr_with_sort(
        conn,
        user_id,
        filter,
        limit,
        entry::EntrySortOrder::PublishedAt,
    )
}

fn fetch_entries_for_ssr_with_sort(
    conn: &rusqlite::Connection,
    user_id: i64,
    filter: &entry::EntryFilter,
    limit: i64,
    sort_order: entry::EntrySortOrder,
) -> SsrEntryResult {
    let pagination = entry::ContinuationParams {
        oldest_first: false,
        limit: limit + 1, // fetch one extra to check for continuation
        continuation: None,
        ot: None,
        nt: None,
        sort_order,
    };

    let mut entries = entry::list_by_user_with_continuation(conn, user_id, filter, &pagination)
        .unwrap_or_default();

    // Emit a composite `<sort_ts>|<id>` cursor matching the GReader API. The
    // next-page predicate is bounded-OR `(sort_ts < ?ts) OR (sort_ts = ?ts AND id < ?id)`,
    // which keeps Load More correct under non-monotonic id↔sort_ts data.
    let has_more = entries.len() as i64 > limit;
    if has_more {
        entries.pop();
    }
    let continuation = if has_more {
        entries.last().and_then(|e| {
            match entry::fetch_sort_ts(conn, e.entry.id, sort_order) {
                Ok(Some(ts)) => Some(entry::ContinuationCursor::encode_composite(&ts, e.entry.id)),
                Ok(None) => None,
                Err(err) => {
                    // SSR degrades gracefully (matches the unwrap_or_default above);
                    // log so silent truncation is at least observable in ops.
                    tracing::warn!(
                        entry_id = e.entry.id,
                        error = ?err,
                        "fetch_sort_ts failed during SSR cursor emission; \
                         page will render without Load More"
                    );
                    None
                }
            }
        })
    } else {
        None
    };

    // Fetch summary statuses
    let entry_ids: Vec<i64> = entries.iter().map(|e| e.entry.id).collect();
    let summary_statuses =
        entry_summary::get_statuses_for_entries(conn, user_id, &entry_ids).unwrap_or_default();

    let ssr_entries = entries_to_ssr(entries, &summary_statuses);
    let views = ssr_entries_to_views(&ssr_entries);
    let has_continuation = continuation.is_some();
    let data = SsrEntryListData {
        entries: ssr_entries,
        continuation,
    };

    let json = serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string());
    let json = escape_json_for_script(&json);
    SsrEntryResult {
        json,
        views,
        has_continuation,
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
) -> (Flash, AppShellTemplate) {
    let user_id = admin.user.id;
    let theme = state
        .db
        .read_user(move |c| user_settings::get_theme(c, user_id).unwrap_or(None))
        .await
        .unwrap_or(None);

    // Reuse the same shell helpers as other pages by adapting the admin
    // extractor into a PageAuthUser shape (sidebar/flash bootstrap don't
    // care which it is, only about user + session).
    let auth_user = PageAuthUser {
        user: admin.user,
        session: admin.session,
    };
    let sidebar_bootstrap_json = sidebar_bootstrap_json(&state, &auth_user).await;
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    (
        flash,
        AppShellTemplate {
            title: "Admin Panel - RDRS",
            element_tag: "rdrs-admin-page",
            script_path: "/static/js/pages/admin.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
            title: "User Settings - RDRS",
            element_tag: "rdrs-user-settings-page",
            script_path: "/static/js/pages/user-settings.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
            title: "Feeds - RDRS",
            element_tag: "rdrs-feeds-page",
            script_path: "/static/js/pages/feeds.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
            title: "Settings - RDRS",
            element_tag: "rdrs-settings-page",
            script_path: "/static/js/pages/settings.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    )
}

/// Common entry-list page data, shared by the SSR-still handlers
/// (category / feed / search). Removed in B3 along with their templates.
struct EntryListConfig {
    entries_per_page: i64,
    has_save_services: bool,
    has_kagi_configured: bool,
    ssr_entries_json: String,
    ssr_entry_views: Vec<SsrEntryView>,
    ssr_has_continuation: bool,
    ssr_reading_pane: Option<SsrReadingPaneEntry>,
    ssr_reading_pane_json: String,
    theme: Option<String>,
    sidebar_categories: Vec<SidebarCategory>,
    sidebar_unread_count: i64,
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

// Search page: SSR entries on first load when ?q= is provided
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub search_query: String,
    pub empty_message: String,
    pub ssr_entries_json: String,
    pub ssr_entry_views: Vec<SsrEntryView>,
    pub ssr_has_continuation: bool,
    pub ssr_reading_pane: Option<SsrReadingPaneEntry>,
    pub ssr_reading_pane_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
}

impl IntoResponse for SearchTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

#[derive(serde::Deserialize, Default)]
pub struct SearchPageQuery {
    pub q: Option<String>,
    pub entry: Option<i64>,
}

pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<SearchPageQuery>,
    flash: Flash,
) -> (Flash, SearchTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let secret = state.config.image_proxy_secret.clone();
    let proxy_base_url = state.config.public_base_url.clone();
    let search_query = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let q_for_ssr = search_query.clone();
    let rp_entry_id = query.entry;

    let cfg = state
        .db
        .read_user(move |c| {
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);
            let save_services = user_settings::has_save_services(c, user_id).unwrap_or(false);
            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();
            let kagi_configured = save_config
                .kagi
                .as_ref()
                .map(|k| k.is_configured())
                .unwrap_or(false);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let (ssr_json, ssr_views, ssr_continuation) = match q_for_ssr {
                Some(q) => {
                    let filter = entry::EntryFilter {
                        search: Some(q),
                        ..Default::default()
                    };
                    let ssr = fetch_entries_for_ssr(c, user_id, &filter, epp);
                    (ssr.json, ssr.views, ssr.has_continuation)
                }
                None => {
                    // Emit a valid (but empty) SSR payload so the JS component hydrates the
                    // server-rendered empty placeholder in place rather than re-rendering it.
                    let json = escape_json_for_script(r#"{"entries":[],"continuation":null}"#);
                    (json, vec![], false)
                }
            };

            let rp = rp_entry_id.and_then(|eid| {
                fetch_reading_pane_entry(c, user_id, eid, &secret, proxy_base_url.as_deref())
            });
            let rp_json = rp
                .as_ref()
                .map(|e| escape_json_for_script(&serde_json::to_string(e).unwrap_or_default()))
                .unwrap_or_default();

            EntryListConfig {
                entries_per_page: epp,
                has_save_services: save_services,
                has_kagi_configured: kagi_configured,
                ssr_entries_json: ssr_json,
                ssr_entry_views: ssr_views,
                ssr_has_continuation: ssr_continuation,
                ssr_reading_pane: rp,
                ssr_reading_pane_json: rp_json,
                theme,
                sidebar_categories: sidebar_cats,
                sidebar_unread_count: sidebar_unread,
            }
        })
        .await
        .unwrap_or(EntryListConfig {
            entries_per_page: user_settings::DEFAULT_ENTRIES_PER_PAGE,
            has_save_services: false,
            has_kagi_configured: false,
            ssr_entries_json: "null".to_string(),
            ssr_entry_views: vec![],
            ssr_has_continuation: false,
            ssr_reading_pane: None,
            ssr_reading_pane_json: String::new(),
            theme: None,
            sidebar_categories: vec![],
            sidebar_unread_count: 0,
        });

    let empty_message = if search_query.is_some() {
        "No matching entries.".to_string()
    } else {
        "Enter a search term and press Enter to search.".to_string()
    };

    (
        flash.clone(),
        SearchTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            search_query: search_query.unwrap_or_default(),
            empty_message,
            ssr_entries_json: cfg.ssr_entries_json,
            ssr_entry_views: cfg.ssr_entry_views,
            ssr_has_continuation: cfg.ssr_has_continuation,
            ssr_reading_pane: cfg.ssr_reading_pane,
            ssr_reading_pane_json: cfg.ssr_reading_pane_json,
            theme: cfg.theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories: cfg.sidebar_categories,
            sidebar_unread_count: cfg.sidebar_unread_count,
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
            title: "Statistics - RDRS",
            element_tag: "rdrs-statistics-page",
            script_path: "/static/js/pages/statistics.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
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
            title: "Categories - RDRS",
            element_tag: "rdrs-categories-page",
            script_path: "/static/js/pages/categories.js",
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_bootstrap_json,
            flash_bootstrap_json,
        },
    )
}
