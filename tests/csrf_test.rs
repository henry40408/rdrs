//! The first-line CSRF guard, exercised through the real router so its wiring
//! into the layer stack is covered — the classification itself is unit-tested
//! in `middleware::csrf`.

mod common;
use common::default_test_config;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{AppState, Config, Db, auth, create_router, services};
use std::sync::Arc;

async fn test_server() -> TestServer {
    test_server_with_config(default_test_config()).await
}

async fn test_server_with_config(config: Config) -> TestServer {
    TestServer::new(build_router(config).await)
}

/// Like [`test_server_with_config`], but with cookies saved and replayed
/// across requests — needed by any test that spans more than one round trip
/// (e.g. checking that a cookie is not re-minted on a second request).
async fn test_server_with_config_saving_cookies(config: Config) -> TestServer {
    TestServer::builder()
        .save_cookies()
        .build(build_router(config).await)
}

async fn build_router(config: Config) -> axum::Router {
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
    create_router(state)
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

/// With `cookie_secure = true`, the full `anonymous_session` → `csrf_guard`
/// round trip must still work end to end (using the `__Host-` prefixed
/// names), and the CSRF cookie must not be *re-minted* — given a new,
/// different token — on a second request just because `anonymous_session`'s
/// "already present?" check was only looking at the unprefixed name.
///
/// Note this does *not* assert the second response carries no `Set-Cookie` at
/// all: `slide_session_cookie` deliberately reissues the session/CSRF cookies
/// (refreshed `Max-Age`, identical value) on every request that carries a
/// verified token, anonymous sessions included — see its doc comment. What
/// must not happen is `anonymous_session` *also* deciding the cookie is
/// missing (because it only checked the wrong name) and minting a second,
/// independent one — that would still resolve to the same derived value here,
/// but is exactly the bug this test would catch if `derive_csrf` were ever
/// non-deterministic or the two layers disagreed on which name to check.
#[tokio::test]
async fn secure_anonymous_session_round_trips_and_does_not_remint_csrf_cookie() {
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let server = test_server_with_config_saving_cookies(config).await;

    let first = server.get("/login").await;
    first.assert_status_ok();
    let session = first
        .maybe_cookie("__Host-session_token")
        .expect("anonymous_session must mint the __Host- prefixed session cookie");
    let csrf = first
        .maybe_cookie("__Host-csrf_token")
        .expect("anonymous_session must mint the __Host- prefixed CSRF cookie");
    assert!(
        first.maybe_cookie("session_token").is_none(),
        "the unprefixed session cookie must not also be minted when cookie_secure is true"
    );
    assert!(
        first.maybe_cookie("csrf_token").is_none(),
        "the unprefixed CSRF cookie must not also be minted when cookie_secure is true"
    );

    // The synchronizer-token guard: a same-origin POST carrying the __Host-
    // session cookie (via the saved jar) and the matching CSRF header must
    // reach the handler.
    let register = server
        .post("/api/register")
        .add_header("sec-fetch-site", "same-origin")
        .add_header("x-csrf-token", csrf.value())
        .json(&serde_json::json!({ "username": "u", "password": "password123" }))
        .await;
    register.assert_status(StatusCode::CREATED);

    // Second request: the session and CSRF cookies come back with exactly the
    // same values (whether reissued by `slide_session_cookie`'s Max-Age
    // refresh, or left alone) — never a freshly-minted, different token.
    let second = server.get("/login").await;
    second.assert_status_ok();
    if let Some(reissued) = second.maybe_cookie("__Host-session_token") {
        assert_eq!(
            reissued.value(),
            session.value(),
            "the session token must never change across requests for the same session"
        );
    }
    if let Some(reissued) = second.maybe_cookie("__Host-csrf_token") {
        assert_eq!(
            reissued.value(),
            csrf.value(),
            "the CSRF token must never change across requests for the same session"
        );
    }
}
