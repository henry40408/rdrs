pub mod auth;
pub mod item;
pub mod subscription;
pub mod tag;
pub mod types;
pub mod user;

use axum::{
    Router,
    routing::{get, post},
};

use crate::AppState;

/// Response header carrying how many rows a bulk write actually changed.
///
/// The `GReader` protocol answers these endpoints with a bare `OK` body that
/// third-party clients parse literally, so the count rides beside it in a
/// vendor-prefixed header instead. rdrs' own JS reads it to report "Marked N
/// entries as read" from the server's number rather than guessing from the DOM;
/// clients that ignore the header see the unchanged `OK` they expect.
pub const AFFECTED_HEADER: &str = "x-rdrs-affected";

/// Standard `GReader` `OK` body plus the affected-row count.
pub fn ok_with_affected(count: i64) -> ([(&'static str, String); 1], String) {
    ([(AFFECTED_HEADER, count.to_string())], "OK".to_string())
}

/// Build all Google Reader API routes.
pub fn greader_routes() -> Router<AppState> {
    Router::new()
        // ClientLogin (outside /reader prefix)
        .route("/accounts/ClientLogin", post(auth::client_login))
        // POST token
        .route("/reader/api/0/token", get(auth::get_post_token))
        // Subscription management
        .route(
            "/reader/api/0/subscription/list",
            get(subscription::subscription_list),
        )
        .route(
            "/reader/api/0/subscription/edit",
            post(subscription::subscription_edit),
        )
        .route(
            "/reader/api/0/subscription/quickadd",
            post(subscription::quickadd),
        )
        .route(
            "/reader/api/0/subscription/export",
            get(subscription::export),
        )
        .route(
            "/reader/api/0/subscription/import",
            post(subscription::import),
        )
        .route("/reader/api/0/subscribed", get(subscription::subscribed))
        // Stream / item endpoints
        .route(
            "/reader/api/0/stream/contents/{*stream}",
            get(item::stream_contents),
        )
        .route("/reader/api/0/stream/items/ids", get(item::stream_item_ids))
        .route(
            "/reader/api/0/stream/items/count",
            get(item::stream_item_count),
        )
        .route(
            "/reader/api/0/stream/items/contents",
            get(item::stream_items_contents).post(item::stream_items_contents_post),
        )
        // Tag operations
        .route("/reader/api/0/tag/list", get(tag::tag_list))
        .route("/reader/api/0/edit-tag", post(tag::edit_tag))
        .route(
            "/reader/api/0/mark-all-as-read",
            post(tag::mark_all_as_read),
        )
        .route("/reader/api/0/disable-tag", post(tag::disable_tag))
        .route("/reader/api/0/rename-tag", post(tag::rename_tag))
        // User info & misc
        .route("/reader/api/0/user-info", get(user::user_info))
        .route("/reader/api/0/unread-count", get(user::unread_count))
        .route("/reader/api/0/preference/list", get(user::preference_list))
        .route(
            "/reader/api/0/preference/stream/list",
            get(user::preference_stream_list),
        )
        .route("/reader/api/0/friend/list", get(user::friend_list))
}
