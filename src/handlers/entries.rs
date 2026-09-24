use askama::Template;
use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Redirect, Response},
};

use crate::{
    AppState,
    error::{AppError, AppResult},
    handlers::pages::{EntryRowView, ReadingPaneView, format_relative_time, row_view_from},
    handlers::return_to::path_and_query,
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

/// Reading-pane fragment template.
#[derive(Template)]
#[template(path = "_reading_pane.html")]
pub struct ReadingPaneFragment {
    pub pane: ReadingPaneView,
    /// See [`crate::middleware::auth::PageAuthUser::csrf_token`]; forms can't
    /// rely on `csrf.js` running.
    pub csrf_token: String,
}

crate::handlers::impl_html_response!(
    ReadingPaneFragment,
    ReadingPaneWithFlash,
    SummarizePending,
    SummarizeCleared,
    SummaryFragment,
    EntryActionMulti,
    OpenEntryMulti,
);

/// Flash for the swap-helper `<template data-flash>` block; `level` is one of
/// the `<rdrs-flash>` levels (`success | error | info | warning`).
#[derive(Debug, Clone)]
pub struct FlashPayload {
    pub level: &'static str,
    pub message: String,
}

impl FlashPayload {
    /// As a cookie-borne [`FlashMessage`], to survive the scriptless redirect.
    fn to_message(&self) -> FlashMessage {
        match self.level {
            "success" => FlashMessage::success(&self.message),
            "error" => FlashMessage::error(&self.message),
            "warning" => FlashMessage::warning(&self.message),
            _ => FlashMessage::info(&self.message),
        }
    }
}

/// Reading pane plus optional toast; returned by Save / Fetch Full Content.
#[derive(Template)]
#[template(path = "_reading_pane_with_flash.html")]
pub struct ReadingPaneWithFlash {
    pub pane: ReadingPaneView,
    pub flash: Option<FlashPayload>,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

/// `POST /entries/{id}/summarize` response; swaps only `#rp-summary-container`
/// so the article body stays put.
#[derive(Template)]
#[template(path = "_summarize_pending.html")]
pub struct SummarizePending {
    pub id: i64,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

/// `POST /entries/{id}/summarize/cancel` response: empty summary container.
#[derive(Template)]
#[template(path = "_summary_cleared.html")]
pub struct SummarizeCleared;

/// `GET /entries/{id}/summary/fragment`; refreshed by the SSE `summary` event.
#[derive(Template)]
#[template(path = "_summary_fragment.html")]
pub struct SummaryFragment {
    pub pane: ReadingPaneView,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
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

/// List routes honouring `?entry={id}`. `/search` is excluded: it does not
/// deep-link an entry.
fn is_entry_list_path(path: &str) -> bool {
    matches!(
        path,
        "/" | "/entries" | "/entries/read" | "/entries/starred" | "/entries/summarized"
    ) || is_scoped_entries_path(path, "categories")
        || is_scoped_entries_path(path, "feeds")
}

/// The entry-list page from `Referer`, if any. Only path and query are used,
/// so redirects built from it stay same-origin.
fn referring_entry_list(headers: &HeaderMap) -> Option<url::Url> {
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok())?;
    let url = url::Url::parse(referer).ok()?;
    is_entry_list_path(url.path()).then_some(url)
}

/// A top-level navigation rather than a swap-helper `fetch()`; fragments here
/// render blank as documents, so these must redirect instead.
fn is_document_navigation(headers: &HeaderMap) -> bool {
    headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document")
}

/// A prefetch/prerender, which must not mark the entry read. `Sec-Purpose`
/// may be `prefetch;prerender` (hence substring); `Purpose` is legacy Chromium,
/// `X-Moz` is Firefox.
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

/// Keep a speculative response out of the prefetch cache, or the real click
/// would be served from it and never mark the entry read.
fn deny_storage(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
}

/// Mark read in a detached task and refresh the sidebar counts over SSE.
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

/// Redirect for a document navigation to `/entries/{id}/fragment`: the
/// referring list (with its filters) plus `?entry={id}`, else All Entries.
fn fragment_document_redirect(headers: &HeaderMap, entry_id: i64) -> String {
    let Some(mut url) = referring_entry_list(headers) else {
        return format!("/entries?entry={entry_id}");
    };

    // Replace any stale `entry` param.
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

/// Redirect for a scriptless entry-action POST. Unlike
/// [`fragment_document_redirect`], the referrer is reused verbatim so a row
/// action doesn't open the pane; falls back to the entry in All Entries.
fn action_document_redirect(headers: &HeaderMap, entry_id: i64) -> String {
    match referring_entry_list(headers) {
        Some(url) => path_and_query(&url),
        None => format!("/entries?entry={entry_id}"),
    }
}

/// `GET /entries/{id}/fragment` — open an entry (marks it read); 404 if not
/// the user's.
pub async fn entry_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
    axum::extract::Query(query): axum::extract::Query<FragmentQuery>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;
    let view = query.content_view();

    // Loaded before the document-navigation branch: a scriptless open marks
    // read too.
    let found = entry::find_by_id_for_user(&state.db, user_id, entry_id).await?;

    let speculative = is_speculative_load(&headers) || query.is_prefetch();
    let mark_read = !speculative && found.as_ref().is_some_and(|e| e.entry.read_at.is_none());

    // Document navigations (no-JS, new tab, refresh) would see a blank page, so
    // redirect to the list with the pane open. A missing entry redirects too
    // (lists ignore a bad `?entry=`); the `fetch()` path still 404s.
    if is_document_navigation(&headers) {
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

    // Optimistic; tied to `mark_read` so speculative loads render true state.
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

    // After the render only to stay off the critical path.
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

/// `GET /entries/{id}/summary/fragment`. Does not mark the entry read.
pub async fn summary_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummaryFragment> {
    let user_id = auth_user.user.id;
    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    // has_save/has_kagi are irrelevant to the summary container.
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

/// `(has_save, has_kagi)` for the pane's Save / Summarize buttons.
pub(crate) async fn load_pane_action_flags(
    state: &AppState,
    user_id: i64,
) -> AppResult<(bool, bool)> {
    // An unreadable credential shows as absent; settings explains why.
    let cfg = crate::models::user_settings::get_save_services_config(
        &state.db,
        user_id,
        state.config.service_token_key(),
    )
    .await?
    .or_default();
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
    /// `original` renders the feed's content; anything else the fetched article.
    pub view: Option<String>,
    /// `1` marks an offline-sync prefetch that must not mark anything read.
    /// Needed because `fetch()` cannot set `Sec-Purpose`.
    pub offline: Option<u8>,
}

impl FragmentQuery {
    /// See [`Self::offline`].
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

/// Which body the reading pane renders; identical until an article is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ContentView {
    /// The stored fetched article, if any.
    #[default]
    Full,
    /// What the feed published (`?view=original`).
    Original,
}

/// Who a render is for; a speculative copy must not carry the open-tracking
/// pixel, or it would report an open nobody performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderPurpose {
    Reader,
    /// `?offline=1` mirror or a `Sec-Purpose` prefetch/prerender.
    Speculative,
}

/// Build a `ReadingPaneView` from an already-loaded entry: sanitize, inject the
/// pixel, and resolve the summary.
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
    // Must run after `sanitize_html`, which would strip/proxy the pixel. Skipped
    // for speculative renders, whose images `offline.js` may walk.
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
        // Same-origin page, so a root-relative URL suffices.
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

/// `(summary_text, in_flight, error)`: cache first, then the `entry_summary`
/// table. Summaries are [`sanitize_summary`]-ed on output, like feed content.
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
                // Fall through: a retry may have updated the DB row.
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

/// State to re-render the pane's Star button after a toggle.
#[derive(Debug, Clone)]
pub struct PaneStarFormView {
    pub id: i64,
    pub is_starred: bool,
}

/// Entry-action response: updated row, optional toast, optional pane Star form.
#[derive(Template)]
#[template(path = "_entry_actions_multi.html")]
pub struct EntryActionMulti {
    pub r: EntryRowView,
    pub flash: Option<FlashPayload>,
    pub pane_star_form: Option<PaneStarFormView>,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

/// Open-entry response: the reading pane plus the now-read row.
#[derive(Template)]
#[template(path = "_open_entry_multi.html")]
pub struct OpenEntryMulti {
    pub pane: ReadingPaneView,
    pub r: EntryRowView,
    /// See [`ReadingPaneFragment::csrf_token`].
    pub csrf_token: String,
}

/// Answer an entry-action POST: the fragment for `fetch()`, or a redirect back
/// to the list (carrying `flash` as a cookie) for a scriptless submit.
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

/// `POST /entries/{id}/star` — idempotent.
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

/// `POST /entries/{id}/unstar` — idempotent.
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

/// Renders optimistically and writes in a detached task.
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

/// `POST /entries/{id}/read` — idempotent; no toast.
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

/// `POST /entries/{id}/unread` — idempotent; toasts only on an actual change.
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

/// Renders optimistically and writes in a detached task.
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

/// `POST /entries/{id}/summarize` — queue a summary job; Kagi config is
/// validated by the worker.
pub async fn summarize_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    let ewf = entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    // DB record before cache, so the state is always consistent.
    entry_summary::upsert_pending(&state.db, user_id, entry_id).await?;

    // Empty link is a sentinel; the worker reports the error.
    let entry_link = ewf
        .entry
        .link
        .as_deref()
        .map(strip_tracking_params)
        .unwrap_or_default();

    // Must precede the enqueue, or the worker could finish first.
    state.summary_cache.set_pending(user_id, entry_id);

    // Best-effort; the DB record is already pending.
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

    // No flash: the pending state in the pane is the feedback.
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

/// `POST /entries/{id}/summarize/cancel` — cancel an in-flight summary or clear
/// a failed one, deleting the record.
pub async fn summarize_cancel_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    entry::find_by_id_for_user(&state.db, user_id, entry_id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    entry_summary::delete(&state.db, user_id, entry_id).await?;

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

    // No flash: the empty pane is the confirmation.
    Ok(entry_action_response(
        SummarizeCleared,
        None,
        &headers,
        entry_id,
    ))
}

/// `POST /entries/{id}/fetch-full-content` — fetch, store and show the source
/// article.
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

    let flash = match fetch_and_extract(&link, &state.config.user_agent, &state.fetcher).await {
        Ok(extracted) => {
            // Stored raw; the pane sanitises it like any other body.
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

/// `POST /entries/{id}/save` — send the entry to every configured save service.
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
    let save_config = user_settings::get_save_services_config(
        &state.db,
        user_id,
        state.config.service_token_key(),
    )
    .await?
    .usable()?;

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
    // Save changes nothing visible, so the flash is the only feedback.
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
    fn document_redirect_keeps_the_list_scope_and_filters() {
        for (referer, id, expected) in [
            ("https://rdrs.example/", 42, "/?entry=42"),
            (
                "https://rdrs.example/feeds/7/entries?status=unread",
                42,
                "/feeds/7/entries?status=unread&entry=42",
            ),
            (
                "https://rdrs.example/categories/3/entries?q=rust",
                9,
                "/categories/3/entries?q=rust&entry=9",
            ),
            // A stale `entry` param is replaced, not duplicated.
            (
                "https://rdrs.example/entries/starred?entry=1",
                2,
                "/entries/starred?entry=2",
            ),
        ] {
            let h = with_referer(referer);
            assert_eq!(fragment_document_redirect(&h, id), expected, "{referer}");
        }
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
