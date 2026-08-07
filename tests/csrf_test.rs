//! The first-line CSRF guard, exercised through the real router so its wiring
//! into the layer stack is covered — the classification itself is unit-tested
//! in `middleware::csrf`.

mod common;
use common::default_test_config;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{AppState, Config, Db, auth, create_router, services};
use std::sync::{Arc, Mutex};

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
    // another origin. It is rejected with 403 before `POST /api/setup` runs,
    // so the body never matters.
    let res = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "cross-site")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_origin_via_origin_header_is_rejected() {
    let server = test_server().await;
    let res = server
        .post("/api/setup")
        .add_header("origin", "https://evil.example.com")
        .add_header("host", "app.example.com")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn same_origin_post_reaches_the_handler() {
    let server = test_server().await;
    // Same-origin: the guard passes it through, so `POST /api/setup` runs and
    // creates the first user (201) rather than the guard's 403.
    let res = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status(StatusCode::CREATED);
}

/// Every `Set-Cookie` on a response, as raw header strings — the only way to
/// see a *removal* cookie, which a parsed jar renders indistinguishable from a
/// live one with an empty value.
fn set_cookie_headers(res: &axum_test::TestResponse) -> Vec<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(std::string::ToString::to_string))
        .collect()
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

    let set_cookies = set_cookie_headers(&res);

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
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .add_header("x-csrf-token", csrf.value())
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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

/// A CSRF cookie that is *present* but no longer derives from the session must
/// be re-minted, so the browser heals itself on the next page load.
///
/// This is the shape of the production failure: `anonymous_session` used to
/// check only whether a CSRF cookie existed, never whether it still matched, so
/// a browser holding a cookie from an earlier generation got a 403 on every
/// unsafe request until that cookie expired — up to `SESSION_EXPIRY_DAYS` — with
/// no way out from inside the app, since logout is itself behind `csrf_guard`.
#[tokio::test]
async fn a_stale_csrf_cookie_is_reminted_and_unblocks_the_next_post() {
    let server = test_server().await;

    let first = server.get("/login").await;
    let session = first
        .maybe_cookie("session_token")
        .expect("anonymous_session must mint a session cookie");
    let expected = first
        .maybe_cookie("csrf_token")
        .expect("anonymous_session must mint a CSRF cookie");

    // Same session, but the browser presents a CSRF cookie that does not derive
    // from it.
    let second = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "session_token",
            session.value().to_owned(),
        ))
        .add_cookie(cookie::Cookie::new(
            "csrf_token",
            "from-an-earlier-generation",
        ))
        .await;
    second.assert_status_ok();
    let healed = second
        .maybe_cookie("csrf_token")
        .expect("a CSRF cookie that does not match the session must be replaced");
    assert_eq!(
        healed.value(),
        expected.value(),
        "the re-minted cookie must carry the token derived from this session"
    );

    // And the token handed back actually satisfies the synchronizer-token guard.
    let register = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new(
            "session_token",
            session.value().to_owned(),
        ))
        .add_header("x-csrf-token", healed.value())
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    register.assert_status(StatusCode::CREATED);
}

/// On a Secure deployment the live CSRF cookie is `__Host-csrf_token`; a
/// leftover unprefixed `csrf_token` (from before the upgrade that introduced
/// the prefixed names, or before an operator flipped `RDRS_COOKIE_SECURE`) is
/// never refreshed, so its value drifts away from the session. Two generations
/// coexisting is exactly what let the front end and the back end disagree about
/// which cookie counts — so the stale one is evicted rather than left to expire.
#[tokio::test]
async fn a_leftover_unprefixed_csrf_cookie_is_evicted_on_a_secure_deployment() {
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let server = test_server_with_config(config).await;

    let first = server.get("/login").await;
    let session = first
        .maybe_cookie("__Host-session_token")
        .expect("a Secure deployment mints the __Host- prefixed session cookie");
    let csrf = first
        .maybe_cookie("__Host-csrf_token")
        .expect("a Secure deployment mints the __Host- prefixed CSRF cookie");

    let second = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "__Host-session_token",
            session.value().to_owned(),
        ))
        .add_cookie(cookie::Cookie::new(
            "__Host-csrf_token",
            csrf.value().to_owned(),
        ))
        .add_cookie(cookie::Cookie::new("csrf_token", "left-over-generation"))
        .await;
    second.assert_status_ok();

    let set_cookies = set_cookie_headers(&second);
    assert!(
        set_cookies
            .iter()
            .any(|h| h.starts_with("csrf_token=") && h.contains("Max-Age=0")),
        "the stale unprefixed CSRF cookie must be expired, got {set_cookies:?}"
    );
    // The removal must not leave the page tokenless: `slide_session_cookie`
    // skips its own reissue once it sees a Set-Cookie under either CSRF name,
    // so the live cookie has to ride along with the removal.
    let live = second
        .maybe_cookie("__Host-csrf_token")
        .expect("the live CSRF cookie must be reissued alongside the removal");
    assert_eq!(
        live.value(),
        csrf.value(),
        "evicting the stale cookie must not change the live token"
    );
}

/// Establish an anonymous session and return its (session cookie value, CSRF
/// token) pair — what a browser holds after one page load.
async fn anonymous_pair(server: &TestServer) -> (String, String) {
    let res = server.get("/login").await;
    let session = res
        .maybe_cookie("session_token")
        .expect("anonymous_session must mint a session cookie");
    let csrf = res
        .maybe_cookie("csrf_token")
        .expect("anonymous_session must mint a CSRF cookie");
    (session.value().to_owned(), csrf.value().to_owned())
}

#[tokio::test]
async fn an_x_csrf_token_header_that_does_not_derive_from_the_session_is_rejected() {
    let server = test_server().await;
    let (session, csrf) = anonymous_pair(&server).await;

    let rejected = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new("session_token", session.clone()))
        .add_header("x-csrf-token", "not-this-session's-token")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    rejected.assert_status(StatusCode::FORBIDDEN);

    // The same request with the session's own token gets through.
    let accepted = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new("session_token", session))
        .add_header("x-csrf-token", csrf)
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    accepted.assert_status(StatusCode::CREATED);
}

/// The body path: a native form POST carries the token as a `_csrf` field
/// instead of a header, and the guard has to buffer the body to find it.
#[tokio::test]
async fn a_form_post_is_gated_on_its_csrf_field() {
    let server = test_server().await;
    let (session, csrf) = anonymous_pair(&server).await;

    // The route needs a login, but `csrf_guard` runs first — a missing `_csrf`
    // is a 403 before the handler's own auth check is ever reached.
    let missing = server
        .post("/feeds/1/entries/mark-read")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new("session_token", session.clone()))
        .form(&[("scope", "all")])
        .await;
    missing.assert_status(StatusCode::FORBIDDEN);

    // With the field present the guard passes it through and rebuilds the body,
    // so the request reaches the handler — which redirects an anonymous visitor
    // to the login page rather than 403ing.
    let passed = server
        .post("/feeds/1/entries/mark-read")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new("session_token", session))
        .form(&[("scope", "all"), ("_csrf", &csrf)])
        .await;
    assert_ne!(
        passed.status_code(),
        StatusCode::FORBIDDEN,
        "a form carrying its session's own token must clear the guard"
    );
}

/// A body too large to buffer is rejected rather than read: `CSRF_MAX_BODY_BYTES`
/// caps what a malicious client can make the guard hold in memory.
#[tokio::test]
async fn a_body_over_the_buffering_limit_is_rejected() {
    let server = test_server().await;
    let (session, _) = anonymous_pair(&server).await;

    let res = server
        .post("/feeds/1/entries/mark-read")
        .add_header("sec-fetch-site", "same-origin")
        .add_header("content-type", "application/x-www-form-urlencoded")
        .add_cookie(cookie::Cookie::new("session_token", session))
        .text("x".repeat((1 << 20) + 1))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

/// A `tracing` writer that keeps everything in memory so a test can assert on
/// what was logged.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The observability half of the fix. Both CSRF layers answer with a bodyless
/// 403, so with nothing in the log there is no way to tell which one fired —
/// the production incident had to be diagnosed by reading the source. The
/// rejection must name the check and identify the session by its salted
/// `secret::audit_id`, never by the credential itself.
///
/// This installs a *global* subscriber, which a process accepts only once. That
/// is fine under `cargo nextest` — which the project mandates, and which runs
/// every test in its own process — and it is the only test here that does so.
#[tokio::test]
async fn a_token_mismatch_is_logged_without_leaking_the_session_cookie() {
    let logs = CapturedLogs::default();
    let writer = logs.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            // Off, or the styling escapes land between a field name and its
            // `=` and every substring assertion below silently misses.
            .with_ansi(false)
            .finish(),
    )
    .expect("no other subscriber may already be installed in this process");

    let server = test_server().await;
    let (session, _) = anonymous_pair(&server).await;

    let res = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "same-origin")
        .add_cookie(cookie::Cookie::new("session_token", session.clone()))
        .add_header("x-csrf-token", "not-this-session's-token")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);

    let logged = logs.contents();
    assert!(
        logged.contains("csrf.mismatch"),
        "the rejection must name the check that fired, got: {logged}"
    );
    assert!(
        logged.contains("session="),
        "the rejection must carry a session identifier to correlate on, got: {logged}"
    );
    assert!(
        !logged.contains(&session),
        "the session cookie must never reach the log, got: {logged}"
    );

    // The first-line guard logs under its own event, so the two are
    // distinguishable in an access log.
    let cross_site = server
        .post("/api/setup")
        .add_header("sec-fetch-site", "cross-site")
        .json(&serde_json::json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    cross_site.assert_status(StatusCode::FORBIDDEN);
    assert!(
        logs.contents().contains("csrf.cross_site"),
        "a cross-site rejection must be told apart from a token mismatch"
    );
}
