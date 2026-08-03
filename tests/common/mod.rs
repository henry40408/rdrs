//! Shared helpers for the integration test suites.
//!
//! Included via `mod common;` from each `tests/*.rs` binary. Only put helpers
//! here that every (or nearly every) suite uses — per-suite `create_test_app`
//! definitions stay in their own files because each needs a unique
//! shared-memory database name to stay isolated from the other test binaries.

use axum_test::{TestResponse, TestServer};
use rdrs::Config;

/// A fresh, default-configured [`rdrs::middleware::RateLimiter`] for an
/// `AppState` literal.
///
/// Integration tests drive the router without `ConnectInfo`, so
/// `Config::client_ip` resolves every request in a test to `127.0.0.1` — all
/// requests within one test share a single bucket. That is safe only because
/// a successful credential check releases its reservation; a test that
/// performs more than five *failed* attempts against one `AppState` needs its
/// own disabled limiter (`RateLimiter::new(0, 60)`) instead of sharing this
/// one.
#[allow(dead_code)]
pub fn test_rate_limiter() -> std::sync::Arc<rdrs::middleware::RateLimiter> {
    std::sync::Arc::new(rdrs::middleware::RateLimiter::default())
}

/// Echo the server-set `csrf_token` cookie back as a default `X-CSRF-Token`
/// header on every later request from `server` — exactly what the browser's
/// `csrf.js` does once a session exists.
///
/// Call it with the `POST /api/session` (or setup) response, which sets the CSRF
/// cookie for the freshly created session. Without it, an authenticated
/// mutation is rejected by the synchronizer-token guard, since a `save_cookies`
/// test server stores the CSRF cookie but never turns it into a header on its
/// own. A request with no session cookie needs none of this — the guard lets it
/// through to the handler's own auth check.
///
/// Safe to call again whenever the session token rotates (masquerade start and
/// stop both rotate it, and the CSRF token is derived from it): the default
/// headers are cleared first, because [`TestServer::add_header`] *appends*, and
/// a stale `X-CSRF-Token` left ahead of the fresh one is the header the guard
/// reads. Clearing is safe for every suite here — no test sets a
/// server-default header other than this one; the `Remote-User` and
/// `Accept-Encoding` cases are all per-request.
#[allow(dead_code)] // not every test binary performs authenticated mutations
pub fn apply_csrf(server: &mut TestServer, login_response: &TestResponse) {
    let token = login_response.cookie("csrf_token").value().to_string();
    server.clear_headers();
    server.add_header("x-csrf-token", token);
}

/// Create an account straight in the database, ready to sign in with.
///
/// `POST /api/setup` only ever creates the *first* account — every later one
/// comes from an admin plus a redeemed invite, which is three requests of
/// ceremony for a test that just needs a second user to exist. Tests that are
/// actually about the invite flow drive the real endpoints; everything else
/// uses this.
///
/// Seeds the same default category the real paths do, so a freshly seeded
/// account behaves like one created through the UI.
#[allow(dead_code)] // not every suite needs a second account
pub async fn seed_account(
    db: &rdrs::Db,
    username: &str,
    password: &str,
    role: rdrs::Role,
) -> rdrs::User {
    let hash = rdrs::auth::hash_password(password).unwrap();
    let user = rdrs::models::user::create_user(db, username, &hash, role)
        .await
        .unwrap();
    rdrs::models::category::create_category(db, user.id, "Uncategorized")
        .await
        .unwrap();
    user
}

/// The flash messages a response set, as one string ready for substring
/// assertions.
///
/// The flash cookie holds a JSON array, which the cookie layer may or may not
/// percent-encode depending on the characters in the message — so the value is
/// decoded here rather than at each call site. Panics if the response set no
/// flash cookie, which for a `FlashRedirect` handler is itself the bug.
#[allow(dead_code)] // only the suites that assert on flash text need it
pub fn flash_text(response: &TestResponse) -> String {
    let raw = response.cookie("flash").value().to_string();
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&raw[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Default in-memory `Config` shared across the integration test suites.
#[allow(dead_code)] // a few suites build their own Config inline
pub fn default_test_config() -> Config {
    Config {
        database_url: ":memory:".to_string(),
        server_bind: "127.0.0.1:8080".parse().unwrap(),
        multi_user_enabled: true,
        secret: vec![0u8; 32],
        secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:8080".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
        cookie_secure: false,
        auth_proxy_header: String::new(),
        trusted_proxy_networks: Vec::new(),
        auth_proxy_user_creation: false,
        disable_local_auth: false,
        auth_proxy_groups_header: String::new(),
        auth_proxy_admin_group: String::new(),
        auth_proxy_logout_url: None,
        login_rate_limit_attempts: rdrs::middleware::rate_limit::LOGIN_MAX_ATTEMPTS,
        login_rate_limit_window_secs: rdrs::middleware::rate_limit::LOGIN_WINDOW_SECS,
        hsts: false,
        hsts_max_age: 31_536_000,
        hsts_include_subdomains: true,
    }
}
