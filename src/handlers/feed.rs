use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::{category, feed};
use crate::services::{feed_discovery, opml};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateFeedRequest {
    pub url: String,
    pub category_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFeedRequest {
    pub category_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FetchMetadataRequest {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct FeedResponse {
    pub id: i64,
    pub category_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct FeedMetadataResponse {
    pub feed_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
}

impl From<feed::Feed> for FeedResponse {
    fn from(f: feed::Feed) -> Self {
        FeedResponse {
            id: f.id,
            category_id: f.category_id,
            url: f.url,
            title: f.title,
            description: f.description,
            site_url: f.site_url,
            created_at: f.created_at.to_rfc3339(),
            updated_at: f.updated_at.to_rfc3339(),
        }
    }
}

pub async fn list_feeds(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<Vec<FeedResponse>>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let feeds = feed::list_by_user(&conn, auth_user.user.id)?;
    let response: Vec<FeedResponse> = feeds.into_iter().map(Into::into).collect();

    Ok(Json(response))
}

pub async fn create_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<CreateFeedRequest>,
) -> AppResult<(StatusCode, Json<FeedResponse>)> {
    let url = req.url.trim();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    // Verify category ownership
    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

        category::find_by_id_and_user(&conn, req.category_id, auth_user.user.id)?
            .ok_or(AppError::CategoryNotFound)?;
    }

    // Discover feed metadata
    let discovered = feed_discovery::discover_feed(url).await?;

    // Create the feed
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let new_feed = feed::create_feed(
        &conn,
        req.category_id,
        &discovered.feed_url,
        discovered.title.as_deref(),
        discovered.description.as_deref(),
        discovered.site_url.as_deref(),
    )?;

    Ok((StatusCode::CREATED, Json(new_feed.into())))
}

pub async fn get_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<FeedResponse>> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    // Find the feed first
    let f = feed::find_by_id(&conn, id)?.ok_or(AppError::FeedNotFound)?;

    // Verify ownership through category
    category::find_by_id_and_user(&conn, f.category_id, auth_user.user.id)?
        .ok_or(AppError::FeedNotFound)?;

    Ok(Json(f.into()))
}

pub async fn update_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateFeedRequest>,
) -> AppResult<Json<FeedResponse>> {
    let url = req.url.trim();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    // Find the feed first and verify current ownership
    let f = feed::find_by_id(&conn, id)?.ok_or(AppError::FeedNotFound)?;

    category::find_by_id_and_user(&conn, f.category_id, auth_user.user.id)?
        .ok_or(AppError::FeedNotFound)?;

    // Verify new category ownership
    category::find_by_id_and_user(&conn, req.category_id, auth_user.user.id)?
        .ok_or(AppError::CategoryNotFound)?;

    let updated = feed::update_feed(
        &conn,
        id,
        f.category_id,
        req.category_id,
        url,
        req.title.as_deref(),
        req.description.as_deref(),
        req.site_url.as_deref(),
    )?;

    Ok(Json(updated.into()))
}

pub async fn delete_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    // Find the feed first and verify ownership
    let f = feed::find_by_id(&conn, id)?.ok_or(AppError::FeedNotFound)?;

    category::find_by_id_and_user(&conn, f.category_id, auth_user.user.id)?
        .ok_or(AppError::FeedNotFound)?;

    feed::delete_feed(&conn, id, f.category_id)?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn fetch_metadata(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<FetchMetadataRequest>,
) -> AppResult<Json<FeedMetadataResponse>> {
    let url = req.url.trim();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    // Just verify the user is authenticated (already done by AuthUser extractor)
    // We don't need to check DB here as this is just a metadata fetch
    let _ = (state, auth_user);

    let discovered = feed_discovery::discover_feed(url).await?;

    Ok(Json(FeedMetadataResponse {
        feed_url: discovered.feed_url,
        title: discovered.title,
        description: discovered.description,
        site_url: discovered.site_url,
    }))
}

pub async fn export_opml(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<impl IntoResponse> {
    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    let categories = category::list_by_user(&conn, auth_user.user.id)?;
    let feeds = feed::list_by_user(&conn, auth_user.user.id)?;

    let opml_content = opml::export_opml(&categories, &feeds);

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"subscriptions.opml\"",
            ),
        ],
        opml_content,
    ))
}

#[derive(Debug, Deserialize)]
pub struct ImportOpmlRequest {
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub categories_created: i32,
    pub feeds_created: i32,
    pub feeds_skipped: i32,
}

pub async fn import_opml(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<ImportOpmlRequest>,
) -> AppResult<Json<ImportResult>> {
    let outlines = opml::parse_opml(&req.content)?;

    let mut categories_created = 0;
    let mut feeds_created = 0;
    let mut feeds_skipped = 0;

    let conn = state
        .db
        .lock()
        .map_err(|_| AppError::Internal("Database lock error".to_string()))?;

    for outline in outlines {
        // Find or create category
        let cat = match category::find_by_name_and_user(
            &conn,
            &outline.category_name,
            auth_user.user.id,
        )? {
            Some(cat) => cat,
            None => {
                let new_cat =
                    category::create_category(&conn, auth_user.user.id, &outline.category_name)?;
                categories_created += 1;
                new_cat
            }
        };

        // Create feeds
        for opml_feed in outline.feeds {
            // Check if feed already exists in this category
            if feed::find_by_url_and_category(&conn, &opml_feed.xml_url, cat.id)?.is_some() {
                feeds_skipped += 1;
                continue;
            }

            // Create the feed
            feed::create_feed(
                &conn,
                cat.id,
                &opml_feed.xml_url,
                opml_feed.title.as_deref(),
                None,
                opml_feed.html_url.as_deref(),
            )?;
            feeds_created += 1;
        }
    }

    Ok(Json(ImportResult {
        categories_created,
        feeds_created,
        feeds_skipped,
    }))
}
