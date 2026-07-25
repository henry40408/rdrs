//! The first-line CSRF guard, exercised through the real router so its wiring
//! into the layer stack is covered — the classification itself is unit-tested
//! in `middleware::csrf`.

mod common;
use common::default_test_config;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{AppState, Db, auth, create_router, services};
use std::sync::Arc;

async fn test_server() -> TestServer {
    let config = default_test_config();
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let state = AppState {
        db,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        summarizer_inflight: rdrs::handlers::summarizer::new_inflight_registry(),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
        login_rate_limiter: common::test_rate_limiter(),
    };
    TestServer::new(create_router(state))
}

#[tokio::test]
async fn cross_site_post_is_rejected_before_the_handler() {
    let server = test_server().await;
    // `Sec-Fetch-Site: cross-site` is a browser telling us this POST came from
    // another origin. It is rejected with 403 before `POST /api/register` runs,
    // so the body never matters.
    let res = server
        .post("/api/register")
        .add_header("sec-fetch-site", "cross-site")
        .json(&serde_json::json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_via_origin_header_is_rejected() {
    let server = test_server().await;
    let res = server
        .post("/api/register")
        .add_header("origin", "https://evil.example.com")
        .add_header("host", "app.example.com")
        .json(&serde_json::json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn same_origin_post_reaches_the_handler() {
    let server = test_server().await;
    // Same-origin: the guard passes it through, so `POST /api/register` runs and
    // creates the first user (201) rather than the guard's 403.
    let res = server
        .post("/api/register")
        .add_header("sec-fetch-site", "same-origin")
        .json(&serde_json::json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn safe_get_is_never_blocked_cross_site() {
    let server = test_server().await;
    // A cross-site GET must still work — the guard only gates state-changing
    // methods.
    let res = server
        .get("/login")
        .add_header("sec-fetch-site", "cross-site")
        .await;
    res.assert_status_ok();
}

#[tokio::test]
async fn non_browser_client_without_headers_reaches_the_handler() {
    let server = test_server().await;
    // No Sec-Fetch-Site, no Origin → a native client (bearer-authenticated, not
    // CSRF-exposed). The guard lets it through; registration succeeds.
    let res = server
        .post("/api/register")
        .json(&serde_json::json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn logged_out_page_request_emits_exactly_one_set_cookie_per_name() {
    // `anonymous_session` mints a fresh (session_token, csrf_token) pair for a
    // logged-out visitor, and `slide_session_cookie` (layered outside it, so it
    // sees the same response) must recognize both are already present and not
    // append a second Set-Cookie for either name.
    let server = test_server().await;
    let res = server.get("/login").await;
    res.assert_status_ok();

    let set_cookies: Vec<String> = res
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(std::string::ToString::to_string))
        .collect();

    for name in ["session_token", "csrf_token"] {
        let prefix = format!("{name}=");
        let matches: Vec<_> = set_cookies
            .iter()
            .filter(|s| s.starts_with(&prefix))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one Set-Cookie for {name}, got {matches:?} (all: {set_cookies:?})"
        );
    }
}
