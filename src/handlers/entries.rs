use askama::Template;
use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};

use crate::{
    error::{AppError, AppResult},
    handlers::pages::{format_relative_time, row_view_from, EntryRowView, ReadingPaneView},
    middleware::auth::PageAuthUser,
    models::{entry, entry_summary, user_settings},
    services::{
        fetch_and_extract, sanitize_html,
        save::{linkding, BookmarkData},
        SummaryJob, SummaryStatus,
    },
    AppState,
};

/// Fragment template for the reading pane — renders `_reading_pane.html`
/// and is returned by `GET /entries/{id}/fragment`.
#[derive(Template)]
#[template(path = "_reading_pane.html")]
pub struct ReadingPaneFragment {
    pub pane: ReadingPaneView,
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

/// Multi-target response: swaps the reading pane and (optionally) pops a
/// toast on the page-level `<rdrs-flash>`. Returned by the Save /
/// Fetch-Full-Content form-actions.
#[derive(Template)]
#[template(path = "_reading_pane_with_flash.html")]
pub struct ReadingPaneWithFlash {
    pub pane: ReadingPaneView,
    pub flash: Option<FlashPayload>,
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
}

impl IntoResponse for SummaryFragment {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
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
) -> AppResult<Response> {
    let user_id = auth_user.user.id;

    // `/entries/{id}/fragment` is a partial-only route: it renders just the
    // `<template data-swap-target>` blocks the swap helper consumes, which
    // display as a blank page when loaded as a top-level document. A real
    // browser navigation here (the swap helper's error fallback, a click that
    // lands before `app.js` wires up the interceptor, open-in-new-tab, or a
    // refresh of the URL) must NOT show that blank page. Browsers tag
    // top-level navigations with `Sec-Fetch-Dest: document` (a `fetch()` sends
    // `empty`), so redirect those to the full entries page with the pane
    // pre-opened via `?entry=`.
    if headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document") {
        return Ok(Redirect::to(&format!("/entries?entry={entry_id}")).into_response());
    }

    // Read current state on the READ connection (not blocked by a background
    // sync's write transaction under WAL).
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let was_unread = ewf.entry.read_at.is_none();
    let feed_id = ewf.entry.feed_id;

    // Optimistically reflect the read state in the rendered row + pane.
    if was_unread {
        ewf.entry.read_at = Some(chrono::Utc::now());
    }

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;
    let pane = build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi).await?;
    let row = row_view_from(&ewf, status);
    let sidebar_unread_payload_json =
        build_sidebar_unread_with_delta(&state, user_id, feed_id, if was_unread { -1 } else { 0 })
            .await?;

    // Enqueue the real write off the critical path (only when it changes state).
    if was_unread {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::mark_as_read(conn, entry_id) {
                tracing::warn!("async mark_as_read failed for entry {entry_id}: {e}");
            }
        });
        state.sidebar_cache.bust(user_id);
        state.events.emit_sidebar(user_id);
    }

    Ok(OpenEntryMulti {
        pane,
        r: row,
        sidebar_unread_payload_json,
    }
    .into_response())
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
    let ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    // has_save/has_kagi are irrelevant to the summary container; pass false.
    let pane = build_reading_pane_view(&state, user_id, &ewf, false, false).await?;
    Ok(SummaryFragment { pane })
}

/// Read the user's save-services config + Kagi config to drive the
/// conditional Save / Summarize buttons in the reading pane.
pub(crate) async fn load_pane_action_flags(
    state: &AppState,
    user_id: i64,
) -> AppResult<(bool, bool)> {
    state
        .db
        .read_user(move |conn| {
            let cfg = crate::models::user_settings::get_save_services_config(conn, user_id)?;
            let has_save = cfg.has_any_service();
            let has_kagi = cfg
                .kagi
                .as_ref()
                .map(|c| c.is_configured())
                .unwrap_or(false);
            Ok::<_, AppError>((has_save, has_kagi))
        })
        .await?
}

/// Build a `ReadingPaneView` from an already-loaded `EntryWithFeed`. The
/// content sanitizer + summary lookup happen here so callers that already
/// have the entry (e.g. `entry_fragment`, which loads it inside its write
/// transaction) don't re-hit the DB for the entry itself. `has_save` /
/// `has_kagi` come from `load_pane_action_flags`. Save / Fetch action
/// feedback is delivered via flash (see `ReadingPaneWithFlash`), not
/// inline status text.
///
/// Summary resolution prefers the in-memory cache and falls back to the
/// persistent `entry_summary` table so a server restart (or any other
/// path that bypassed the cache) does not hide an already-completed
/// summary on the next entry open.
pub(crate) async fn build_reading_pane_view(
    state: &AppState,
    user_id: i64,
    ewf: &entry::EntryWithFeed,
    has_save: bool,
    has_kagi: bool,
) -> AppResult<ReadingPaneView> {
    let entry_id = ewf.entry.id;
    let raw_content = ewf
        .entry
        .content
        .as_deref()
        .or(ewf.entry.summary.as_deref())
        .unwrap_or("");

    let link_str = ewf.entry.link.clone().unwrap_or_default();
    let base_url = if link_str.is_empty() {
        None
    } else {
        Some(link_str.as_str())
    };
    let referrer = ewf.custom_referrer.as_deref();
    let proxy_base_url = state.config.public_base_url.as_deref();
    let content_html = sanitize_html(
        raw_content,
        &state.config.image_proxy_secret,
        base_url,
        referrer,
        proxy_base_url,
    );

    let (summary_text, summary_in_flight, summary_error) =
        resolve_summary(state, user_id, entry_id).await?;

    let published_at = ewf.entry.published_at;
    Ok(ReadingPaneView {
        id: entry_id,
        title: ewf
            .entry
            .title
            .as_deref()
            .map(crate::services::decode_html_entities)
            .unwrap_or_else(|| "(no title)".to_string()),
        link: ewf.entry.link.clone(),
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
        is_full_content: false,
    })
}

/// Resolve `(summary_text, summary_in_flight, summary_error)` for an entry.
/// Reads the in-memory cache first; on miss or terminal-failed state falls back
/// to the `entry_summary` table so a completed summary persisted in a previous
/// session is still surfaced.
async fn resolve_summary(
    state: &AppState,
    user_id: i64,
    entry_id: i64,
) -> AppResult<(Option<String>, bool, Option<String>)> {
    if let Some(cached) = state.summary_cache.get(user_id, entry_id) {
        match cached.status {
            SummaryStatus::Completed => return Ok((cached.summary_text, false, None)),
            SummaryStatus::Pending | SummaryStatus::Processing => return Ok((None, true, None)),
            SummaryStatus::Failed => {
                // Fall through to DB — a retry may have refreshed the row
                // without yet updating the cache.
            }
        }
    }
    let db_entry = state
        .db
        .read_user(move |conn| entry_summary::find_by_user_and_entry(conn, user_id, entry_id))
        .await??;
    match db_entry {
        Some(s) => match s.status {
            SummaryStatus::Completed => Ok((s.summary_text, false, None)),
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

/// Multi-target action response template. Renders the updated entry row, the
/// sidebar-unread payload, (optionally) a `<template data-flash>` block for
/// actions that want toast feedback (e.g. Mark Unread), and (optionally) a
/// pane-star-form swap block (Star / Unstar — keeps the pane button label in
/// sync with the new starred state).
#[derive(Template)]
#[template(path = "_entry_actions_multi.html")]
pub struct EntryActionMulti {
    pub r: EntryRowView,
    pub sidebar_unread_payload_json: String,
    pub flash: Option<FlashPayload>,
    pub pane_star_form: Option<PaneStarFormView>,
}

impl IntoResponse for EntryActionMulti {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Multi-target response for opening an entry. Renders three `<template
/// data-swap-target>` blocks: the reading pane, the (now-read) entry row,
/// and the sidebar-unread payload. Returned by `GET /entries/{id}/fragment`
/// so the title-link click both shows the entry AND clears its unread
/// state from the list + sidebar in one round trip.
#[derive(Template)]
#[template(path = "_open_entry_multi.html")]
pub struct OpenEntryMulti {
    pub pane: ReadingPaneView,
    pub r: EntryRowView,
    pub sidebar_unread_payload_json: String,
}

impl IntoResponse for OpenEntryMulti {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// Builds the sidebar-unread JSON payload, applying an in-memory `delta` to
/// one feed's unread count so an optimistic response (whose DB write hasn't
/// landed yet) shows the correct number. `delta` is `-1` (marked read),
/// `+1` (marked unread), or `0` (no change, e.g. star/unstar). Mirrors
/// `unread_counts_per_feed`'s "positive counts only" shape.
pub(crate) async fn build_sidebar_unread_with_delta(
    state: &AppState,
    user_id: i64,
    feed_id: i64,
    delta: i64,
) -> AppResult<String> {
    let mut counts = state
        .db
        .read_user(move |conn| entry::unread_counts_per_feed(conn, user_id))
        .await??;
    if delta != 0 {
        match counts.iter_mut().find(|c| c.feed_id == feed_id) {
            Some(c) => c.unread = (c.unread + delta).max(0),
            None if delta > 0 => counts.push(entry::UnreadCount {
                feed_id,
                unread: delta,
            }),
            None => {}
        }
        counts.retain(|c| c.unread > 0);
    }
    Ok(serde_json::to_string(&counts).unwrap_or_else(|_| "[]".to_string()))
}

/// `POST /entries/{id}/star` — idempotently mark the entry as starred.
/// No-op when the entry is already starred. Response includes the pane-
/// star-form swap so the reading pane button label flips to "Unstar"
/// if the pane is visible.
pub async fn star_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    set_starred_state(state, auth_user.user.id, entry_id, true).await
}

/// `POST /entries/{id}/unstar` — idempotently mark the entry as unstarred.
/// No-op when the entry is already unstarred. Same pane-star-form swap
/// so the pane button can flip back to "Star".
pub async fn unstar_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    set_starred_state(state, auth_user.user.id, entry_id, false).await
}

/// Shared core for the idempotent star/unstar handlers. Renders the response
/// optimistically and enqueues the write off the critical path.
async fn set_starred_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_starred: bool,
) -> AppResult<EntryActionMulti> {
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let changed = ewf.entry.starred_at.is_some() != desired_starred;

    // Optimistically reflect the new starred state in the row + pane button.
    ewf.entry.starred_at = if desired_starred {
        Some(ewf.entry.starred_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    // Starring does not affect unread counts (delta = 0).
    let payload_json =
        build_sidebar_unread_with_delta(&state, user_id, ewf.entry.feed_id, 0).await?;
    let pane_star_form = Some(PaneStarFormView {
        id: ewf.entry.id,
        is_starred: ewf.entry.starred_at.is_some(),
    });

    if changed {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::set_starred_for_user(conn, user_id, entry_id, desired_starred) {
                tracing::warn!("async set_starred failed for entry {entry_id}: {e}");
            }
        });
        // No sidebar_cache.bust here: starring does not change unread counts.
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
        flash: None,
        pane_star_form,
    })
}

/// `POST /entries/{id}/read` — idempotently mark the entry as read, then
/// return a multi-target HTML fragment updating the row + sidebar-unread
/// block. No-op if the entry is already read. No flash toast — marking
/// read is the normal reading flow and doesn't need explicit feedback.
pub async fn read_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    set_read_state(state, auth_user.user.id, entry_id, true).await
}

/// `POST /entries/{id}/unread` — idempotently mark the entry as unread.
/// Emits a "Marked as unread." flash *only* when the call actually changed
/// state, so a stale-label double-click doesn't re-toast.
pub async fn unread_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    set_read_state(state, auth_user.user.id, entry_id, false).await
}

/// Shared core for the two idempotent read/unread handlers. Renders the
/// response optimistically and enqueues the write off the critical path.
async fn set_read_state(
    state: AppState,
    user_id: i64,
    entry_id: i64,
    desired_read: bool,
) -> AppResult<EntryActionMulti> {
    let mut ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let status = state
        .db
        .read_user(move |conn| {
            Ok::<_, AppError>(
                entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                    .get(&entry_id)
                    .copied(),
            )
        })
        .await??;

    let changed = ewf.entry.read_at.is_some() != desired_read;
    let feed_id = ewf.entry.feed_id;

    // Optimistically reflect the new read state in the row.
    ewf.entry.read_at = if desired_read {
        Some(ewf.entry.read_at.unwrap_or_else(chrono::Utc::now))
    } else {
        None
    };

    // Unread count: -1 when newly read, +1 when newly unread, else unchanged.
    let delta = if changed {
        if desired_read {
            -1
        } else {
            1
        }
    } else {
        0
    };
    let payload_json = build_sidebar_unread_with_delta(&state, user_id, feed_id, delta).await?;

    let flash = if !desired_read && changed {
        Some(FlashPayload {
            level: "success",
            message: "Marked as unread.".to_string(),
        })
    } else {
        None
    };

    if changed {
        state.db.user_detached(move |conn| {
            if let Err(e) = entry::set_read_for_user(conn, user_id, entry_id, desired_read) {
                tracing::warn!("async set_read failed for entry {entry_id}: {e}");
            }
        });
        state.sidebar_cache.bust(user_id);
        state.events.emit_sidebar(user_id);
    }

    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
        flash,
        pane_star_form: None,
    })
}

/// `POST /entries/{id}/summarize` — queue a summarization job for the entry
/// and return the reading-pane fragment with `summary_in_flight = true` so the
/// Summarize button is rendered disabled while the job is in flight.
///
/// Ownership is validated via `find_by_id_for_user` (returns 404 for entries
/// not belonging to the user). The pending DB record is created before the
/// cache is marked so the background worker always sees a consistent state.
/// Kagi-config validation is deferred to the background worker.
pub async fn summarize_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummarizePending> {
    let user_id = auth_user.user.id;

    // Fetch the entry and extract the link needed by SummaryJob. Ownership is
    // enforced by find_by_id_for_user's `c.user_id = ?2` join constraint.
    let entry_link = state
        .db
        .user(move |conn| {
            let ewf = entry::find_by_id_for_user(conn, user_id, entry_id)?
                .ok_or(AppError::EntryNotFound)?;

            // Create / reset the pending record in the DB before setting the
            // in-memory cache so the state is always consistent.
            entry_summary::upsert_pending(conn, user_id, entry_id)?;

            // The link may be absent; the background worker will surface an error
            // if so. We use an empty string as a sentinel to let the queue accept
            // the job and fail gracefully rather than returning 400 here.
            Ok::<_, crate::error::AppError>(ewf.entry.link.clone().unwrap_or_default())
        })
        .await??;

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

    Ok(SummarizePending { id: entry_id })
}

/// `POST /entries/{id}/summarize/cancel` — cancel an in-flight / queued
/// summarization (or clear a failed one) and delete the record, returning the
/// summary container to its empty state.
///
/// Cancel (in-flight) and Clear (failed) share this endpoint: both mean "stop
/// and remove this summary". A failed record simply has no live token, so the
/// registry lookup misses and we just delete. Ownership is enforced by
/// `find_by_id_for_user`'s join constraint (404 otherwise).
pub async fn summarize_cancel_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<SummarizeCleared> {
    let user_id = auth_user.user.id;

    // Validate ownership and delete the record in one write txn.
    state
        .db
        .user(move |conn| {
            entry::find_by_id_for_user(conn, user_id, entry_id)?.ok_or(AppError::EntryNotFound)?;
            entry_summary::delete(conn, user_id, entry_id)?;
            Ok::<_, crate::error::AppError>(())
        })
        .await??;

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

    Ok(SummarizeCleared)
}

/// `POST /entries/{id}/fetch-full-content` — fetch the source article from
/// the entry's `link`, sanitize, and return the reading pane with the
/// article body replaced by the new HTML. The response sets
/// `pane.is_full_content = true` so the template swaps "Fetch Full
/// Content" for a "Show Original" link; clicking it re-renders the pane
/// via `GET /entries/{id}/fragment` which restores the feed-supplied body.
pub async fn fetch_full_content_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<ReadingPaneWithFlash> {
    let user_id = auth_user.user.id;

    let ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    let link = ewf
        .entry
        .link
        .clone()
        .ok_or_else(|| AppError::Validation("Entry has no link".to_string()))?;

    let (has_save, has_kagi) = load_pane_action_flags(&state, user_id).await?;

    let (pane, flash) = match fetch_and_extract(&link, &state.config.user_agent).await {
        Ok(extracted) => {
            let sanitized = sanitize_html(
                &extracted.content,
                &state.config.image_proxy_secret,
                Some(&link),
                ewf.custom_referrer.as_deref(),
                None,
            );
            let mut pane =
                build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi).await?;
            pane.content_html = sanitized;
            pane.is_full_content = true;
            (
                pane,
                FlashPayload {
                    level: "success",
                    message: "Fetched full content.".to_string(),
                },
            )
        }
        Err(e) => (
            build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi).await?,
            FlashPayload {
                level: "error",
                message: format!("Failed to fetch full content: {e}"),
            },
        ),
    };
    Ok(ReadingPaneWithFlash {
        pane,
        flash: Some(flash),
    })
}

/// `POST /entries/{id}/save` — send the entry to every configured save
/// service (currently Linkding) and return the reading pane with an inline
/// status message.
pub async fn save_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<ReadingPaneWithFlash> {
    let user_id = auth_user.user.id;

    let (ewf, save_config) = state
        .db
        .read_user(move |conn| {
            let ewf = entry::find_by_id_for_user(conn, user_id, entry_id)?
                .ok_or(AppError::EntryNotFound)?;
            let cfg = user_settings::get_save_services_config(conn, user_id)?;
            Ok::<_, AppError>((ewf, cfg))
        })
        .await??;

    let link = ewf
        .entry
        .link
        .clone()
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
    if let Some(linkding_cfg) = save_config.linkding.as_ref() {
        if linkding_cfg.is_configured() {
            match linkding::save_to_linkding(linkding_cfg, &bookmark).await {
                Ok(result) if result.success => {
                    succeeded.push("Linkding".to_string());
                }
                Ok(result) => failed.push(format!("Linkding: {}", result.message)),
                Err(e) => failed.push(format!("Linkding: {e}")),
            }
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
    let pane = build_reading_pane_view(&state, user_id, &ewf, has_save, has_kagi).await?;
    Ok(ReadingPaneWithFlash {
        pane,
        flash: Some(flash),
    })
}
