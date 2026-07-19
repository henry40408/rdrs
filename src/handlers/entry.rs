use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::auth::AuthUser;
use crate::models::{SummaryStatus, category, entry, entry_summary, user_settings};
use crate::services::http::JOB_QUEUE_TIMEOUT;
use crate::services::save::{BookmarkData, SaveResult, linkding};
use crate::services::{SummaryJob, fetch_and_extract, sanitize_html};

#[derive(Debug, Serialize)]
pub struct FetchFullContentResponse {
    pub title: Option<String>,
    pub content: String,
    pub sanitized_content: String,
}

#[derive(Debug, Deserialize)]
pub struct NeighborsQuery {
    #[serde(default)]
    pub unread_only: bool,
    #[serde(default)]
    pub starred_only: bool,
    #[serde(default)]
    pub read_only: bool,
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    pub has_summary: Option<bool>,
    /// Unread-snapshot boundary (UTC `YYYY-MM-DD HH:MM:SS`), forwarded into
    /// `EntryFilter::read_after`. Sent by app.js from the page's
    /// `data-snapshot-at` attribute on unread views.
    pub read_after: Option<String>,
}

pub async fn get_entry_neighbors(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<NeighborsQuery>,
) -> AppResult<Json<entry::EntryNeighbors>> {
    let user_id = auth_user.user.id;
    // Verify entry belongs to user
    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;
    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    let filter = entry::EntryFilter {
        feed_id: query.feed_id,
        category_id: query.category_id,
        unread_only: query.unread_only,
        starred_only: query.starred_only,
        read_only: query.read_only,
        has_summary: query.has_summary,
        search: None,
        read_after: query.read_after,
        ..Default::default()
    };
    let neighbors = entry::find_neighbors(&state.db, user_id, id, &filter).await?;
    Ok(Json(neighbors))
}

pub async fn fetch_full_content(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<FetchFullContentResponse>> {
    // Verify entry exists and belongs to user
    let user_id = auth_user.user.id;
    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Check if entry has a link
    let link = entry_with_feed
        .entry
        .link
        .ok_or_else(|| AppError::Validation("Entry has no link".to_string()))?;

    let custom_referrer = entry_with_feed.custom_referrer;

    // Fetch and extract content
    let extracted = fetch_and_extract(&link, &state.config.user_agent).await?;

    // Sanitize the content (use the entry link as base URL for relative images)
    let sanitized_content = sanitize_html(
        &extracted.content,
        &state.config.image_proxy_secret,
        Some(&link),
        custom_referrer.as_deref(),
        None, // Web UI doesn't need absolute URLs
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

    // Check cache first for in-flight jobs
    if let Some(cached) = state.summary_cache.get(user_id, id) {
        return Ok(Json(SummaryResponse {
            status: cached.status,
            summary_text: cached.summary_text,
            error: cached.error_message,
            created_at: Some(cached.created_at),
        }));
    }

    // Get entry and verify ownership.
    // Check DB for existing summary
    if let Some(db_summary) = entry_summary::find_by_user_and_entry(&state.db, user_id, id).await? {
        return Ok(Json(SummaryResponse {
            status: db_summary.status,
            summary_text: db_summary.summary_text,
            error: db_summary.error_message,
            created_at: Some(db_summary.created_at),
        }));
    }

    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    // Verify entry belongs to user
    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Check if entry has a link
    let link = entry_with_feed
        .entry
        .link
        .clone()
        .ok_or_else(|| AppError::Validation("Entry has no link to summarize".to_string()))?;

    // Verify Kagi is configured
    let config = user_settings::get_save_services_config(&state.db, user_id).await?;
    let kagi = config
        .kagi
        .ok_or_else(|| AppError::Validation("Kagi is not configured".to_string()))?;

    if !kagi.is_configured() {
        return Err(AppError::Validation("Kagi is not configured".to_string()));
    }

    // Create pending record in DB
    entry_summary::upsert_pending(&state.db, user_id, id).await?;

    // Set pending status in cache
    state.summary_cache.set_pending(user_id, id);

    let job = SummaryJob {
        user_id,
        entry_id: id,
        entry_link: link,
    };

    tokio::time::timeout(JOB_QUEUE_TIMEOUT, state.summary_tx.send(job))
        .await
        .map_err(|_e| {
            AppError::Internal("Summary queue is full, please try again later".to_string())
        })?
        .map_err(|e| AppError::Internal(format!("Failed to queue summary job: {e}")))?;

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

    // Check cache first for in-flight status
    if let Some(cached) = state.summary_cache.get(user_id, id) {
        return Ok(Json(SummaryResponse {
            status: cached.status,
            summary_text: cached.summary_text,
            error: cached.error_message,
            created_at: Some(cached.created_at),
        }));
    }

    // Verify entry ownership and get from DB
    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Get from DB
    let result = if let Some(db_summary) =
        entry_summary::find_by_user_and_entry(&state.db, user_id, id).await?
    {
        Some(SummaryResponse {
            status: db_summary.status,
            summary_text: db_summary.summary_text,
            error: db_summary.error_message,
            created_at: Some(db_summary.created_at),
        })
    } else {
        None
    };

    match result {
        Some(response) => Ok(Json(response)),
        None => Err(AppError::NotFound("No summary found".to_string())),
    }
}

/// DELETE /api/entries/{id}/summary - Delete summary from cache and DB
pub async fn delete_entry_summary(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    let user_id = auth_user.user.id;

    // Verify entry ownership and delete from DB
    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Delete from DB
    entry_summary::delete(&state.db, user_id, id).await?;

    // Remove from cache
    state.summary_cache.remove(user_id, id);
    // A summary was removed; the completed-summary count may have dropped — refresh the sidebar badge.
    state.sidebar_cache.bust(user_id);
    state.events.emit_summary(user_id, id, None);
    state.events.emit_sidebar(user_id);

    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn save_to_services(
    auth_user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<SaveToServicesResponse>> {
    // Get entry and verify ownership
    let user_id = auth_user.user.id;
    let entry_with_feed = entry::find_by_id_with_feed(&state.db, id)
        .await?
        .ok_or(AppError::EntryNotFound)?;

    // Verify entry belongs to user
    let cat = category::find_by_id(&state.db, entry_with_feed.category_id)
        .await?
        .ok_or(AppError::CategoryNotFound)?;
    if cat.user_id != user_id {
        return Err(AppError::EntryNotFound);
    }

    // Check if entry has a link
    let link = entry_with_feed
        .entry
        .link
        .clone()
        .ok_or_else(|| AppError::Validation("Entry has no link to save".to_string()))?;

    // Get save services config
    let save_config = user_settings::get_save_services_config(&state.db, user_id).await?;

    if !save_config.has_any_service() {
        return Err(AppError::Validation(
            "No save services configured".to_string(),
        ));
    }

    let entry_data = BookmarkData {
        url: link,
        title: entry_with_feed.entry.title.clone(),
        description: entry_with_feed.entry.summary.clone(),
        tags: vec![],
    };

    // Save to all configured services in parallel
    let mut results = Vec::new();

    // Linkding
    if let Some(linkding_config) = &save_config.linkding
        && linkding_config.is_configured()
    {
        let result = linkding::save_to_linkding(linkding_config, &entry_data).await?;
        results.push(result);
    }

    // Future services can be added here:
    // if let Some(pocket_config) = &save_config.pocket { ... }

    let all_success = results.iter().all(|r| r.success);

    Ok(Json(SaveToServicesResponse {
        results,
        all_success,
    }))
}
