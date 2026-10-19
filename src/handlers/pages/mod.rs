use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
};

use std::borrow::Cow;
use std::collections::HashMap;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::{Flash, FlashMessage, FlashRedirect};
use crate::models::SummaryStatus;
use crate::models::user_settings;
use crate::models::{category, entry, entry_summary, feed};
use crate::utils::han;

mod script_json;
mod search_text;
mod time_format;

use script_json::serialize_sidebar_for_script;
pub use time_format::{
    FRESH_MAX_DAYS, WARNING_MAX_DAYS, compute_freshness, format_relative_time,
    format_relative_time_compact,
};

mod account;
mod admin;
mod auth;
mod categories;
mod error;
mod feeds;
mod offline;
mod search;
mod settings;
mod statistics;

pub use account::*;
pub use admin::*;
pub use auth::*;
pub use categories::*;
pub use error::*;
pub use feeds::*;
pub use offline::*;
pub use search::*;
pub use settings::*;
pub use statistics::*;

// --- Entries-family shared view structs ---

/// Uppercased first character for the favicon letter-chip fallback; "?" if empty.
pub(crate) fn feed_initial(feed_title: &str) -> String {
    feed_title
        .chars()
        .next()
        .map_or_else(|| "?".to_string(), |c| c.to_uppercase().to_string())
}

/// Stable favicon fallback colour index (0..6) derived from the feed id.
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
    /// Summary status string for the template's `{% match %}`.
    pub fn summary_status_str(&self) -> Option<&'static str> {
        self.summary_status.map(|s| s.as_str())
    }

    /// See `feed_initial`.
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    /// See `feed_color_index`.
    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
    }

    /// `Some("zh-Hans")` for a Simplified title, so Traditional-locale browsers
    /// don't mix `PingFang TC`/`SC` glyphs on one line. See `utils::han`.
    pub fn title_lang(&self) -> Option<&'static str> {
        han::lang_attr(&self.title)
    }
}

/// View-model for the reading pane (`_reading_pane.html`).
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
    /// Latest summary failure; distinguishes `failed` from "no summary".
    pub summary_error: Option<String>,
    pub has_kagi: bool,
    pub has_save: bool,
    /// `content_html` is the fetched article, not the feed's body.
    pub is_full_content: bool,
    /// A fetched article is stored, so the pane can switch back without re-fetching.
    pub has_stored_full_content: bool,
}

impl ReadingPaneView {
    /// See `feed_initial`.
    pub fn feed_initial(&self) -> String {
        feed_initial(&self.feed_title)
    }

    /// See `feed_color_index`.
    pub fn feed_color_index(&self) -> u8 {
        feed_color_index(self.feed_id)
    }

    /// See [`EntryRowView::title_lang`].
    pub fn title_lang(&self) -> Option<&'static str> {
        han::lang_attr(&self.title)
    }

    /// Body language, decided separately: title and body may differ in script.
    pub fn content_lang(&self) -> Option<&'static str> {
        han::lang_attr(&self.content_html)
    }
}

/// Breadcrumb segment; `href = None` marks the current page.
#[derive(Debug, Clone)]
pub struct BreadcrumbItem {
    pub label: String,
    pub href: Option<String>,
}

/// Status-filter tab on feed/category pages; keys `1`–`4` select by position.
#[derive(Debug, Clone)]
pub struct FilterTab {
    pub label: String,
    pub href: String,
    pub active: bool,
}

/// Render-time snapshot for unread navigation, in the UTC format
/// `datetime('now')` writes to `entry.read_at`; echoed back as `read_after`.
pub(crate) fn snapshot_now() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Layout context shared by entries-family pages (`_entries_layout.html`).
/// `Default` means no breadcrumbs, filter tabs, search box or sidebar highlight.
#[derive(Debug, Clone, Default)]
pub struct EntriesLayoutContext {
    pub active: &'static str,
    pub description: Option<String>,
    pub empty_title: &'static str,
    pub empty_detail: &'static str,
    pub path: String,
    /// All/Read/Starred/Summarized tab bar; false on `/` (unread isn't a tab).
    pub show_tab_bar: bool,
    /// `GReader` stream id scoping the "Mark as Read..." dropdown.
    pub mark_as_read_scope: Option<String>,
    pub breadcrumb_items: Vec<BreadcrumbItem>,
    /// Feed whose favicon is shown beside the title (feed page only).
    pub header_feed_icon_id: Option<i64>,
    /// Sidebar category to highlight and expand.
    pub active_category_id: Option<i64>,
    /// Sidebar feed to highlight.
    pub active_feed_id: Option<i64>,
    /// Status-filter tabs for feed/category pages.
    pub filter_tabs: Option<Vec<FilterTab>>,
    /// `?status=` preserved across Load More.
    pub status_filter: Option<String>,
    /// Show "Mark Above as Read" (marks only rows currently in the DOM).
    pub show_mark_above: bool,
    /// Render onboarding instead of the empty state (landing page, no feeds).
    pub onboarding: bool,
    /// See `snapshot_now()`.
    pub snapshot_at: String,
    /// Current inline-search query.
    pub search: Option<String>,
    /// Inline-search form action; `None` hides the box.
    pub search_action: Option<String>,
    /// Matches for the active search, for "Mark N matching as Read".
    pub matching_count: Option<i64>,
    /// Why the active search could not run; the list is empty while it is set.
    pub search_error: Option<String>,
}

pub(crate) fn row_view_from(
    e: &entry::EntryWithFeed,
    summary_status: Option<SummaryStatus>,
) -> EntryRowView {
    let title = e.entry.title.as_deref().map_or_else(
        || "(no title)".to_string(),
        crate::services::decode_html_entities,
    );
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
        link: e
            .entry
            .link
            .as_deref()
            .map(crate::services::strip_tracking_params),
        published_at_iso: published_at.to_rfc3339(),
        published_relative: format_relative_time_compact(Some(published_at)),
        is_read: e.entry.read_at.is_some(),
        is_starred: e.entry.starred_at.is_some(),
        summary_status,
    }
}

/// Rows per list page. Read per request (not cached with the chrome) so a
/// saved change applies at once; clamped because callers cast it to `usize`.
async fn entries_page_size(state: &AppState, user_id: i64) -> i64 {
    user_settings::get_entries_per_page(&state.db, user_id)
        .await
        .unwrap_or(user_settings::DEFAULT_ENTRIES_PER_PAGE)
        .clamp(
            user_settings::MIN_ENTRIES_PER_PAGE,
            user_settings::MAX_ENTRIES_PER_PAGE,
        )
}

/// A list page's `?q=` run through the search-syntax parser.
struct ScopedSearch {
    /// The query as typed; `None` when blank.
    raw: Option<String>,
    query: Option<entry::query::QueryNode>,
    /// Set when `raw` does not parse; the page must then list nothing rather
    /// than fall back to an unfiltered list.
    error: Option<String>,
}

/// `Search syntax error (near character N): …`, shared by `/search` and the
/// list pages so the two cannot drift.
fn search_error_message(q: &str, e: &entry::query::ParseError) -> String {
    // Byte offset → 1-based character position for the message.
    let char_pos = q.get(..e.position).map_or(0, |p| p.chars().count()) + 1;
    format!(
        "Search syntax error (near character {char_pos}): {}",
        e.message
    )
}

fn scoped_search(q: Option<&str>) -> ScopedSearch {
    let Some(raw) = q.filter(|s| !s.trim().is_empty()) else {
        return ScopedSearch {
            raw: None,
            query: None,
            error: None,
        };
    };
    match entry::query::parse(raw) {
        Ok(ast) => ScopedSearch {
            raw: Some(raw.to_string()),
            query: Some(ast),
            error: None,
        },
        Err(e) => ScopedSearch {
            raw: Some(raw.to_string()),
            query: None,
            error: Some(search_error_message(raw, &e)),
        },
    }
}

/// [`build_entries_page`], but an unparseable search lists nothing.
async fn build_searched_entries_page(
    state: &AppState,
    user_id: i64,
    mut filter: entry::EntryFilter,
    search: &ScopedSearch,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> (Vec<EntryRowView>, Option<String>) {
    if search.error.is_some() {
        return (Vec::new(), None);
    }
    filter.query.clone_from(&search.query);
    build_entries_page(state, user_id, filter, sort, page_size, cursor).await
}

/// One page of rows (the look-ahead sentinel dropped) plus the next composite
/// cursor, if more exist.
async fn fetch_entries_page(
    state: &AppState,
    user_id: i64,
    filter: &entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> Result<(Vec<entry::EntryWithFeed>, Option<String>), AppError> {
    let params = entry::ContinuationParams {
        oldest_first: false,
        limit: page_size + 1,
        continuation: cursor,
        ot: None,
        nt: None,
        sort_order: sort,
    };
    let rows =
        entry::list_by_user_with_continuation_ts(&state.db, user_id, filter, &params).await?;
    #[allow(
        clippy::cast_sign_loss,
        reason = "`entries_page_size` clamps to MIN..=MAX_ENTRIES_PER_PAGE, so this is small and positive"
    )]
    let page_size = page_size as usize;
    // Cursor from the last kept row, not the dropped sentinel.
    let next = entry::next_continuation(&rows, page_size, |e| e.entry.id);
    let rows = rows.into_iter().take(page_size).map(|(e, _)| e).collect();
    Ok((rows, next))
}

/// Fetch a page of rows plus the next composite cursor, if more exist.
pub(crate) async fn build_entries_page(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> (Vec<EntryRowView>, Option<String>) {
    let result = async {
        let (rows, next) =
            fetch_entries_page(state, user_id, &filter, sort, page_size, cursor).await?;
        let ids: Vec<i64> = rows.iter().map(|e| e.entry.id).collect();
        let statuses = entry_summary::get_statuses_for_entries(&state.db, user_id, &ids).await?;
        Ok::<_, AppError>((rows, next, statuses))
    }
    .await
    .ok()
    .unwrap_or_else(|| (Vec::new(), None, HashMap::new()));
    let (rows, next_cursor, statuses) = result;
    let views = rows
        .iter()
        .map(|e| row_view_from(e, statuses.get(&e.entry.id).copied()))
        .collect();
    (views, next_cursor)
}

/// List-page query. `fragment=1` returns the Load-More fragment after cursor `after`.
#[derive(serde::Deserialize, Default)]
pub struct EntriesQuery {
    pub fragment: Option<u8>,
    pub after: Option<String>,
    /// `unread` / `read` / `starred` on feed/category pages; anything else shows all.
    pub status: Option<String>,
    /// Deep-link entry to pre-open. Does not mark it read; ignored if not the user's.
    pub entry: Option<i64>,
    /// Inline-search query in the `/search` syntax; blank means no filter.
    pub q: Option<String>,
    /// Render-time snapshot applied as [`entry::EntryFilter::read_after`], so
    /// entries read mid-session don't vanish from later pages. Unread views
    /// only; garbage can only widen the user's own results.
    pub snapshot: Option<String>,
    /// `pane=1` on category pages returns the left column plus an emptied pane.
    pub pane: Option<u8>,
}

/// Best-effort `?entry={id}` reading pane; any failure yields an empty pane.
/// Must not mark the entry read — only the row click (`entry_fragment`) does.
async fn maybe_build_reading_pane(
    state: &AppState,
    user_id: i64,
    entry_id: Option<i64>,
) -> Option<ReadingPaneView> {
    let entry_id = entry_id?;
    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await
        .ok()??;
    let (has_save, has_kagi) = crate::handlers::entries::load_pane_action_flags(state, user_id)
        .await
        .ok()?;
    crate::handlers::entries::build_reading_pane_view(
        state,
        user_id,
        &ewf,
        has_save,
        has_kagi,
        crate::handlers::entries::ContentView::Full,
        // A list page rendered with `?entry=` is the reader looking at it.
        crate::handlers::entries::RenderPurpose::Reader,
    )
    .await
    .ok()
}

/// Load-More response fragment.
#[derive(Template)]
#[template(path = "_entries_fragment.html")]
pub(crate) struct EntriesFragmentTemplate {
    pub entries: Vec<EntryRowView>,
    pub next_cursor: Option<String>,
    /// Next Load-More form action.
    pub path: Cow<'static, str>,
    pub status_filter: Option<String>,
    pub q: Option<String>,
    /// Echoed, never re-stamped, so every page shares one boundary.
    /// See [`EntriesQuery::snapshot`].
    pub snapshot: Option<String>,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
}

crate::handlers::impl_html_response!(
    EntriesFragmentTemplate,
    EntriesRefreshFragmentTemplate,
    EntriesPaneFragmentTemplate,
    OfflineTemplate,
    LoginTemplate,
    SetupTemplate,
    InviteTemplate,
    SettingsTemplate,
    UserSettingsTemplate,
    AdminTemplate,
    StatisticsTemplate,
    CategoriesTemplate,
    FeedsTemplate,
    FeedEditTemplate,
    FeedsImportTemplate,
    EntriesPageTemplate,
    SearchTemplate,
    SearchRefreshFragmentTemplate,
    SearchFragmentTemplate,
);

/// Scoped-search refresh: replaces the list and the mark-matching button.
#[derive(Template)]
#[template(path = "_entries_refresh_fragment.html")]
pub(crate) struct EntriesRefreshFragmentTemplate {
    pub entries: Vec<EntryRowView>,
    pub next_cursor: Option<String>,
    pub entries_layout: EntriesLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
}

/// Category-switch fragment (`?pane=1`); shares `_list_pane.html` with the full
/// page so the header can't drift.
#[derive(Template)]
#[template(path = "_entries_pane_fragment.html")]
pub(crate) struct EntriesPaneFragmentTemplate {
    pub title: String,
    pub entries: Vec<EntryRowView>,
    pub next_cursor: Option<String>,
    pub entries_layout: EntriesLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
}

/// `GET /` (unread). `?fragment=1&after=` returns Load More; `?fragment=1`
/// alone returns the list refresh used after bulk mark-as-read.
pub async fn unread_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let user_id = auth_user.user.id;
    let page_size = entries_page_size(&state, user_id).await;
    let search = scoped_search(query.q.as_deref());
    let filter = entry::EntryFilter {
        unread_only: true,
        // Only set by the Load-More form.
        read_after: query.snapshot.clone(),
        ..Default::default()
    };

    if query.fragment == Some(1) && query.after.is_some() {
        let cursor = query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse);
        let (entries, next_cursor) = build_searched_entries_page(
            &state,
            user_id,
            filter,
            &search,
            entry::EntrySortOrder::PublishedAt,
            page_size,
            cursor,
        )
        .await;
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: "/".into(),
                status_filter: None,
                q: search.raw,
                snapshot: query.snapshot.clone(),
                csrf_token: auth_user.csrf_token.clone(),
            },
        )
            .into_response();
    }

    let (entries, next_cursor) = build_searched_entries_page(
        &state,
        user_id,
        filter,
        &search,
        entry::EntrySortOrder::PublishedAt,
        page_size,
        None,
    )
    .await;

    // Empty inbox: onboarding if there are no feeds, else "All caught up".
    // A search with no hits says nothing about the inbox.
    let no_feeds = entries.is_empty()
        && search.raw.is_none()
        && feed::count_by_user(&state.db, user_id).await.unwrap_or(0) == 0;

    let entries_layout = EntriesLayoutContext {
        active: "unread",
        empty_title: "All caught up",
        empty_detail: "You've read every unread entry — new items land here as your feeds refresh.",
        path: "/".to_string(),
        mark_as_read_scope: Some("user/-/state/com.google/reading-list".to_string()),
        // Hidden during search: it would be confusable with the marks-all-loaded action.
        show_mark_above: search.raw.is_none(),
        onboarding: no_feeds,
        snapshot_at: snapshot_now(),
        search_action: (!no_feeds).then(|| "/".to_string()),
        search: search.raw,
        search_error: search.error,
        ..Default::default()
    };

    // List refresh (fragment=1, no cursor): re-render page 1 in place.
    if query.fragment == Some(1) {
        return (
            flash,
            EntriesRefreshFragmentTemplate {
                entries,
                next_cursor,
                entries_layout,
                csrf_token: auth_user.csrf_token.clone(),
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        EntriesPageTemplate {
            title: "Unread".into(),
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            csrf_token: auth_user.csrf_token.clone(),
            entries_layout,
        },
    )
        .into_response()
}

/// Per-tab settings for [`entries_tab_page`].
struct EntriesTab {
    path: &'static str,
    title: &'static str,
    active: &'static str,
    empty_title: &'static str,
    empty_detail: &'static str,
    mark_as_read_scope: Option<&'static str>,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
}

/// Renders an `/entries` tab, or its Load-More fragment with `?fragment=1`.
async fn entries_tab_page(
    tab: EntriesTab,
    auth_user: PageAuthUser,
    state: AppState,
    flash: Flash,
    query: EntriesQuery,
) -> Response {
    let user_id = auth_user.user.id;
    let page_size = entries_page_size(&state, user_id).await;
    let fragment = query.fragment == Some(1);
    let cursor = if fragment {
        query
            .after
            .as_deref()
            .and_then(entry::ContinuationCursor::parse)
    } else {
        None
    };
    let search = scoped_search(query.q.as_deref());
    let (entries, next_cursor) = build_searched_entries_page(
        &state, user_id, tab.filter, &search, tab.sort, page_size, cursor,
    )
    .await;

    if fragment && query.after.is_some() {
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: tab.path.into(),
                status_filter: None,
                q: search.raw,
                snapshot: query.snapshot,
                csrf_token: auth_user.csrf_token,
            },
        )
            .into_response();
    }

    let entries_layout = EntriesLayoutContext {
        active: tab.active,
        empty_title: tab.empty_title,
        empty_detail: tab.empty_detail,
        path: tab.path.to_string(),
        show_tab_bar: true,
        mark_as_read_scope: tab.mark_as_read_scope.map(str::to_string),
        snapshot_at: snapshot_now(),
        search_action: Some(tab.path.to_string()),
        search: search.raw,
        search_error: search.error,
        ..Default::default()
    };

    // Search refresh (fragment=1, no cursor): replace the list in place.
    if fragment {
        return (
            flash,
            EntriesRefreshFragmentTemplate {
                entries,
                next_cursor,
                entries_layout,
                csrf_token: auth_user.csrf_token,
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;
    (
        flash,
        EntriesPageTemplate {
            title: tab.title.into(),
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor,
            csrf_token: auth_user.csrf_token.clone(),
            entries_layout,
        },
    )
        .into_response()
}

/// `GET /entries` (all).
pub async fn entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let tab = EntriesTab {
        path: "/entries",
        title: "Entries",
        active: "all",
        empty_title: "Nothing to read yet",
        empty_detail: "Subscribe to a few feeds and their entries will gather here.",
        mark_as_read_scope: Some("user/-/state/com.google/reading-list"),
        filter: entry::EntryFilter::default(),
        sort: entry::EntrySortOrder::PublishedAt,
    };
    entries_tab_page(tab, auth_user, state, flash, query).await
}

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
                format!("/feeds/{feed_id}/entries")
            } else {
                "/entries".to_string()
            }
        }
        "category" => {
            if let Some(cat_id) = query.category {
                format!("/categories/{cat_id}/entries")
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

    let redirect_url = format!("{base_url}?entry={id}");
    Redirect::to(&redirect_url)
}

/// `GET /entries/read`.
pub async fn read_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let tab = EntriesTab {
        path: "/entries/read",
        title: "Read Entries",
        active: "read",
        empty_title: "No read entries yet",
        empty_detail: "Entries stay here once you've opened and read them.",
        mark_as_read_scope: None,
        filter: entry::EntryFilter {
            read_only: true,
            ..Default::default()
        },
        sort: entry::EntrySortOrder::PublishedAt,
    };
    entries_tab_page(tab, auth_user, state, flash, query).await
}

/// `GET /entries/starred`.
pub async fn starred_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let tab = EntriesTab {
        path: "/entries/starred",
        title: "Starred Entries",
        active: "starred",
        empty_title: "No starred entries",
        empty_detail: "Star an entry and it'll wait for you here.",
        mark_as_read_scope: None,
        filter: entry::EntryFilter {
            starred_only: true,
            ..Default::default()
        },
        sort: entry::EntrySortOrder::PublishedAt,
    };
    entries_tab_page(tab, auth_user, state, flash, query).await
}

/// `GET /entries/offline` — entries kept for offline reading; also the service
/// worker's fallback. No Load More, search or bulk actions: they need the network.
pub async fn offline_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let user_id = auth_user.user.id;
    let keep = user_settings::get_offline_keep(&state.db, user_id)
        .await
        .unwrap_or(user_settings::OFFLINE_KEEP_OFF);
    let entries = entry::list_offline_set(&state.db, user_id, keep)
        .await
        .unwrap_or_default();
    let ids: Vec<i64> = entries.iter().map(|e| e.entry.id).collect();
    let statuses = entry_summary::get_statuses_for_entries(&state.db, user_id, &ids)
        .await
        .unwrap_or_default();
    let entries: Vec<EntryRowView> = entries
        .iter()
        .map(|e| row_view_from(e, statuses.get(&e.entry.id).copied()))
        .collect();

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;

    (
        flash,
        EntriesPageTemplate {
            title: "Offline".into(),
            git_version: crate::GIT_VERSION,
            layout,
            entries,
            reading_pane,
            next_cursor: None,
            csrf_token: auth_user.csrf_token.clone(),
            entries_layout: EntriesLayoutContext {
                active: "offline",
                empty_title: if keep == user_settings::OFFLINE_KEEP_OFF {
                    "Offline reading is off"
                } else {
                    "Nothing saved yet"
                },
                empty_detail: if keep == user_settings::OFFLINE_KEEP_OFF {
                    "Turn it on in Settings to keep entries readable without a connection."
                } else {
                    "Entries are mirrored while you are online. Come back once something has synced."
                },
                path: "/entries/offline".to_string(),
                snapshot_at: snapshot_now(),
                ..Default::default()
            },
        },
    )
        .into_response()
}

/// `GET /entries/summarized`.
pub async fn summarized_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<EntriesQuery>,
) -> Response {
    let tab = EntriesTab {
        path: "/entries/summarized",
        title: "Summarized Entries",
        active: "summarized",
        empty_title: "No summaries yet",
        empty_detail: "Entries you summarize are collected on this page.",
        mark_as_read_scope: None,
        filter: entry::EntryFilter {
            has_summary: Some(true),
            ..Default::default()
        },
        sort: entry::EntrySortOrder::SummarizedAt,
    };
    entries_tab_page(tab, auth_user, state, flash, query).await
}

/// Category- or feed-specific settings for [`scoped_entries_page`].
struct EntriesScope {
    /// Page URL, also the Load-More and search target.
    path: String,
    title: String,
    category_id: Option<i64>,
    feed_id: Option<i64>,
    mark_as_read_scope: String,
    breadcrumb_items: Vec<BreadcrumbItem>,
    empty_title: &'static str,
    empty_detail: &'static str,
    header_feed_icon_id: Option<i64>,
    active_category_id: i64,
}

/// Status tabs for a scoped list; the bare URL is Unread.
fn filter_tabs(base: &str, status: Option<&str>) -> Vec<FilterTab> {
    [
        ("All", Some("all")),
        ("Unread", None),
        ("Read", Some("read")),
        ("Starred", Some("starred")),
    ]
    .into_iter()
    .map(|(label, value)| FilterTab {
        label: label.to_string(),
        href: value.map_or_else(|| base.to_string(), |v| format!("{base}?status={v}")),
        active: status == value || (value.is_none() && status == Some("unread")),
    })
    .collect()
}

/// Category/feed list: full page, Load More (`?fragment=1&after=`), search
/// refresh (`?fragment=1`) or sidebar pane (`?pane=1`).
async fn scoped_entries_page(
    scope: EntriesScope,
    auth_user: PageAuthUser,
    state: AppState,
    flash: Flash,
    query: EntriesQuery,
) -> Response {
    let user_id = auth_user.user.id;
    let page_size = entries_page_size(&state, user_id).await;

    // No `?status=` means unread.
    let status = query.status.as_deref();
    let mut filter = entry::EntryFilter {
        category_id: scope.category_id,
        feed_id: scope.feed_id,
        ..Default::default()
    };
    match status.unwrap_or("unread") {
        "all" => {}
        "read" => filter.read_only = true,
        "starred" => filter.starred_only = true,
        _ => filter.unread_only = true,
    }
    // See [`EntriesQuery::snapshot`].
    filter.read_after = query.snapshot.clone();
    let search = scoped_search(query.q.as_deref());
    let cursor = query
        .after
        .as_deref()
        .and_then(entry::ContinuationCursor::parse);

    let (entries, next_cursor) = build_searched_entries_page(
        &state,
        user_id,
        filter,
        &search,
        entry::EntrySortOrder::PublishedAt,
        page_size,
        cursor,
    )
    .await;

    if query.fragment == Some(1) && query.after.is_some() {
        let fragment = EntriesFragmentTemplate {
            entries,
            next_cursor,
            path: scope.path.into(),
            status_filter: query.status.clone(),
            q: search.raw,
            snapshot: query.snapshot.clone(),
            csrf_token: auth_user.csrf_token.clone(),
        };
        return (flash, fragment).into_response();
    }

    // Unread-only regardless of tab, to match what `mark_read_by_filter` touches.
    let matching_count = if let Some(ast) = search.query.clone() {
        let mark_filter = entry::EntryFilter {
            category_id: scope.category_id,
            feed_id: scope.feed_id,
            query: Some(ast),
            unread_only: true,
            ..Default::default()
        };
        entry::count_by_user(&state.db, user_id, &mark_filter)
            .await
            .ok()
    } else {
        None
    };

    let entries_layout = EntriesLayoutContext {
        active: "",
        empty_title: scope.empty_title,
        empty_detail: scope.empty_detail,
        filter_tabs: Some(filter_tabs(&scope.path, status)),
        search_action: Some(scope.path.clone()),
        path: scope.path,
        mark_as_read_scope: Some(scope.mark_as_read_scope),
        breadcrumb_items: scope.breadcrumb_items,
        header_feed_icon_id: scope.header_feed_icon_id,
        active_category_id: Some(scope.active_category_id),
        active_feed_id: scope.feed_id,
        status_filter: query.status.clone(),
        // Hidden during search: it would be confusable with "Mark N matching".
        show_mark_above: search.raw.is_none(),
        snapshot_at: snapshot_now(),
        search: search.raw,
        search_error: search.error,
        matching_count,
        ..Default::default()
    };

    // Search-refresh fragment (fragment=1, no cursor): replace list + button slot.
    if query.fragment == Some(1) {
        return (
            flash,
            EntriesRefreshFragmentTemplate {
                entries,
                next_cursor,
                entries_layout,
                csrf_token: auth_user.csrf_token.clone(),
            },
        )
            .into_response();
    }

    // Sidebar pane fragment (pane=1); ignores `?entry=` since switching closes it.
    if query.pane == Some(1) {
        return (
            flash,
            EntriesPaneFragmentTemplate {
                title: scope.title,
                entries,
                next_cursor,
                entries_layout,
                csrf_token: auth_user.csrf_token.clone(),
            },
        )
            .into_response();
    }

    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let reading_pane = maybe_build_reading_pane(&state, user_id, query.entry).await;
    let template = EntriesPageTemplate {
        title: scope.title.into(),
        git_version: crate::GIT_VERSION,
        layout,
        entries,
        reading_pane,
        next_cursor,
        entries_layout,
        csrf_token: auth_user.csrf_token.clone(),
    };
    (flash, template).into_response()
}

/// `GET /categories/{id}/entries`.
pub async fn category_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntriesQuery>,
    flash: Flash,
) -> Result<Response, AppError> {
    let Some(cat) = category::find_by_id_and_user(&state.db, id, auth_user.user.id).await? else {
        let page = render_not_found(
            &state,
            &auth_user,
            &flash,
            "Category not found",
            "This category doesn't exist or you don't have access to it.",
        )
        .await;
        return Ok((flash, page).into_response());
    };

    let scope = EntriesScope {
        path: format!("/categories/{id}/entries"),
        mark_as_read_scope: format!("user/-/label/{}", cat.name),
        breadcrumb_items: vec![
            BreadcrumbItem {
                label: "Categories".to_string(),
                href: Some("/categories".to_string()),
            },
            BreadcrumbItem {
                label: cat.name.clone(),
                href: None,
            },
        ],
        title: cat.name,
        category_id: Some(id),
        feed_id: None,
        empty_title: "Nothing in this category",
        empty_detail: "The feeds in this category haven't brought in any entries yet.",
        header_feed_icon_id: None,
        active_category_id: id,
    };
    Ok(scoped_entries_page(scope, auth_user, state, flash, query).await)
}

/// `GET /feeds/{id}/entries`.
pub async fn feed_entries_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<EntriesQuery>,
    flash: Flash,
) -> Result<Response, AppError> {
    let user_id = auth_user.user.id;

    let lookup = async {
        let f = feed::find_by_id(&state.db, id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&state.db, f.category_id)
            .await?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::FeedNotFound);
        }
        let has_icon = crate::models::image::exists(&state.db, "feed", f.id).await?;
        Ok::<_, AppError>((
            f.title.unwrap_or_else(|| "(untitled feed)".to_string()),
            f.url,
            has_icon,
            cat.id,
            cat.name,
        ))
    }
    .await;

    let (feed_title, feed_url, feed_has_icon, cat_id, cat_name) = match lookup {
        Ok(v) => v,
        Err(AppError::FeedNotFound | AppError::CategoryNotFound) => {
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

    let scope = EntriesScope {
        path: format!("/feeds/{id}/entries"),
        mark_as_read_scope: format!("feed/{feed_url}"),
        breadcrumb_items: vec![
            BreadcrumbItem {
                label: "Feeds".to_string(),
                href: Some("/feeds".to_string()),
            },
            BreadcrumbItem {
                label: cat_name,
                href: Some(format!("/categories/{cat_id}/entries")),
            },
            BreadcrumbItem {
                label: feed_title.clone(),
                href: None,
            },
        ],
        title: feed_title,
        category_id: None,
        feed_id: Some(id),
        empty_title: "Nothing in this feed",
        empty_detail: "This feed hasn't published anything yet, or it's still syncing.",
        header_feed_icon_id: feed_has_icon.then_some(id),
        active_category_id: cat_id,
    };
    Ok(scoped_entries_page(scope, auth_user, state, flash, query).await)
}

#[derive(serde::Deserialize)]
pub struct MarkReadForm {
    pub q: Option<String>,
    /// Current `?status=` tab, preserved on redirect.
    pub status: Option<String>,
}

/// `POST /categories/{id}/entries/mark-read` — mark entries matching `q` read.
pub async fn category_mark_read_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MarkReadForm>,
) -> Response {
    mark_read_scoped(
        &state,
        auth_user.user.id,
        Some(id),
        None,
        form.q,
        form.status,
        &format!("/categories/{id}/entries"),
    )
    .await
}

/// `POST /feeds/{id}/entries/mark-read` — same, scoped to a feed.
pub async fn feed_mark_read_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MarkReadForm>,
) -> Response {
    mark_read_scoped(
        &state,
        auth_user.user.id,
        None,
        Some(id),
        form.q,
        form.status,
        &format!("/feeds/{id}/entries"),
    )
    .await
}

/// `base_path` plus URL-encoded `q`/`status`, each omitted when absent.
fn build_scoped_redirect(base_path: &str, search: Option<&str>, status: Option<&str>) -> String {
    let mut params = Vec::new();
    if let Some(s) = search {
        params.push(format!(
            "q={}",
            url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
        ));
    }
    if let Some(s) = status {
        params.push(format!(
            "status={}",
            url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>()
        ));
    }
    if params.is_empty() {
        base_path.to_string()
    } else {
        format!("{}?{}", base_path, params.join("&"))
    }
}

async fn mark_read_scoped(
    state: &AppState,
    user_id: i64,
    category_id: Option<i64>,
    feed_id: Option<i64>,
    q: Option<String>,
    status: Option<String>,
    base_path: &str,
) -> Response {
    let parsed = scoped_search(q.as_deref());

    // Must guard here: a blank `q` would match, and mark read, the whole scope.
    let Some(raw) = parsed.raw.as_deref() else {
        let redirect = build_scoped_redirect(base_path, None, status.as_deref());
        return FlashRedirect::info(&redirect, "No search term — nothing marked.").into_response();
    };
    let search = raw.trim().to_string();
    // An unparseable query matches nothing to mark; say why instead.
    if let Some(err) = parsed.error {
        let redirect = build_scoped_redirect(base_path, Some(&search), status.as_deref());
        return FlashRedirect::error(&redirect, err).into_response();
    }

    let filter = entry::EntryFilter {
        category_id,
        feed_id,
        query: parsed.query,
        ..Default::default()
    };
    // Failure becomes an error flash, not a 500.
    let affected = match entry::mark_read_by_filter(&state.db, user_id, &filter).await {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::warn!(
                event = "entry.mark_read_scoped_failed",
                user_id,
                category_id = ?category_id,
                feed_id = ?feed_id,
                error = %e,
                "mark_read_by_filter failed"
            );
            None
        }
    };

    let redirect = build_scoped_redirect(base_path, Some(&search), status.as_deref());
    match affected {
        Some(n) => {
            if n > 0 {
                state.sidebar_cache.bust(user_id);
                state.events.emit_sidebar(user_id);
            }
            FlashRedirect::success(&redirect, format!("Marked {n} matching entries as read."))
                .into_response()
        }
        None => FlashRedirect::error(
            &redirect,
            "Failed to mark matching entries as read. Please try again.",
        )
        .into_response(),
    }
}

/// Shared layout fields (`{{ layout.<field> }}`) for logged-in templates.
pub struct AppLayoutContext {
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_bootstrap_json: String,
    /// Server-rendered so flashes are visible without JavaScript.
    pub flash_messages: Vec<FlashMessage>,
    /// Admin link in the no-JS nav fallback.
    pub is_admin: bool,
    /// Unread count for the no-JS nav fallback.
    pub total_unread: i64,
    /// For the no-JS sign-out form; not every page has its own token.
    pub csrf_token: String,
    /// Offline cache name (`secret::offline_id`). Inlined so `offline.js` can
    /// drop another account's cached articles before any network call.
    pub offline_key: String,
    pub offline_keep: i64,
}

/// Build the shared layout context for a logged-in page.
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
        total_summarized: chrome.total_summarized,
        via_forward_auth: auth_user.via_forward_auth,
        sidebar_sort: chrome.sidebar_prefs.sort,
        sidebar_hide_read: chrome.sidebar_prefs.hide_read,
    };
    let sidebar_bootstrap_json = serialize_sidebar_for_script(&sidebar);

    AppLayoutContext {
        theme: chrome.theme,
        git_version: crate::GIT_VERSION,
        sidebar_bootstrap_json,
        flash_messages: flash.messages.clone(),
        is_admin,
        total_unread: sidebar.total_unread,
        csrf_token: auth_user.csrf_token.clone(),
        offline_key: crate::secret::offline_id(&state.config.secret, auth_user.user.id),
        offline_keep: chrome.offline_keep,
    }
}

/// Every entry-list page renders `entries.html`. `git_version` duplicates
/// `layout`'s because `base.html` references it outside the layout's blocks.
#[derive(Template)]
#[template(path = "entries.html")]
pub struct EntriesPageTemplate {
    pub title: Cow<'static, str>,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub entries: Vec<EntryRowView>,
    pub reading_pane: Option<ReadingPaneView>,
    /// Always `None` on `/entries/offline`, which renders its whole set.
    pub next_cursor: Option<String>,
    pub entries_layout: EntriesLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`]. Leaf-level
    /// because the form macros are shared with the swap fragments.
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::row_view_from;
    use super::search_text::{build_snippet, highlight_html};
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
                full_content: None,
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
    fn highlight_wraps_matches_and_escapes_the_rest() {
        for (text, terms, expected) in [
            (
                "Sunrise Over Kyoto",
                &["sunrise"][..],
                "<mark>Sunrise</mark> Over Kyoto",
            ),
            (
                "SUNRISE sunrise Sunrise",
                &["sunrise"],
                "<mark>SUNRISE</mark> <mark>sunrise</mark> <mark>Sunrise</mark>",
            ),
            // "learn" is contained in "learning"; ranges merge into one wrapper.
            (
                "machine learning",
                &["learn", "learning"],
                "machine <mark>learning</mark>",
            ),
            ("Weather report", &["sunrise"], "Weather report"),
            (
                "<b>sunrise</b>",
                &["sunrise"],
                "&lt;b&gt;<mark>sunrise</mark>&lt;/b&gt;",
            ),
            ("Hi <world>", &[], "Hi &lt;world&gt;"),
            ("Hi <world>", &[""], "Hi &lt;world&gt;"),
        ] {
            assert_eq!(highlight_html(text, terms), expected, "{text} {terms:?}");
        }
    }

    #[test]
    fn highlight_wraps_all_terms_regardless_of_order() {
        // Every term is highlighted, not just the first.
        let expected = "<mark>人工智慧</mark> and <mark>AI</mark> news";
        assert_eq!(
            highlight_html("人工智慧 and AI news", &["人工智慧", "AI"]),
            expected
        );
        assert_eq!(
            highlight_html("人工智慧 and AI news", &["AI", "人工智慧"]),
            expected
        );
    }

    #[test]
    fn build_snippet_strips_script_and_style_bodies() {
        let out = build_snippet(
            Some("<p>hello</p><script>alert('x')</script><style>.a{}</style> world"),
            &[],
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
        let html = format!("<p>{lead}Sunrise lit the harbor early today over calm water.</p>");
        let out = build_snippet(Some(&html), &["sunrise"], 80);
        assert!(
            out.contains("Sunrise"),
            "snippet should include match: {out}"
        );
        assert!(out.starts_with('…'), "should ellipsis-prefix: {out}");
    }

    #[test]
    fn build_snippet_centers_on_earliest_matching_term() {
        let lead = "lorem ipsum ".repeat(40);
        let html = format!("<p>{lead}Harbor then meadow later.</p>");
        let out = build_snippet(Some(&html), &["meadow", "harbor"], 80);
        assert!(out.contains("Harbor"), "should center on earliest: {out}");
        assert!(out.starts_with('…'), "should ellipsis-prefix: {out}");
    }

    #[test]
    fn build_snippet_falls_back_to_lead_when_no_match() {
        let html = "<p>Monsoon clouds gathered over the valley before the rains arrived.</p>";
        let out = build_snippet(Some(html), &["sunrise"], 30);
        assert!(out.starts_with("Monsoon"));
        assert!(out.ends_with('…'));
    }

    #[test]
    fn build_snippet_strips_html_comments() {
        let out = build_snippet(Some("hello <!-- secret note --> world"), &[], 200);
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
