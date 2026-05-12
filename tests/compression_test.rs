//! Verifies that responses are brotli-compressed when the client advertises
//! `Accept-Encoding: br`, and left untouched otherwise.

use std::sync::Arc;

use axum::http::{header, HeaderValue};
use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;

fn open_shared_memory(name: &str) -> Connection {
    let uri = format!("file:{}?mode=memory&cache=shared", name);
    Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

fn default_test_config() -> Config {
    Config {
        database_url: ":memory:".to_string(),
        server_port: 3000,
        signup_enabled: true,
        multi_user_enabled: true,
        image_proxy_secret: vec![0u8; 32],
        image_proxy_secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:3000".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
    }
}

fn create_test_server(config: Config) -> TestServer {
    let write_conn = open_shared_memory("test_compression");
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory("test_compression");

    let (db, _handle) = DbPool::new(write_conn, read_conn);
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    TestServer::builder().build(create_router(state))
}

#[tokio::test]
async fn test_login_page_gzip_when_accepted() {
    let server = create_test_server(default_test_config());

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
    let server = create_test_server(default_test_config());

    let response = server.get("/login").await;

    response.assert_status_ok();
    assert!(
        response.headers().get(header::CONTENT_ENCODING).is_none(),
        "Responses must not be compressed when client does not advertise support"
    );
}

#[tokio::test]
async fn test_login_page_brotli_when_accepted() {
    let server = create_test_server(default_test_config());

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
