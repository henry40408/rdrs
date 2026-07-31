use axum::{
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::middleware::AuthUser;
use crate::models::{category, feed, image};

pub async fn get_feed_icon(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let user_id = auth_user.user.id;
    let f = feed::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::FeedNotFound)?;

    category::find_by_id_and_user(&state.db, f.category_id, user_id)
        .await?
        .ok_or(AppError::FeedNotFound)?;

    let Some(img) = image::find(&state.db, image::ENTITY_FEED, id).await? else {
        return Err(AppError::NotFound("Icon not found".into()));
    };

    // `private`, not `public`: this endpoint is behind `AuthUser` and scoped to
    // the caller's categories, and because the handler sets `Cache-Control`
    // itself, `no_store_for_authenticated` steps aside and adds no
    // `Vary: Cookie`. A `public` response would therefore be storable by a
    // shared cache keyed on the URL alone, which could then hand feed 42's icon
    // to someone who is not subscribed to it. `private` keeps the day-long
    // browser cache — the point of the header — while barring shared storage.
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, img.content_type),
            (header::CACHE_CONTROL, "private, max-age=86400".to_string()),
        ],
        img.data,
    )
        .into_response())
}
