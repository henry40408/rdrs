use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::middleware::auth::AuthUser;
use crate::models::{category, entry, feed, user_settings};
use crate::services::save::{linkding, BookmarkData, SaveResult};
use crate::services::{fetch_and_extract, refresh_feed, sanitize_html, SummaryJob, SummaryStatus, SyncResult};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListEntriesQuery {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub starred_only: bool,
    pub search: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Entry with summary status
#[derive(Debug, Serialize)]
pub struct EntryWithSummary {
    #[serde(flatten)]
    pub entry: entry::EntryWithFeed,
    pub has_summary: bool,
}

#[derive(Debug, Serialize)]
pub struct EntriesResponse {
    pub entries: Vec<EntryWithSummary>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list_entries(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<ListEntriesQuery>,
) -> AppResult<Json<EntriesResponse>> {
    let user_id = auth_user.user.id;

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify category belongs to user if specified
    if let Some(category_id) = query.category_id {
        let cat = category::find_by_id(&conn, category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::CategoryNotFound);
        }
    }

    // Verify feed belongs to user if specified
    if let Some(feed_id) = query.feed_id {
        let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::FeedNotFound);
        }
    }

    let filter = entry::EntryFilter {
        feed_id: query.feed_id,
        category_id: query.category_id,
        unread_only: query.unread_only,
        starred_only: query.starred_only,
        search: query.search.clone(),
    };

    let entries =
        entry::list_by_user(&conn, user_id, &filter, query.limit, query.offset)?;
    let total = entry::count_by_user(&conn, user_id, &filter)?;

    // Check summary status for each entry
    let entries_with_summary: Vec<EntryWithSummary> = entries
        .into_iter()
        .map(|e| {
            let has_summary = state.summary_cache.has_completed_summary(user_id, e.entry.id);
            EntryWithSummary {
                entry: e,
                has_summary,
            }
        })
        .collect();

    Ok(Json(EntriesResponse {
        entries: entries_with_summary,
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
    pub has_summary: bool,
}

pub async fn get_entry(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<EntryResponse>> {
    let user_id = auth_user.user.id;

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    let entry_with_feed = entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

    // Verify entry belongs to user
    let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Use entry link as base URL for resolving relative image paths
    let base_url = entry_with_feed.entry.link.as_deref();
    let sanitized_content = entry_with_feed
        .entry
        .content
        .as_ref()
        .map(|c| sanitize_html(c, &state.config.image_proxy_secret, base_url));

    // Check if there's a completed summary
    let has_summary = state.summary_cache.has_completed_summary(user_id, id);

    Ok(Json(EntryResponse {
        entry: entry_with_feed,
        sanitized_content,
        has_summary,
    }))
}

pub async fn list_feed_entries(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(feed_id): Path<i64>,
    Query(query): Query<ListEntriesQuery>,
) -> AppResult<Json<EntriesResponse>> {
    let user_id = auth_user.user.id;

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

    // Verify feed belongs to user
    let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
    let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::FeedNotFound);
    }

    let filter = entry::EntryFilter {
        feed_id: Some(feed_id),
        category_id: None,
        unread_only: query.unread_only,
        starred_only: query.starred_only,
        search: query.search,
    };

    let entries =
        entry::list_by_user(&conn, user_id, &filter, query.limit, query.offset)?;
    let total = entry::count_by_user(&conn, user_id, &filter)?;

    // Check summary status for each entry
    let entries_with_summary: Vec<EntryWithSummary> = entries
        .into_iter()
        .map(|e| {
            let has_summary = state.summary_cache.has_completed_summary(user_id, e.entry.id);
            EntryWithSummary {
                entry: e,
                has_summary,
            }
        })
        .collect();

    Ok(Json(EntriesResponse {
        entries: entries_with_summary,
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
    pub category_id: Option<i64>,
    /// Mark entries older than this many days as read.
    /// None means mark all entries.
    pub older_than_days: Option<i64>,
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

    let older_than_days = body.older_than_days;

    let marked_count = if let Some(feed_id) = body.feed_id {
        // Verify feed belongs to user
        let f = feed::find_by_id(&conn, feed_id)?.ok_or(AppError::FeedNotFound)?;
        let cat = category::find_by_id(&conn, f.category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::FeedNotFound);
        }
        entry::mark_all_read_by_feed(&conn, feed_id, older_than_days)?
    } else if let Some(category_id) = body.category_id {
        // Verify category belongs to user
        let cat = category::find_by_id(&conn, category_id)?.ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::CategoryNotFound);
        }
        entry::mark_all_read_by_category(&conn, category_id, older_than_days)?
    } else {
        entry::mark_all_read_by_user(&conn, auth_user.user.id, older_than_days)?
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

#[derive(Debug, Serialize)]
pub struct FetchFullContentResponse {
    pub title: Option<String>,
    pub content: String,
    pub sanitized_content: String,
}

pub async fn get_entry_neighbors(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<entry::EntryNeighbors>> {
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

    let neighbors = entry::find_neighbors(&conn, auth_user.user.id, id)?;
    Ok(Json(neighbors))
}

pub async fn fetch_full_content(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<FetchFullContentResponse>> {
    // Verify entry exists and belongs to user
    let link = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

        let entry_with_feed =
            entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

        let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::EntryNotFound);
        }

        // Check if entry has a link
        entry_with_feed
            .entry
            .link
            .ok_or_else(|| AppError::Validation("Entry has no link".to_string()))?
    };

    // Fetch and extract content
    let extracted = fetch_and_extract(&link, &state.config.user_agent).await?;

    // Sanitize the content (use the entry link as base URL for relative images)
    let sanitized_content = sanitize_html(
        &extracted.content,
        &state.config.image_proxy_secret,
        Some(&link),
    );

    Ok(Json(FetchFullContentResponse {
        title: extracted.title,
        content: extracted.content,
        sanitized_content,
    }))
}

#[derive(Debug, Serialize)]
pub struct SaveToServicesResponse {
    pub results: Vec<SaveResult>,
    pub all_success: bool,
}

/// Response for summary-related endpoints
#[derive(Debug, Serialize)]
pub struct SummaryResponse {
    pub status: SummaryStatus,
    pub summary_text: Option<String>,
    pub error: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// POST /api/entries/{id}/summarize - Queue or return cached summary
pub async fn summarize_entry(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<SummaryResponse>> {
    let user_id = auth_user.user.id;

    // Check cache first
    if let Some(cached) = state.summary_cache.get(user_id, id) {
        return Ok(Json(SummaryResponse {
            status: cached.status,
            summary_text: cached.summary_text,
            error: cached.error_message,
            created_at: Some(cached.created_at),
        }));
    }

    // Get entry and verify ownership
    let link = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

        let entry_with_feed =
            entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

        // Verify entry belongs to user
        let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::EntryNotFound);
        }

        // Check if entry has a link
        let link =
            entry_with_feed.entry.link.clone().ok_or_else(|| {
                AppError::Validation("Entry has no link to summarize".to_string())
            })?;

        // Verify Kagi is configured
        let config = user_settings::get_save_services_config(&conn, user_id)?;
        let kagi = config
            .kagi
            .ok_or_else(|| AppError::Validation("Kagi is not configured".to_string()))?;

        if !kagi.is_configured() {
            return Err(AppError::Validation("Kagi is not configured".to_string()));
        }

        link
    };

    // Set pending status and queue the job
    state.summary_cache.set_pending(user_id, id);

    let job = SummaryJob {
        user_id,
        entry_id: id,
        entry_link: link,
    };

    state
        .summary_tx
        .send(job)
        .await
        .map_err(|e| AppError::Internal(format!("Failed to queue summary job: {}", e)))?;

    // Return pending status
    Ok(Json(SummaryResponse {
        status: SummaryStatus::Pending,
        summary_text: None,
        error: None,
        created_at: Some(chrono::Utc::now()),
    }))
}

/// GET /api/entries/{id}/summary - Get summary status
pub async fn get_entry_summary(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<SummaryResponse>> {
    let user_id = auth_user.user.id;

    // Verify entry ownership
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

        let entry_with_feed =
            entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

        let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::EntryNotFound);
        }
    }

    // Get from cache
    if let Some(cached) = state.summary_cache.get(user_id, id) {
        Ok(Json(SummaryResponse {
            status: cached.status,
            summary_text: cached.summary_text,
            error: cached.error_message,
            created_at: Some(cached.created_at),
        }))
    } else {
        // No summary exists
        Err(AppError::NotFound("No summary found".to_string()))
    }
}

/// DELETE /api/entries/{id}/summary - Delete summary from cache
pub async fn delete_entry_summary(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let user_id = auth_user.user.id;

    // Verify entry ownership
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

        let entry_with_feed =
            entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

        let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != user_id {
            return Err(AppError::EntryNotFound);
        }
    }

    // Remove from cache
    state.summary_cache.remove(user_id, id);

    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn save_to_services(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<SaveToServicesResponse>> {
    // Get entry and verify ownership
    let (entry_data, save_config) = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("DB lock failed".to_string()))?;

        let entry_with_feed =
            entry::find_by_id_with_feed(&conn, id)?.ok_or(AppError::EntryNotFound)?;

        // Verify entry belongs to user
        let cat = category::find_by_id(&conn, entry_with_feed.category_id)?
            .ok_or(AppError::CategoryNotFound)?;
        if cat.user_id != auth_user.user.id {
            return Err(AppError::EntryNotFound);
        }

        // Check if entry has a link
        let link = entry_with_feed
            .entry
            .link
            .clone()
            .ok_or_else(|| AppError::Validation("Entry has no link to save".to_string()))?;

        // Get save services config
        let config = user_settings::get_save_services_config(&conn, auth_user.user.id)?;

        if !config.has_any_service() {
            return Err(AppError::Validation(
                "No save services configured".to_string(),
            ));
        }

        let bookmark = BookmarkData {
            url: link,
            title: entry_with_feed.entry.title.clone(),
            description: entry_with_feed.entry.summary.clone(),
            tags: vec![],
        };

        (bookmark, config)
    };

    // Save to all configured services in parallel
    let mut results = Vec::new();

    // Linkding
    if let Some(linkding_config) = &save_config.linkding {
        if linkding_config.is_configured() {
            let result = linkding::save_to_linkding(linkding_config, &entry_data).await?;
            results.push(result);
        }
    }

    // Future services can be added here:
    // if let Some(pocket_config) = &save_config.pocket { ... }

    let all_success = results.iter().all(|r| r.success);

    Ok(Json(SaveToServicesResponse {
        results,
        all_success,
    }))
}
