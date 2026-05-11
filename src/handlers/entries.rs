use askama::Template;
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};

use crate::{
    error::{AppError, AppResult},
    handlers::pages::{format_relative_time, row_view_from, EntryRowView, ReadingPaneView},
    middleware::auth::PageAuthUser,
    models::{entry, entry_summary},
    services::{sanitize_html, SummaryJob, SummaryStatus},
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

/// `GET /entries/{id}/fragment` — returns the reading-pane HTML fragment for
/// the given entry. The entry must belong to the authenticated user; otherwise
/// a 404 is returned (same semantics as the JSON `/api/entries/{id}` endpoint
/// it replaces for SSR consumers).
pub async fn entry_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<OpenEntryMulti> {
    let user_id = auth_user.user.id;

    // Verify ownership, mark unread→read, re-fetch the entry to reflect the
    // new state, and pick up the summary status — all in a single write
    // transaction. Marking-as-read on a `GET` is unusual REST-wise, but it
    // matches the feed-reader convention (Reeder / FreshRSS / etc. behave the
    // same way) and the operation is idempotent on the read row.
    let (ewf, status) = state
        .db
        .user(move |conn| {
            let pre = entry::find_by_id_for_user(conn, user_id, entry_id)?
                .ok_or(AppError::EntryNotFound)?;
            if pre.entry.read_at.is_none() {
                entry::mark_as_read(conn, entry_id)?;
            }
            let post = entry::find_by_id_for_user(conn, user_id, entry_id)?
                .ok_or(AppError::EntryNotFound)?;
            let status = entry_summary::get_statuses_for_entries(conn, user_id, &[entry_id])?
                .get(&entry_id)
                .copied();
            Ok::<_, AppError>((post, status))
        })
        .await??;

    let pane = build_reading_pane_view(&state, user_id, &ewf);
    let row = row_view_from(&ewf, status);
    let sidebar_unread_payload_json = build_sidebar_unread(&state, user_id).await?;
    Ok(OpenEntryMulti {
        pane,
        r: row,
        sidebar_unread_payload_json,
    })
}

/// Build a `ReadingPaneView` for the given entry, scoped to `user_id`.
/// Returns `AppError::EntryNotFound` if the entry does not exist or belongs
/// to a different user.
pub(crate) async fn load_reading_pane(
    state: &AppState,
    user_id: i64,
    entry_id: i64,
) -> AppResult<ReadingPaneView> {
    let ewf = state
        .db
        .read_user(move |conn| entry::find_by_id_for_user(conn, user_id, entry_id))
        .await??
        .ok_or(AppError::EntryNotFound)?;
    Ok(build_reading_pane_view(state, user_id, &ewf))
}

/// Build a `ReadingPaneView` from an already-loaded `EntryWithFeed`. The
/// content sanitizer + summary-cache lookup happen here so callers that
/// already have the entry (e.g. `entry_fragment`, which loads it inside its
/// write transaction) don't re-hit the DB.
pub(crate) fn build_reading_pane_view(
    state: &AppState,
    user_id: i64,
    ewf: &entry::EntryWithFeed,
) -> ReadingPaneView {
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

    let cache_entry = state.summary_cache.get(user_id, entry_id);
    let (summary_text, summary_in_flight) = match cache_entry.as_ref().map(|e| &e.status) {
        Some(SummaryStatus::Completed) => (cache_entry.and_then(|e| e.summary_text.clone()), false),
        Some(SummaryStatus::Pending) | Some(SummaryStatus::Processing) => (None, true),
        _ => (None, false),
    };

    let published_at = ewf.entry.published_at;
    ReadingPaneView {
        id: entry_id,
        title: ewf
            .entry
            .title
            .clone()
            .unwrap_or_else(|| "(no title)".to_string()),
        link: ewf.entry.link.clone(),
        feed_title: ewf.feed_title.clone().unwrap_or_default(),
        author: ewf.entry.author.clone(),
        published_at_iso: published_at.map(|t| t.to_rfc3339()),
        published_relative: format_relative_time(published_at).0,
        content_html,
        is_read: ewf.entry.read_at.is_some(),
        is_starred: ewf.entry.starred_at.is_some(),
        summary_text,
        summary_in_flight,
    }
}

/// Multi-target action response template. Renders two `<template data-swap-target>` blocks:
/// one for the updated entry row and one for the sidebar-unread payload.
#[derive(Template)]
#[template(path = "_entry_actions_multi.html")]
pub struct EntryActionMulti {
    pub r: EntryRowView,
    pub sidebar_unread_payload_json: String,
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

/// Build the JSON payload for the `#sidebar-unread` block by querying unread
/// counts per feed for the given user. Used by star/read action endpoints and
/// the dedicated `/sidebar/unread` polling endpoint (T7).
pub(crate) async fn build_sidebar_unread(state: &AppState, user_id: i64) -> AppResult<String> {
    let counts = state
        .db
        .read_user(move |conn| entry::unread_counts_per_feed(conn, user_id))
        .await??;
    Ok(serde_json::to_string(&counts).unwrap_or_else(|_| "[]".to_string()))
}

/// `POST /entries/{id}/star` — toggle the starred state for the entry, then
/// return a multi-target HTML fragment updating the row + sidebar-unread block.
pub async fn star_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    let user_id = auth_user.user.id;
    let (ewf, status) = state
        .db
        .user(move |conn| {
            let ewf = entry::toggle_starred(conn, user_id, entry_id)?;
            let status = if let Some(ref e) = ewf {
                entry_summary::get_statuses_for_entries(conn, user_id, &[e.entry.id])?
                    .get(&e.entry.id)
                    .copied()
            } else {
                None
            };
            Ok::<_, AppError>((ewf, status))
        })
        .await??;
    let ewf = ewf.ok_or(AppError::EntryNotFound)?;
    let payload_json = build_sidebar_unread(&state, user_id).await?;
    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
    })
}

/// `POST /entries/{id}/read` — toggle the read state for the entry, then
/// return a multi-target HTML fragment updating the row + sidebar-unread block.
pub async fn read_entry_form(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    AxumPath(entry_id): AxumPath<i64>,
) -> AppResult<EntryActionMulti> {
    let user_id = auth_user.user.id;
    let (ewf, status) = state
        .db
        .user(move |conn| {
            let ewf = entry::toggle_read(conn, user_id, entry_id)?;
            let status = if let Some(ref e) = ewf {
                entry_summary::get_statuses_for_entries(conn, user_id, &[e.entry.id])?
                    .get(&e.entry.id)
                    .copied()
            } else {
                None
            };
            Ok::<_, AppError>((ewf, status))
        })
        .await??;
    let ewf = ewf.ok_or(AppError::EntryNotFound)?;
    let payload_json = build_sidebar_unread(&state, user_id).await?;
    Ok(EntryActionMulti {
        r: row_view_from(&ewf, status),
        sidebar_unread_payload_json: payload_json,
    })
}

/// Fragment template for the sidebar-unread polling block — renders `_sidebar_unread.html`
/// and is returned by `GET /sidebar/unread`.
#[derive(Template)]
#[template(path = "_sidebar_unread.html")]
pub struct SidebarUnreadFragment {
    pub payload_json: String,
}

impl IntoResponse for SidebarUnreadFragment {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}

/// `GET /sidebar/unread` — returns the `_sidebar_unread.html` partial with the
/// current unread-count payload. Used by `app.js` as the polling target for the
/// sidebar unread-count display (polled every 20 s via `setInterval`).
pub async fn sidebar_unread_fragment(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
) -> AppResult<SidebarUnreadFragment> {
    let user_id = auth_user.user.id;
    let payload_json = build_sidebar_unread(&state, user_id).await?;
    Ok(SidebarUnreadFragment { payload_json })
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
) -> AppResult<ReadingPaneFragment> {
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
    // the in-flight reading pane (the DB record is already pending).
    let _ = state
        .summary_tx
        .send(SummaryJob {
            user_id,
            entry_id,
            entry_link,
        })
        .await;

    let pane = load_reading_pane(&state, user_id, entry_id).await?;
    Ok(ReadingPaneFragment { pane })
}
