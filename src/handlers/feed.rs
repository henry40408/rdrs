use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::{category, feed, image};
use crate::services::{feed_discovery, opml};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateFeedRequest {
    pub url: String,
    pub category_id: i64,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateFeedRequest {
    pub category_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub custom_user_agent: Option<String>,
    #[serde(default)]
    pub http2_disabled: bool,
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
    pub fetched_at: Option<String>,
    pub fetch_error: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub has_icon: bool,
}

#[derive(Debug, Serialize)]
pub struct FeedMetadataResponse {
    pub feed_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
}

impl FeedResponse {
    fn from_feed(f: feed::Feed, has_icon: bool) -> Self {
        FeedResponse {
            id: f.id,
            category_id: f.category_id,
            url: f.url,
            title: f.title,
            description: f.description,
            site_url: f.site_url,
            fetched_at: f.fetched_at.map(|dt| dt.to_rfc3339()),
            fetch_error: f.fetch_error,
            custom_user_agent: f.custom_user_agent,
            http2_disabled: f.http2_disabled,
            created_at: f.created_at.to_rfc3339(),
            updated_at: f.updated_at.to_rfc3339(),
            has_icon,
        }
    }
}

pub async fn list_feeds(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> AppResult<Json<Vec<FeedResponse>>> {
    let user_id = auth_user.user.id;
    let response = state
        .db
        .user(move |conn| {
            let feeds = feed::list_by_user(conn, user_id)?;
            let response: Vec<FeedResponse> = feeds
                .into_iter()
                .map(|f| {
                    let has_icon = image::exists(conn, image::ENTITY_FEED, f.id).unwrap_or(false);
                    FeedResponse::from_feed(f, has_icon)
                })
                .collect();
            Ok::<_, AppError>(response)
        })
        .await??;

    Ok(Json(response))
}

pub async fn create_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<CreateFeedRequest>,
) -> AppResult<(StatusCode, Json<FeedResponse>)> {
    let url = req.url.trim().to_string();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    // Verify category ownership
    let user_id = auth_user.user.id;
    let category_id = req.category_id;
    state
        .db
        .user(move |conn| {
            category::find_by_id_and_user(conn, category_id, user_id)?
                .ok_or(AppError::CategoryNotFound)
        })
        .await??;

    // Discover feed metadata
    let discovered = feed_discovery::discover_feed(&url, &state.config.user_agent).await?;

    // Create the feed
    let custom_user_agent = req.custom_user_agent;
    let http2_disabled = req.http2_disabled;
    let new_feed = state
        .db
        .user(move |conn| {
            feed::create_feed(
                conn,
                category_id,
                &discovered.feed_url,
                discovered.title.as_deref(),
                discovered.description.as_deref(),
                discovered.site_url.as_deref(),
                custom_user_agent.as_deref(),
                http2_disabled,
            )
        })
        .await??;

    Ok((
        StatusCode::CREATED,
        Json(FeedResponse::from_feed(new_feed, false)),
    ))
}

pub async fn get_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Json<FeedResponse>> {
    let user_id = auth_user.user.id;
    let (f, has_icon) = state
        .db
        .user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;

            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;

            let has_icon = image::exists(conn, image::ENTITY_FEED, f.id)?;
            Ok::<_, AppError>((f, has_icon))
        })
        .await??;

    Ok(Json(FeedResponse::from_feed(f, has_icon)))
}

pub async fn update_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateFeedRequest>,
) -> AppResult<Json<FeedResponse>> {
    let url = req.url.trim().to_string();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    let user_id = auth_user.user.id;
    let (updated, has_icon) = state
        .db
        .user(move |conn| {
            // Find the feed first and verify current ownership
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;

            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;

            // Verify new category ownership
            category::find_by_id_and_user(conn, req.category_id, user_id)?
                .ok_or(AppError::CategoryNotFound)?;

            let updated = feed::update_feed(
                conn,
                id,
                f.category_id,
                req.category_id,
                &url,
                req.title.as_deref(),
                req.description.as_deref(),
                req.site_url.as_deref(),
                req.custom_user_agent.as_deref(),
                req.http2_disabled,
            )?;

            let has_icon = image::exists(conn, image::ENTITY_FEED, updated.id)?;
            Ok::<_, AppError>((updated, has_icon))
        })
        .await??;

    Ok(Json(FeedResponse::from_feed(updated, has_icon)))
}

pub async fn delete_feed(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let user_id = auth_user.user.id;
    state
        .db
        .user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;

            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;

            feed::delete_feed(conn, id, f.category_id)?;
            Ok::<_, AppError>(())
        })
        .await??;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn fetch_metadata(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<FetchMetadataRequest>,
) -> AppResult<Json<FeedMetadataResponse>> {
    let url = req.url.trim().to_string();

    if url.is_empty() {
        return Err(AppError::Validation("URL cannot be empty".to_string()));
    }

    // Just verify the user is authenticated (already done by AuthUser extractor)
    let _ = auth_user;

    let discovered = feed_discovery::discover_feed(&url, &state.config.user_agent).await?;

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
    let user_id = auth_user.user.id;
    let opml_content = state
        .db
        .user(move |conn| {
            let categories = category::list_by_user(conn, user_id)?;
            let feeds = feed::list_by_user(conn, user_id)?;
            Ok::<_, AppError>(opml::export_opml(&categories, &feeds))
        })
        .await??;

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

    let user_id = auth_user.user.id;
    let result = state
        .db
        .user(move |conn| {
            let mut categories_created = 0;
            let mut feeds_created = 0;
            let mut feeds_skipped = 0;

            for outline in outlines {
                // Find or create category
                let cat =
                    match category::find_by_name_and_user(conn, &outline.category_name, user_id)? {
                        Some(cat) => cat,
                        None => {
                            let new_cat =
                                category::create_category(conn, user_id, &outline.category_name)?;
                            categories_created += 1;
                            new_cat
                        }
                    };

                // Create feeds
                for opml_feed in outline.feeds {
                    // Check if feed already exists in this category
                    if feed::find_by_url_and_category(conn, &opml_feed.xml_url, cat.id)?.is_some() {
                        feeds_skipped += 1;
                        continue;
                    }

                    // Create the feed
                    feed::create_feed(
                        conn,
                        cat.id,
                        &opml_feed.xml_url,
                        opml_feed.title.as_deref(),
                        None,
                        opml_feed.html_url.as_deref(),
                        None,
                        None,
                    )?;
                    feeds_created += 1;
                }
            }

            Ok::<_, AppError>(ImportResult {
                categories_created,
                feeds_created,
                feeds_skipped,
            })
        })
        .await??;

    Ok(Json(result))
}

pub async fn get_feed_icon(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;
    let img = state
        .db
        .user(move |conn| {
            let f = feed::find_by_id(conn, id)?.ok_or(AppError::FeedNotFound)?;

            category::find_by_id_and_user(conn, f.category_id, user_id)?
                .ok_or(AppError::FeedNotFound)?;

            match image::find(conn, image::ENTITY_FEED, id)? {
                Some(img) => Ok::<_, AppError>(img),
                None => Err(AppError::NotFound("Icon not found".into())),
            }
        })
        .await??;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, img.content_type),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        img.data,
    )
        .into_response())
}
