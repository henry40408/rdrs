use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::middleware::auth::AuthUser;
use crate::models::{category, entry, feed};
use crate::services::{refresh_feed, sanitize_html, SyncResult};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListEntriesQuery {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub starred_only: bool,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize)]
pub struct EntriesResponse {
    pub entries: Vec<entry::EntryWithFeed>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list_entries(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<ListEntriesQuery>,
) -> AppResult<Json<EntriesResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify category belongs to user if specified
    if let Some(category_id) = query.category_id {
        let cat = category::find_by_id(&conn, category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::CategoryNotFound);
        }
    }

    // Verify feed belongs to user if specified
    if let Some(feed_id) = query.feed_id {
        let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::FeedNotFound);
        }
    }

    let filter = entry::EntryFilter {
        feed_id: query.feed_id,
        category_id: query.category_id,
        unread_only: query.unread_only,
        starred_only: query.starred_only,
    };

    let entries =
        entry::list_by_user(&conn, auth_user.user.id, &filter, query.limit, query.offset)?;
    let total = entry::count_by_user(&conn, auth_user.user.id, &filter)?;

    Ok(Json(EntriesResponse {
        entries,
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

#[derive(Debug, Serialize)]
pub struct EntryResponse {
    #[serde(flatten)]
    pub entry: entry::EntryWithFeed,
    pub sanitized_content: Option<String>,
}

pub async fn get_entry(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<EntryResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    let entry_with_feed = entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

    // Verify entry belongs to user
    let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != auth_user.user.id {
        return Err(AppError::EntryNotFound);
    }

    let sanitized_content = entry_with_feed
        .entry
        .content
        .as_ref()
        .map(|c| sanitize_html(c, &state.config.image_proxy_secret));

    Ok(Json(EntryResponse {
        entry: entry_with_feed,
        sanitized_content,
    }))
}

pub async fn list_feed_entries(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(feed_id): Path<i64>,
    Query(query): Query<ListEntriesQuery>,
) -> AppResult<Json<EntriesResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify feed belongs to user
    let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
    let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != auth_user.user.id {
        return Err(AppError::FeedNotFound);
    }

    let filter = entry::EntryFilter {
        feed_id: Some(feed_id),
        category_id: None,
        unread_only: query.unread_only,
        starred_only: query.starred_only,
    };

    let entries =
        entry::list_by_user(&conn, auth_user.user.id, &filter, query.limit, query.offset)?;
    let total = entry::count_by_user(&conn, auth_user.user.id, &filter)?;

    Ok(Json(EntriesResponse {
        entries,
        total,
        limit: query.limit,
        offset: query.offset,
    }))
}

pub async fn mark_entry_read(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<entry::Entry>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify entry belongs to user
    let entry_with_feed = entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;
    let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != auth_user.user.id {
        return Err(AppError::EntryNotFound);
    }

    let updated = entry::mark_as_read(&conn, id)?;
    Ok(Json(updated))
}

pub async fn mark_entry_unread(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<entry::Entry>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify entry belongs to user
    let entry_with_feed = entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;
    let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != auth_user.user.id {
        return Err(AppError::EntryNotFound);
    }

    let updated = entry::mark_as_unread(&conn, id)?;
    Ok(Json(updated))
}

pub async fn toggle_entry_star(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<entry::Entry>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify entry belongs to user
    let entry_with_feed = entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;
    let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != auth_user.user.id {
        return Err(AppError::EntryNotFound);
    }

    let updated = entry::toggle_star(&conn, id)?;
    Ok(Json(updated))
}

#[derive(Debug, Deserialize)]
pub struct MarkAllReadRequest {
    pub feed_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct MarkAllReadResponse {
    pub marked_count: i64,
}

pub async fn mark_all_read(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<MarkAllReadRequest>,
) -> AppResult<Json<MarkAllReadResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    let marked_count = if let Some(feed_id) = body.feed_id {
        // Verify feed belongs to user
        let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::FeedNotFound);
        }
        entry::mark_all_read_by_feed(&conn, feed_id)?
    } else {
        entry::mark_all_read_by_user(&conn, auth_user.user.id)?
    };

    Ok(Json(MarkAllReadResponse { marked_count }))
}

#[derive(Debug, Serialize)]
pub struct UnreadStatsResponse {
    pub by_feed: std::collections::HashMap<i64, i64>,
    pub by_category: std::collections::HashMap<i64, i64>,
}

pub async fn get_unread_stats(
    auth_user: AuthUser,
    State(state): State<AppState>,
) -> AppResult<Json<UnreadStatsResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    let by_feed = entry::count_unread_by_feed(&conn, auth_user.user.id)?;
    let by_category = entry::count_unread_by_category(&conn, auth_user.user.id)?;

    Ok(Json(UnreadStatsResponse {
        by_feed,
        by_category,
    }))
}

pub async fn refresh_feed_handler(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(feed_id): Path<i64>,
) -> AppResult<Json<SyncResult>> {
    // Verify feed belongs to user
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;
        let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::FeedNotFound);
        }
    }

    let result = refresh_feed(state.db.clone(), feed_id, &state.config.user_agent).await?;
    Ok(Json(result))
}
