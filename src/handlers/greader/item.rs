use axum::{
    extract::{Path, Query, State},
    Form, Json,
};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::models::{category, entry, feed};
use crate::AppState;

use super::auth::GReaderUser;
use super::types::{
    entry_id_to_item_id, item_id_to_entry_id, GReaderAlternateLink, GReaderContent, GReaderItem,
    GReaderLink, GReaderOrigin, ItemRef, StreamContentsResponse, StreamId, StreamItemIdsResponse,
};

// --- stream/contents ---

#[derive(Debug, Deserialize)]
pub struct StreamContentsQuery {
    /// Number of items to return (default 20)
    pub n: Option<i64>,
    /// Continuation token (last item ID from previous page)
    pub c: Option<String>,
    /// Exclude tag (e.g., `user/-/state/com.google/read`)
    pub xt: Option<String>,
    /// Include tag
    pub it: Option<String>,
    /// Oldest timestamp (seconds since epoch)
    pub ot: Option<i64>,
    /// Newest timestamp (seconds since epoch)
    pub nt: Option<i64>,
    /// Sort order: `o` for oldest first
    pub r: Option<String>,
}

/// `GET /reader/api/0/stream/contents/*stream`
pub async fn stream_contents(
    auth: GReaderUser,
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Query(query): Query<StreamContentsQuery>,
) -> AppResult<Json<StreamContentsResponse>> {
    let stream_id = StreamId::parse(&stream)?;
    let user_id = auth.user.id;
    let count = query.n.unwrap_or(20).min(1000);

    // Build filter from stream ID and query params
    let filter = build_entry_filter(&stream_id, &query)?;
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1, // fetch one extra for continuation
        continuation_id: query.c.as_ref().and_then(|c| c.parse::<i64>().ok()),
        ot: query.ot,
        nt: query.nt,
    };

    let response = state
        .db
        .user(move |conn| {
            // Resolve stream-specific constraints
            let mut effective_filter = filter;

            // Resolve feed_id from stream if it's a Feed stream
            if let StreamId::Feed(ref url) = stream_id {
                let f = feed::find_by_url_for_user(conn, url, user_id)?
                    .ok_or(AppError::FeedNotFound)?;
                effective_filter.feed_id = Some(f.id);
            }

            // Resolve category_id from stream if it's a Label stream
            if let StreamId::Label(ref name) = stream_id {
                let cat = category::find_by_name_and_user(conn, name, user_id)?
                    .ok_or(AppError::CategoryNotFound)?;
                effective_filter.category_id = Some(cat.id);
            }

            // Fetch entries with continuation-based pagination
            let entries = entry::list_by_user_with_continuation(
                conn,
                user_id,
                &effective_filter,
                &pagination,
            )?;

            let has_more = entries.len() as i64 > count;
            let entries: Vec<_> = entries.into_iter().take(count as usize).collect();

            let continuation = if has_more {
                entries.last().map(|e| e.entry.id.to_string())
            } else {
                None
            };

            let items: Vec<GReaderItem> = entries
                .iter()
                .map(entry_with_feed_to_greader_item)
                .collect();

            let updated = entries
                .first()
                .map(|e| e.entry.updated_at.timestamp())
                .unwrap_or(0);

            Ok::<_, AppError>(StreamContentsResponse {
                id: stream_id.to_string(),
                updated,
                continuation,
                items,
            })
        })
        .await??;

    Ok(Json(response))
}

// --- stream/items/ids ---

#[derive(Debug, Deserialize)]
pub struct StreamItemIdsQuery {
    /// Stream ID
    pub s: Option<String>,
    /// Count
    pub n: Option<i64>,
    /// Continuation
    pub c: Option<String>,
    /// Exclude tag
    pub xt: Option<String>,
    /// Include tag
    pub it: Option<String>,
    /// Oldest timestamp
    pub ot: Option<i64>,
    /// Newest timestamp
    pub nt: Option<i64>,
    /// Sort order
    pub r: Option<String>,
}

/// `GET /reader/api/0/stream/items/ids`
pub async fn stream_item_ids(
    auth: GReaderUser,
    State(state): State<AppState>,
    Query(query): Query<StreamItemIdsQuery>,
) -> AppResult<Json<StreamItemIdsResponse>> {
    let stream_str = query
        .s
        .as_deref()
        .unwrap_or("user/-/state/com.google/reading-list");
    let stream_id = StreamId::parse(stream_str)?;
    let user_id = auth.user.id;
    let count = query.n.unwrap_or(20).min(10000);

    let filter = build_entry_filter_from_params(
        &stream_id,
        query.xt.as_deref(),
        query.it.as_deref(),
    )?;
    let pagination = entry::ContinuationParams {
        oldest_first: query.r.as_deref() == Some("o"),
        limit: count + 1,
        continuation_id: query.c.as_ref().and_then(|c| c.parse::<i64>().ok()),
        ot: query.ot,
        nt: query.nt,
    };

    let response = state
        .db
        .user(move |conn| {
            let mut effective_filter = filter;

            if let StreamId::Feed(ref url) = stream_id {
                let f = feed::find_by_url_for_user(conn, url, user_id)?
                    .ok_or(AppError::FeedNotFound)?;
                effective_filter.feed_id = Some(f.id);
            }

            if let StreamId::Label(ref name) = stream_id {
                let cat = category::find_by_name_and_user(conn, name, user_id)?
                    .ok_or(AppError::CategoryNotFound)?;
                effective_filter.category_id = Some(cat.id);
            }

            let entries = entry::list_ids_by_user(
                conn,
                user_id,
                &effective_filter,
                &pagination,
            )?;

            let has_more = entries.len() as i64 > count;
            let entries: Vec<_> = entries.into_iter().take(count as usize).collect();

            let continuation = if has_more {
                entries.last().map(|(id, _)| id.to_string())
            } else {
                None
            };

            let item_refs = entries
                .iter()
                .map(|(id, timestamp_usec)| ItemRef {
                    id: id.to_string(),
                    timestamp_usec: timestamp_usec.to_string(),
                })
                .collect();

            Ok::<_, AppError>(StreamItemIdsResponse {
                item_refs,
                continuation,
            })
        })
        .await??;

    Ok(Json(response))
}

// --- stream/items/count ---

#[derive(Debug, Deserialize)]
pub struct StreamItemCountQuery {
    /// Stream ID
    pub s: Option<String>,
}

/// `GET /reader/api/0/stream/items/count`
pub async fn stream_item_count(
    auth: GReaderUser,
    State(state): State<AppState>,
    Query(query): Query<StreamItemCountQuery>,
) -> AppResult<String> {
    let stream_str = query
        .s
        .as_deref()
        .unwrap_or("user/-/state/com.google/reading-list");
    let stream_id = StreamId::parse(stream_str)?;
    let user_id = auth.user.id;

    let count = state
        .db
        .user(move |conn| {
            let mut filter = entry::EntryFilter::default();

            match &stream_id {
                StreamId::ReadingList => {}
                StreamId::Read => filter.read_only = true,
                StreamId::Starred => filter.starred_only = true,
                StreamId::KeptUnread => filter.unread_only = true,
                StreamId::Label(name) => {
                    let cat = category::find_by_name_and_user(conn, name, user_id)?
                        .ok_or(AppError::CategoryNotFound)?;
                    filter.category_id = Some(cat.id);
                }
                StreamId::Feed(url) => {
                    let f = feed::find_by_url_for_user(conn, url, user_id)?
                        .ok_or(AppError::FeedNotFound)?;
                    filter.feed_id = Some(f.id);
                }
            }

            entry::count_by_user(conn, user_id, &filter)
        })
        .await??;

    Ok(count.to_string())
}

// --- stream/items/contents ---

/// `GET /reader/api/0/stream/items/contents` (query params)
pub async fn stream_items_contents(
    auth: GReaderUser,
    State(state): State<AppState>,
    Query(query_params): Query<Vec<(String, String)>>,
) -> AppResult<Json<StreamContentsResponse>> {
    let item_ids: Vec<String> = query_params
        .iter()
        .filter(|(key, _)| key == "i")
        .map(|(_, value)| value.clone())
        .collect();

    fetch_items_by_ids(auth, state, item_ids).await
}

/// `POST /reader/api/0/stream/items/contents` (form data)
pub async fn stream_items_contents_post(
    auth: GReaderUser,
    State(state): State<AppState>,
    Form(form_data): Form<Vec<(String, String)>>,
) -> AppResult<Json<StreamContentsResponse>> {
    let item_ids: Vec<String> = form_data
        .iter()
        .filter(|(key, _)| key == "i")
        .map(|(_, value)| value.clone())
        .collect();

    fetch_items_by_ids(auth, state, item_ids).await
}

/// Shared implementation for stream/items/contents (GET and POST).
async fn fetch_items_by_ids(
    auth: GReaderUser,
    state: AppState,
    item_ids: Vec<String>,
) -> AppResult<Json<StreamContentsResponse>> {
    let user_id = auth.user.id;

    if item_ids.is_empty() {
        return Ok(Json(StreamContentsResponse {
            id: "user/-/state/com.google/reading-list".to_string(),
            updated: 0,
            continuation: None,
            items: vec![],
        }));
    }

    let entry_ids: Vec<i64> = item_ids
        .iter()
        .map(|s| item_id_to_entry_id(s))
        .collect::<AppResult<Vec<_>>>()?;

    let response = state
        .db
        .user(move |conn| {
            let entries = entry::find_by_ids_with_feed(conn, user_id, &entry_ids)?;

            let items: Vec<GReaderItem> = entries
                .iter()
                .map(entry_with_feed_to_greader_item)
                .collect();

            let updated = entries
                .first()
                .map(|e| e.entry.updated_at.timestamp())
                .unwrap_or(0);

            Ok::<_, AppError>(StreamContentsResponse {
                id: "user/-/state/com.google/reading-list".to_string(),
                updated,
                continuation: None,
                items,
            })
        })
        .await??;

    Ok(Json(response))
}

// --- Helpers ---

/// Build an `EntryFilter` from stream ID and query params.
fn build_entry_filter(
    stream_id: &StreamId,
    query: &StreamContentsQuery,
) -> AppResult<entry::EntryFilter> {
    build_entry_filter_from_params(stream_id, query.xt.as_deref(), query.it.as_deref())
}

fn build_entry_filter_from_params(
    stream_id: &StreamId,
    xt: Option<&str>,
    it: Option<&str>,
) -> AppResult<entry::EntryFilter> {
    let mut filter = entry::EntryFilter::default();

    // Apply stream ID filter
    match stream_id {
        StreamId::ReadingList => {} // all entries
        StreamId::Read => filter.read_only = true,
        StreamId::Starred => filter.starred_only = true,
        StreamId::KeptUnread => filter.unread_only = true,
        StreamId::Label(_) => {} // category_id set later
        StreamId::Feed(_) => {}  // feed_id set later
    }

    // Apply exclude tag
    if let Some(xt_str) = xt {
        if let Ok(xt_stream) = StreamId::parse(xt_str) {
            match xt_stream {
                StreamId::Read => filter.unread_only = true,
                StreamId::Starred => {} // exclude starred — no direct filter, ignore for now
                _ => {}
            }
        }
    }

    // Apply include tag
    if let Some(it_str) = it {
        if let Ok(it_stream) = StreamId::parse(it_str) {
            match it_stream {
                StreamId::Read => filter.read_only = true,
                StreamId::Starred => filter.starred_only = true,
                _ => {}
            }
        }
    }

    Ok(filter)
}

/// Convert an `EntryWithFeed` to a Google Reader `GReaderItem`.
fn entry_with_feed_to_greader_item(ewf: &entry::EntryWithFeed) -> GReaderItem {
    let e = &ewf.entry;

    // Build categories array
    let mut categories = vec!["user/-/state/com.google/reading-list".to_string()];

    if e.read_at.is_some() {
        categories.push("user/-/state/com.google/read".to_string());
    }

    if e.starred_at.is_some() {
        categories.push("user/-/state/com.google/starred".to_string());
    }

    categories.push(format!("user/-/label/{}", ewf.category_name));

    let published = e
        .published_at
        .unwrap_or(e.created_at)
        .timestamp();
    let updated = e.updated_at.timestamp();
    let crawl_time_msec = (e.created_at.timestamp_millis()).to_string();
    let timestamp_usec = (published * 1_000_000).to_string();

    let link = e.link.as_deref().unwrap_or("");
    let content = e.content.as_deref().or(e.summary.as_deref()).unwrap_or("");

    GReaderItem {
        id: entry_id_to_item_id(e.id),
        published,
        updated,
        crawl_time_msec,
        timestamp_usec,
        title: e.title.clone().unwrap_or_default(),
        categories,
        summary: GReaderContent {
            content: content.to_string(),
        },
        canonical: vec![GReaderLink {
            href: link.to_string(),
        }],
        alternate: vec![GReaderAlternateLink {
            href: link.to_string(),
            link_type: "text/html".to_string(),
        }],
        author: e.author.clone().unwrap_or_default(),
        origin: GReaderOrigin {
            stream_id: format!("feed/{}", ewf.feed_url),
            title: ewf.feed_title.clone().unwrap_or_default(),
            html_url: ewf.feed_url.clone(),
        },
        // RDRS extensions
        entry_id: e.id,
        feed_id: e.feed_id,
        category_id: ewf.category_id,
        category_name: ewf.category_name.clone(),
        feed_has_icon: ewf.feed_has_icon,
        read_at: e.read_at.map(|dt| dt.to_rfc3339()),
        starred_at: e.starred_at.map(|dt| dt.to_rfc3339()),
        published_at: e.published_at.map(|dt| dt.to_rfc3339()),
        content: e.content.clone(),
    }
}
