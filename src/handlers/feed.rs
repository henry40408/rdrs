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
use crate::services::feed_discovery;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct FetchMetadataRequest {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct FeedMetadataResponse {
    pub feed_url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
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
