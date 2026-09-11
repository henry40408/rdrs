pub mod admin;
pub mod auth;
pub mod categories;
pub mod entries;
pub mod entry;
pub mod events;
pub mod favicon;
pub mod feed;
pub mod feeds;
pub mod greader;
pub mod health;
pub mod invite;
pub mod offline;
pub mod pages;
pub mod passkey;
pub mod pixel;
pub mod proxy;
pub mod static_assets;
pub mod summarizer;
pub mod user;

/// Implement `IntoResponse` for Askama templates the way every page and
/// fragment answers: the rendered HTML, or a 500 carrying the render error.
macro_rules! impl_html_response {
    ($($ty:ty),+ $(,)?) => {$(
        impl ::axum::response::IntoResponse for $ty {
            fn into_response(self) -> ::axum::response::Response {
                match ::askama::Template::render(&self) {
                    Ok(html) => ::axum::response::IntoResponse::into_response(
                        ::axum::response::Html(html),
                    ),
                    Err(e) => ::axum::response::IntoResponse::into_response((
                        ::axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        e.to_string(),
                    )),
                }
            }
        }
    )+};
}
pub(crate) use impl_html_response;
