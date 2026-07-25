//! Verifies that responses are brotli-compressed when the client advertises
//! `Accept-Encoding: br`, and left untouched otherwise.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{HeaderValue, header};
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
        summarizer_inflight: rdrs::handlers::summarizer::new_inflight_registry(),
        events: rdrs::services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
        login_rate_limiter: common::test_rate_limiter(),
    };

    TestServer::builder().build(create_router(state))
}

#[tokio::test]
async fn test_login_page_gzip_when_accepted() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .get("/login")
        .add_header(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"))
        .await;

    response.assert_status_ok();
    let encoding = response.headers().get(header::CONTENT_ENCODING).expect(
        "CompressionLayer should set Content-Encoding when client sends Accept-Encoding: gzip",
    );
    assert_eq!(encoding.to_str().unwrap(), "gzip");
}

#[tokio::test]
async fn test_login_page_not_compressed_without_accept_encoding() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/login").await;

    response.assert_status_ok();
    assert!(
        response.headers().get(header::CONTENT_ENCODING).is_none(),
        "Responses must not be compressed when client does not advertise support"
    );
}

#[tokio::test]
async fn test_login_page_brotli_when_accepted() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .get("/login")
        .add_header(header::ACCEPT_ENCODING, HeaderValue::from_static("br"))
        .await;

    response.assert_status_ok();
    let encoding = response.headers().get(header::CONTENT_ENCODING).expect(
        "CompressionLayer should set Content-Encoding when client sends Accept-Encoding: br",
    );
    assert_eq!(encoding.to_str().unwrap(), "br");
}
