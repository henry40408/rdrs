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
    let img = state
        .db
        .read_user(move |conn| {
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
