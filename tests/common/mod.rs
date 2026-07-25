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
#[allow(dead_code)] // not every test binary performs authenticated mutations
pub fn apply_csrf(server: &mut TestServer, login_response: &TestResponse) {
    let token = login_response.cookie("csrf_token").value().to_string();
    server.add_header("x-csrf-token", token);
}

/// Default in-memory `Config` shared across the integration test suites.
#[allow(dead_code)] // a few suites build their own Config inline
pub fn default_test_config() -> Config {
    Config {
        database_url: ":memory:".to_string(),
        server_bind: "127.0.0.1:8080".parse().unwrap(),
        signup_enabled: true,
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
    }
}
