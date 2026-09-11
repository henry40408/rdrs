//! Verifies that responses are brotli-compressed when the client advertises
//! `Accept-Encoding: br`, and left untouched otherwise.

mod common;
use common::default_test_config;

use axum::http::{HeaderValue, header};
use axum_test::TestServer;
use rdrs::{Config, create_router};

async fn create_test_server(config: Config) -> TestServer {
    TestServer::builder().build(create_router(common::test_state(config).await))
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
