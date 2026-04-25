use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::config::DEFAULT_USER_AGENT;
use crate::error::AppError;
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::entry_summary;
use crate::models::statistics;
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
fn format_relative_time(dt: Option<chrono::DateTime<chrono::Utc>>) -> (String, String) {
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
fn compute_freshness(
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
fn resolve_statistics_period(query: &StatisticsQuery) -> (String, String, String) {
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
        continuation_id: None,
        ot: None,
        nt: None,
        sort_order,
    };

    let mut entries = entry::list_by_user_with_continuation(conn, user_id, filter, &pagination)
        .unwrap_or_default();

    let continuation = if entries.len() as i64 > limit {
        let last = entries.pop().unwrap();
        Some(last.entry.id.to_string())
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

#[derive(Template)]
#[template(path = "unread.html")]
pub struct UnreadTemplate {
    pub username: String,
    pub role: String,
    pub sign_in_time: String,
    pub unread_count: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
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

impl IntoResponse for UnreadTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> (Flash, UnreadTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        // When masquerading, check if original user is admin
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter {
        unread_only: true,
        ..Default::default()
    };
    let cfg = fetch_entry_list_config(&state, user_id, filter, query.entry).await;

    (
        flash.clone(),
        UnreadTemplate {
            username: auth_user.user.username.clone(),
            role: auth_user.user.role.as_str().to_string(),
            sign_in_time: auth_user
                .session
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            unread_count: cfg.sidebar_unread_count,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
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

/// User info for SSR admin display.
pub struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub is_disabled: bool,
    pub created_at: String,
}

#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub username: String,
    pub current_user_id: i64,
    pub original_user_id: i64,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub users: Vec<AdminUserRow>,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
}

impl IntoResponse for AdminTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, AdminTemplate) {
    let is_masquerading = admin.session.is_masquerading();
    let original_user_id = admin.session.original_user_id.unwrap_or(admin.user.id);

    let user_id = admin.user.id;
    let (users, theme, sidebar_categories, sidebar_unread_count) = state
        .db
        .read_user(move |c| {
            let user_list = crate::models::user::list_all(c).unwrap_or_default();
            let rows: Vec<AdminUserRow> = user_list
                .into_iter()
                .map(|u| {
                    let is_disabled = u.is_disabled();
                    let role = u.role.as_str().to_string();
                    let created_at = u.created_at.format("%Y-%m-%d").to_string();
                    AdminUserRow {
                        id: u.id,
                        username: u.username,
                        role,
                        is_disabled,
                        created_at,
                    }
                })
                .collect();
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);
            (rows, theme, sidebar_cats, sidebar_unread)
        })
        .await
        .unwrap_or((vec![], None, vec![], 0));

    (
        flash.clone(),
        AdminTemplate {
            username: admin.user.username,
            current_user_id: admin.user.id,
            original_user_id,
            is_masquerading,
            flash_messages: flash.messages,
            users,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
        },
    )
}

#[derive(Template)]
#[template(path = "user-settings.html")]
pub struct UserSettingsTemplate {
    pub username: String,
    pub role: String,
    pub created_at: String,
    pub logged_in_at: String,
    pub entries_per_page: i64,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub linkding_configured: bool,
    pub linkding_api_url: String,
    pub kagi_configured: bool,
    pub kagi_language: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
}

impl IntoResponse for UserSettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn user_settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, UserSettingsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (
        entries_per_page,
        linkding_configured,
        linkding_api_url,
        kagi_configured,
        kagi_language,
        theme,
        sidebar_categories,
        sidebar_unread_count,
    ) = state
        .db
        .read_user(move |c| {
            let epp = user_settings::get_entries_per_page(c, user_id)
                .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE);

            let save_config =
                user_settings::get_save_services_config(c, user_id).unwrap_or_default();

            let linkding = save_config.linkding.as_ref();
            let linkding_configured = linkding.map(|c| c.is_configured()).unwrap_or(false);
            let api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

            let kagi = save_config.kagi.as_ref();
            let kagi_configured = kagi.map(|c| c.is_configured()).unwrap_or(false);
            let kagi_lang = kagi.and_then(|c| c.language.clone()).unwrap_or_default();

            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            (
                epp,
                linkding_configured,
                api_url,
                kagi_configured,
                kagi_lang,
                theme,
                sidebar_cats,
                sidebar_unread,
            )
        })
        .await
        .unwrap_or((
            user_settings::DEFAULT_ENTRIES_PER_PAGE,
            false,
            String::new(),
            false,
            String::new(),
            None,
            vec![],
            0,
        ));

    (
        flash.clone(),
        UserSettingsTemplate {
            username: auth_user.user.username,
            role: auth_user.user.role.as_str().to_string(),
            created_at: auth_user
                .user
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            logged_in_at: auth_user
                .session
                .created_at
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            entries_per_page,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            linkding_configured,
            linkding_api_url,
            kagi_configured,
            kagi_language,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
        },
    )
}

/// A category with its feed count for SSR display.
pub struct CategoryWithCount {
    pub id: i64,
    pub name: String,
    pub feed_count: usize,
}

#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub categories: Vec<CategoryWithCount>,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
}

impl IntoResponse for CategoriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (categories_with_counts, theme, sidebar_categories, sidebar_unread_count) = state
        .db
        .read_user(move |c| {
            let cats = category::list_by_user(c, user_id).unwrap_or_default();
            let cats_with_counts: Vec<CategoryWithCount> = cats
                .into_iter()
                .map(|cat| {
                    let feed_count = feed::list_by_category(c, cat.id)
                        .map(|f| f.len())
                        .unwrap_or(0);
                    CategoryWithCount {
                        id: cat.id,
                        name: cat.name,
                        feed_count,
                    }
                })
                .collect();
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);
            (cats_with_counts, theme, sidebar_cats, sidebar_unread)
        })
        .await
        .unwrap_or((vec![], None, vec![], 0));

    (
        flash.clone(),
        CategoriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            categories: categories_with_counts,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
        },
    )
}

#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<String>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}

/// A feed row for SSR display.
#[derive(serde::Serialize)]
pub struct FeedRow {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub category_id: i64,
    pub category_name: String,
    pub has_icon: bool,
    pub fetch_error: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: bool,
    pub custom_referrer: Option<String>,
    pub unread_count: i64,
    #[serde(skip)]
    pub fetched_at_relative: String,
    #[serde(skip)]
    pub fetched_at_datetime: String,
    #[serde(skip)]
    pub feed_updated_at_relative: String,
    #[serde(skip)]
    pub feed_updated_at_datetime: String,
    #[serde(skip)]
    pub freshness_class: String,
    #[serde(skip)]
    pub freshness_key: String,
}

/// A category option for SSR dropdowns.
pub struct CategoryOption {
    pub id: i64,
    pub name: String,
    pub feed_count: usize,
}

#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub feeds: Vec<FeedRow>,
    pub categories: Vec<CategoryOption>,
    pub feed_data_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
    pub total_feed_count: usize,
    pub active_filter: String,
    pub active_sort: String,
    pub active_category: Option<i64>,
}

impl IntoResponse for FeedsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<FeedsQuery>,
    flash: Flash,
) -> (Flash, FeedsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let (feeds_data, cats_data, theme, sidebar_categories, sidebar_unread_count) = state
        .db
        .read_user(move |c| {
            let cats = category::list_by_user(c, user_id).unwrap_or_default();
            let all_feeds = feed::list_by_user(c, user_id).unwrap_or_default();
            let unread_map = entry::count_unread_by_feed(c, user_id).unwrap_or_default();

            // Build category name map
            let cat_map: std::collections::HashMap<i64, String> = cats
                .iter()
                .map(|cat| (cat.id, cat.name.clone()))
                .collect();

            // Count feeds per category
            let mut feed_count_by_cat: std::collections::HashMap<i64, usize> =
                std::collections::HashMap::new();
            for f in &all_feeds {
                *feed_count_by_cat.entry(f.category_id).or_insert(0) += 1;
            }

            let feed_rows: Vec<FeedRow> = all_feeds
                .into_iter()
                .map(|f| {
                    let has_icon: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                            [f.id],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
                    let (updated_rel, updated_dt) = if f.feed_updated_at.is_some() {
                        format_relative_time(f.feed_updated_at)
                    } else if f.fetched_at.map(|ft| (chrono::Utc::now() - ft).num_days() <= 30).unwrap_or(false) {
                        ("No date info".to_string(), String::new())
                    } else {
                        ("Never".to_string(), String::new())
                    };
                    let (freshness_class, freshness_key) =
                        compute_freshness(f.feed_updated_at, f.fetched_at);
                    FeedRow {
                        title: f.title.clone().unwrap_or_else(|| f.url.clone()),
                        category_name: cat_map
                            .get(&f.category_id)
                            .cloned()
                            .unwrap_or_else(|| "Unknown".to_string()),
                        has_icon: has_icon > 0,
                        unread_count: *unread_map.get(&f.id).unwrap_or(&0),
                        id: f.id,
                        url: f.url,
                        category_id: f.category_id,
                        fetch_error: f.fetch_error,
                        description: f.description,
                        site_url: f.site_url,
                        custom_user_agent: f.custom_user_agent,
                        http2_disabled: f.http2_disabled,
                        custom_referrer: f.custom_referrer,
                        fetched_at_relative: fetched_rel,
                        fetched_at_datetime: fetched_dt,
                        feed_updated_at_relative: updated_rel,
                        feed_updated_at_datetime: updated_dt,
                        freshness_class,
                        freshness_key,
                    }
                })
                .collect();

            let cat_options: Vec<CategoryOption> = cats
                .into_iter()
                .map(|cat| {
                    let fc = feed_count_by_cat.get(&cat.id).copied().unwrap_or(0);
                    CategoryOption {
                        id: cat.id,
                        name: cat.name,
                        feed_count: fc,
                    }
                })
                .collect();

            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);
            (feed_rows, cat_options, theme, sidebar_cats, sidebar_unread)
        })
        .await
        .unwrap_or((vec![], vec![], None, vec![], 0));

    let active_filter = query.filter.as_deref().unwrap_or("all").to_string();
    let active_sort = query.sort.as_deref().unwrap_or("title").to_string();
    let active_category = query
        .category
        .as_deref()
        .and_then(|s| s.parse::<i64>().ok());

    let mut feeds_data = feeds_data;
    let total_feed_count = feeds_data.len();

    if let Some(cat_id) = active_category {
        feeds_data.retain(|f| f.category_id == cat_id);
    }

    match active_filter.as_str() {
        "errors" => feeds_data.retain(|f| f.fetch_error.is_some()),
        "stale" => feeds_data.retain(|f| f.freshness_key == "stale"),
        _ => {}
    }

    match active_sort.as_str() {
        "unread" => feeds_data.sort_by(|a, b| b.unread_count.cmp(&a.unread_count)),
        "category" => feeds_data.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        _ => feeds_data.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }

    // Build a JSON map: { feedId: { url, title, description, ... }, ... }
    let feed_data_map: std::collections::HashMap<i64, &FeedRow> =
        feeds_data.iter().map(|f| (f.id, f)).collect();
    let feed_data_json = serde_json::to_string(&feed_data_map).unwrap_or_else(|_| "{}".to_string());
    let feed_data_json = escape_json_for_script(&feed_data_json);

    (
        flash.clone(),
        FeedsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            feeds: feeds_data,
            categories: cats_data,
            feed_data_json,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
            total_feed_count,
            active_filter,
            active_sort,
            active_category,
        },
    )
}

#[derive(Template)]
#[template(path = "entries.html")]
pub struct EntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
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

impl IntoResponse for EntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> (Flash, EntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter::default();
    let cfg = fetch_entry_list_config(&state, auth_user.user.id, filter, query.entry).await;

    (
        flash.clone(),
        EntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
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

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
    pub theme: Option<String>,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
}

impl IntoResponse for SettingsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn settings_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_agent_is_default = state.config.user_agent == DEFAULT_USER_AGENT;

    let user_id = auth_user.user.id;
    let (theme, sidebar_categories, sidebar_unread_count) = state
        .db
        .read_user(move |c| {
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);
            (theme, sidebar_cats, sidebar_unread)
        })
        .await
        .unwrap_or((None, vec![], 0));

    (
        flash.clone(),
        SettingsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            signup_enabled: state.config.signup_enabled,
            multi_user_enabled: state.config.multi_user_enabled,
            theme,
            sidebar_categories,
            sidebar_unread_count,
        },
    )
}

// Archive entries pages (read/starred/summarized)
#[derive(Template)]
#[template(path = "entries_archive.html")]
pub struct ArchiveEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub page_mode: String,
    pub page_title: String,
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

impl IntoResponse for ArchiveEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Helper to fetch common entry-list config + SSR entries from DB.
/// Common entry-list page data.
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

async fn fetch_entry_list_config(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    reading_pane_entry_id: Option<i64>,
) -> EntryListConfig {
    fetch_entry_list_config_with_sort(
        state,
        user_id,
        filter,
        reading_pane_entry_id,
        entry::EntrySortOrder::PublishedAt,
    )
    .await
}

async fn fetch_entry_list_config_with_sort(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    reading_pane_entry_id: Option<i64>,
    sort_order: entry::EntrySortOrder,
) -> EntryListConfig {
    let secret = state.config.image_proxy_secret.clone();
    let proxy_base_url = state.config.public_base_url.clone();
    state
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
            let ssr = fetch_entries_for_ssr_with_sort(c, user_id, &filter, epp, sort_order);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let rp = reading_pane_entry_id.and_then(|eid| {
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
                ssr_entries_json: ssr.json,
                ssr_entry_views: ssr.views,
                ssr_has_continuation: ssr.has_continuation,
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
        })
}

pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        read_only: true,
        ..Default::default()
    };
    let cfg = fetch_entry_list_config_with_sort(
        &state,
        auth_user.user.id,
        filter,
        query.entry,
        entry::EntrySortOrder::ReadAt,
    )
    .await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            page_mode: "read".to_string(),
            page_title: "Read Entries".to_string(),
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

pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        starred_only: true,
        ..Default::default()
    };
    let cfg = fetch_entry_list_config(&state, auth_user.user.id, filter, query.entry).await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            page_mode: "starred".to_string(),
            page_title: "Starred Entries".to_string(),
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

pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> (Flash, ArchiveEntriesTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let filter = entry::EntryFilter {
        has_summary: Some(true),
        ..Default::default()
    };
    let cfg = fetch_entry_list_config(&state, auth_user.user.id, filter, query.entry).await;

    (
        flash.clone(),
        ArchiveEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            page_mode: "summarized".to_string(),
            page_title: "Summarized Entries".to_string(),
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

// Category entries page
#[derive(Template)]
#[template(path = "category_entries.html")]
pub struct CategoryEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub category_id: i64,
    pub category_name: String,
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

impl IntoResponse for CategoryEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> Result<(Flash, CategoryEntriesTemplate), AppError> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let rp_entry_id = query.entry;
    let secret = state.config.image_proxy_secret.clone();
    let proxy_base_url = state.config.public_base_url.clone();
    let (category_name, cfg) = state
        .db
        .read_user(move |c| {
            let cat =
                category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
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

            let filter = entry::EntryFilter {
                category_id: Some(id),
                unread_only: true,
                ..Default::default()
            };
            let ssr = fetch_entries_for_ssr(c, user_id, &filter, epp);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let rp = rp_entry_id.and_then(|eid| {
                fetch_reading_pane_entry(c, user_id, eid, &secret, proxy_base_url.as_deref())
            });
            let rp_json = rp
                .as_ref()
                .map(|e| escape_json_for_script(&serde_json::to_string(e).unwrap_or_default()))
                .unwrap_or_default();

            Ok::<_, AppError>((
                cat.name,
                EntryListConfig {
                    entries_per_page: epp,
                    has_save_services: save_services,
                    has_kagi_configured: kagi_configured,
                    ssr_entries_json: ssr.json,
                    ssr_entry_views: ssr.views,
                    ssr_has_continuation: ssr.has_continuation,
                    ssr_reading_pane: rp,
                    ssr_reading_pane_json: rp_json,
                    theme,
                    sidebar_categories: sidebar_cats,
                    sidebar_unread_count: sidebar_unread,
                },
            ))
        })
        .await??;

    Ok((
        flash.clone(),
        CategoryEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            category_id: id,
            category_name,
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

// Feed entries page
#[derive(Template)]
#[template(path = "feed_entries.html")]
pub struct FeedEntriesTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub entries_per_page: i64,
    pub has_save_services: bool,
    pub has_kagi_configured: bool,
    pub feed_id: i64,
    pub feed_url: String,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub category_id: i64,
    pub category_name: String,
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

impl IntoResponse for FeedEntriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntryQuery>,
    flash: Flash,
) -> Result<(Flash, FeedEntriesTemplate), AppError> {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };

    let user_id = auth_user.user.id;
    let rp_entry_id = query.entry;
    let secret = state.config.image_proxy_secret.clone();
    let proxy_base_url = state.config.public_base_url.clone();
    let (feed_url, feed_title, feed_has_icon, category_id, category_name, cfg) = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
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
            let feed_url = f.url.clone();
            let feed_title = f.title.unwrap_or_else(|| f.url.clone());
            let has_icon: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);

            let filter = entry::EntryFilter {
                feed_id: Some(id),
                unread_only: true,
                ..Default::default()
            };
            let ssr = fetch_entries_for_ssr(c, user_id, &filter, epp);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let rp = rp_entry_id.and_then(|eid| {
                fetch_reading_pane_entry(c, user_id, eid, &secret, proxy_base_url.as_deref())
            });
            let rp_json = rp
                .as_ref()
                .map(|e| escape_json_for_script(&serde_json::to_string(e).unwrap_or_default()))
                .unwrap_or_default();

            Ok::<_, AppError>((
                feed_url,
                feed_title,
                has_icon > 0,
                cat.id,
                cat.name,
                EntryListConfig {
                    entries_per_page: epp,
                    has_save_services: save_services,
                    has_kagi_configured: kagi_configured,
                    ssr_entries_json: ssr.json,
                    ssr_entry_views: ssr.views,
                    ssr_has_continuation: ssr.has_continuation,
                    ssr_reading_pane: rp,
                    ssr_reading_pane_json: rp_json,
                    theme,
                    sidebar_categories: sidebar_cats,
                    sidebar_unread_count: sidebar_unread,
                },
            ))
        })
        .await??;

    Ok((
        flash.clone(),
        FeedEntriesTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            entries_per_page: cfg.entries_per_page,
            has_save_services: cfg.has_save_services,
            has_kagi_configured: cfg.has_kagi_configured,
            feed_id: id,
            feed_url,
            feed_title,
            feed_has_icon,
            category_id,
            category_name,
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
    ))
}

#[derive(Template)]
#[template(path = "statistics.html")]
pub struct StatisticsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
    pub active_period: String,
    pub custom_from: String,
    pub custom_to: String,
    pub overview: statistics::PersonalOverview,
    pub daily_read_counts: Vec<statistics::DailyReadCount>,
    pub daily_read_max: i64,
    pub categories: Vec<statistics::CategoryCount>,
    pub category_max: i64,
    pub top_feeds: Vec<statistics::FeedCount>,
    pub feed_max: i64,
    pub show_admin_stats: bool,
    pub admin_counts: Option<statistics::AdminCounts>,
    pub admin_entry_stats: Option<statistics::AdminEntryStats>,
}

impl IntoResponse for StatisticsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<StatisticsQuery>,
    flash: Flash,
) -> (Flash, StatisticsTemplate) {
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

    let (
        theme,
        sidebar_categories,
        sidebar_unread_count,
        overview,
        daily_read_counts,
        categories,
        top_feeds,
        admin_counts,
        admin_entry_stats,
    ) = state
        .db
        .read_user(move |c| {
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let overview =
                statistics::get_personal_overview(c, user_id, &from_c, &to_c).unwrap_or_default();
            let daily = statistics::get_daily_read_counts(c, user_id, &chart_from_c, &to_c)
                .unwrap_or_default();
            let cats =
                statistics::get_entries_by_category(c, user_id, &from_c, &to_c).unwrap_or_default();
            let feeds =
                statistics::get_top_feeds(c, user_id, &from_c, &to_c, 10).unwrap_or_default();

            let admin_counts = if show_admin_stats {
                statistics::get_admin_counts(c).ok()
            } else {
                None
            };
            let admin_entry_stats = if show_admin_stats {
                statistics::get_admin_entry_stats(c, &from_c, &to_c).ok()
            } else {
                None
            };

            (
                theme,
                sidebar_cats,
                sidebar_unread,
                overview,
                daily,
                cats,
                feeds,
                admin_counts,
                admin_entry_stats,
            )
        })
        .await
        .unwrap_or_default();

    let daily_read_max = daily_read_counts.iter().map(|d| d.count).max().unwrap_or(0);
    let category_max = categories.iter().map(|c| c.count).max().unwrap_or(0);
    let feed_max = top_feeds.iter().map(|f| f.count).max().unwrap_or(0);

    let (custom_from, custom_to) = if active_period == "custom" {
        (query.from.unwrap_or_default(), query.to.unwrap_or_default())
    } else {
        (String::new(), String::new())
    };

    (
        flash.clone(),
        StatisticsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
            active_period,
            custom_from,
            custom_to,
            overview,
            daily_read_counts,
            daily_read_max,
            categories,
            category_max,
            top_feeds,
            feed_max,
            show_admin_stats,
            admin_counts,
            admin_entry_stats,
        },
    )
}
