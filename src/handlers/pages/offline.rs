use axum::{
    http::header,
    response::{IntoResponse, Response},
};

/// Service-worker offline page. Must stay user-agnostic: the Cache API ignores
/// `Cache-Control`, so anything personal would leak to the next user.
pub struct OfflineTemplate {
    pub git_version: &'static str,
}

impl IntoResponse for OfflineTemplate {
    fn into_response(self) -> Response {
        crate::handlers::views::status::offline(&self).into_response()
    }
}

/// Short, not `immutable`: the URL has no build stamp.
const OFFLINE_CACHE_CONTROL: &str = "public, max-age=3600";

/// `GET /offline`. No auth. The explicit public `Cache-Control` both prevents
/// `no-store` and tells `slide_session_cookie` not to attach a session cookie.
pub async fn offline_page() -> Response {
    (
        [(header::CACHE_CONTROL, OFFLINE_CACHE_CONTROL)],
        OfflineTemplate {
            git_version: crate::GIT_VERSION,
        },
    )
        .into_response()
}
