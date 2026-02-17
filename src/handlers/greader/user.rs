use axum::{extract::State, Json};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::models::{category, entry, feed};
use crate::AppState;

use super::auth::GReaderUser;
use super::types::{UnreadCount, UnreadCountResponse, UserInfoResponse};

/// `GET /reader/api/0/user-info`
pub async fn user_info(auth: GReaderUser) -> AppResult<Json<UserInfoResponse>> {
    let user = &auth.user;
    Ok(Json(UserInfoResponse {
        user_id: user.id.to_string(),
        user_name: user.username.clone(),
        user_profile_id: user.id.to_string(),
        user_email: format!("{}@localhost", user.username),
    }))
}

/// `GET /reader/api/0/unread-count`
pub async fn unread_count(
    auth: GReaderUser,
    State(state): State<AppState>,
) -> AppResult<Json<UnreadCountResponse>> {
    let user_id = auth.user.id;

    let response = state
        .db
        .user(move |conn| {
            let feed_unreads = entry::count_unread_by_feed(conn, user_id)?;
            let category_unreads = entry::count_unread_by_category(conn, user_id)?;
            let feeds = feed::list_by_user(conn, user_id)?;
            let categories = category::list_by_user(conn, user_id)?;

            let mut unreadcounts: Vec<UnreadCount> = Vec::new();

            // Per-feed unread counts
            for f in &feeds {
                let count = feed_unreads.get(&f.id).copied().unwrap_or(0);
                unreadcounts.push(UnreadCount {
                    id: format!("feed/{}", f.url),
                    count,
                    newest_item_timestamp_usec: "0".to_string(),
                });
            }

            // Per-category unread counts
            for cat in &categories {
                let count = category_unreads.get(&cat.id).copied().unwrap_or(0);
                unreadcounts.push(UnreadCount {
                    id: format!("user/-/label/{}", cat.name),
                    count,
                    newest_item_timestamp_usec: "0".to_string(),
                });
            }

            // Total unread count
            let total: i64 = feed_unreads.values().sum();
            unreadcounts.push(UnreadCount {
                id: "user/-/state/com.google/reading-list".to_string(),
                count: total,
                newest_item_timestamp_usec: "0".to_string(),
            });

            Ok::<_, AppError>(UnreadCountResponse {
                max: 1000,
                unreadcounts,
            })
        })
        .await??;

    Ok(Json(response))
}

/// `GET /reader/api/0/preference/list`
pub async fn preference_list(_auth: GReaderUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "prefs": [] })))
}

/// `GET /reader/api/0/preference/stream/list`
pub async fn preference_stream_list(_auth: GReaderUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "streamprefs": {} })))
}

/// `GET /reader/api/0/friend/list`
pub async fn friend_list(_auth: GReaderUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "friends": [] })))
}
