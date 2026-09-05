use askama::Template;
use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::{
    AppState,
    error::{AppError, AppResult},
    handlers::pages::{EntryRowView, ReadingPaneView, format_relative_time, row_view_from},
    middleware::auth::PageAuthUser,
    middleware::flash::{FlashMessage, FlashRedirect},
    models::{entry, entry_summary, user_settings},
    services::{
        SummaryJob, SummaryStatus, fetch_and_extract,
        pixel::PixelContext,
        sanitize_html, sanitize_summary,
        save::{BookmarkData, linkding},
        strip_tracking_params,
    },
};

/// Fragment template for the reading pane — renders `_reading_pane.html`
/// and is returned by `GET /entries/{id}/fragment`.
#[derive(Template)]
#[template(path = "_reading_pane.html")]
pub struct ReadingPaneFragment {
    pub pane: ReadingPaneView,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`]. Carried by
    /// every fragment that renders a form, because the markup is shared with
    /// the full-page render and `csrf.js` is not guaranteed to be running.
    pub csrf_token: String,
}

impl IntoResponse for ReadingPaneFragment {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// One-shot flash payload for the swap-helper `<template data-flash>` block.
/// `level` is one of `success | error | info | warning` — matching the
/// `<rdrs-flash>` API in `static/js/components/rdrs-flash.js`.
#[derive(Debug, Clone)]
pub struct FlashPayload {
    pub level: &'static str,
    pub message: String,
}

impl FlashPayload {
    /// The same message as a cookie-borne [`FlashMessage`], for the scriptless
    /// path: there is no swap helper to hand a `<template data-flash>` to, so
    /// the message has to survive a redirect and be rendered by the next page.
    fn to_message(&self) -> FlashMessage {
        match self.level {
            "success" => FlashMessage::success(&self.message),
            "error" => FlashMessage::error(&self.message),
            "warning" => FlashMessage::warning(&self.message),
            // `info` and anything that ever drifts out of the four levels the
            // `<rdrs-flash>` API accepts.
            _ => FlashMessage::info(&self.message),
        }
    }
}

/// Multi-target response: swaps the reading pane and (optionally) pops a
/// toast on the page-level `<rdrs-flash>`. Returned by the Save /
/// Fetch-Full-Content form-actions.
#[derive(Template)]
#[template(path = "_reading_pane_with_flash.html")]
pub struct ReadingPaneWithFlash {
    pub pane: ReadingPaneView,
    pub flash: Option<FlashPayload>,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

impl IntoResponse for ReadingPaneWithFlash {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Multi-target response for `POST /entries/{id}/summarize`. Swaps only
/// the `#rp-summary-container` block so the reading-pane article body
/// (which may currently hold an externally-fetched full-content view)
/// stays put.
#[derive(Template)]
#[template(path = "_summarize_pending.html")]
pub struct SummarizePending {
    pub id: i64,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

impl IntoResponse for SummarizePending {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Response for `POST /entries/{id}/summarize/cancel`. Swaps
/// `#rp-summary-container` back to its empty state after a cancel / clear.
#[derive(Template)]
#[template(path = "_summary_cleared.html")]
pub struct SummarizeCleared;

impl IntoResponse for SummarizeCleared {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// `GET /entries/{id}/summary/fragment` — re-renders `#rp-summary-container`
/// for the entry's current summary state. Used by the SSE client to refresh
/// the open reading pane when a `summary` event arrives.
#[derive(Template)]
#[template(path = "_summary_fragment.html")]
pub struct SummaryFragment {
    pub pane: ReadingPaneView,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

impl IntoResponse for SummaryFragment {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// `/{kind}/{id}/entries` with a purely numeric `{id}` — the scoped feed /
/// category list routes.
fn is_scoped_entries_path(path: &str, kind: &str) -> bool {
    path.strip_prefix('/')
        .and_then(|p| p.strip_prefix(kind))
        .and_then(|p| p.strip_prefix('/'))
        .and_then(|p| p.strip_suffix("/entries"))
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// List routes that render entry rows and honour `?entry={id}` to pre-open the
/// reading pane (see `EntriesQuery`). `/search` is intentionally excluded: it
/// uses `SearchQuery` and does not deep-link an entry, so a fragment navigation
/// from search still falls back to All Entries.
fn is_entry_list_path(path: &str) -> bool {
    matches!(
        path,
        "/" | "/entries" | "/entries/read" | "/entries/starred" | "/entries/summarized"
    ) || is_scoped_entries_path(path, "categories")
        || is_scoped_entries_path(path, "feeds")
}

/// The entry-list page a request came from, recovered from the same-origin
/// `Referer`. `None` when the header is absent, unparseable, or points at
/// something that is not an entry list.
///
/// Only the path and query are ever used, so a redirect built from it is always
/// same-origin.
fn referring_entry_list(headers: &HeaderMap) -> Option<url::Url> {
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok())?;
    let url = url::Url::parse(referer).ok()?;
    is_entry_list_path(url.path()).then_some(url)
}

/// Render a recovered referrer back down to the `path?query` form a `Location`
/// header wants.
fn path_and_query(url: &url::Url) -> String {
    match url.query() {
        Some(q) => format!("{}?{}", url.path(), q),
        None => url.path().to_string(),
    }
}

/// A real top-level browser navigation rather than a `fetch()` from the swap
/// helper: browsers tag navigations `Sec-Fetch-Dest: document`, `fetch()` sends
/// `empty`. Every fragment response in this module renders as a blank page when
/// loaded as a document, so the two paths have to be told apart.
fn is_document_navigation(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document")
}

/// A speculative load the reader never asked for — a prefetch or prerender.
/// Opening an entry marks it read, and a browser guessing at what might be
/// clicked next must not get to make that decision.
///
/// `Sec-Purpose` is the current header (prerender sends `prefetch;prerender`,
/// hence the substring test), `Purpose` is what Chromium sent before it, `X-Moz`
/// is Firefox's. Nothing here opts into speculation today, so this guards
/// against a future one or an extension deciding for the reader.
fn is_speculative_load(headers: &HeaderMap) -> bool {
    let header_says_prefetch = |name: &str, exact: bool| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                if exact {
                    v.eq_ignore_ascii_case("prefetch")
                } else {
                    v.to_ascii_lowercase().contains("prefetch")
                }
            })
    };
    header_says_prefetch("sec-purpose", false)
        || header_says_prefetch("purpose", true)
        || header_says_prefetch("x-moz", true)
}

/// Mark a speculative response uncacheable. Without this the browser could
/// serve the reader's real click straight out of its prefetch cache — and since
/// that response was produced *without* the mark-as-read, the entry would
/// silently never become read.
fn deny_storage(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
}

/// Dispatch the open-marks-read side effect: the write goes off the critical
/// path (a detached task — nothing downstream waits on it) and the sidebar's
/// cached unread counts are invalidated and re-pushed over SSE.
fn dispatch_mark_read_on_open(state: &AppState, user_id: i64, entry_id: i64) {
    let db = state.db.clone();
    tokio::spawn(async move {
        if let Err(e) = entry::mark_as_read(&db, entry_id).await {
            tracing::warn!(event = "entry.mark_read_failed", entry_id, error = %e, "async mark_as_read failed");
        }
    });
    state.sidebar_cache.bust(user_id);
    state.events.emit_sidebar(user_id);
}

/// Build the redirect target for a real top-level navigation to the
/// partial-only `/entries/{id}/fragment` route, which renders bare `<template>`
/// blocks and so is blank as a document. The browser must land on a full list
/// page with the pane pre-opened via `?entry={id}`.
///
/// The originating scope is recovered from the `Referer` and preserved along
/// with its filters (`status`, scoped-search `q`). Falls back to All Entries
/// when the referrer is unusable.
fn fragment_document_redirect(headers: &HeaderMap, entry_id: i64) -> String {
    let Some(mut url) = referring_entry_list(headers) else {
        return format!("/entries?entry={entry_id}");
    };

    // Preserve the originating filters, swapping any stale `entry` for this one.
    let preserved: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "entry")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut qp = url.query_pairs_mut();
        qp.clear();
        for (k, v) in &preserved {
            qp.append_pair(k, v);
        }
        qp.append_pair("entry", &entry_id.to_string());
    }

    path_and_query(&url)
}

/// Build the redirect target for a scriptless entry-action POST. The action has
/// already been applied; this only decides where the browser lands.
///
/// Unlike [`fragment_document_redirect`], the referrer is reused *verbatim*: an
/// action fired from a list row must not drag the reader into that entry's pane,
/// and one fired from the pane already carries `?entry=` in its own URL. Falls
/// back to All Entries with the pane opened on the entry — with no SSR flash
/// banner yet, seeing the new state is the only feedback available.
fn action_document_redirect(headers: &HeaderMap, entry_id: i64) -> String {
    match referring_entry_list(headers) {
        Some(url) => path_and_query(&url),
        None => format!("/entries?entry={entry_id}"),
    }
}

/// `GET /entries/{id}/fragment` — returns the reading-pane HTML fragment for
/// the given entry. The entry must belong to the authenticated user; otherwise
/// a 404 is returned (same semantics as the JSON `/api/entries/{id}` endpoint
/// it replaces for SSR consumers).
pub async fn entry_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
    axum::extract::Query(query): axum::extract::Query<FragmentQuery>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;
    let view = query.content_view();

    // Read current state on the READ connection, which a background sync's write
    // transaction does not block under WAL. Before the document-navigation
    // branch below, because opening an entry marks it read and deciding that
    // needs the entry — a scriptless reader is opening it just as much.
    let found = entry::find_by_id_for_user(&state.db, user_id, entry_id).await?;

    let speculative = is_speculative_load(&headers) || query.is_prefetch();
    let mark_read = !speculative && found.as_ref().is_some_and(|e| e.entry.read_at.is_none());

    // `/entries/{id}/fragment` renders only the `<template data-swap-target>`
    // blocks the swap helper consumes, which display as a blank page when loaded
    // as a document. A real browser navigation here — a scriptless click, the
    // swap helper's error fallback, a click landing before `app.js` wires up,
    // open-in-new-tab, a refresh — must not show that. Those redirect to the
    // originating list page with the pane pre-opened, keeping the reader in
    // their current scope instead of dumping them into All Entries.
    //
    // A missing entry redirects too rather than 404ing: the list handlers ignore
    // an unresolvable `?entry=`, so a stale link lands on the list. The
    // `fetch()` path below still 404s, which is what `app.js` falls back on.
    if is_document_navigation(&headers) {
        // Nothing to render on this path, so the write is dispatched here.
        if mark_read {
            dispatch_mark_read_on_open(&state, user_id, entry_id);
        }
        let mut response =
            Redirect::to(&fragment_document_redirect(&headers, entry_id)).into_response();
        if speculative {
            deny_storage(&mut response);
        }
        return Ok(response);
    }

    let mut ewf = found.ok_or(AppError::EntryNotFound)?;
    let status = entry_summary::get_statuses_for_entries(&state.db, user_id, &[entry_id])
        .await?
        .get(&entry_id)
        .copied();

    // Optimistically reflect the read state in the rendered row + pane. Tied to
    // `mark_read`, not to the entry's prior state: a speculative load leaves the
    // entry alone, so it must render it as it actually is.
    if mark_read {
        ewf.entry.read_at = Some(chrono::Utc::now());
    }

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;
    let purpose = if speculative {
        RenderPurpose::Speculative
    } else {
        RenderPurpose::Reader
    };
    let pane =
        build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi, view, purpose).await?;
    let row = row_view_from(&ewf, status);

    // Kept behind the render simply to stay off the critical path.
    //
    // This position used to be load-bearing: Load More re-queried the unread
    // list, so an entry marked read here vanished from it and the row `app.js`
    // was waiting for never arrived (issue #482). Load More now paginates
    // against the page's render-time snapshot, so the ordering no longer decides
    // anything — the previously-breaking order was re-tested and passes.
    if mark_read {
        dispatch_mark_read_on_open(&state, user_id, entry_id);
    }

    let mut response = OpenEntryMulti {
        pane,
        r: row,
        csrf_token: auth_user.csrf_token,
    }
    .into_response();
    if speculative {
        deny_storage(&mut response);
    }
    Ok(response)
}

/// `GET /entries/{id}/summary/fragment` — returns the summary container swap
/// fragment for the entry. Ownership enforced by `find_by_id_for_user` (404
/// otherwise). Does NOT mark the entry read (unlike `entry_fragment`).
pub async fn summary_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummaryFragment> {
    let user_id = auth_user.user.id;
    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    // has_save/has_kagi are irrelevant to the summary container; pass false.
    let pane = build_reading_pane_view(
        &state,
        user_id,
        &ewf,
        false,
        false,
        ContentView::Full,
        RenderPurpose::Reader,
    )
    .await?;
    Ok(SummaryFragment {
        pane,
        csrf_token: auth_user.csrf_token,
    })
}

/// Read the user's save-services config + Kagi config to drive the
/// conditional Save / Summarize buttons in the reading pane.
pub(crate) async fn load_pane_action_flags(
    state: &AppState,
    user_id: i64,
) -> AppResult<(bool, bool)> {
    let cfg = crate::models::user_settings::get_save_services_config(&state.db, user_id).await?;
    let has_save = cfg.has_any_service();
    let has_kagi = cfg
        .kagi
        .as_ref()
        .is_some_and(super::super::services::summarize::kagi::KagiConfig::is_configured);
    Ok((has_save, has_kagi))
}

/// Query string for `GET /entries/{id}/fragment`.
#[derive(Debug, Default, serde::Deserialize)]
pub struct FragmentQuery {
    /// `original` renders what the feed published even when a fetched article
    /// is stored. Anything else — including absence — renders the fetched
    /// article, so the way back is simply dropping the parameter.
    pub view: Option<String>,
    /// `1` marks the request an offline-sync prefetch: render the pane as the
    /// entry actually is and change nothing.
    ///
    /// Without it, mirroring a reader's queue for offline reading would mark
    /// every entry in it read — opening an entry is what marks it read, and a
    /// sync opens all of them. The existing `is_speculative_load` check cannot
    /// carry this: it reads `Sec-Purpose`, and `Sec-` headers are forbidden to
    /// `fetch()`, so the client physically cannot set one.
    pub offline: Option<u8>,
}

impl FragmentQuery {
    /// Whether this request must leave the entry alone. See [`Self::offline`].
    fn is_prefetch(&self) -> bool {
        self.offline == Some(1)
    }

    fn content_view(&self) -> ContentView {
        match self.view.as_deref() {
            Some("original") => ContentView::Original,
            _ => ContentView::Full,
        }
    }
}

/// Which body the reading pane should render.
///
/// Only meaningful once an article has been fetched and stored; before that
/// both variants render what the feed published.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ContentView {
    /// The stored fetched article when there is one — the default, so a
    /// refresh or a scriptless page load keeps showing what was fetched.
    #[default]
    Full,
    /// What the feed published, even when a fetched article exists. Reached by
    /// `?view=original`; the way back is the same URL without it.
    Original,
}

/// Who a reading-pane render is actually for.
///
/// Only the open-tracking pixel reads this, and it is the same distinction
/// `mark_read` already makes: mirroring the queue for offline reading, or a
/// browser speculatively fetching an entry, is not the reader opening anything.
/// The pixel is an `<img>`, so anything that renders — or merely walks the
/// images of — such a copy would report an open nobody performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPurpose {
    /// A reader is looking at this now.
    Reader,
    /// A copy taken ahead of time: the service worker's `?offline=1` mirror, or
    /// a `Sec-Purpose` prefetch/prerender.
    Speculative,
}

/// Build a `ReadingPaneView` from an already-loaded `EntryWithFeed`. The
/// sanitizer and summary lookup happen here so callers that already hold the
/// entry — `entry_fragment` loads it inside its write transaction — don't re-hit
/// the DB. Save / Fetch feedback is delivered via flash, not inline status text.
///
/// Summary resolution prefers the in-memory cache and falls back to the
/// persistent `entry_summary` table, so a restart does not hide an
/// already-completed summary on the next open.
///
/// `view` picks between the stored fetched article and what the feed published.
/// [`ContentView::Full`] is the default and falls back to the feed's own content
/// when nothing has been fetched, so callers need not check first.
pub(crate) async fn build_reading_pane_view(
    state: &AppState,
    user_id: i64,
    ewf: &entry::EntryWithFeed,
    has_save: bool,
    has_kagi: bool,
    view: ContentView,
    purpose: RenderPurpose,
) -> AppResult<ReadingPaneView> {
    let entry_id = ewf.entry.id;
    // A stored fetched article wins unless the reader asked for the original,
    // so it survives a refresh, a new tab and a scriptless page load — which is
    // the whole point of storing it.
    let stored_full = ewf.entry.full_content.as_deref().filter(|s| !s.is_empty());
    let showing_full = matches!(view, ContentView::Full) && stored_full.is_some();
    let raw_content = if showing_full {
        stored_full.unwrap_or("")
    } else {
        ewf.entry
            .content
            .as_deref()
            .or(ewf.entry.summary.as_deref())
            .unwrap_or("")
    };

    let base_url = Some(ewf.content_base_url());
    let referrer = ewf.custom_referrer.as_deref();
    let proxy_base_url = state.config.public_base_url.as_deref();
    let content_html = sanitize_html(
        raw_content,
        &state.config.secret,
        base_url,
        referrer,
        proxy_base_url,
    );
    // Strictly after `sanitize_html`, never before: the sanitiser strips 1x1
    // images and rewrites every `<img src>` through the image proxy, either of
    // which destroys the pixel. See `services::pixel`.
    //
    // Skipped entirely for a speculative render: `offline.js` walks the images
    // of everything it mirrors, so a pixel in that copy would report an open for
    // every entry merely queued for offline reading. It does not find one today
    // only because the fragment's `<img>`s sit inside a `<template>`, which
    // `querySelectorAll` does not descend into — an accident, not a guarantee.
    let enabled_at = match purpose {
        RenderPurpose::Reader => {
            user_settings::get_pixel_tracking_enabled_at(&state.db, user_id).await?
        }
        RenderPurpose::Speculative => None,
    };
    let content_html = PixelContext {
        user_id,
        enabled_at,
        secret: &state.config.secret,
        // Rendered into this origin's own page, so a root-relative URL is both
        // enough and cheaper than baking in a host that may be misconfigured.
        base_url: None,
    }
    .maybe_inject(content_html, entry_id, ewf.entry.created_at);

    let (summary_text, summary_in_flight, summary_error) =
        resolve_summary(state, user_id, entry_id).await?;

    let published_at = ewf.entry.published_at;
    Ok(ReadingPaneView {
        id: entry_id,
        title: ewf.entry.title.as_deref().map_or_else(
            || "(no title)".to_string(),
            crate::services::decode_html_entities,
        ),
        link: ewf.entry.link.as_deref().map(strip_tracking_params),
        feed_title: ewf.feed_title.clone().unwrap_or_default(),
        feed_id: ewf.entry.feed_id,
        feed_has_icon: ewf.feed_has_icon,
        author: ewf
            .entry
            .author
            .as_deref()
            .map(crate::services::decode_html_entities),
        published_at_iso: published_at.map(|t| t.to_rfc3339()),
        published_relative: format_relative_time(published_at).0,
        content_html,
        is_read: ewf.entry.read_at.is_some(),
        is_starred: ewf.entry.starred_at.is_some(),
        summary_text,
        summary_in_flight,
        summary_error,
        has_kagi,
        has_save,
        is_full_content: showing_full,
        has_stored_full_content: stored_full.is_some(),
    })
}

/// Resolve `(summary_text, summary_in_flight, summary_error)` for an entry.
/// Reads the in-memory cache first; on a miss or terminal-failed state falls
/// back to the `entry_summary` table, so a summary persisted in a previous
/// session is still surfaced.
///
/// A completed summary is passed through [`sanitize_summary`] on the way out
/// rather than in, which matches how feed content is handled and lets rows
/// already in the table benefit without a migration. Both templates that render
/// one read it from here, so this is the whole surface.
async fn resolve_summary(
    state: &AppState,
    user_id: i64,
    entry_id: i64,
) -> AppResult<(Option<String>, bool, Option<String>)> {
    if let Some(cached) = state.summary_cache.get(user_id, entry_id) {
        match cached.status {
            SummaryStatus::Completed => {
                return Ok((
                    cached.summary_text.as_deref().map(sanitize_summary),
                    false,
                    None,
                ));
            }
            SummaryStatus::Pending | SummaryStatus::Processing => return Ok((None, true, None)),
            SummaryStatus::Failed => {
                // Fall through to DB — a retry may have refreshed the row
                // without yet updating the cache.
            }
        }
    }
    let db_entry = entry_summary::find_by_user_and_entry(&state.db, user_id, entry_id).await?;
    match db_entry {
        Some(s) => match s.status {
            SummaryStatus::Completed => {
                Ok((s.summary_text.as_deref().map(sanitize_summary), false, None))
            }
            SummaryStatus::Pending | SummaryStatus::Processing => Ok((None, true, None)),
            SummaryStatus::Failed => Ok((None, false, s.error_message)),
        },
        None => Ok((None, false, None)),
    }
}

/// Just enough state to re-render the reading-pane Star button form. Set by
/// star/unstar handlers so the multi-target response can refresh the pane's
/// button label + action URL after a toggle without round-tripping the
/// whole reading pane.
#[derive(Debug, Clone)]
pub struct PaneStarFormView {
    pub id: i64,
    pub is_starred: bool,
}

/// Multi-target action response template: the updated entry row, optionally a
/// `<template data-flash>` block for actions that want toast feedback, and
/// optionally a pane-star-form swap block that keeps the pane button's label in
/// sync with the new starred state.
#[derive(Template)]
#[template(path = "_entry_actions_multi.html")]
pub struct EntryActionMulti {
    pub r: EntryRowView,
    pub flash: Option<FlashPayload>,
    pub pane_star_form: Option<PaneStarFormView>,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

impl IntoResponse for EntryActionMulti {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Multi-target response for opening an entry: the reading pane and the
/// now-read entry row. Returned by `GET /entries/{id}/fragment` so a title-link
/// click both shows the entry and clears its unread state in one round trip. The
/// sidebar's counts follow over SSE via `emit_sidebar`.
#[derive(Template)]
#[template(path = "_open_entry_multi.html")]
pub struct OpenEntryMulti {
    pub pane: ReadingPaneView,
    pub r: EntryRowView,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

impl IntoResponse for OpenEntryMulti {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Answer an entry-action POST. The swap helper's `fetch()` gets the
/// multi-target fragment; a scriptless form submit is a top-level navigation,
/// and every one of these fragments is blank as a document, so the browser goes
/// back to the list it came from. The action itself has already been applied.
///
/// `flash` is the same message the fragment carries as `<template data-flash>`,
/// handed over as a cookie so the page landed on renders it. Without it a
/// scriptless action that changes nothing visible — Save being the pure case,
/// its whole effect on someone else's server — completes in total silence.
/// Actions whose result is visible in the markup pass `None`, matching the swap
/// helper, which raises no toast for them either.
fn entry_action_response(
    fragment: impl IntoResponse,
    flash: Option<FlashMessage>,
    headers: &HeaderMap,
    entry_id: i64,
) -> Response {
    if is_document_navigation(headers) {
        let location = action_document_redirect(headers, entry_id);
        // 303 either way, so the browser re-issues the follow-up as a GET.
        return match flash {
            Some(message) => FlashRedirect::to(location, message).into_response(),
            None => Redirect::to(&location).into_response(),
        };
    }
    fragment.into_response()
}

/// `POST /entries/{id}/star` — idempotently mark the entry as starred.
/// No-op when the entry is already starred. Response includes the pane-
/// star-form swap so the reading pane button label flips to "Unstar"
/// if the pane is visible.
pub async fn star_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let multi = set_starred_state(
        state,
        auth_user.user.id,
        entry_id,
        true,
        auth_user.csrf_token,
    )
    .await?;
    let flash = multi.flash.as_ref().map(FlashPayload::to_message);
    Ok(entry_action_response(multi, flash, &headers, entry_id))
}

/// `POST /entries/{id}/unstar` — idempotently mark the entry as unstarred.
/// No-op when the entry is already unstarred. Same pane-star-form swap
/// so the pane button can flip back to "Star".
pub async fn unstar_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let multi = set_starred_state(
        state,
        auth_user.user.id,
        entry_id,
        false,
        auth_user.csrf_token,
    )
    .await?;
    let flash = multi.flash.as_ref().map(FlashPayload::to_message);
    Ok(entry_action_response(multi, flash, &headers, entry_id))
}

/// Shared core for the idempotent star/unstar handlers. Renders the response
/// optimistically and enqueues the write off the critical path.
async fn set_starred_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_starred: bool,
    csrf_token: String,
) -> AppResult<EntryActionMulti> {
    let mut ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    let status = entry_summary::get_statuses_for_entries(&state.db, user_id, &[entry_id])
        .await?
        .get(&entry_id)
        .copied();

    let changed = ewf.entry.starred_at.is_some() != desired_starred;

    // Optimistically reflect the new starred state in the row + pane button.
    ewf.entry.starred_at = if desired_starred {
        Some(ewf.entry.starred_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    let pane_star_form = Some(PaneStarFormView {
        id: ewf.entry.id,
        is_starred: ewf.entry.starred_at.is_some(),
    });

    if changed {
        let db = state.db.clone();
        tokio::spawn(async move {
            if let Err(e) =
                entry::set_starred_for_user(&db, user_id, entry_id, desired_starred).await
            {
                tracing::warn!(event = "entry.set_starred_failed", entry_id, error = %e, "async set_starred failed");
            }
        });
        // No sidebar_cache.bust here: starring does not change unread counts.
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        flash: None,
        pane_star_form,
        csrf_token,
    })
}

/// `POST /entries/{id}/read` — idempotently mark the entry as read, then
/// return a multi-target HTML fragment updating the row + sidebar-unread
/// block. No-op if the entry is already read. No flash toast — marking
/// read is the normal reading flow and doesn't need explicit feedback.
pub async fn read_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let multi = set_read_state(
        state,
        auth_user.user.id,
        entry_id,
        true,
        auth_user.csrf_token,
    )
    .await?;
    let flash = multi.flash.as_ref().map(FlashPayload::to_message);
    Ok(entry_action_response(multi, flash, &headers, entry_id))
}

/// `POST /entries/{id}/unread` — idempotently mark the entry as unread.
/// Emits a "Marked as unread." flash *only* when the call actually changed
/// state, so a stale-label double-click doesn't re-toast.
pub async fn unread_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let multi = set_read_state(
        state,
        auth_user.user.id,
        entry_id,
        false,
        auth_user.csrf_token,
    )
    .await?;
    let flash = multi.flash.as_ref().map(FlashPayload::to_message);
    Ok(entry_action_response(multi, flash, &headers, entry_id))
}

/// Shared core for the two idempotent read/unread handlers. Renders the
/// response optimistically and enqueues the write off the critical path.
async fn set_read_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_read: bool,
    csrf_token: String,
) -> AppResult<EntryActionMulti> {
    let mut ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    let status = entry_summary::get_statuses_for_entries(&state.db, user_id, &[entry_id])
        .await?
        .get(&entry_id)
        .copied();

    let changed = ewf.entry.read_at.is_some() != desired_read;

    // Optimistically reflect the new read state in the row.
    ewf.entry.read_at = if desired_read {
        Some(ewf.entry.read_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    let flash = if !desired_read && changed {
        Some(FlashPayload {
            level: "success",
            message: "Marked as unread.".to_string(),
        })
    } else {
        None
    };

    if changed {
        let db = state.db.clone();
        tokio::spawn(async move {
            if let Err(e) = entry::set_read_for_user(&db, user_id, entry_id, desired_read).await {
                tracing::warn!(event = "entry.set_read_failed", entry_id, error = %e, "async set_read failed");
            }
        });
        state.sidebar_cache.bust(user_id);
        state.events.emit_sidebar(user_id);
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        flash,
        pane_star_form: None,
        csrf_token,
    })
}

/// `POST /entries/{id}/summarize` — queue a summarization job and return the
/// reading pane with `summary_in_flight = true`, so the Summarize button renders
/// disabled while the job runs.
///
/// Ownership is validated via `find_by_id_for_user` (404 otherwise). The pending
/// DB record is created before the cache is marked, so the background worker
/// always sees a consistent state; Kagi-config validation is deferred to it.
pub async fn summarize_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    // Fetch the entry and extract the link needed by SummaryJob. Ownership is
    // enforced by find_by_id_for_user's `c.user_id = ?2` join constraint.
    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    // Create / reset the pending record in the DB before setting the
    // in-memory cache so the state is always consistent.
    entry_summary::upsert_pending(&state.db, user_id, entry_id).await?;

    // The link may be absent; the background worker will surface an error
    // if so. We use an empty string as a sentinel to let the queue accept
    // the job and fail gracefully rather than returning 400 here.
    let entry_link = ewf
        .entry
        .link
        .as_deref()
        .map(strip_tracking_params)
        .unwrap_or_default();

    // Mark pending in the in-memory cache BEFORE enqueuing so the background
    // worker cannot complete before the cache entry exists.
    state.summary_cache.set_pending(user_id, entry_id);

    // Best-effort enqueue: if the channel is full or closed, we still return
    // the in-flight pending fragment (the DB record is already pending).
    let _ = state
        .summary_tx
        .send(SummaryJob {
            user_id,
            entry_id,
            entry_link,
        })
        .await;

    state
        .events
        .emit_summary(user_id, entry_id, Some(SummaryStatus::Pending));

    // No flash: the redirect lands with `?entry=`, so the pane comes back
    // showing the summary container in its pending state — the result is the
    // feedback, exactly as it is for the swap helper.
    Ok(entry_action_response(
        SummarizePending {
            id: entry_id,
            csrf_token: auth_user.csrf_token,
        },
        None,
        &headers,
        entry_id,
    ))
}

/// `POST /entries/{id}/summarize/cancel` — cancel an in-flight or queued
/// summarization (or clear a failed one) and delete the record, returning the
/// summary container to its empty state.
///
/// Cancel and Clear share this endpoint: both mean "stop and remove this
/// summary". A failed record has no live token, so the registry lookup misses
/// and it is simply deleted. Ownership is enforced by `find_by_id_for_user`.
pub async fn summarize_cancel_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    // Validate ownership and delete the record in one write txn.
    entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    entry_summary::delete(&state.db, user_id, entry_id).await?;

    // Cancel + drop any in-flight / queued token for this entry.
    let token = {
        let mut map = state.summary_cancels.lock().unwrap();
        map.remove(&(user_id, entry_id))
    };
    if let Some(token) = token {
        token.cancel();
    }

    state.summary_cache.remove(user_id, entry_id);
    state.sidebar_cache.bust(user_id);
    state.events.emit_summary(user_id, entry_id, None);
    state.events.emit_sidebar(user_id);

    // No flash, same reason as `summarize_entry_form`: the pane returns to its
    // empty state and that *is* the confirmation.
    Ok(entry_action_response(
        SummarizeCleared,
        None,
        &headers,
        entry_id,
    ))
}

/// `POST /entries/{id}/fetch-full-content` — fetch the source article from the
/// entry's `link`, sanitize it, and return the reading pane with the article body
/// replaced. `pane.is_full_content = true` makes the template swap "Fetch Full
/// Content" for "Show Original", which re-renders the pane via
/// `GET /entries/{id}/fragment` and restores the feed-supplied body.
///
/// Scriptless callers are turned away before the fetch — see below.
pub async fn fetch_full_content_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    let mut ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    let link = ewf
        .entry
        .link
        .clone()
        .ok_or_else(|| AppError::Validation("Entry has no link".to_string()))?;

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;

    // Only the outcome differs between these arms — the pane is built the same
    // way either side, from `ewf`, which the success arm updates in place.
    let flash = match fetch_and_extract(&link, &state.config.user_agent, &state.fetcher).await {
        Ok(extracted) => {
            // Store the *raw* extraction and let the pane sanitise it like any
            // other body. Persisting is what makes this survive a refresh, a
            // second tab and a scriptless page load; before it did not, so the
            // scriptless path had to decline outright rather than claim a
            // success the reader could see was not there.
            entry::set_full_content_for_user(&state.db, user_id, entry_id, &extracted.content)
                .await?;
            ewf.entry.full_content = Some(extracted.content);
            FlashPayload {
                level: "success",
                message: "Fetched full content.".to_string(),
            }
        }
        Err(e) => FlashPayload {
            level: "error",
            message: format!("Failed to fetch full content: {e}"),
        },
    };
    let pane = build_reading_pane_view(
        &state,
        user_id,
        &ewf,
        has_save,
        has_kagi,
        ContentView::Full,
        RenderPurpose::Reader,
    )
    .await?;
    // The scriptless path now reaches here and gets a redirect, so its message
    // has to ride along as a cookie — and it can finally be the truthful one,
    // because the page it lands on renders the article this just stored.
    let message = flash.to_message();
    Ok(entry_action_response(
        ReadingPaneWithFlash {
            pane,
            flash: Some(flash),
            csrf_token: auth_user.csrf_token,
        },
        Some(message),
        &headers,
        entry_id,
    ))
}

/// `POST /entries/{id}/save` — send the entry to every configured save
/// service (currently Linkding) and return the reading pane with an inline
/// status message.
pub async fn save_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    let save_config = user_settings::get_save_services_config(&state.db, user_id).await?;

    let link = ewf
        .entry
        .link
        .as_deref()
        .map(strip_tracking_params)
        .ok_or_else(|| AppError::Validation("Entry has no link to save".to_string()))?;
    if !save_config.has_any_service() {
        return Err(AppError::Validation(
            "No save services configured".to_string(),
        ));
    }

    let bookmark = BookmarkData {
        url: link,
        title: ewf.entry.title.clone(),
        description: ewf.entry.summary.clone(),
        tags: vec![],
    };

    let mut succeeded: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    if let Some(linkding_cfg) = save_config.linkding.as_ref()
        && linkding_cfg.is_configured()
    {
        match linkding::save_to_linkding(linkding_cfg, &bookmark).await {
            Ok(result) if result.success => {
                succeeded.push("Linkding".to_string());
            }
            Ok(result) => failed.push(format!("Linkding: {}", result.message)),
            Err(e) => failed.push(format!("Linkding: {e}")),
        }
    }

    let flash = if failed.is_empty() {
        FlashPayload {
            level: "success",
            message: format!("Saved to {}.", succeeded.join(", ")),
        }
    } else if succeeded.is_empty() {
        FlashPayload {
            level: "error",
            message: format!("Save failed — {}", failed.join("; ")),
        }
    } else {
        FlashPayload {
            level: "warning",
            message: format!(
                "Saved to {}. Failed: {}",
                succeeded.join(", "),
                failed.join("; ")
            ),
        }
    };

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;
    let pane = build_reading_pane_view(
        &state,
        user_id,
        &ewf,
        has_save,
        has_kagi,
        ContentView::Full,
        RenderPurpose::Reader,
    )
    .await?;
    // Save's entire effect is on the bookmarking service; nothing about the
    // entry changes. Without carrying this message the scriptless reader gets a
    // redirect back to an identical page and no way to tell it worked.
    let message = flash.to_message();
    Ok(entry_action_response(
        ReadingPaneWithFlash {
            pane,
            flash: Some(flash),
            csrf_token: auth_user.csrf_token,
        },
        Some(message),
        &headers,
        entry_id,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn with_referer(url: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::REFERER, HeaderValue::from_str(url).unwrap());
        h
    }

    #[test]
    fn document_redirect_preserves_unread_scope() {
        let h = with_referer("https://rdrs.example/");
        assert_eq!(fragment_document_redirect(&h, 42), "/?entry=42");
    }

    #[test]
    fn document_redirect_preserves_feed_scope_and_filters() {
        let h = with_referer("https://rdrs.example/feeds/7/entries?status=unread");
        assert_eq!(
            fragment_document_redirect(&h, 42),
            "/feeds/7/entries?status=unread&entry=42"
        );
    }

    #[test]
    fn document_redirect_preserves_category_scoped_search() {
        let h = with_referer("https://rdrs.example/categories/3/entries?q=rust");
        assert_eq!(
            fragment_document_redirect(&h, 9),
            "/categories/3/entries?q=rust&entry=9"
        );
    }

    #[test]
    fn document_redirect_replaces_stale_entry_param() {
        let h = with_referer("https://rdrs.example/entries/starred?entry=1");
        assert_eq!(
            fragment_document_redirect(&h, 2),
            "/entries/starred?entry=2"
        );
    }

    #[test]
    fn document_redirect_falls_back_without_referer() {
        assert_eq!(
            fragment_document_redirect(&HeaderMap::new(), 5),
            "/entries?entry=5"
        );
    }

    #[test]
    fn document_redirect_rejects_non_list_referer() {
        // `/search`, arbitrary pages, and the mark-read action are not entry-list
        // routes; each must fall back to All Entries rather than redirect there.
        for path in [
            "https://rdrs.example/search?q=x",
            "https://rdrs.example/settings",
            "https://rdrs.example/feeds/7/entries/mark-read",
            "https://rdrs.example/feeds/abc/entries",
        ] {
            let h = with_referer(path);
            assert_eq!(fragment_document_redirect(&h, 5), "/entries?entry=5");
        }
    }

    #[test]
    fn action_redirect_returns_to_the_list_without_opening_the_pane() {
        // An action fired from a list row must leave the reader in the list —
        // no `?entry=` is grafted on, unlike the fragment redirect.
        let h = with_referer("https://rdrs.example/feeds/7/entries?status=unread");
        assert_eq!(
            action_document_redirect(&h, 42),
            "/feeds/7/entries?status=unread"
        );
    }

    #[test]
    fn action_redirect_keeps_an_open_pane_open() {
        // Fired from the reading pane, whose URL already carries `?entry=`.
        let h = with_referer("https://rdrs.example/?entry=42");
        assert_eq!(action_document_redirect(&h, 42), "/?entry=42");
    }

    #[test]
    fn action_redirect_falls_back_to_the_entry_without_referer() {
        // No usable referrer: show the entry, the only feedback available until
        // the flash banner is server-rendered.
        assert_eq!(
            action_document_redirect(&HeaderMap::new(), 5),
            "/entries?entry=5"
        );
        let h = with_referer("https://rdrs.example/settings");
        assert_eq!(action_document_redirect(&h, 5), "/entries?entry=5");
    }

    #[test]
    fn speculative_load_matches_every_prefetch_header_shape() {
        let mut h = HeaderMap::new();
        assert!(!is_speculative_load(&h));

        // A prerender's `Sec-Purpose` carries both tokens.
        for value in ["prefetch", "prefetch;prerender", "Prefetch"] {
            h.insert("sec-purpose", HeaderValue::from_str(value).unwrap());
            assert!(is_speculative_load(&h), "sec-purpose: {value}");
        }
        h.remove("sec-purpose");

        // Legacy Chromium and Firefox.
        h.insert("purpose", HeaderValue::from_static("prefetch"));
        assert!(is_speculative_load(&h));
        h.remove("purpose");
        h.insert("x-moz", HeaderValue::from_static("prefetch"));
        assert!(is_speculative_load(&h));
        h.remove("x-moz");

        // A reader actually opening the entry.
        h.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        assert!(!is_speculative_load(&h));
    }

    #[test]
    fn document_navigation_only_matches_a_top_level_navigation() {
        let mut h = HeaderMap::new();
        assert!(!is_document_navigation(&h));

        // The swap helper's `fetch()`.
        h.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
        assert!(!is_document_navigation(&h));

        h.insert("sec-fetch-dest", HeaderValue::from_static("document"));
        assert!(is_document_navigation(&h));
    }
}
