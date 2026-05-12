use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use std::collections::HashMap;

use crate::error::{AppError, AppResult};
use crate::middleware::auth::{PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::user_settings;
use crate::models::SummaryStatus;
use crate::models::{category, entry, entry_summary, feed};
use crate::AppState;

/// Escape a JSON string for safe embedding inside HTML `<script>` tags.
/// Replaces `</` with `<\/` to prevent `</script>` breakout attacks.
fn escape_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

// ============================================================================
// Entries-family shared view structs (PR-10)
// ============================================================================

/// View-model for one row in the entries list (`_entry_row.html`).
#[derive(Debug, Clone)]
pub struct EntryRowView {
    pub id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub feed_has_icon: bool,
    pub category_id: i64,
    pub category_name: String,
    pub title: String,
    pub link: Option<String>,
    pub published_at_iso: String,
    pub published_relative: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub summary_status: Option<SummaryStatus>,
}

impl EntryRowView {
    /// Stringified summary status for the Askama template `{% match %}` branch.
    /// Returns `Some("completed" | "pending" | "processing" | "failed")` when a
    /// summary row exists for this entry, else `None`.
    pub fn summary_status_str(&self) -> Option<&'static str> {
        self.summary_status.map(|s| s.as_str())
    }
}

/// View-model for the reading pane (`_reading_pane.html`).
/// `has_kagi` / `has_save` gate the conditional Summarize / Save buttons.
/// Action feedback (Save / Fetch Full Content) is delivered as a flash
/// message via the swap helper's `<template data-flash>` block — see
/// `_reading_pane_with_flash.html`.
#[derive(Debug, Clone)]
pub struct ReadingPaneView {
    pub id: i64,
    pub title: String,
    pub link: Option<String>,
    pub feed_title: String,
    pub author: Option<String>,
    pub published_at_iso: Option<String>,
    pub published_relative: String,
    pub content_html: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub summary_text: Option<String>,
    pub summary_in_flight: bool,
    pub has_kagi: bool,
    pub has_save: bool,
}

/// Layout context shared by all 5 entries-family pages (`_entries_layout.html`).
#[derive(Debug, Clone)]
pub struct EntriesLayoutContext {
    pub active: &'static str,
    pub description: Option<String>,
    pub empty_message: &'static str,
    pub path: String,
    /// Render the All/Read/Starred/Summarized tab bar above the list. True
    /// for the 4 entries-tabs (`active = "all" | "read" | "starred" |
    /// "summarized"`), false for `/` (unread) since unread is not a tab.
    pub show_tab_bar: bool,
    /// Render the "Mark as Read..." dropdown above the list. True for `/`
    /// (unread) and `/entries` (all) — the two views where bulk-marking
    /// matters; false for read/starred/summarized.
    pub show_mark_as_read: bool,
}

/// Map an `EntryWithFeed` (+ optional summary status) to an `EntryRowView`.
pub(crate) fn row_view_from(
    e: &entry::EntryWithFeed,
    summary_status: Option<SummaryStatus>,
) -> EntryRowView {
    let title = e
        .entry
        .title
        .clone()
        .unwrap_or_else(|| "(no title)".to_string());
    let published_at = e.entry.published_at.unwrap_or(e.entry.created_at);
    EntryRowView {
        id: e.entry.id,
        feed_id: e.entry.feed_id,
        feed_title: e
            .feed_title
            .clone()
            .unwrap_or_else(|| "(no feed)".to_string()),
        feed_has_icon: e.feed_has_icon,
        category_id: e.category_id,
        category_name: e.category_name.clone(),
        title,
        link: e.entry.link.clone(),
        published_at_iso: published_at.to_rfc3339(),
        published_relative: format_relative_time_compact(Some(published_at)),
        is_read: e.entry.read_at.is_some(),
        is_starred: e.entry.starred_at.is_some(),
        summary_status,
    }
}

/// Fetch a page of entries and map them to `EntryRowView`s.
/// Returns `(rows, next_cursor)` where `next_cursor` is `Some(offset + page_size)`
/// when more results exist beyond this page.
pub(crate) async fn build_entries_page(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    offset: i64,
) -> (Vec<EntryRowView>, Option<i64>) {
    let result = state
        .db
        .read_user(move |conn| {
            let rows = entry::list_by_user(conn, user_id, &filter, sort, page_size + 1, offset)?;
            let ids: Vec<i64> = rows.iter().map(|e| e.entry.id).collect();
            let statuses = entry_summary::get_statuses_for_entries(conn, user_id, &ids)?;
            Ok::<_, AppError>((rows, statuses))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_else(|| (Vec::new(), HashMap::new()));
    let (rows, statuses) = result;

    let next_cursor = if rows.len() as i64 > page_size {
        Some(offset + page_size)
    } else {
        None
    };
    let views = rows
        .iter()
        .take(page_size as usize)
        .map(|e| row_view_from(e, statuses.get(&e.entry.id).copied()))
        .collect();
    (views, next_cursor)
}

/// Query parameters for the Load-More fragment dispatch on the 5 entries pages.
/// When `fragment == Some(1)`, the handler returns an `EntriesFragmentTemplate`
/// (prefix-rerender from offset 0 to `after + page_size`) instead of the full page.
#[derive(serde::Deserialize, Default)]
pub struct EntriesQuery {
    pub fragment: Option<u8>,
    pub after: Option<i64>,
}

/// Fragment template for the Load-More response.
/// Wraps a re-rendered `data-entries-list` div in a multi-target `<template>` block
/// so `app.js` swap() replaces `[data-entries-list]` in-place.
#[derive(Template)]
#[template(path = "_entries_fragment.html")]
pub(crate) struct EntriesFragmentTemplate {
    pub entries: Vec<EntryRowView>,
    pub next_cursor: Option<i64>,
    pub path: &'static str,
}

impl IntoResponse for EntriesFragmentTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Compact relative-time formatter for the entry list. Returns short
/// forms like `now` / `46m` / `3h` / `2d` / `5mo` / `1y`. Long form
/// (`format_relative_time`) is kept for places with more breathing
/// room (reading pane, feeds page, admin tables).
pub fn format_relative_time_compact(dt: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(dt) = dt else {
        return "—".to_string();
    };
    let duration = chrono::Utc::now().signed_duration_since(dt);
    let seconds = duration.num_seconds();
    if seconds < 60 {
        "now".to_string()
    } else if seconds < 3600 {
        format!("{}m", duration.num_minutes())
    } else if seconds < 86400 {
        format!("{}h", duration.num_hours())
    } else if seconds < 2_592_000 {
        format!("{}d", duration.num_days())
    } else if seconds < 31_536_000 {
        format!("{}mo", duration.num_days() / 30)
    } else {
        format!("{}y", duration.num_days() / 365)
    }
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

/// Serves `/` (unread) rendered fully server-side. Unread entries are fetched
/// from the DB, mapped to `EntryRowView`s, and rendered via `_entries_layout.html`
/// which includes `_entry_row.html` per row. The reading pane is an empty
/// placeholder until the user selects an entry (swap via `app.js`).
///
/// When `?fragment=1&after=<offset>` is present the handler returns a
/// `EntriesFragmentTemplate` (prefix-rerender from 0 to `after + page_size`).
pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter {
        unread_only: true,
        ..Default::default()
    };

    if query.fragment == Some(1) {
        let after = query.after.unwrap_or(0).max(0);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            after,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/",
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        0,
    )
    .await;

    (
        flash,
        UnreadTemplate {
            title: "Unread",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane: None,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "unread",
                description: None,
                empty_message: "No unread entries — nice work.",
                path: "/".to_string(),
                show_tab_bar: false,
                show_mark_as_read: true,
            },
        },
    )
        .into_response()
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

/// Query parameters for `/feeds`. Drives server-side filter / sort so the
/// URL stays the stable source of truth.
#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<String>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}

/// Serves `/feeds` rendered fully server-side. Feed rows, category options,
/// and filter pills are computed from the DB. Mutating actions go through
/// the form-action endpoints under `/feeds/*` (PR-8 T1). Re-fetch icons via
/// the `<img src="/api/feeds/{id}/icon">` endpoint, which stays alive.
pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<FeedsQuery>,
) -> (Flash, FeedsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    let (mut rows, categories, total_feed_count) = state
        .db
        .read_user(move |conn| {
            let cats = category::list_by_user(conn, user_id).unwrap_or_default();
            let all_feeds = feed::list_by_user(conn, user_id).unwrap_or_default();
            let unread_map = entry::count_unread_by_feed(conn, user_id).unwrap_or_default();

            let cat_map: std::collections::HashMap<i64, String> = cats
                .iter()
                .map(|cat| (cat.id, cat.name.clone()))
                .collect();
            let mut count_by_cat: std::collections::HashMap<i64, i64> =
                std::collections::HashMap::new();
            for f in &all_feeds {
                *count_by_cat.entry(f.category_id).or_insert(0) += 1;
            }

            let total_feed_count = all_feeds.len() as i64;

            let row_views: Vec<FeedRowView> = all_feeds
                .into_iter()
                .map(|f| {
                    let has_icon: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                            [f.id],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
                    let (updated_rel, updated_dt) = if f.feed_updated_at.is_some() {
                        format_relative_time(f.feed_updated_at)
                    } else if f
                        .fetched_at
                        .map(|ft| (chrono::Utc::now() - ft).num_days() <= 30)
                        .unwrap_or(false)
                    {
                        ("No date info".to_string(), String::new())
                    } else {
                        ("Never".to_string(), String::new())
                    };
                    let (freshness_class, freshness_key) =
                        compute_freshness(f.feed_updated_at, f.fetched_at);
                    FeedRowView {
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
                        fetched_at_relative: fetched_rel,
                        fetched_at_datetime: fetched_dt,
                        feed_updated_at_relative: updated_rel,
                        feed_updated_at_datetime: updated_dt,
                        freshness_class,
                        freshness_key,
                    }
                })
                .collect();

            let cat_options: Vec<FeedCategoryOption> = cats
                .into_iter()
                .map(|cat| FeedCategoryOption {
                    feed_count: count_by_cat.get(&cat.id).copied().unwrap_or(0),
                    id: cat.id,
                    name: cat.name,
                })
                .collect();

            Ok::<_, AppError>((row_views, cat_options, total_feed_count))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    let active_filter_raw = query.filter.as_deref().unwrap_or("all").to_string();
    let active_sort = query.sort.as_deref().unwrap_or("title").to_string();
    let active_category = query.category.as_deref().and_then(|s| {
        if s.is_empty() {
            None
        } else {
            s.parse::<i64>().ok()
        }
    });

    if let Some(cid) = active_category {
        rows.retain(|r| r.category_id == cid);
    }
    match active_filter_raw.as_str() {
        "errors" => rows.retain(|r| r.fetch_error.is_some()),
        "stale" => rows.retain(|r| r.freshness_key == "stale"),
        _ => {}
    }
    match active_sort.as_str() {
        "unread" => rows.sort_by(|a, b| b.unread_count.cmp(&a.unread_count)),
        "category" => rows.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        _ => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
    let active_filter = match active_filter_raw.as_str() {
        "errors" | "stale" | "all" => active_filter_raw,
        _ => "all".to_string(),
    };

    let cat_param = active_category
        .map(|c| format!("category={c}&"))
        .unwrap_or_default();
    let filter_links = vec![
        FeedFilterLink {
            label: "All",
            href: format!("/feeds?{}sort={}&filter=all", cat_param, active_sort),
            active: active_filter == "all",
        },
        FeedFilterLink {
            label: "Errors",
            href: format!("/feeds?{}sort={}&filter=errors", cat_param, active_sort),
            active: active_filter == "errors",
        },
        FeedFilterLink {
            label: "Stale",
            href: format!("/feeds?{}sort={}&filter=stale", cat_param, active_sort),
            active: active_filter == "stale",
        },
    ];

    (
        flash,
        FeedsTemplate {
            title: "Feeds",
            git_version: crate::GIT_VERSION,
            layout,
            feeds: rows,
            categories,
            total_feed_count,
            active_filter,
            active_sort,
            active_category_id: active_category.unwrap_or(-1),
            filter_links,
        },
    )
}

/// Serves `/feeds/{id}/edit` rendered fully server-side. The form posts to
/// `POST /feeds/{id}/edit` (T1 form-action endpoint). A second form on the
/// page posts to `/feeds/{id}/fetch-metadata` to re-discover and persist
/// title/description/site_url.
pub async fn feed_edit_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Path(id): Path<i64>,
) -> AppResult<(Flash, FeedEditTemplate)> {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    let (feed_view, cats) = state
        .db
        .read_user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;
            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;
            let cats = category::list_by_user(conn, user_id)?;
            Ok::<_, AppError>((
                FeedEditView {
                    id: f.id,
                    url: f.url,
                    title: f.title.unwrap_or_default(),
                    description: f.description.unwrap_or_default(),
                    site_url: f.site_url.unwrap_or_default(),
                    category_id: f.category_id,
                    custom_user_agent: f.custom_user_agent.unwrap_or_default(),
                    http2_disabled: f.http2_disabled,
                    custom_referrer: f.custom_referrer.unwrap_or_default(),
                },
                cats.into_iter()
                    .map(|c| FeedCategoryOption {
                        id: c.id,
                        name: c.name,
                        feed_count: 0,
                    })
                    .collect::<Vec<_>>(),
            ))
        })
        .await??;

    Ok((
        flash,
        FeedEditTemplate {
            title: "Edit Feed",
            git_version: crate::GIT_VERSION,
            layout,
            feed: feed_view,
            categories: cats,
        },
    ))
}

/// Serves `/feeds/import` rendered fully server-side. Multipart form posts to
/// `POST /feeds/import` (T1 form-action endpoint).
pub async fn feeds_import_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, FeedsImportTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    (
        flash,
        FeedsImportTemplate {
            title: "Import OPML",
            git_version: crate::GIT_VERSION,
            layout,
        },
    )
}

/// Serves the CSR shell for `/entries` (all). The list itself is loaded by
/// `<rdrs-entries-page>` (mode `all`) from `/reader/api/0/stream/contents`.
/// Serves `/entries` rendered fully server-side. All entries (no filter) are
/// fetched and rendered via `_entries_layout.html`. The reading pane is an empty
/// placeholder until the user selects an entry.
///
/// When `?fragment=1&after=<offset>` is present the handler returns a
/// `EntriesFragmentTemplate` (prefix-rerender from 0 to `after + page_size`).
pub async fn entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter::default();

    if query.fragment == Some(1) {
        let after = query.after.unwrap_or(0).max(0);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            after,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries",
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        0,
    )
    .await;

    (
        flash,
        EntriesTemplate {
            title: "Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane: None,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "all",
                description: None,
                empty_message: "No entries.",
                path: "/entries".to_string(),
                show_tab_bar: true,
                show_mark_as_read: true,
            },
        },
    )
        .into_response()
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

/// Serves `/entries/read` rendered fully server-side. Read-only entries are
/// fetched and rendered via `_entries_layout.html`.
///
/// When `?fragment=1&after=<offset>` is present the handler returns a
/// `EntriesFragmentTemplate` (prefix-rerender from 0 to `after + page_size`).
pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter {
        read_only: true,
        ..Default::default()
    };

    if query.fragment == Some(1) {
        let after = query.after.unwrap_or(0).max(0);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            after,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/read",
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        0,
    )
    .await;

    (
        flash,
        ReadEntriesTemplate {
            title: "Read Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane: None,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "read",
                description: None,
                empty_message: "No read entries.",
                path: "/entries/read".to_string(),
                show_tab_bar: true,
                show_mark_as_read: false,
            },
        },
    )
        .into_response()
}

/// Serves `/entries/starred` rendered fully server-side. Starred entries are
/// fetched and rendered via `_entries_layout.html`.
///
/// When `?fragment=1&after=<offset>` is present the handler returns a
/// `EntriesFragmentTemplate` (prefix-rerender from 0 to `after + page_size`).
pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter {
        starred_only: true,
        ..Default::default()
    };

    if query.fragment == Some(1) {
        let after = query.after.unwrap_or(0).max(0);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            after,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/starred",
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        0,
    )
    .await;

    (
        flash,
        StarredEntriesTemplate {
            title: "Starred Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane: None,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "starred",
                description: None,
                empty_message: "No starred entries.",
                path: "/entries/starred".to_string(),
                show_tab_bar: true,
                show_mark_as_read: false,
            },
        },
    )
        .into_response()
}

/// Serves `/entries/summarized` rendered fully server-side. Entries that have
/// an associated summary are fetched and rendered via `_entries_layout.html`.
///
/// When `?fragment=1&after=<offset>` is present the handler returns a
/// `EntriesFragmentTemplate` (prefix-rerender from 0 to `after + page_size`).
pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;
    let filter = entry::EntryFilter {
        has_summary: Some(true),
        ..Default::default()
    };

    if query.fragment == Some(1) {
        let after = query.after.unwrap_or(0).max(0);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            after,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/summarized",
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        0,
    )
    .await;

    (
        flash,
        SummarizedEntriesTemplate {
            title: "Summarized Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane: None,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "summarized",
                description: None,
                empty_message: "No summarized entries.",
                path: "/entries/summarized".to_string(),
                show_tab_bar: true,
                show_mark_as_read: false,
            },
        },
    )
        .into_response()
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

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

/// Serves `/search` rendered fully server-side. With no `?q=`, shows an empty
/// search form. With a non-empty `q`, runs `entry::list_by_user` filtered on
/// the search term (LIKE `%q%` over title + content, case-insensitive),
/// limited to 50 results sorted by published_at DESC. No pagination —
/// reading-pane integration arrives in PR-10's swap helper.
pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<SearchQuery>,
) -> (Flash, SearchTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let q = query.q.unwrap_or_default().trim().to_string();
    let user_id = auth_user.user.id;

    let results = if q.is_empty() {
        Vec::new()
    } else {
        let q_for_filter = q.clone();
        state
            .db
            .read_user(move |conn| {
                let filter = entry::EntryFilter {
                    search: Some(q_for_filter.clone()),
                    ..Default::default()
                };
                // SQL search hits inside HTML attributes (`<a href="…bitcoin…">`),
                // so a fraction of rows are "phantom matches" with no visible
                // <mark>. Walk the result set in OFFSET-paged batches and keep
                // only rows where the query appears in the visible title or
                // stripped snippet, until we hit the display cap, exhaust the
                // upstream result set, or trip a safety bound.
                const TARGET: usize = 50;
                const BATCH: i64 = 100;
                const MAX_ITERATIONS: usize = 5;
                const MAX_SCANNED: usize = 1000;
                let q_lower = q_for_filter.to_ascii_lowercase();
                let mut visible: Vec<SearchResultView> = Vec::with_capacity(TARGET);
                let mut offset: i64 = 0;
                let mut scanned: usize = 0;

                for _ in 0..MAX_ITERATIONS {
                    if visible.len() >= TARGET || scanned >= MAX_SCANNED {
                        break;
                    }
                    let batch = entry::list_by_user(
                        conn,
                        user_id,
                        &filter,
                        entry::EntrySortOrder::PublishedAt,
                        BATCH,
                        offset,
                    )?;
                    let batch_len = batch.len();
                    scanned += batch_len;
                    offset += batch_len as i64;

                    for e in batch {
                        let title = e
                            .entry
                            .title
                            .clone()
                            .unwrap_or_else(|| "(no title)".to_string());
                        let snippet = build_snippet(
                            e.entry.content.as_deref().or(e.entry.summary.as_deref()),
                            &q_for_filter,
                            200,
                        );
                        let visible_hit = title.to_ascii_lowercase().contains(&q_lower)
                            || snippet.to_ascii_lowercase().contains(&q_lower);
                        if !visible_hit {
                            continue;
                        }
                        visible.push(SearchResultView {
                            entry_id: e.entry.id,
                            title_html: highlight_html(&title, &q_for_filter),
                            feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                            published_relative: format_relative_time(e.entry.published_at).0,
                            snippet_html: highlight_html(&snippet, &q_for_filter),
                        });
                        if visible.len() >= TARGET {
                            break;
                        }
                    }

                    if batch_len < BATCH as usize {
                        break; // upstream exhausted
                    }
                }

                Ok::<_, AppError>(visible)
            })
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    };

    (
        flash,
        SearchTemplate {
            title: "Search",
            git_version: crate::GIT_VERSION,
            layout,
            q,
            results,
        },
    )
}

/// Strip HTML tags (including `<script>` / `<style>` bodies) and collapse
/// whitespace into a single line of plain text.
fn strip_to_plain_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut last_space = true;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(end_tag) = skip_until {
            if let Some(pos) = raw[i..].to_ascii_lowercase().find(end_tag) {
                i += pos + end_tag.len();
                skip_until = None;
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                continue;
            } else {
                break;
            }
        }
        let ch = bytes[i] as char;
        match ch {
            '<' => {
                let lower = raw[i..].to_ascii_lowercase();
                if lower.starts_with("<script") {
                    skip_until = Some("</script>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<style") {
                    skip_until = Some("</style>");
                    i += 1;
                    continue;
                }
                if lower.starts_with("<!--") {
                    if let Some(pos) = raw[i + 4..].find("-->") {
                        i += 4 + pos + 3;
                        if !last_space {
                            out.push(' ');
                            last_space = true;
                        }
                        continue;
                    } else {
                        break;
                    }
                }
                in_tag = true;
                i += 1;
            }
            '>' if in_tag => {
                in_tag = false;
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ if in_tag => {
                i += 1;
            }
            c if c.is_whitespace() => {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
                i += 1;
            }
            _ => {
                let ch_len = raw[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                out.push_str(&raw[i..i + ch_len]);
                last_space = false;
                i += ch_len;
            }
        }
    }
    out.trim().to_string()
}

/// Build a query-aware snippet: returns a `max_chars`-wide window centered on
/// the first case-insensitive match of `query` in the plain-text content, with
/// `…` prefix/suffix where the window doesn't reach the original boundaries.
/// Falls back to the leading `max_chars` characters if no match is found
/// (or if `query` is empty).
fn build_snippet(html: Option<&str>, query: &str, max_chars: usize) -> String {
    let raw = match html {
        Some(s) if !s.is_empty() => s,
        _ => return String::new(),
    };
    let plain = strip_to_plain_text(raw);
    let total_chars = plain.chars().count();
    if total_chars <= max_chars {
        return plain;
    }

    // Try to center on the first match (ASCII-case-insensitive).
    let q = query.trim();
    if !q.is_empty() {
        let plain_lower = plain.to_ascii_lowercase();
        let q_lower = q.to_ascii_lowercase();
        if let Some(byte_pos) = plain_lower.find(&q_lower) {
            // Convert byte position → char index.
            let match_char_idx = plain[..byte_pos].chars().count();
            let context_before = max_chars / 3;
            let start_char = match_char_idx.saturating_sub(context_before);
            let end_char = (start_char + max_chars).min(total_chars);
            // Recompute start to fill the window if we hit the tail.
            let start_char = end_char.saturating_sub(max_chars);

            let window: String = plain
                .chars()
                .skip(start_char)
                .take(end_char - start_char)
                .collect();
            let prefix = if start_char > 0 { "…" } else { "" };
            let suffix = if end_char < total_chars { "…" } else { "" };
            return format!("{}{}{}", prefix, window.trim(), suffix);
        }
    }

    // Fallback: leading window.
    let truncated: String = plain.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

/// Wrap case-insensitive (ASCII-only — matches the SQLite LIKE COLLATE NOCASE
/// behavior of the search query) matches of `query` in `<mark>` tags. Returns
/// HTML with the non-match parts and the matched text both escaped, plus the
/// `<mark>...</mark>` wrappers around hits. Use with `|safe` in templates.
fn highlight_html(text: &str, query: &str) -> String {
    if query.is_empty() {
        return html_escape_minimal(text);
    }
    let q_lower = query.to_ascii_lowercase();
    let q_bytes = q_lower.len();
    if q_bytes == 0 {
        return html_escape_minimal(text);
    }
    let t_lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len() + 16);
    let mut last = 0;
    let mut start = 0;
    while start <= t_lower.len() {
        match t_lower[start..].find(&q_lower) {
            Some(rel) => {
                let abs = start + rel;
                out.push_str(&html_escape_minimal(&text[last..abs]));
                out.push_str("<mark>");
                out.push_str(&html_escape_minimal(&text[abs..abs + q_bytes]));
                out.push_str("</mark>");
                last = abs + q_bytes;
                start = last;
            }
            None => break,
        }
    }
    out.push_str(&html_escape_minimal(&text[last..]));
    out
}

fn html_escape_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `GET /feeds/{id}/entries` — SSR list of entries from a single feed.
/// Supports the `?fragment=1&after=N` Load-More overload like the other
/// entries-family pages.
pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntriesQuery>,
    flash: Flash,
) -> Result<Response, AppError> {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;

    let feed_title = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            Ok::<_, AppError>(f.title.unwrap_or_else(|| "(untitled feed)".to_string()))
        })
        .await??;

    let filter = entry::EntryFilter {
        feed_id: Some(id),
        ..Default::default()
    };
    let offset = query.after.unwrap_or(0);

    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        offset,
    )
    .await;

    let path = format!("/feeds/{}/entries", id);

    if query.fragment == Some(1) {
        let fragment = EntriesFragmentTemplate {
            entries,
            next_cursor,
            path: Box::leak(path.into_boxed_str()),
        };
        return Ok((flash, fragment).into_response());
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let template = FeedEntriesTemplate {
        title: feed_title,
        git_version: crate::GIT_VERSION,
        layout,
        entries,
        reading_pane: None,
        next_cursor,
        entries_layout: EntriesLayoutContext {
            active: "",
            description: None,
            empty_message: "No entries in this feed.",
            path,
            show_tab_bar: false,
            show_mark_as_read: false,
        },
    };

    Ok((flash, template).into_response())
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

/// One row of the SSR `/categories` table.
pub struct CategoryRowView {
    pub id: i64,
    pub name: String,
    pub feed_count: i64,
}

/// Per-route template for `/categories`.
#[derive(Template)]
#[template(path = "categories.html")]
pub struct CategoriesTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub categories: Vec<CategoryRowView>,
}

impl IntoResponse for CategoriesTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// One row of the SSR `/feeds` table.
pub struct FeedRowView {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub category_id: i64,
    pub category_name: String,
    pub has_icon: bool,
    pub fetch_error: Option<String>,
    pub unread_count: i64,
    pub fetched_at_relative: String,
    pub fetched_at_datetime: String,
    pub feed_updated_at_relative: String,
    pub feed_updated_at_datetime: String,
    pub freshness_class: String,
    pub freshness_key: String,
}

/// Category option for selects + sidebar counts on `/feeds`.
pub struct FeedCategoryOption {
    pub id: i64,
    pub name: String,
    pub feed_count: i64,
}

/// Filter pill (All / Errors / Stale) on the `/feeds` filter bar.
pub struct FeedFilterLink {
    pub label: &'static str,
    pub href: String,
    pub active: bool,
}

/// Per-route template for `/feeds`.
#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub feeds: Vec<FeedRowView>,
    pub categories: Vec<FeedCategoryOption>,
    pub total_feed_count: i64,
    pub active_filter: String,
    pub active_sort: String,
    /// `-1` sentinel for "no category filter" — keeps Askama integer
    /// comparisons straightforward in the filter dropdown.
    pub active_category_id: i64,
    pub filter_links: Vec<FeedFilterLink>,
}

impl IntoResponse for FeedsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Editable view of a single feed for `/feeds/{id}/edit`.
pub struct FeedEditView {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub site_url: String,
    pub category_id: i64,
    pub custom_user_agent: String,
    pub http2_disabled: bool,
    pub custom_referrer: String,
}

/// Per-route template for `/feeds/{id}/edit`.
#[derive(Template)]
#[template(path = "feed_edit.html")]
pub struct FeedEditTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub feed: FeedEditView,
    pub categories: Vec<FeedCategoryOption>,
}

impl IntoResponse for FeedEditTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/feeds/import`.
#[derive(Template)]
#[template(path = "feeds_import.html")]
pub struct FeedsImportTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
}

impl IntoResponse for FeedsImportTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Per-route template for `/` (unread). Extends `_entries_layout.html`.
/// `git_version` is duplicated at the leaf level because `base.html`
/// references the bare `{{ git_version }}` outside the blocks owned by
/// `app_layout.html` (Askama 0.15 quirk — see other templates for the pattern).
#[derive(Template)]
#[template(path = "unread.html")]
pub struct UnreadTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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
    pub title: String,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<i64>,
    pub entries_layout: EntriesLayoutContext,
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

/// One row of the SSR `/search` results list. `title_html` and `snippet_html`
/// are pre-escaped strings with `<mark>` tags wrapping case-insensitive
/// matches of the query — render them with the `|safe` Askama filter.
pub struct SearchResultView {
    pub entry_id: i64,
    pub title_html: String,
    pub feed_title: String,
    pub published_relative: String,
    pub snippet_html: String,
}

/// Per-route template for `/search`.
#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub q: String,
    pub results: Vec<SearchResultView>,
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

/// Serves `/categories` rendered fully server-side. The category list is
/// loaded directly from the DB (`category::list_by_user` + per-category
/// feed counts derived from `feed::list_by_user`). Each row carries its
/// own POST forms targeting `/categories/{id}/rename` and
/// `/categories/{id}/delete` (PR-7 T1 form-action endpoints), and a
/// top-of-page POST form targets `/categories` for creation.
pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    let categories = state
        .db
        .read_user(move |conn| {
            let cats = crate::models::category::list_by_user(conn, user_id)?;
            let feeds = crate::models::feed::list_by_user(conn, user_id)?;
            let mut counts: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
            for f in &feeds {
                *counts.entry(f.category_id).or_insert(0) += 1;
            }
            Ok::<_, AppError>(
                cats.into_iter()
                    .map(|c| CategoryRowView {
                        feed_count: *counts.get(&c.id).unwrap_or(&0),
                        id: c.id,
                        name: c.name,
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    (
        flash,
        CategoriesTemplate {
            title: "Categories",
            git_version: crate::GIT_VERSION,
            layout,
            categories,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{build_snippet, highlight_html};

    #[test]
    fn highlight_wraps_simple_match() {
        let out = highlight_html("Bitcoin Price Soars", "bitcoin");
        assert_eq!(out, "<mark>Bitcoin</mark> Price Soars");
    }

    #[test]
    fn highlight_wraps_multiple_matches_case_insensitive() {
        let out = highlight_html("BITCOIN bitcoin Bitcoin", "bitcoin");
        assert_eq!(
            out,
            "<mark>BITCOIN</mark> <mark>bitcoin</mark> <mark>Bitcoin</mark>"
        );
    }

    #[test]
    fn highlight_no_match_returns_escaped_only() {
        let out = highlight_html("Ethereum news", "bitcoin");
        assert_eq!(out, "Ethereum news");
    }

    #[test]
    fn highlight_escapes_html_special_chars() {
        let out = highlight_html("<b>bitcoin</b>", "bitcoin");
        assert_eq!(out, "&lt;b&gt;<mark>bitcoin</mark>&lt;/b&gt;");
    }

    #[test]
    fn highlight_empty_query_returns_escaped() {
        let out = highlight_html("Hi <world>", "");
        assert_eq!(out, "Hi &lt;world&gt;");
    }

    #[test]
    fn build_snippet_strips_script_and_style_bodies() {
        let out = build_snippet(
            Some("<p>hello</p><script>alert('x')</script><style>.a{}</style> world"),
            "",
            200,
        );
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
        assert!(!out.contains("alert"));
        assert!(!out.contains(".a{"));
    }

    #[test]
    fn build_snippet_centers_window_on_match() {
        // Match buried far past the leading 200 chars.
        let lead = "lorem ipsum ".repeat(40);
        let html = format!("<p>{}Bitcoin price soars today across exchanges.</p>", lead);
        let out = build_snippet(Some(&html), "bitcoin", 80);
        assert!(
            out.contains("Bitcoin"),
            "snippet should include match: {out}"
        );
        assert!(out.starts_with('…'), "should ellipsis-prefix: {out}");
    }

    #[test]
    fn build_snippet_falls_back_to_lead_when_no_match() {
        let html = "<p>Ethereum dominated headlines this week as the merge approached.</p>";
        let out = build_snippet(Some(html), "bitcoin", 30);
        assert!(out.starts_with("Ethereum"));
        assert!(out.ends_with('…'));
    }

    #[test]
    fn build_snippet_strips_html_comments() {
        let out = build_snippet(Some("hello <!-- secret note --> world"), "", 200);
        assert_eq!(out, "hello world");
    }
}
