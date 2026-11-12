pub mod admin;
pub mod auth;
pub mod categories;
pub mod client_info;
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
pub mod return_to;
pub mod static_assets;
pub mod summarizer;
pub mod user;
mod views;

/// A top-level navigation rather than a swap-helper `fetch()`. Fragments render
/// blank as documents, so such a request must get a page or a redirect instead.
pub(crate) fn is_document_navigation(headers: &axum::http::HeaderMap) -> bool {
    headers.get("sec-fetch-dest").and_then(|v| v.to_str().ok()) == Some("document")
}
