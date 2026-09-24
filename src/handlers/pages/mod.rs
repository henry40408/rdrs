use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};

use std::borrow::Cow;
use std::collections::HashMap;

use crate::AppState;
use crate::error::AppError;
use crate::middleware::auth::{LoginRedirect, PageAdminUser, PageAuthUser};
use crate::middleware::flash::{Flash, FlashMessage, FlashRedirect};
use crate::models::SummaryStatus;
use crate::models::api_token;
use crate::models::session;
use crate::models::user_settings;
use crate::models::{category, entry, entry_summary, feed};
use crate::utils::han;

mod script_json;
mod search_text;
mod time_format;

use script_json::serialize_sidebar_for_script;
use search_text::{build_snippet, highlight_html};
pub use time_format::{
    FRESH_MAX_DAYS, WARNING_MAX_DAYS, compute_freshness, format_relative_time,
    format_relative_time_compact,
};

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
    /// Current scoped-search keyword.
    pub search: Option<String>,
    /// Scoped-search form action; `None` hides the box.
    pub search_action: Option<String>,
    /// Matches for the active search, for "Mark N matching as Read".
    pub matching_count: Option<i64>,
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

/// Fetch a page of rows plus the next composite cursor, if more exist.
pub(crate) async fn build_entries_page(
    state: &AppState,
    user_id: i64,
    filter: entry::EntryFilter,
    sort: entry::EntrySortOrder,
    page_size: i64,
    cursor: Option<entry::ContinuationCursor>,
) -> (Vec<EntryRowView>, Option<String>) {
    let result = async move {
        let params = entry::ContinuationParams {
            oldest_first: false,
            limit: page_size + 1,
            continuation: cursor,
            ot: None,
            nt: None,
            sort_order: sort,
        };
        let rows =
            entry::list_by_user_with_continuation(&state.db, user_id, &filter, &params).await?;
        #[allow(
            clippy::cast_sign_loss,
            reason = "`entries_page_size` clamps to MIN..=MAX_ENTRIES_PER_PAGE, so this is small and positive"
        )]
        let kept_len = rows.len().min(page_size as usize);
        // Cursor from the last kept row, not the dropped sentinel.
        let next = if rows.len() as i64 > page_size {
            match rows.iter().take(kept_len).next_back() {
                Some(e) => entry::fetch_sort_ts(&state.db, e.entry.id, sort)
                    .await?
                    .map(|ts| entry::ContinuationCursor::encode_composite(&ts, e.entry.id)),
                None => None,
            }
        } else {
            None
        };
        let ids: Vec<i64> = rows.iter().take(kept_len).map(|e| e.entry.id).collect();
        let statuses = entry_summary::get_statuses_for_entries(&state.db, user_id, &ids).await?;
        Ok::<_, AppError>((rows, kept_len, next, statuses))
    }
    .await
    .ok()
    .unwrap_or_else(|| (Vec::new(), 0, None, HashMap::new()));
    let (rows, kept_len, next_cursor, statuses) = result;
    let views = rows
        .iter()
        .take(kept_len)
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
    /// Scoped-search keyword; blank means no filter.
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

#[derive(serde::Deserialize)]
pub struct StatisticsQuery {
    pub period: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Returns (`from`, `to_exclusive`, `active_period`) as ISO dates.
pub fn resolve_statistics_period(query: &StatisticsQuery) -> (String, String, String) {
    let today = chrono::Utc::now().date_naive();
    let days_ago = |n| today - chrono::Duration::days(n);
    let parse = |s: &Option<String>| {
        s.as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    };

    // Custom ranges cap at a year; invalid ones fall back to 7d.
    let (from, to, period) = match query.period.as_deref().unwrap_or("7d") {
        "30d" => (days_ago(30), today, "30d"),
        "90d" => (days_ago(90), today, "90d"),
        "all" => (chrono::NaiveDate::default(), today, "all"),
        "custom" => match (parse(&query.from), parse(&query.to)) {
            (Some(f), Some(t)) if f <= t => (f, t.min(f + chrono::Duration::days(365)), "custom"),
            _ => (days_ago(7), today, "7d"),
        },
        _ => (days_ago(7), today, "7d"),
    };
    (
        from.to_string(),
        (to + chrono::Duration::days(1)).to_string(),
        period.to_string(),
    )
}

/// Service-worker offline page. Must stay user-agnostic: the Cache API ignores
/// `Cache-Control`, so anything personal would leak to the next user.
#[derive(Template)]
#[template(path = "offline.html")]
pub struct OfflineTemplate {
    pub git_version: &'static str,
}

/// Short, not `immutable`: the URL has no build stamp.
const OFFLINE_CACHE_CONTROL: &str = "public, max-age=3600";

/// `GET /offline`. No auth. The explicit public `Cache-Control` both prevents
/// `no-store` and tells `slide_session_cookie` not to attach a session cookie.
pub async fn offline_page() -> Response {
    (
        [(header::CACHE_CONTROL, OFFLINE_CACHE_CONTROL)],
        OfflineTemplate {
            git_version: crate::GIT_VERSION,
        },
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTemplate {
    /// First-run setup link; only when no accounts exist.
    pub setup_available: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    pub local_auth_enabled: bool,
    /// CSRF token for the no-JS `POST /login`, from the anonymous session.
    pub csrf_token: String,
    /// Sign-in error for the no-JS path.
    pub error: Option<String>,
    /// Validated page to land on after sign-in; see [`login_next`].
    pub next: Option<String>,
}

/// `/login?next=` (and the form's `next` field) when it is a safe, same-origin
/// page; see [`crate::middleware::auth::LoginRedirect`].
pub fn login_next(raw: Option<&str>) -> Option<String> {
    use crate::handlers::return_to::{is_login_destination, safe_return_to};
    raw.and_then(|r| safe_return_to(r, is_login_destination))
}

/// `/login` query.
#[derive(serde::Deserialize)]
pub struct LoginQuery {
    pub next: Option<String>,
}

pub async fn login_page(
    auth: Option<PageAuthUser>,
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    flash: Flash,
    Query(query): Query<LoginQuery>,
) -> Response {
    // Not `next`: a signed-in user sent here was refused a page (e.g. admin).
    if auth.is_some() {
        return Redirect::to("/").into_response();
    }

    let setup_available = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_setup(count));

    (
        flash.clone(),
        LoginTemplate {
            setup_available,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            local_auth_enabled: !state.config.disable_local_auth,
            csrf_token: crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
            error: None,
            next: login_next(query.next.as_deref()),
        },
    )
        .into_response()
}

#[derive(Template)]
#[template(path = "setup.html")]
pub struct SetupTemplate {
    pub error: Option<String>,
    pub flash_messages: Vec<FlashMessage>,
    pub git_version: &'static str,
    /// Passed through so `minlength`/`maxlength` match server validation.
    pub password_min_length: usize,
    pub password_max_length: usize,
    /// CSRF token for the no-JS `POST /setup`.
    pub csrf_token: String,
}

/// `GET /setup` — first-run only; redirects to `/login` once any account exists.
pub async fn setup_page(
    State(state): State<AppState>,
    jar: axum_extra::extract::CookieJar,
    flash: Flash,
) -> Response {
    let can_setup = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_setup(count));

    if !can_setup {
        return Redirect::to("/login").into_response();
    }

    (
        flash.clone(),
        SetupTemplate {
            error: None,
            flash_messages: flash.messages,
            git_version: crate::GIT_VERSION,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
            csrf_token: crate::middleware::csrf_token_from_jar(&jar, &state.config.secret),
        },
    )
        .into_response()
}

/// Invite "set your password" page: form, dead end, or throttled notice. The
/// dead end must not reveal why (unknown / expired / used) or whose link it is.
#[derive(Template)]
#[template(path = "invite.html")]
pub struct InviteTemplate {
    pub git_version: &'static str,
    /// Anonymous-session CSRF token; empty when no form renders.
    pub csrf_token: String,
    /// `None` renders the dead end; `Some` renders the form for that account.
    pub username: Option<String>,
    /// Echoed into the form action so the POST lands on the same link.
    pub token: String,
    pub error: Option<String>,
    pub password_min_length: usize,
    pub password_max_length: usize,
}

impl InviteTemplate {
    pub fn form(token: &str, username: String, csrf_token: String) -> Self {
        Self {
            git_version: crate::GIT_VERSION,
            csrf_token,
            username: Some(username),
            token: token.to_string(),
            error: None,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
        }
    }

    pub fn error(token: &str, username: String, message: &str, csrf_token: String) -> Self {
        Self {
            error: Some(message.to_string()),
            ..Self::form(token, username, csrf_token)
        }
    }

    /// The one response every failing token gets, whatever the reason.
    pub fn invalid() -> Self {
        Self {
            git_version: crate::GIT_VERSION,
            // Renders no form, so there is nothing to protect.
            csrf_token: String::new(),
            username: None,
            token: String::new(),
            error: None,
            password_min_length: crate::auth::PASSWORD_MIN_LENGTH,
            password_max_length: crate::auth::PASSWORD_MAX_LENGTH,
        }
    }

    /// Rate-limited; unlike [`InviteTemplate::invalid`], the link may be fine.
    pub fn throttled(retry_after_secs: u64) -> Self {
        Self {
            error: Some(format!(
                "Too many attempts. Please try again in {retry_after_secs} seconds."
            )),
            ..Self::invalid()
        }
    }
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
        let (entries, next_cursor) = build_entries_page(
            &state,
            user_id,
            filter,
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
                q: None,
                snapshot: query.snapshot.clone(),
                csrf_token: auth_user.csrf_token.clone(),
            },
        )
            .into_response();
    }

    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
        entry::EntrySortOrder::PublishedAt,
        page_size,
        None,
    )
    .await;

    // Empty inbox: onboarding if there are no feeds, else "All caught up".
    let no_feeds =
        entries.is_empty() && feed::count_by_user(&state.db, user_id).await.unwrap_or(0) == 0;

    let entries_layout = EntriesLayoutContext {
        active: "unread",
        empty_title: "All caught up",
        empty_detail: "You've read every unread entry — new items land here as your feeds refresh.",
        path: "/".to_string(),
        mark_as_read_scope: Some("user/-/state/com.google/reading-list".to_string()),
        show_mark_above: true,
        onboarding: no_feeds,
        snapshot_at: snapshot_now(),
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

/// Pull a one-time invite link out of the flash. The message is replaced, not
/// dropped: `Flash` only clears the cookie when messages remain, so an empty
/// list would leave the link to reappear on the next load.
fn extract_invite_link(flash: &mut Flash) -> Option<String> {
    let link = flash
        .messages
        .iter()
        .find(|m| m.message.contains("/invite/"))
        .map(|m| m.message.clone())?;

    flash.messages = vec![crate::middleware::flash::FlashMessage::success(
        "Account link ready.",
    )];
    Some(link)
}

pub async fn admin_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    mut flash: Flash,
) -> (Flash, AdminTemplate) {
    let invite_link = extract_invite_link(&mut flash);
    let auth_user = PageAuthUser {
        user: admin.user.clone(),
        session: admin.session.clone(),
        via_forward_auth: admin.via_forward_auth,
        csrf_token: admin.csrf_token.clone(),
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let original_admin_id = admin.session.original_user_id.unwrap_or(admin.user.id);
    let effective_admin_id = admin.user.id;

    let users = crate::models::user::list_all(&state.db)
        .await
        .ok()
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
                // `"!"` marks an account that never set a password.
                awaiting_password: u.password_hash == "!",
                invite_expires_at: None,
            }
        })
        .collect();

    // Per-row lookup is fine: self-hosted instances have few accounts.
    let mut users: Vec<AdminUserView> = users;
    for row in &mut users {
        row.invite_expires_at = crate::models::user_invite::find_live_for_user(&state.db, row.id)
            .await
            .ok()
            .flatten()
            .map(|invite| invite.expires_at.format("%Y-%m-%d %H:%M").to_string());
    }

    let can_create_account = crate::models::user::count(&state.db)
        .await
        .is_ok_and(|count| state.config.can_create_account(count));

    // Must mirror `handlers::admin::require_recent_authentication`.
    let needs_reauth = !admin.via_forward_auth
        && !admin.session.authenticated_recently(chrono::Utc::now())
        && !state.config.disable_local_auth;

    (
        flash,
        AdminTemplate {
            title: "Admin Panel",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            users,
            can_create_account,
            needs_reauth,
            invite_link,
        },
    )
}

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
        let linkding_configured = linkding
            .is_some_and(super::super::services::save::linkding::LinkdingConfig::is_configured);
        let linkding_api_url = linkding.map(|c| c.api_url.clone()).unwrap_or_default();

        let kagi = save_config.kagi.as_ref();
        let kagi_configured =
            kagi.is_some_and(super::super::services::summarize::kagi::KagiConfig::is_configured);
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

/// `/feeds` filter/sort query; the URL is the source of truth.
#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<String>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}

/// `GET /feeds`.
pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<FeedsQuery>,
) -> (Flash, FeedsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    // Gated on the setting, so an opted-in reader sees the column even when empty.
    let open_rate_shown = user_settings::get_pixel_tracking_enabled_at(&state.db, user_id)
        .await
        .unwrap_or(None)
        .is_some();

    let (mut rows, categories, total_feed_count) = {
        let cats = category::list_by_user(&state.db, user_id)
            .await
            .unwrap_or_default();
        let all_feeds = feed::list_by_user(&state.db, user_id)
            .await
            .unwrap_or_default();
        let unread_map = entry::count_unread_by_feed(&state.db, user_id)
            .await
            .unwrap_or_default();
        let open_rates: std::collections::HashMap<i64, crate::models::entry_open::FeedOpenRate> =
            crate::models::entry_open::open_rates_by_feed(&state.db, user_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| (r.feed_id, r))
                .collect();

        let cat_map: std::collections::HashMap<i64, String> =
            cats.iter().map(|cat| (cat.id, cat.name.clone())).collect();
        let mut count_by_cat: std::collections::HashMap<i64, i64> =
            std::collections::HashMap::new();
        for f in &all_feeds {
            *count_by_cat.entry(f.category_id).or_insert(0) += 1;
        }

        let total_feed_count = all_feeds.len() as i64;

        let feed_ids: Vec<i64> = all_feeds.iter().map(|f| f.id).collect();
        let feeds_with_icon = crate::models::image::existing_ids(
            &state.db,
            crate::models::image::ENTITY_FEED,
            &feed_ids,
        )
        .await
        .unwrap_or_default();

        let row_views: Vec<FeedRowView> = all_feeds
            .into_iter()
            .map(|f| {
                let has_icon = feeds_with_icon.contains(&f.id);
                let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
                let (updated_rel, updated_dt) = if f.feed_updated_at.is_some() {
                    format_relative_time(f.feed_updated_at)
                } else if f.fetched_at.is_some_and(|ft| {
                    (chrono::Utc::now() - ft).num_days() <= time_format::FRESH_MAX_DAYS
                }) {
                    ("No date info".to_string(), String::new())
                } else {
                    ("Never".to_string(), String::new())
                };
                let (freshness_class, freshness_key) =
                    compute_freshness(f.feed_updated_at, f.fetched_at);
                let open_rate = open_rates.get(&f.id);
                let open_rate_percent =
                    open_rate.and_then(crate::models::entry_open::FeedOpenRate::percent);
                // Show raw counts: "40%" alone hides the sample size.
                let open_rate_label = open_rate_percent
                    .zip(open_rate)
                    .map(|(pct, r)| format!("{pct}% ({}/{})", r.opened, r.tracked));
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
                    open_rate_label,
                    open_rate_percent,
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

        (row_views, cat_options, total_feed_count)
    };

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
        // Ascending (least-opened first); feeds without a rate sort last.
        "open_rate" => rows.sort_by_key(|a| (a.open_rate_percent.is_none(), a.open_rate_percent)),
        _ => rows.sort_by_key(|a| a.title.to_lowercase()),
    }
    let active_filter = match active_filter_raw.as_str() {
        "errors" | "stale" | "all" => active_filter_raw,
        _ => "all".to_string(),
    };

    let cat_param = active_category
        .map(|c| format!("category={c}&"))
        .unwrap_or_default();
    // This exact view, for row forms and the edit link to come back to.
    let return_to = {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        if let Some(c) = active_category {
            q.append_pair("category", &c.to_string());
        }
        q.append_pair("sort", &active_sort);
        q.append_pair("filter", &active_filter);
        format!("/feeds?{}", q.finish())
    };
    let edit_query = crate::handlers::return_to::return_to_query(&return_to);
    let filter_links = vec![
        FeedFilterLink {
            label: "All",
            href: format!("/feeds?{cat_param}sort={active_sort}&filter=all"),
            active: active_filter == "all",
        },
        FeedFilterLink {
            label: "Errors",
            href: format!("/feeds?{cat_param}sort={active_sort}&filter=errors"),
            active: active_filter == "errors",
        },
        FeedFilterLink {
            label: "Stale",
            href: format!("/feeds?{cat_param}sort={active_sort}&filter=stale"),
            active: active_filter == "stale",
        },
    ];

    (
        flash,
        FeedsTemplate {
            title: "Feeds",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            feeds: rows,
            categories,
            total_feed_count,
            active_filter,
            active_sort,
            active_category_id: active_category,
            filter_links,
            return_to,
            edit_query,
            fresh_max_days: time_format::FRESH_MAX_DAYS,
            warning_max_days: time_format::WARNING_MAX_DAYS,
            open_rate_shown,
            min_tracked_for_rate: crate::models::entry_open::MIN_TRACKED_FOR_RATE,
        },
    )
}

/// `?return_to=` on pages reached from a filtered list.
#[derive(serde::Deserialize)]
pub struct ReturnToQuery {
    pub return_to: Option<String>,
}

/// `GET /feeds/{id}/edit`.
pub async fn feed_edit_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Path(id): Path<i64>,
    Query(query): Query<ReturnToQuery>,
) -> Result<Response, AppError> {
    let user_id = auth_user.user.id;
    let return_to = crate::handlers::return_to::feeds_list(query.return_to.as_deref());

    let lookup = async {
        let f = feed::find_by_id(&state.db, id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        category::find_by_id_and_user(&state.db, f.category_id, user_id)
            .await?
            .ok_or(AppError::FeedNotFound)?;
        let cats = category::list_by_user(&state.db, user_id).await?;
        let referrer_suggestions = referrer_suggestions(f.site_url.as_deref(), &f.url);
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
                referrer_suggestions,
            },
            cats.into_iter()
                .map(|c| FeedCategoryOption {
                    id: c.id,
                    name: c.name,
                    feed_count: 0,
                })
                .collect::<Vec<_>>(),
        ))
    }
    .await;

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
            csrf_token: auth_user.csrf_token.clone(),
            feed: feed_view,
            categories: cats,
            return_to,
            user_agent_suggestions: crate::config::CUSTOM_USER_AGENT_SUGGESTIONS,
        },
    )
        .into_response())
}

/// `GET /feeds/import`.
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
            csrf_token: auth_user.csrf_token.clone(),
        },
    )
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
    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        tab.filter,
        entry::EntrySortOrder::PublishedAt,
        page_size,
        cursor,
    )
    .await;

    if fragment {
        return (
            flash,
            EntriesFragmentTemplate {
                entries,
                next_cursor,
                path: tab.path.into(),
                status_filter: None,
                q: None,
                snapshot: query.snapshot,
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
            entries_layout: EntriesLayoutContext {
                active: tab.active,
                empty_title: tab.empty_title,
                empty_detail: tab.empty_detail,
                path: tab.path.to_string(),
                show_tab_bar: true,
                mark_as_read_scope: tab.mark_as_read_scope.map(str::to_string),
                snapshot_at: snapshot_now(),
                ..Default::default()
            },
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

/// `GET /settings`. Admin-only: it exposes deployment internals.
pub async fn settings_page(
    admin: PageAdminUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, SettingsTemplate) {
    let auth_user = PageAuthUser {
        user: admin.user.clone(),
        session: admin.session.clone(),
        via_forward_auth: admin.via_forward_auth,
        csrf_token: admin.csrf_token.clone(),
    };
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_agent_is_default = state.config.user_agent == crate::config::DEFAULT_USER_AGENT;

    (
        flash,
        SettingsTemplate {
            title: "App",
            git_version: crate::GIT_VERSION,
            layout,
            database_url: crate::config::redact_database_url(&state.config.database_url),
            server_bind: state.config.server_bind,
            user_agent: state.config.user_agent.clone(),
            user_agent_is_default,
            multi_user_enabled: state.config.multi_user_enabled,
            secret_generated: state.config.secret_generated,
            webauthn_rp_id: state.config.webauthn_rp_id.clone(),
            webauthn_rp_origin: state.config.webauthn_rp_origin.clone(),
            webauthn_rp_name: state.config.webauthn_rp_name.clone(),
            auth_proxy_header: state.config.auth_proxy_header.clone(),
            trusted_proxy_networks: state
                .config
                .trusted_proxy_networks
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            auth_proxy_user_creation: state.config.auth_proxy_user_creation,
            auth_proxy_groups_header: state.config.auth_proxy_groups_header.clone(),
            auth_proxy_admin_group: state.config.auth_proxy_admin_group.clone(),
            disable_local_auth: state.config.disable_local_auth,
            auth_proxy_logout_url: state
                .config
                .auth_proxy_logout_url
                .clone()
                .unwrap_or_default(),
        },
    )
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
    let search = query.q.clone().filter(|s| !s.trim().is_empty());
    filter.search = search.clone();
    let cursor = query
        .after
        .as_deref()
        .and_then(entry::ContinuationCursor::parse);

    let (entries, next_cursor) = build_entries_page(
        &state,
        user_id,
        filter,
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
            q: search,
            snapshot: query.snapshot.clone(),
            csrf_token: auth_user.csrf_token.clone(),
        };
        return (flash, fragment).into_response();
    }

    // Unread-only regardless of tab, to match what `mark_read_by_filter` touches.
    let matching_count = if let Some(ref s) = search {
        let mark_filter = entry::EntryFilter {
            category_id: scope.category_id,
            feed_id: scope.feed_id,
            search: Some(s.clone()),
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
        show_mark_above: search.is_none(),
        snapshot_at: snapshot_now(),
        search,
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

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

/// `GET /search` — newest 50 matches, no pagination; empty `q` shows the form.
pub async fn search_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<SearchQuery>,
) -> (Flash, SearchTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let q = query.q.unwrap_or_default().trim().to_string();
    let user_id = auth_user.user.id;

    let mut error: Option<String> = None;
    let results = if q.is_empty() {
        Vec::new()
    } else {
        match entry::query::parse(&q) {
            Err(e) => {
                // Byte offset → 1-based character position for the message.
                let char_pos = q.get(..e.position).map_or(0, |p| p.chars().count()) + 1;
                error = Some(format!(
                    "Search syntax error (near character {char_pos}): {}",
                    e.message
                ));
                Vec::new()
            }
            Ok(ast) => {
                let terms = entry::query::free_text_terms(&ast);
                let needles: Vec<&str> = terms.iter().map(String::as_str).collect();
                let filter = entry::EntryFilter {
                    query: Some(ast),
                    ..Default::default()
                };
                const LIMIT: i64 = 50;
                let rows = entry::list_by_user(
                    &state.db,
                    user_id,
                    &filter,
                    entry::EntrySortOrder::PublishedAt,
                    LIMIT,
                    0,
                )
                .await
                .unwrap_or_default();
                rows.into_iter()
                    .map(|e| {
                        let title = e
                            .entry
                            .title
                            .clone()
                            .unwrap_or_else(|| "(no title)".to_string());
                        let snippet = build_snippet(
                            e.entry.content.as_deref().or(e.entry.summary.as_deref()),
                            &needles,
                            200,
                        );
                        let (published_relative, published_at_iso) =
                            format_relative_time(e.entry.published_at);
                        SearchResultView {
                            entry_id: e.entry.id,
                            title_html: highlight_html(&title, &needles),
                            feed_title: e.feed_title.clone().unwrap_or_else(|| e.feed_url.clone()),
                            published_relative,
                            published_at_iso,
                            snippet_html: highlight_html(&snippet, &needles),
                        }
                    })
                    .collect()
            }
        }
    };

    (
        flash,
        SearchTemplate {
            title: "Search",
            git_version: crate::GIT_VERSION,
            layout,
            q,
            error,
            results,
        },
    )
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
    let search = q.as_ref().and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });

    // Must guard here: a blank `q` would match, and mark read, the whole scope.
    let Some(search) = search else {
        let redirect = build_scoped_redirect(base_path, None, status.as_deref());
        return FlashRedirect::info(&redirect, "No search term — nothing marked.").into_response();
    };

    let filter = entry::EntryFilter {
        category_id,
        feed_id,
        search: Some(search.clone()),
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

/// Chrome-wrapped 404 page for logged-in routes.
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

/// Build an [`ErrorTemplate`] 404.
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

/// Router fallback: 404 page when logged in, login redirect otherwise.
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

/// `/settings` server-config table. `git_version` duplicates `layout`'s because
/// `base.html` references it outside the layout's blocks.
#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub database_url: String,
    pub server_bind: std::net::SocketAddr,
    pub user_agent: String,
    pub user_agent_is_default: bool,
    pub multi_user_enabled: bool,
    pub secret_generated: bool,
    pub webauthn_rp_id: String,
    pub webauthn_rp_origin: String,
    pub webauthn_rp_name: String,
    pub auth_proxy_header: String,
    pub trusted_proxy_networks: String,
    pub auth_proxy_user_creation: bool,
    pub auth_proxy_groups_header: String,
    pub auth_proxy_admin_group: String,
    pub disable_local_auth: bool,
    pub auth_proxy_logout_url: String,
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
    /// See [`SetupTemplate::password_min_length`].
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

/// One row of the SSR `/admin` user table.
pub struct AdminUserView {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub disabled: bool,
    pub created_at: String,
    pub created_at_iso: String,
    pub is_self: bool,
    /// No password set yet (invite not redeemed).
    pub awaiting_password: bool,
    /// Expiry of the outstanding invite; the link itself is unrecoverable.
    pub invite_expires_at: Option<String>,
}

/// Per-route template for `/admin`.
#[derive(Template)]
#[template(path = "admin.html")]
pub struct AdminTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub users: Vec<AdminUserView>,
    /// One-time invite link from the last create/reissue; shown once only.
    pub invite_link: Option<String>,
    pub can_create_account: bool,
    /// Show the re-auth form up front, before any destructive action.
    pub needs_reauth: bool,
}

/// One daily-read chart bar; may span several days once bucketed (see
/// [`crate::models::statistics::bucket_daily_counts`]).
pub struct DailyReadView {
    pub date_label: String,
    pub count: i64,
    /// See `bar_percent`.
    pub height_percent: u8,
    pub short_label: String,
    /// Only the first busiest bucket, so just one count label is drawn.
    pub is_max: bool,
}

/// One row in the "Entries by Category" list, with pre-computed bar width.
pub struct CategoryStatsView {
    pub name: String,
    pub count: i64,
    pub width_percent: u8,
}

/// One row in the "Top Feeds" list, with pre-computed bar width.
pub struct FeedStatsView {
    pub title: String,
    pub count: i64,
    pub width_percent: u8,
}

/// "Feeds by Open Rate" row. The bar is the absolute rate, not scaled to the max.
pub struct FeedOpenRateView {
    pub title: String,
    pub percent: i64,
    pub opened: i64,
    pub tracked: i64,
    pub width_percent: u8,
}

/// `count` as a whole percentage of `max`: whole because it selects a `pct-N`
/// class (CSP forbids inline `style`). Non-zero counts floor at 1%.
fn bar_percent(count: i64, max: i64) -> u8 {
    if max <= 0 || count <= 0 {
        return 0;
    }
    let pct = count.saturating_mul(100).saturating_add(max / 2) / max;
    u8::try_from(pct.clamp(1, 100)).unwrap_or(100)
}

/// Format a byte count for display (binary units, one decimal above 1 KB).
fn format_db_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Site-wide stats block shown to non-masquerading admins.
pub struct AdminStatsView {
    pub total_users: i64,
    pub total_feeds: i64,
    pub total_entries: i64,
    pub read_rate_fmt: String,
}

/// Reclaimable-space card, rendered only when the backend can measure it.
pub struct ReclaimableView {
    pub size_fmt: String,
    pub frag_pct: i64,
}

/// Database storage + record stats block (admin, non-masquerading).
pub struct AdminDatabaseStatsView {
    pub size_fmt: String,
    /// `None` on `PostgreSQL`, which can't report free space without an extension.
    pub reclaimable: Option<ReclaimableView>,
    pub total_entries: i64,
    pub avg_per_day_fmt: String,
    pub coverage_fmt: String,
    pub tombstone_count: i64,
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
    /// Lowest open rate first; empty when tracking is off or no feed has enough data.
    pub open_rate_feeds: Vec<FeedOpenRateView>,
    /// Start of the tracked window (retention may push it past the opt-in date).
    pub tracked_since: Option<String>,
    pub min_tracked_for_rate: i64,
    pub admin: Option<AdminStatsView>,
    pub admin_db: Option<AdminDatabaseStatsView>,
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
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub categories: Vec<CategoryRowView>,
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
    /// `"42% (10/24)"`; `None` below `entry_open::MIN_TRACKED_FOR_RATE`.
    pub open_rate_label: Option<String>,
    /// Sort key; `None` sorts last.
    pub open_rate_percent: Option<i64>,
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
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub feeds: Vec<FeedRowView>,
    pub categories: Vec<FeedCategoryOption>,
    pub total_feed_count: i64,
    pub active_filter: String,
    pub active_sort: String,
    pub active_category_id: Option<i64>,
    pub filter_links: Vec<FeedFilterLink>,
    /// This filtered view, posted back by row forms as `return_to`.
    pub return_to: String,
    /// `return_to` as a query string for the edit links.
    pub edit_query: String,
    /// Thresholds `compute_freshness` applies, shown in the help text.
    pub fresh_max_days: i64,
    pub warning_max_days: i64,
    /// Show the "Open rate" column only when tracking is enabled.
    pub open_rate_shown: bool,
    pub min_tracked_for_rate: i64,
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
    /// Referrer `<datalist>` options; see `referrer_suggestions`.
    pub referrer_suggestions: Vec<String>,
}

/// Deduplicated HTTP(S) origins to offer as `Referer`: the site first (it
/// serves the embedded images), then the feed URL's origin.
fn referrer_suggestions(site_url: Option<&str>, feed_url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for candidate in [site_url, Some(feed_url)].into_iter().flatten() {
        let Ok(parsed) = url::Url::parse(candidate.trim()) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let origin = match parsed.port() {
            Some(port) => format!("{}://{host}:{port}/", parsed.scheme()),
            None => format!("{}://{host}/", parsed.scheme()),
        };
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out
}

/// Per-route template for `/feeds/{id}/edit`.
#[derive(Template)]
#[template(path = "feed_edit.html")]
pub struct FeedEditTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
    pub feed: FeedEditView,
    pub categories: Vec<FeedCategoryOption>,
    /// The list Save and Cancel return to; see [`crate::handlers::return_to`].
    pub return_to: String,
    /// User-agent `<datalist>` options.
    pub user_agent_suggestions: &'static [&'static str],
}

/// Per-route template for `/feeds/import`.
#[derive(Template)]
#[template(path = "feeds_import.html")]
pub struct FeedsImportTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`].
    pub csrf_token: String,
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

/// `/search` result row. `title_html`/`snippet_html` are pre-escaped with
/// `<mark>` highlights; render with `|safe`.
pub struct SearchResultView {
    pub entry_id: i64,
    pub title_html: String,
    pub feed_title: String,
    pub published_relative: String,
    /// RFC 3339, or empty when unknown.
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
    pub error: Option<String>,
    pub results: Vec<SearchResultView>,
}

/// `GET /statistics`.
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

    let (overview, daily, cats, feeds, admin_counts, admin_entry_stats, admin_db_stats) = {
        let overview =
            crate::models::statistics::get_personal_overview(&state.db, user_id, &from_c, &to_c)
                .await
                .unwrap_or_default();
        let daily = crate::models::statistics::get_daily_read_counts(
            &state.db,
            user_id,
            &chart_from_c,
            &to_c,
        )
        .await
        .unwrap_or_default();
        let cats =
            crate::models::statistics::get_entries_by_category(&state.db, user_id, &from_c, &to_c)
                .await
                .unwrap_or_default();
        let feeds =
            crate::models::statistics::get_top_feeds(&state.db, user_id, &from_c, &to_c, 10)
                .await
                .unwrap_or_default();
        let admin_counts = if show_admin_stats {
            crate::models::statistics::get_admin_counts(&state.db)
                .await
                .ok()
        } else {
            None
        };
        let admin_entry_stats = if show_admin_stats {
            crate::models::statistics::get_admin_entry_stats(&state.db, &from_c, &to_c)
                .await
                .ok()
        } else {
            None
        };
        // Plain read-through: a concurrent miss just recomputes harmlessly.
        let admin_db_stats = if show_admin_stats {
            if let Some(cached) = state.admin_db_stats_cache.get(&()) {
                Some(cached)
            } else {
                let fresh = crate::models::statistics::get_admin_database_stats(&state.db)
                    .await
                    .ok();
                if let Some(stats) = fresh.clone() {
                    state.admin_db_stats_cache.insert((), stats);
                }
                fresh
            }
        } else {
            None
        };
        (
            overview,
            daily,
            cats,
            feeds,
            admin_counts,
            admin_entry_stats,
            admin_db_stats,
        )
    };

    let (custom_from, custom_to) = if active_period == "custom" {
        (
            query.from.clone().unwrap_or_default(),
            query.to.clone().unwrap_or_default(),
        )
    } else {
        (String::new(), String::new())
    };

    // Cap bar count so long ranges stay tappable on mobile.
    const MAX_DAILY_BARS: usize = 14;
    let buckets = crate::models::statistics::bucket_daily_counts(&daily, MAX_DAILY_BARS);

    let daily_max = buckets.iter().map(|b| b.count).max().unwrap_or(0);
    let cat_max = cats.iter().map(|c| c.count).max().unwrap_or(0);
    let feed_max = feeds.iter().map(|f| f.count).max().unwrap_or(0);

    let max_idx = if daily_max > 0 {
        buckets.iter().position(|b| b.count == daily_max)
    } else {
        None
    };
    let daily_read_counts = buckets
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let short_label = b.start.format("%m/%d").to_string();
            let date_label = if b.start == b.end {
                b.start.format("%Y-%m-%d").to_string()
            } else {
                format!("{} – {}", b.start.format("%Y-%m-%d"), b.end.format("%m/%d"))
            };
            DailyReadView {
                date_label,
                count: b.count,
                height_percent: bar_percent(b.count, daily_max),
                short_label,
                is_max: Some(i) == max_idx,
            }
        })
        .collect();

    let categories = cats
        .into_iter()
        .map(|c| CategoryStatsView {
            count: c.count,
            width_percent: bar_percent(c.count, cat_max),
            name: c.name,
        })
        .collect();

    let top_feeds = feeds
        .into_iter()
        .map(|f| FeedStatsView {
            count: f.count,
            width_percent: bar_percent(f.count, feed_max),
            title: f.title,
        })
        .collect();

    // Ignores the period filter: open rate is only meaningful since opt-in.
    let mut open_rate_feeds: Vec<FeedOpenRateView> =
        crate::models::entry_open::open_rates_by_feed(&state.db, user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                let percent = r.percent()?;
                Some(FeedOpenRateView {
                    title: r.title.unwrap_or_else(|| "(untitled feed)".to_string()),
                    percent,
                    opened: r.opened,
                    tracked: r.tracked,
                    width_percent: u8::try_from(percent.clamp(0, 100)).unwrap_or(100),
                })
            })
            .collect();
    open_rate_feeds.sort_by_key(|f| (f.percent, f.title.to_lowercase()));
    open_rate_feeds.truncate(10);

    let tracked_since = crate::models::entry_open::tracking_window(&state.db, user_id)
        .await
        .unwrap_or(crate::models::entry_open::TrackingWindow {
            enabled_at: None,
            oldest_tracked: None,
        })
        .tracked_since()
        .map(|t| t.format("%Y-%m-%d").to_string());

    let admin = match (admin_counts, admin_entry_stats) {
        (Some(c), Some(e)) => Some(AdminStatsView {
            total_users: c.total_users,
            total_feeds: c.total_feeds,
            total_entries: e.total_entries,
            read_rate_fmt: format!("{:.1}", e.read_rate()),
        }),
        _ => None,
    };

    let admin_db = admin_db_stats.map(|s| AdminDatabaseStatsView {
        size_fmt: format_db_bytes(s.db_size_bytes),
        reclaimable: s.reclaimable.map(|r| ReclaimableView {
            size_fmt: format_db_bytes(r.bytes),
            frag_pct: (r.fragmentation_ratio * 100.0).round() as i64,
        }),
        total_entries: s.total_entries,
        avg_per_day_fmt: format!("{}", s.avg_new_entries_per_day.round() as i64),
        coverage_fmt: format!("{}d", s.coverage_days.round() as i64),
        tombstone_count: s.tombstone_count,
    });

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
            open_rate_feeds,
            tracked_since,
            min_tracked_for_rate: crate::models::entry_open::MIN_TRACKED_FOR_RATE,
            admin,
            admin_db,
        },
    )
}

/// `GET /categories`.
pub async fn categories_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
) -> (Flash, CategoriesTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let user_id = auth_user.user.id;

    let categories = async {
        let cats = crate::models::category::list_by_user(&state.db, user_id).await?;
        let feeds = crate::models::feed::list_by_user(&state.db, user_id).await?;
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
    }
    .await
    .ok()
    .unwrap_or_default();

    (
        flash,
        CategoriesTemplate {
            title: "Categories",
            git_version: crate::GIT_VERSION,
            layout,
            csrf_token: auth_user.csrf_token.clone(),
            categories,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{build_snippet, highlight_html, referrer_suggestions, row_view_from};
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
    fn referrer_suggestion_leads_with_the_site_the_feed_describes() {
        assert_eq!(
            referrer_suggestions(
                Some("https://example.com/blog"),
                "https://cdn.example.net/rss"
            ),
            vec![
                "https://example.com/".to_string(),
                "https://cdn.example.net/".to_string()
            ]
        );
    }

    #[test]
    fn referrer_suggestions_are_deduplicated() {
        assert_eq!(
            referrer_suggestions(Some("https://example.com/"), "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestion_falls_back_to_the_feed_url() {
        assert_eq!(
            referrer_suggestions(None, "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
        // A blank site_url is what the DB holds for "unset", not a URL.
        assert_eq!(
            referrer_suggestions(Some(""), "https://example.com/feed.xml"),
            vec!["https://example.com/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestion_keeps_a_non_default_port() {
        assert_eq!(
            referrer_suggestions(None, "http://localhost:8080/feed.xml"),
            vec!["http://localhost:8080/".to_string()]
        );
    }

    #[test]
    fn referrer_suggestions_skip_what_cannot_be_a_referer() {
        assert!(referrer_suggestions(None, "not a url").is_empty());
        assert!(referrer_suggestions(Some("file:///etc/hosts"), "not a url").is_empty());
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

    #[test]
    fn test_format_db_bytes() {
        assert_eq!(super::format_db_bytes(0), "0 B");
        assert_eq!(super::format_db_bytes(512), "512 B");
        assert_eq!(super::format_db_bytes(1536), "1.5 KB");
        assert_eq!(super::format_db_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(super::format_db_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
