mod common;
use common::default_test_config;

use axum::http::{StatusCode, header};
use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
use rdrs::{AppState, Config, Db, auth, create_router, services};
use serde_json::json;

/// Build the router and backing `Db` for a config, without wrapping either in a
/// `TestServer` — the cookie-jar policy is the caller's choice.
async fn build_app(config: Config) -> (axum::Router, Db) {
    let db = Db::connect_in_memory().await.unwrap();

    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db: db.clone(),
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

    (create_router(state), db)
}

/// Build a test server and return it together with the backing `Db` so tests
/// that inspect the `session` table directly can share the same connection.
async fn build_server(config: Config) -> (TestServer, Db) {
    let (app, db) = build_app(config).await;
    (TestServer::builder().save_cookies().build(app), db)
}

/// Like [`build_server`] but without `save_cookies`, so a test can replay a
/// hand-edited cookie instead of the jar echoing back whatever the server set.
async fn build_server_no_save_cookies(config: Config) -> (TestServer, Db) {
    let (app, db) = build_app(config).await;
    (TestServer::new(app), db)
}

async fn create_test_server(config: Config) -> TestServer {
    build_server(config).await.0
}

/// Create a user directly in the database, bypassing `POST /api/register` —
/// so setup does not itself consume a slot from the client's rate-limit
/// budget (registration is a guarded, never-released endpoint; going through
/// it here would leave fewer than 5 attempts free for a test that means to
/// exercise the login endpoint specifically).
async fn create_user_directly(db: &Db, username: &str, password: &str) {
    let hash = auth::hash_password(password).unwrap();
    rdrs::models::user::create_user(db, username, &hash, rdrs::Role::User)
        .await
        .unwrap();
}

use std::sync::Arc;

#[tokio::test]
async fn test_register_first_user_becomes_admin() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
}

#[tokio::test]
async fn test_register_second_user_becomes_user() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "user1");
    assert_eq!(body["role"], "user");
}

#[tokio::test]
async fn test_register_duplicate_username() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "different123"
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_register_disabled_still_allows_first_account() {
    // With signup disabled, a fresh install must still be able to create its
    // first (admin) account — otherwise a source build is unusable. Subsequent
    // registrations stay blocked.
    let config = Config {
        signup_enabled: false,
        ..default_test_config()
    };
    let server = create_test_server(config).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_register_multi_user_disabled() {
    let config = Config {
        signup_enabled: true,
        multi_user_enabled: false,
        ..default_test_config()
    };
    let server = create_test_server(config).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_login_success() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    response.assert_status_ok();
    common::apply_csrf(&mut server, &response);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_login_wrong_password() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "wrongpassword"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_login_nonexistent_user() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "nonexistent",
            "password": "password123"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_login_rate_limited_after_five_failures() {
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "password123").await;

    for _ in 0..5 {
        let response = server
            .post("/api/session")
            .json(&json!({
                "username": "admin",
                "password": "wrongpassword"
            }))
            .await;
        response.assert_status_unauthorized();
    }

    // The 6th failed attempt is throttled before it ever reaches the
    // database or Argon2 verification.
    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "wrongpassword"
        }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_login_rate_limit_does_not_leak_valid_credentials() {
    // Proves the rate-limit check runs before `verify_password`: once the
    // budget is exhausted, even the CORRECT password gets 429, not 200 or
    // 401. If the check ran after verification, a throttled attacker could
    // distinguish "right password, rate limited" from "wrong password,
    // rate limited" by noticing the correct guess still succeeds.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "password123").await;

    for _ in 0..5 {
        server
            .post("/api/session")
            .json(&json!({
                "username": "admin",
                "password": "wrongpassword"
            }))
            .await
            .assert_status_unauthorized();
    }

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_successful_login_does_not_consume_the_budget() {
    // Proves `release` works end-to-end: ten successful logins in a row
    // never hit the 5-attempt budget because each one hands its reservation
    // back immediately after the password verifies.
    let (mut server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "password123").await;

    for _ in 0..10 {
        let response = server
            .post("/api/session")
            .json(&json!({
                "username": "admin",
                "password": "password123"
            }))
            .await;
        response.assert_status_ok();
        // Each login mints a fresh session and CSRF cookie pair; refresh the
        // header so the *next* iteration's POST clears the synchronizer-token
        // guard too. `add_header` accumulates rather than replaces, so the
        // stale header from the previous iteration must be cleared first —
        // otherwise `X-CSRF-Token` would carry two values and the guard would
        // read the oldest (now-mismatched) one. Unrelated to rate limiting —
        // just what repeated login through the same cookie jar requires.
        server.clear_headers();
        common::apply_csrf(&mut server, &response);
    }
}

#[tokio::test]
async fn test_register_is_rate_limited() {
    // A successful registration never releases its reservation — account
    // creation is exactly the abuse this limiter targets — so five
    // registrations exhaust the budget and the sixth is throttled outright.
    let server = create_test_server(default_test_config()).await;

    for i in 0..5 {
        let response = server
            .post("/api/register")
            .json(&json!({
                "username": format!("user{i}"),
                "password": "password123"
            }))
            .await;
        response.assert_status(StatusCode::CREATED);
    }

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user5",
            "password": "password123"
        }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_passkey_auth_start_is_rate_limited() {
    // This endpoint leaks account existence, so it must consume budget on
    // every call even though it never itself succeeds or fails a credential
    // check the way password login does.
    let server = create_test_server(default_test_config()).await;

    for _ in 0..5 {
        let response = server.post("/api/passkey/auth/start").await;
        assert_ne!(response.status_code(), StatusCode::TOO_MANY_REQUESTS);
    }

    let response = server.post("/api/passkey/auth/start").await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_get_current_user() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    login_response.assert_status_ok();
    common::apply_csrf(&mut server, &login_response);

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_get_current_user_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/api/user").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_logout() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    server.delete("/api/session").await.assert_status_ok();

    server.get("/api/user").await.assert_status_unauthorized();
}

// Coverage for password change moved to tests/handlers_test.rs
// (test_change_password_form_*) since the JSON PUT endpoint was removed in
// favour of the SSR form-action endpoint at POST /user-settings/password.

// Admin user CRUD coverage (list / disable / delete / update role + self
// protection) moved to tests/handlers_test.rs (test_*_form_*) when the JSON
// `/api/admin/users*` endpoints were removed in favour of the SSR
// form-action endpoints under POST /admin/users/{id}/* (PR-5 T2).

#[tokio::test]
async fn test_masquerade() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    server
        .post("/admin/users/2/masquerade")
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "user1");

    server
        .post("/api/admin/unmasquerade")
        .await
        .assert_status_ok();

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_masquerade_already_masquerading() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // First masquerade succeeds and lands us on /.
    let response = server.post("/admin/users/2/masquerade").await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/");

    // Second masquerade attempt redirects back to /admin with an error flash
    // (FlashRedirect::error) — never a 4xx because form endpoints always
    // respond with 303 See Other.
    let response = server.post("/admin/users/2/masquerade").await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/admin");
}

#[tokio::test]
async fn test_unmasquerade_not_masquerading() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let response = server.post("/api/admin/unmasquerade").await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_login_page() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Login"));
}

#[tokio::test]
async fn test_register_page() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Register"));
}

#[tokio::test]
async fn test_validation_short_password() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "short"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_validation_empty_username() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "",
            "password": "password123"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_unread_page() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    login_response.assert_status_ok();
    common::apply_csrf(&mut server, &login_response);

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout (PR-10) — sidebar bootstrap JSON still inlined; no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains(r#"id="rdrs-sidebar-bootstrap""#));
    assert!(body.contains(r#""username":"admin""#));
    // Two-pane layout rendered server-side.
    assert!(body.contains("data-entries-list"));
}

#[tokio::test]
async fn test_unread_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/").await;
    // Page routes redirect to login instead of returning 401
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_admin_page_accessible_by_admin() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let response = server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Admin Panel"));
}

#[tokio::test]
async fn test_admin_page_forbidden_for_regular_user() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let response = server.get("/admin").await;
    // Page routes redirect to login instead of returning 403
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_admin_page_unauthorized_without_login() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/admin").await;
    // Page routes redirect to login instead of returning 401
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_unread_page_shows_admin_link_for_admin() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    // Admin nav is rendered client-side by <rdrs-sidebar>; the initial HTML
    // carries `is_admin: true` in the sidebar bootstrap JSON.
    assert!(body.contains(r#""is_admin":true"#));
}

#[tokio::test]
async fn test_unread_page_hides_admin_link_for_regular_user() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("data-testid=\"nav-admin\""));
    assert!(!body.contains(r#"href="/admin""#));
}

#[tokio::test]
async fn test_flash_message_displayed_on_login_page() {
    let server = create_test_server(default_test_config()).await;

    // Set a flash message cookie using add_cookie with cookie::Cookie
    let response = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"success","message":"Test flash message"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Test flash message"));
    assert!(body.contains("banner--success"));
    assert!(body.contains(r#"role="status""#));
}

#[tokio::test]
async fn test_flash_message_cleared_after_display() {
    let server = create_test_server(default_test_config()).await;

    // First request with flash cookie
    let response = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"info","message":"First message"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("First message"));

    // Second request should not have the flash message (cookie was cleared)
    let response = server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("First message"));
}

#[tokio::test]
async fn test_flash_message_on_unread_page() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // Request unread page with flash message
    let response = server
        .get("/")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"warning","message":"Warning test"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    // Flash messages are now embedded in the rdrs-flash-bootstrap JSON
    // for the rdrs-flash element to consume on connect.
    assert!(body.contains(r#"id="rdrs-flash-bootstrap""#));
    assert!(body.contains("Warning test"));
    assert!(body.contains(r#""level":"warning""#));
}

/// Force a logged-in user's session into the refresh window by back-dating
/// `created_at` 5 days and setting `expires_at` 2 hours from now. Returns the
/// aged `expires_at` so callers can assert forward movement.
async fn age_session(db: &Db) -> DateTime<Utc> {
    let aged_expiry = Utc::now() + Duration::hours(2);
    let created = Utc::now() - Duration::days(5);
    rdrs::db_execute!(
        db,
        "UPDATE session SET created_at = $1, expires_at = $2",
        created,
        aged_expiry
    )
    .unwrap();
    aged_expiry
}

async fn read_expiry(db: &Db) -> DateTime<Utc> {
    rdrs::query_scalar!(db, DateTime<Utc>, "SELECT expires_at FROM session LIMIT 1").unwrap()
}

#[tokio::test]
async fn test_api_request_slides_session_expiry_forward() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let aged_expiry = age_session(&db).await;

    server.get("/api/user").await.assert_status_ok();

    let refreshed = read_expiry(&db).await;
    assert!(
        refreshed > aged_expiry,
        "expiry should slide forward: aged={aged_expiry} refreshed={refreshed}"
    );
    // Sliding bumps to roughly now + 7 days (sanity bound).
    let expected_min = Utc::now() + Duration::days(6);
    assert!(
        refreshed >= expected_min,
        "refreshed expiry too small: {refreshed} < {expected_min}"
    );
}

#[tokio::test]
async fn test_page_request_slides_session_expiry_forward() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let aged_expiry = age_session(&db).await;

    server.get("/").await.assert_status_ok();

    let refreshed = read_expiry(&db).await;
    assert!(
        refreshed > aged_expiry,
        "page request should slide expiry forward: aged={aged_expiry} refreshed={refreshed}"
    );
}

#[tokio::test]
async fn test_disable_local_auth_blocks_password_login() {
    let mut config = default_test_config();
    config.disable_local_auth = true;
    config.auth_proxy_header = "Remote-User".to_string();
    config.trusted_proxy_networks = rdrs::config::parse_trusted_networks("127.0.0.0/8").unwrap();
    let server = create_test_server(config).await;

    // Seed a normal password user.
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Password login is now forbidden.
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status(StatusCode::FORBIDDEN);
}

/// Collect raw Set-Cookie header values from a response.
fn set_cookie_headers(res: &axum_test::TestResponse) -> Vec<String> {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(std::string::ToString::to_string))
        .collect()
}

/// Log in a fresh user and return the raw `session_token` Set-Cookie value.
async fn login_session_cookie(server: &TestServer) -> String {
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    res.assert_status_ok();
    set_cookie_headers(&res)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("login must emit a session_token Set-Cookie")
}

#[tokio::test]
async fn test_session_cookie_secure_when_enabled() {
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let cookie = login_session_cookie(&create_test_server(config).await).await;
    assert!(
        cookie.contains("Secure"),
        "cookie_secure must put Secure on the session cookie: {cookie}"
    );
    // The other attributes must survive the shared builder.
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
}

#[tokio::test]
async fn test_session_cookie_not_secure_by_default() {
    // No RDRS_PUBLIC_BASE_URL → plain-HTTP dev run. A Secure cookie would be
    // dropped by the browser and lock the developer out.
    let cookie = login_session_cookie(&create_test_server(default_test_config()).await).await;
    assert!(
        !cookie.contains("Secure"),
        "session cookie must not be Secure without cookie_secure: {cookie}"
    );
}

#[tokio::test]
async fn test_logout_clears_cookie_with_path() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let removal = set_cookie_headers(&res)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("logout must emit a session_token Set-Cookie");
    assert!(
        removal.contains("Path=/"),
        "removal cookie must carry Path=/ to actually delete the session cookie: {removal}"
    );
}

#[tokio::test]
async fn test_tampered_session_cookie_is_rejected() {
    // The cookie value is `<token>.<hmac>`. Swapping the token while keeping the
    // signature is the attack the signing defends against — guessing or leaking
    // a `session.session_token` must not be enough on its own. A server without
    // `save_cookies` lets us replay a hand-edited cookie.
    let (mut server, _db) = build_server_no_save_cookies(default_test_config()).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(&mut server, &login);

    // Pull the signed value out of the Set-Cookie header.
    let set_cookie = set_cookie_headers(&login)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("login must emit a session_token cookie");
    let value = set_cookie
        .trim_start_matches("session_token=")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let (token, sig) = value.rsplit_once('.').expect("cookie value is token.sig");

    // The untouched cookie authenticates.
    server
        .get("/api/user")
        .add_cookie(cookie::Cookie::new("session_token", value.clone()))
        .await
        .assert_status_ok();

    // Token changed, signature kept → rejected before any DB lookup.
    let forged = format!("{token}x.{sig}");
    server
        .get("/api/user")
        .add_cookie(cookie::Cookie::new("session_token", forged))
        .await
        .assert_status_unauthorized();

    // Signature stripped entirely (a raw DB token) → also rejected.
    server
        .get("/api/user")
        .add_cookie(cookie::Cookie::new("session_token", token.to_string()))
        .await
        .assert_status_unauthorized();
}

#[tokio::test]
async fn test_logout_redirect_default_is_login() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["redirect_to"], "/login");
    assert_eq!(body["logout_url_configured"], false);
}

#[tokio::test]
async fn test_logout_redirect_uses_configured_url() {
    let mut config = default_test_config();
    config.auth_proxy_logout_url = Some("https://auth.example.com/logout".to_string());
    let mut server = create_test_server(config).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["redirect_to"], "https://auth.example.com/logout");
    assert_eq!(body["logout_url_configured"], true);
}

#[tokio::test]
async fn test_logout_redirect_uses_relative_configured_url() {
    let mut config = default_test_config();
    config.auth_proxy_logout_url = Some("/logout".to_string());
    let mut server = create_test_server(config).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    assert_eq!(body["redirect_to"], "/logout");
    assert_eq!(body["logout_url_configured"], true);
}

#[tokio::test]
async fn test_login_page_redirects_authenticated_to_root() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/register")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.get("/login").await;
    assert!(res.status_code().is_redirection());
    assert_eq!(res.header("location"), "/");
}

#[tokio::test]
async fn test_login_page_renders_when_anonymous() {
    let server = create_test_server(default_test_config()).await;
    let res = server.get("/login").await;
    res.assert_status_ok();
    assert!(res.text().contains("login-form") || res.text().contains("rdrs"));
}

#[tokio::test]
async fn test_fresh_session_is_not_refreshed() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/register")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "password123" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let before = read_expiry(&db).await;

    server.get("/api/user").await.assert_status_ok();

    let after = read_expiry(&db).await;
    assert_eq!(
        before, after,
        "fresh session should not be refreshed on every request"
    );
}
