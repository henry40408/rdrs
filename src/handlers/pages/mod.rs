use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};

use std::collections::HashMap;

use crate::error::AppError;
use crate::middleware::auth::{LoginRedirect, PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage};
use crate::models::user_settings;
use crate::models::SummaryStatus;
use crate::models::{category, entry, entry_summary, feed};
use crate::AppState;

mod script_json;
mod search_text;
mod time_format;

use script_json::{flash_bootstrap_json, serialize_sidebar_for_script};
use search_text::{build_snippet, highlight_html};
pub use time_format::{compute_freshness, format_relative_time, format_relative_time_compact};

// ============================================================================
// Entries-family shared view structs (PR-10)
// ============================================================================

/// Uppercased first character of a feed title, for the favicon letter-chip
/// fallback shown when a feed has no icon. Returns "?" for an empty title.
pub(crate) fn feed_initial(feed_title: &str) -> String {
    feed_title
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

/// Stable index 0..6 into the favicon fallback colour palette, derived from
/// the feed id so the same feed always gets the same colour.
pub(crate) fn feed_color_index(feed_id: i64) -> u8 {
    feed_id.rem_euclid(6) as u8
}

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

    /// Uppercased first character of the feed title, for the favicon
    /// letter-chip fallback shown when a feed has no icon. Returns "?" for
    /// an empty title.
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    /// Stable index 0..6 into the favicon fallback colour palette, derived
    /// from the feed id so the same feed always gets the same colour.
    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
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
    pub feed_id: i64,
    pub feed_has_icon: bool,
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
    /// `true` after `POST /entries/{id}/fetch-full-content` succeeds —
    /// the `content_html` field then holds the externally-fetched
    /// article body instead of the feed-supplied content. The reading
    /// pane swaps "Fetch Full Content" for a "Show Original" link in
    /// this case so the user can revert.
    pub is_full_content: bool,
}

impl ReadingPaneView {
    /// Uppercased first character of the feed title for the favicon
    /// letter-chip fallback (mirrors `EntryRowView::feed_initial`).
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    /// Stable favicon-palette index derived from the feed id (mirrors
    /// `EntryRowView::feed_color_index`).
    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
    }
}

/// One segment of a breadcrumb trail rendered above the page `<h1>`. `href =
/// None` marks the current page (rendered as plain text, no link).
#[derive(Debug, Clone)]
pub struct BreadcrumbItem {
    pub label: String,
    pub href: Option<String>,
}

/// One tab in the status-filter bar rendered on feed/category entries
/// pages (`?status=unread|read|starred`). The keyboard `1`/`2`/`3`/`4`
/// shortcuts navigate to the corresponding tab by position.
#[derive(Debug, Clone)]
pub struct FilterTab {
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// Render-time snapshot boundary for unread-navigation, in the same UTC
/// `YYYY-MM-DD HH:MM:SS` format `datetime('now')` writes into
/// `entry.read_at`. Emitted as `data-snapshot-at` on `[data-entries-list]`;
/// the client echoes it back as `read_after` on the neighbors API.
pub(crate) fn snapshot_now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Layout context shared by all entries-family pages (`_entries_layout.html`).
#[derive(Debug, Clone)]
pub struct EntriesLayoutContext {
    pub active: &'static str,
    pub description: Option<String>,
    /// Empty-state heading (Tier-1 `.empty-state-title`).
    pub empty_title: &'static str,
    /// Empty-state subtext (Tier-1 `.empty-state-text`).
    pub empty_detail: &'static str,
    pub path: String,
    /// Render the All/Read/Starred/Summarized tab bar above the list. True
    /// for the 4 entries-tabs (`active = "all" | "read" | "starred" |
    /// "summarized"`), false for `/` (unread) since unread is not a tab.
    pub show_tab_bar: bool,
    /// When `Some(stream_id)`, render the "Mark as Read..." dropdown
    /// above the list with the given GReader stream as its scope. The
    /// `<select>` carries `data-mark-read-scope` so `app.js` picks up
    /// the scope dynamically. Stream IDs follow GReader format:
    /// `user/-/state/com.google/reading-list` for global bulk,
    /// `feed/<feed_url>` for per-feed, `user/-/label/<category_name>`
    /// for per-category. `None` means the dropdown is not rendered.
    pub mark_as_read_scope: Option<String>,
    /// Breadcrumb trail rendered above the page title. Empty for the routes
    /// that don't need one (all 5 PR-10 entries-family pages).
    pub breadcrumb_items: Vec<BreadcrumbItem>,
    /// When `Some(feed_id)`, render the feed's favicon next to the page
    /// title via `/api/feeds/{id}/icon`. Only `/feeds/{id}/entries` uses
    /// this — the rest pass `None`.
    pub header_feed_icon_id: Option<i64>,
    /// When `Some(category_id)`, sets `<rdrs-sidebar active-category-id="…">`
    /// so the sidebar highlights the active category. Used by the feed +
    /// category entries pages; `None` elsewhere.
    pub active_category_id: Option<i64>,
    /// Optional status-filter tab bar (All / Unread / Read / Starred) for
    /// feed + category pages. `None` on the 5 PR-10 routes — they use
    /// path-based modes via the `show_tab_bar` flag instead.
    pub filter_tabs: Option<Vec<FilterTab>>,
    /// Forwarded into the Load-More form so subsequent Load-More fetches
    /// preserve the `?status=` query. Mirrors the same field on
    /// `EntriesFragmentTemplate`.
    pub status_filter: Option<String>,
    /// When `true`, render a "Mark Above as Read" button at the bottom of
    /// the list. Clicking it marks every entry currently in the DOM as
    /// read (loaded + Load-More-appended rows; unloaded entries are
    /// untouched). Only the feed + category entries pages set this true.
    pub show_mark_above: bool,
    /// When `true` and the list is empty, the shared layout renders the
    /// getting-started onboarding block (welcome + 3 steps + "Add your first
    /// feed" / "Import OPML" CTAs) instead of the plain empty-state text. Set
    /// only by the landing page (`/`) when the account has no feeds; every
    /// other route leaves it `false`.
    pub onboarding: bool,
    /// UTC instant captured when the page was rendered; see `snapshot_now()`.
    pub snapshot_at: String,
}

/// Map an `EntryWithFeed` (+ optional summary status) to an `EntryRowView`.
pub(crate) fn row_view_from(
    e: &entry::EntryWithFeed,
    summary_status: Option<SummaryStatus>,
) -> EntryRowView {
    let title = e
        .entry
        .title
        .as_deref()
        .map(crate::services::decode_html_entities)
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
/// Returns `(rows, next_cursor)` where `next_cursor` is `Some(<sort_ts>|<id>)`
/// (an opaque composite cursor token) when more results exist beyond this page.
pub(crate) async fn build_entries_page(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> (Vec<EntryRowView>, Option<String>) {
    let result = state
        .db
        .read_user(move |conn| {
            let params = entry::ContinuationParams {
                oldest_first: false,
                limit: page_size + 1,
                continuation: cursor,
                ot: None,
                nt: None,
                sort_order: sort,
            };
            let rows = entry::list_by_user_with_continuation(conn, user_id, &filter, &params)?;
            let kept_len = rows.len().min(page_size as usize);
            // Derive the next cursor from the last *kept* row when an extra
            // (sentinel) row was returned. Mirrors greader/item.rs.
            // Next cursor comes from the last KEPT row (the sentinel row beyond
            // page_size is dropped). `.take(kept_len).next_back()` avoids an
            // index subtraction and is None-safe when there are no rows.
            let next = if rows.len() as i64 > page_size {
                match rows.iter().take(kept_len).next_back() {
                    Some(e) => entry::fetch_sort_ts(conn, e.entry.id, sort)?
                        .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id)),
                    None => None,
                }
            } else {
                None
            };
            let ids: Vec<i64> = rows.iter().take(kept_len).map(|e| e.entry.id).collect();
            let statuses = entry_summary::get_statuses_for_entries(conn, user_id, &ids)?;
            Ok::<_, AppError>((rows, kept_len, next, statuses))
        })
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_else(|| (Vec::new(), 0, None, HashMap::new()));
    let (rows, kept_len, next_cursor, statuses) = result;
    let views = rows
        .iter()
        .take(kept_len)
        .map(|e| row_view_from(e, statuses.get(&e.entry.id).copied()))
        .collect();
    (views, next_cursor)
}

/// Query parameters for the Load-More fragment dispatch on the 5 entries pages.
/// When `fragment == Some(1)`, the handler returns an `EntriesFragmentTemplate`
/// using the opaque cursor token in `after` to continue from where the last page left off.
#[derive(serde::Deserialize, Default)]
pub struct EntriesQuery {
    pub fragment: Option<u8>,
    pub after: Option<String>,
    /// Status filter: `unread` / `read` / `starred`. Only meaningful on the
    /// feed + category entries pages (the 5 PR-10 routes have their own
    /// path-based modes). Any other value (or absence) is treated as
    /// "no filter" (show all).
    pub status: Option<String>,
    /// Deep-link target: when present, the list handler pre-populates the
    /// reading pane with this entry. Honored by every list page (unread,
    /// /entries, read/starred/summarized, /feeds/{id}/entries,
    /// /categories/{id}/entries). Read-only: does not mark the entry read
    /// (use POST /entries/{id}/read for that). Silently ignored when the
    /// entry doesn't exist or belongs to another user.
    pub entry: Option<i64>,
}

/// Best-effort builder for the `?entry={id}` deep-link reading pane.
///
/// Looks up the entry, verifies ownership (`find_by_id_for_user` enforces
/// the join on `user_id`), reads the save / Kagi flags, and renders a
/// `ReadingPaneView`. Any failure (entry missing, wrong owner, DB / sanitize
/// hiccup) returns `None` so the list page still renders with an empty
/// pane — mirroring the page's normal "no entry selected" state.
///
/// Read-only by design: deep links do NOT mark the entry as read. The
/// canonical mark-as-read path remains the entry-row click, which fetches
/// `GET /entries/{id}/fragment` and runs the write transaction inside
/// `entry_fragment`.
async fn maybe_build_reading_pane(
    state: &AppState,
    user_id: i64,
    entry_id: Option<i64>,
) -> Option<ReadingPaneView> {
    let entry_id = entry_id?;
    let ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await
        .ok()?
        .ok()??;
    let (has_save, has_kagi) = crate::handlers::entries::load_pane_action_flags(state, user_id)
        .await
        .ok()?;
    crate::handlers::entries::build_reading_pane_view(state, user_id, &ewf, has_save, has_kagi)
        .await
        .ok()
}

/// Fragment template for the Load-More response.
/// Wraps a re-rendered `data-entries-list` div in a multi-target `<template>` block
/// so `app.js` swap() replaces `[data-entries-list]` in-place.
#[derive(Template)]
#[template(path = "_entries_fragment.html")]
pub(crate) struct EntriesFragmentTemplate {
    pub entries: Vec<EntryRowView>,
    pub next_cursor: Option<String>,
    pub path: &'static str,
    /// Forwarded into the fragment's Load-More form so subsequent
    /// Load-More fetches keep the current `?status=` filter. `None` for
    /// the 5 PR-10 routes (their filters are path-based, not query).
    pub status_filter: Option<String>,
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
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/",
                status_filter: None,
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
        None,
    )
    .await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    // When the unread list is empty, distinguish a brand-new account with no
    // feeds yet (→ getting-started onboarding) from an inbox where everything
    // has been read (→ "All caught up"). Only query when the list is empty.
    let no_feeds = entries.is_empty()
        && state
            .db
            .read_user(move |conn| feed::count_by_user(conn, user_id).unwrap_or(0))
            .await
            .map(|count| count == 0)
            .unwrap_or(false);

    (
        flash,
        UnreadTemplate {
            title: "Unread",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "unread",
                description: None,
                empty_title: "All caught up",
                empty_detail:
                    "You've read every unread entry — new items land here as your feeds refresh.",
                path: "/".to_string(),
                show_tab_bar: false,
                mark_as_read_scope: Some("user/-/state/com.google/reading-list".to_string()),
                breadcrumb_items: vec![],
                header_feed_icon_id: None,
                active_category_id: None,
                filter_tabs: None,
                status_filter: None,
                show_mark_above: true,
                onboarding: no_feeds,
                snapshot_at: snapshot_now(),
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
                created_at_iso: u.created_at.to_rfc3339(),
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
        retention_read_days,
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
            let retention_read_days =
                user_settings::get_retention_read_days(conn, user_id).unwrap_or(0);
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
                retention_read_days,
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
            0,
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
    let created_at_iso = auth_user.user.created_at.to_rfc3339();
    let session_created_at = auth_user
        .session
        .created_at
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let session_created_at_iso = auth_user.session.created_at.to_rfc3339();
    let username = auth_user.user.username.clone();

    (
        flash,
        UserSettingsTemplate {
            title: "Settings",
            git_version: crate::GIT_VERSION,
            layout,
            username,
            role,
            created_at,
            created_at_iso,
            session_created_at,
            session_created_at_iso,
            public_base_url,
            theme,
            entries_per_page,
            retention_read_days,
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

            let cat_map: std::collections::HashMap<i64, String> =
                cats.iter().map(|cat| (cat.id, cat.name.clone())).collect();
            let mut count_by_cat: std::collections::HashMap<i64, i64> =
                std::collections::HashMap::new();
            for f in &all_feeds {
                *count_by_cat.entry(f.category_id).or_insert(0) += 1;
            }

            let total_feed_count = all_feeds.len() as i64;

            // Resolve which feeds have an icon in one query instead of one per feed.
            let feed_ids: Vec<i64> = all_feeds.iter().map(|f| f.id).collect();
            let feeds_with_icon = crate::models::image::existing_ids(
                conn,
                crate::models::image::ENTITY_FEED,
                &feed_ids,
            )
            .unwrap_or_default();

            let row_views: Vec<FeedRowView> = all_feeds
                .into_iter()
                .map(|f| {
                    let has_icon = feeds_with_icon.contains(&f.id);
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
                        has_icon,
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
        "unread" => rows.sort_by_key(|b| std::cmp::Reverse(b.unread_count)),
        "category" => rows.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        _ => rows.sort_by_key(|a| a.title.to_lowercase()),
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
            active_category_id: active_category,
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
) -> Result<Response, AppError> {
    let user_id = auth_user.user.id;

    let lookup = state
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
        .await?;

    let (feed_view, cats) = match lookup {
        Ok(v) => v,
        Err(AppError::FeedNotFound) => {
            let page = render_not_found(
                &state,
                &auth_user,
                &flash,
                "Feed not found",
                "This feed doesn't exist or you don't have access to it.",
            )
            .await;
            return Ok((flash, page).into_response());
        }
        Err(e) => return Err(e),
    };

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    Ok((
        flash,
        FeedEditTemplate {
            title: "Edit Feed",
            git_version: crate::GIT_VERSION,
            layout,
            feed: feed_view,
            categories: cats,
        },
    )
        .into_response())
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
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries",
                status_filter: None,
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
        None,
    )
    .await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        EntriesTemplate {
            title: "Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "all",
                description: None,
                empty_title: "Nothing to read yet",
                empty_detail: "Subscribe to a few feeds and their entries will gather here.",
                path: "/entries".to_string(),
                show_tab_bar: true,
                mark_as_read_scope: Some("user/-/state/com.google/reading-list".to_string()),
                breadcrumb_items: vec![],
                header_feed_icon_id: None,
                active_category_id: None,
                filter_tabs: None,
                status_filter: None,
                show_mark_above: false,
                onboarding: false,
                snapshot_at: snapshot_now(),
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
            title: "App",
            git_version: crate::GIT_VERSION,
            layout,
            database_url: state.config.database_url.clone(),
            server_port: state.config.server_port,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            signup_enabled: state.config.signup_enabled,
            multi_user_enabled: state.config.multi_user_enabled,
            image_proxy_secret_generated: state.config.image_proxy_secret_generated,
            webauthn_rp_id: state.config.webauthn_rp_id.clone(),
            webauthn_rp_origin: state.config.webauthn_rp_origin.clone(),
            webauthn_rp_name: state.config.webauthn_rp_name.clone(),
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
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/read",
                status_filter: None,
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
        None,
    )
    .await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        ReadEntriesTemplate {
            title: "Read Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "read",
                description: None,
                empty_title: "No read entries yet",
                empty_detail: "Entries stay here once you've opened and read them.",
                path: "/entries/read".to_string(),
                show_tab_bar: true,
                mark_as_read_scope: None,
                breadcrumb_items: vec![],
                header_feed_icon_id: None,
                active_category_id: None,
                filter_tabs: None,
                status_filter: None,
                show_mark_above: false,
                onboarding: false,
                snapshot_at: snapshot_now(),
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
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/starred",
                status_filter: None,
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
        None,
    )
    .await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        StarredEntriesTemplate {
            title: "Starred Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "starred",
                description: None,
                empty_title: "No starred entries",
                empty_detail: "Star an entry and it'll wait for you here.",
                path: "/entries/starred".to_string(),
                show_tab_bar: true,
                mark_as_read_scope: None,
                breadcrumb_items: vec![],
                header_feed_icon_id: None,
                active_category_id: None,
                filter_tabs: None,
                status_filter: None,
                show_mark_above: false,
                onboarding: false,
                snapshot_at: snapshot_now(),
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
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
            entry::EntrySortOrder::PublishedAt,
            PAGE_SIZE,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/entries/summarized",
                status_filter: None,
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
        None,
    )
    .await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        SummarizedEntriesTemplate {
            title: "Summarized Entries",
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            entries_layout: EntriesLayoutContext {
                active: "summarized",
                description: None,
                empty_title: "No summaries yet",
                empty_detail: "Entries you summarize are collected on this page.",
                path: "/entries/summarized".to_string(),
                show_tab_bar: true,
                mark_as_read_scope: None,
                breadcrumb_items: vec![],
                header_feed_icon_id: None,
                active_category_id: None,
                filter_tabs: None,
                status_filter: None,
                show_mark_above: false,
                onboarding: false,
                snapshot_at: snapshot_now(),
            },
        },
    )
        .into_response()
}

/// `GET /categories/{id}/entries` — SSR list of entries from every feed
/// in a single category. Supports `?fragment=1&after=N` Load-More.
pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntriesQuery>,
    flash: Flash,
) -> Result<Response, AppError> {
    const PAGE_SIZE: i64 = 50;
    let user_id = auth_user.user.id;

    let lookup = state
        .db
        .read_user(move |c| {
            let cat =
                category::find_by_id_and_user(c, id, user_id)?.ok_or(AppError::CategoryNotFound)?;
            Ok::<_, AppError>(cat.name)
        })
        .await?;

    let category_name = match lookup {
        Ok(name) => name,
        Err(AppError::CategoryNotFound) => {
            let page = render_not_found(
                &state,
                &auth_user,
                &flash,
                "Category not found",
                "This category doesn't exist or you don't have access to it.",
            )
            .await;
            return Ok((flash, page).into_response());
        }
        Err(e) => return Err(e),
    };

    let status = query.status.as_deref();
    let effective_status = status.unwrap_or("unread");
    let mut filter = entry::EntryFilter {
        category_id: Some(id),
        ..Default::default()
    };
    match effective_status {
        "all" => {}
        "read" => filter.read_only = true,
        "starred" => filter.starred_only = true,
        _ => filter.unread_only = true,
    }
    let cursor = query
        .after
        .as_deref()
        .and_then(entry::ContinuationCursor::parse);

    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        cursor,
    )
    .await;

    let path = format!("/categories/{}/entries", id);
    let status_filter = query.status.clone();

    if query.fragment == Some(1) {
        let fragment = EntriesFragmentTemplate {
            entries,
            next_cursor,
            path: Box::leak(path.into_boxed_str()),
            status_filter,
        };
        return Ok((flash, fragment).into_response());
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let mark_as_read_scope = Some(format!("user/-/label/{}", category_name));
    let base = format!("/categories/{}/entries", id);
    let filter_tabs = Some(vec![
        FilterTab {
            label: "All".to_string(),
            href: format!("{}?status=all", base),
            active: status == Some("all"),
        },
        FilterTab {
            label: "Unread".to_string(),
            href: base.clone(),
            active: status.is_none() || status == Some("unread"),
        },
        FilterTab {
            label: "Read".to_string(),
            href: format!("{}?status=read", base),
            active: status == Some("read"),
        },
        FilterTab {
            label: "Starred".to_string(),
            href: format!("{}?status=starred", base),
            active: status == Some("starred"),
        },
    ]);
    let breadcrumb_items = vec![
        BreadcrumbItem {
            label: "Categories".to_string(),
            href: Some("/categories".to_string()),
        },
        BreadcrumbItem {
            label: category_name.clone(),
            href: None,
        },
    ];

    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    let template = CategoryEntriesTemplate {
        title: category_name,
        git_version: crate::GIT_VERSION,
        layout,
        entries,
        reading_pane,
        next_cursor,
        entries_layout: EntriesLayoutContext {
            active: "",
            description: None,
            empty_title: "Nothing in this category",
            empty_detail: "The feeds in this category haven't brought in any entries yet.",
            path,
            show_tab_bar: false,
            mark_as_read_scope,
            breadcrumb_items,
            header_feed_icon_id: None,
            active_category_id: Some(id),
            filter_tabs,
            status_filter,
            show_mark_above: true,
            onboarding: false,
            snapshot_at: snapshot_now(),
        },
    };

    Ok((flash, template).into_response())
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
                        let (published_relative, published_at_iso) =
                            format_relative_time(e.entry.published_at);
                        visible.push(SearchResultView {
                            entry_id: e.entry.id,
                            title_html: highlight_html(&title, &q_for_filter),
                            feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                            published_relative,
                            published_at_iso,
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

    let lookup = state
        .db
        .read_user(move |c| {
            let f = feed::find_by_id(c, id)?.ok_or(AppError::FeedNotFound)?;
            let cat = category::find_by_id(c, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
            if cat.user_id != user_id {
                return Err(AppError::FeedNotFound);
            }
            let has_icon = crate::models::image::exists(c, "feed", f.id)?;
            Ok::<_, AppError>((
                f.title.unwrap_or_else(|| "(untitled feed)".to_string()),
                f.url,
                has_icon,
                cat.id,
                cat.name,
            ))
        })
        .await?;

    let (feed_title, feed_url, feed_has_icon, cat_id, cat_name) = match lookup {
        Ok(v) => v,
        Err(AppError::FeedNotFound) | Err(AppError::CategoryNotFound) => {
            let page = render_not_found(
                &state,
                &auth_user,
                &flash,
                "Feed not found",
                "This feed doesn't exist or you don't have access to it.",
            )
            .await;
            return Ok((flash, page).into_response());
        }
        Err(e) => return Err(e),
    };

    // Default status is "unread": the base URL (no `?status=`) shows
    // unread + starred-but-unread entries. `?status=all` explicitly
    // overrides the default.
    let status = query.status.as_deref();
    let effective_status = status.unwrap_or("unread");
    let mut filter = entry::EntryFilter {
        feed_id: Some(id),
        ..Default::default()
    };
    match effective_status {
        "all" => {}
        "read" => filter.read_only = true,
        "starred" => filter.starred_only = true,
        _ => filter.unread_only = true,
    }
    let cursor = query
        .after
        .as_deref()
        .and_then(entry::ContinuationCursor::parse);

    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        PAGE_SIZE,
        cursor,
    )
    .await;

    let path = format!("/feeds/{}/entries", id);
    let status_filter = query.status.clone();

    if query.fragment == Some(1) {
        let fragment = EntriesFragmentTemplate {
            entries,
            next_cursor,
            path: Box::leak(path.into_boxed_str()),
            status_filter,
        };
        return Ok((flash, fragment).into_response());
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let mark_as_read_scope = Some(format!("feed/{}", feed_url));
    let base = format!("/feeds/{}/entries", id);
    let filter_tabs = Some(vec![
        FilterTab {
            label: "All".to_string(),
            href: format!("{}?status=all", base),
            active: status == Some("all"),
        },
        FilterTab {
            label: "Unread".to_string(),
            href: base.clone(),
            active: status.is_none() || status == Some("unread"),
        },
        FilterTab {
            label: "Read".to_string(),
            href: format!("{}?status=read", base),
            active: status == Some("read"),
        },
        FilterTab {
            label: "Starred".to_string(),
            href: format!("{}?status=starred", base),
            active: status == Some("starred"),
        },
    ]);
    let breadcrumb_items = vec![
        BreadcrumbItem {
            label: "Feeds".to_string(),
            href: Some("/feeds".to_string()),
        },
        BreadcrumbItem {
            label: cat_name,
            href: Some(format!("/categories/{}/entries", cat_id)),
        },
        BreadcrumbItem {
            label: feed_title.clone(),
            href: None,
        },
    ];

    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    let template = FeedEntriesTemplate {
        title: feed_title,
        git_version: crate::GIT_VERSION,
        layout,
        entries,
        reading_pane,
        next_cursor,
        entries_layout: EntriesLayoutContext {
            active: "",
            description: None,
            empty_title: "Nothing in this feed",
            empty_detail: "This feed hasn't published anything yet, or it's still syncing.",
            path,
            show_tab_bar: false,
            mark_as_read_scope,
            breadcrumb_items,
            header_feed_icon_id: if feed_has_icon { Some(id) } else { None },
            active_category_id: Some(cat_id),
            filter_tabs,
            status_filter,
            show_mark_above: true,
            onboarding: false,
            snapshot_at: snapshot_now(),
        },
    };

    Ok((flash, template).into_response())
}

/// Shared HTML error page rendered for logged-in routes when a requested
/// resource is missing (e.g. feed/category not found). Replaces the default
/// `AppError` JSON 404 with a chrome-wrapped page.
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub heading: &'static str,
    pub message: String,
}

impl IntoResponse for ErrorTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (StatusCode::NOT_FOUND, Html(html)).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Build a chrome-wrapped 404 page for a logged-in route. The caller
/// supplies a short heading and a longer message; both render inside the
/// standard sidebar/flash chrome.
pub async fn render_not_found(
    state: &AppState,
    auth_user: &PageAuthUser,
    flash: &Flash,
    heading: &'static str,
    message: impl Into<String>,
) -> ErrorTemplate {
    let layout = build_app_layout(state, auth_user, flash).await;
    ErrorTemplate {
        title: "Not Found",
        git_version: crate::GIT_VERSION,
        layout,
        heading,
        message: message.into(),
    }
}

/// Router fallback: chrome-wrapped 404 for logged-in users, login
/// redirect otherwise (matches the behavior of every other page route).
pub async fn not_found_page(
    State(state): State<AppState>,
    flash: Flash,
    auth_user: Result<PageAuthUser, LoginRedirect>,
) -> Response {
    match auth_user {
        Ok(user) => render_not_found(
            &state,
            &user,
            &flash,
            "Page not found",
            "The page you're looking for doesn't exist.",
        )
        .await
        .into_response(),
        Err(redirect) => redirect.into_response(),
    }
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
    let session = &auth_user.session;
    let is_masquerading = session.is_masquerading();
    let chrome = crate::handlers::user::read_chrome_data(
        state,
        auth_user.user.id,
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
        auth_user.user.is_admin()
    };

    let sidebar = crate::handlers::user::SidebarResponse {
        username: auth_user.user.username.clone(),
        is_admin,
        is_masquerading,
        categories: chrome.categories,
        total_unread: chrome.total_unread,
    };
    let sidebar_bootstrap_json = serialize_sidebar_for_script(&sidebar);
    let flash_bootstrap_json = flash_bootstrap_json(&flash.messages);

    AppLayoutContext {
        theme: chrome.theme,
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
    pub database_url: String,
    pub server_port: u16,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub signup_enabled: bool,
    pub multi_user_enabled: bool,
    pub image_proxy_secret_generated: bool,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub webauthn_rp_name: String,
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
    pub created_at_iso: String,
    pub session_created_at: String,
    pub session_created_at_iso: String,
    pub public_base_url: String,
    pub theme: Option<String>,
    pub entries_per_page: i64,
    pub retention_read_days: i64,
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
    pub created_at_iso: String,
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
    pub active_category_id: Option<i64>,
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
    pub next_cursor: Option<String>,
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
    pub next_cursor: Option<String>,
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
    pub next_cursor: Option<String>,
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
    pub next_cursor: Option<String>,
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
    pub next_cursor: Option<String>,
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
    pub next_cursor: Option<String>,
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
    pub title: String,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    pub next_cursor: Option<String>,
    pub entries_layout: EntriesLayoutContext,
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
    /// RFC 3339 UTC string when published_at is known, otherwise empty.
    /// Emitted as the `datetime` attribute on the result row's `<time>`
    /// element so the client-side tooltip can format to browser TZ.
    pub published_at_iso: String,
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
    use super::{build_snippet, highlight_html, row_view_from};
    use crate::models::entry::{Entry, EntryWithFeed};
    use chrono::Utc;

    fn ewf_with_title(title: &str) -> EntryWithFeed {
        let now = Utc::now();
        EntryWithFeed {
            entry: Entry {
                id: 1,
                feed_id: 1,
                guid: "g".to_string(),
                title: Some(title.to_string()),
                link: None,
                content: None,
                summary: None,
                author: None,
                published_at: Some(now),
                read_at: None,
                starred_at: None,
                created_at: now,
                updated_at: now,
            },
            feed_title: Some("Feed".to_string()),
            feed_url: "https://example.com/feed".to_string(),
            site_url: None,
            category_id: 1,
            category_name: "Cat".to_string(),
            feed_has_icon: false,
            custom_referrer: None,
        }
    }

    #[test]
    fn row_view_decodes_hex_entity_in_title() {
        let ewf = ewf_with_title("Collabora&#x27;s CODE 26.04 Release");
        let row = row_view_from(&ewf, None);
        assert_eq!(row.title, "Collabora's CODE 26.04 Release");
    }

    #[test]
    fn row_view_decodes_decimal_and_named_entities() {
        let ewf = ewf_with_title("Tom &amp; Jerry&#39;s &quot;day&quot;");
        let row = row_view_from(&ewf, None);
        assert_eq!(row.title, "Tom & Jerry's \"day\"");
    }

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

    #[test]
    fn feed_initial_uppercases_first_char() {
        let mut ewf = ewf_with_title("anything");
        ewf.feed_title = Some("delta".to_string());
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_initial(), "D");
    }

    #[test]
    fn feed_initial_handles_empty_title() {
        let mut ewf = ewf_with_title("anything");
        ewf.feed_title = Some(String::new());
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_initial(), "?");
    }

    #[test]
    fn feed_initial_uppercases_unicode() {
        let mut ewf = ewf_with_title("anything");
        ewf.feed_title = Some("über".to_string());
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_initial(), "Ü");
    }

    #[test]
    fn feed_color_index_is_stable_and_bounded() {
        let mut ewf = ewf_with_title("anything");
        ewf.entry.feed_id = 13;
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_color_index(), 1); // 13 % 6 == 1

        // rem_euclid (not %) keeps the index non-negative for any id.
        ewf.entry.feed_id = -1;
        let row = row_view_from(&ewf, None);
        assert_eq!(row.feed_color_index(), 5); // (-1).rem_euclid(6) == 5
        assert!(row.feed_color_index() < 6);
    }

    #[test]
    fn feed_initial_fn_uppercases_first_char() {
        assert_eq!(super::feed_initial("daring fireball"), "D");
    }

    #[test]
    fn feed_initial_fn_handles_empty() {
        assert_eq!(super::feed_initial(""), "?");
    }

    #[test]
    fn feed_initial_fn_uppercases_unicode() {
        assert_eq!(super::feed_initial("über"), "Ü");
    }

    #[test]
    fn feed_color_index_fn_is_bounded() {
        assert_eq!(super::feed_color_index(0), 0); // boundary: id 0
        assert_eq!(super::feed_color_index(6), 0); // wraps at palette size
        assert_eq!(super::feed_color_index(13), 1); // 13 % 6 == 1
        assert_eq!(super::feed_color_index(-1), 5); // (-1).rem_euclid(6) == 5
        assert!(super::feed_color_index(i64::MAX) < 6);
    }
}
