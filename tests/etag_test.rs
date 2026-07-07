//! Verifies the `ETagLayer`:
//! - 2xx HTML responses get a weak `ETag` header.
//! - Repeating the request with If-None-Match returns 304.
//! - Non-HTML responses are not touched.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{HeaderValue, StatusCode, header};
use axum_test::TestServer;
use rdrs::{AppState, Config, Db, auth, create_router, services};

async fn create_test_server(config: Config) -> TestServer {
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        events: rdrs::services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };

    TestServer::builder().build(create_router(state))
}

#[tokio::test]
async fn html_response_carries_weak_etag() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/login").await;

    response.assert_status_ok();
    let etag = response
        .headers()
        .get(header::ETAG)
        .expect("HTML response should carry ETag");
    let value = etag.to_str().unwrap();
    assert!(value.starts_with("W/\""), "expected weak ETag, got {value}");
    assert!(value.ends_with('"'), "expected closing quote, got {value}");
}

#[tokio::test]
async fn matching_if_none_match_returns_304() {
    let server = create_test_server(default_test_config()).await;

    let first = server.get("/login").await;
    first.assert_status_ok();
    let etag = first
        .headers()
        .get(header::ETAG)
        .expect("first response should carry ETag")
        .clone();

    let second = server
        .get("/login")
        .add_header(header::IF_NONE_MATCH, etag.clone())
        .await;

    second.assert_status(StatusCode::NOT_MODIFIED);
    // 304 must echo the ETag header.
    assert_eq!(second.headers().get(header::ETAG), Some(&etag));
    // 304 has no body.
    assert!(second.as_bytes().is_empty());
}

#[tokio::test]
async fn non_html_response_has_no_etag() {
    // /favicon.svg returns image/svg+xml — should not be tagged.
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/favicon.svg").await;

    response.assert_status_ok();
    assert!(
        response.headers().get(header::ETAG).is_none(),
        "non-HTML responses must not be tagged"
    );
}

#[tokio::test]
async fn non_matching_if_none_match_returns_full_body() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .get("/login")
        .add_header(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("W/\"deadbeef\""),
        )
        .await;

    response.assert_status_ok();
    assert!(
        !response.as_bytes().is_empty(),
        "non-matching If-None-Match should return full body"
    );
}
