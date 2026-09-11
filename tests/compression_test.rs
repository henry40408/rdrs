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
async fn test_login_page_compressed_with_the_accepted_encoding() {
    let server = create_test_server(default_test_config()).await;

    for encoding in ["gzip", "br"] {
        let response = server
            .get("/login")
            .add_header(header::ACCEPT_ENCODING, HeaderValue::from_static(encoding))
            .await;

        response.assert_status_ok();
        let applied = response
            .headers()
            .get(header::CONTENT_ENCODING)
            .unwrap_or_else(|| {
                panic!(
                    "CompressionLayer should set Content-Encoding for Accept-Encoding: {encoding}"
                )
            });
        assert_eq!(applied, encoding);
    }
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
