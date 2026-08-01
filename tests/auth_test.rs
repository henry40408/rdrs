mod common;
use common::default_test_config;

use axum::http::{StatusCode, header};
use axum_test::TestServer;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD};
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

/// Age a session by `age`, so it lands inside the sliding-refresh window
/// without a test having to wait days for it. The cookie the client holds is
/// unaffected — only the row moves — which is exactly the state a long-lived
/// browser session reaches on its own.
///
/// Takes the raw *cookie value*, which is `<token>.<hmac>` (see
/// `secret::sign_session`); the database stores only the token, so the
/// signature is stripped here rather than at every call site.
async fn backdate_session(db: &Db, session_cookie_value: &str, age: Duration) {
    let token = session_cookie_value
        .rsplit_once('.')
        .expect("session cookie value is <token>.<hmac>")
        .0;
    let now = Utc::now();
    let affected = rdrs::db_execute!(
        db,
        "UPDATE session SET created_at = $1, expires_at = $2 WHERE session_token = $3",
        now - age,
        now - age + Duration::days(7),
        token
    )
    .unwrap();
    assert_eq!(affected, 1, "backdate matched no session row");
}

/// Create a user directly in the database, bypassing `POST /api/setup` —
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
}

#[tokio::test]
async fn setup_closes_permanently_after_the_first_account() {
    // The endpoint that used to be self-service registration now exists only
    // to bootstrap an empty instance. Once one account exists it refuses
    // everything — that is what removes the account-enumeration surface, since
    // there is no longer any anonymous endpoint that accepts a username.
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // A different username, a valid password, multi-user enabled — still no.
    server
        .post("/api/setup")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::FORBIDDEN);

    // ...and the refusal is identical for a name that *does* exist, so the
    // endpoint cannot be used to test whether an account is there.
    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_closed_setup_endpoint_answers_the_same_for_any_username() {
    // Covered separately from the test above because this is the property
    // that matters: whatever an anonymous caller sends once the instance has
    // an account, the answer carries no information about who is registered.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

    let existing = server
        .post("/api/setup")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await;
    let absent = server
        .post("/api/setup")
        .json(&json!({ "username": "nobody-here", "password": "vulture-mango-77-quilt" }))
        .await;

    assert_eq!(existing.status_code(), absent.status_code());
    assert_eq!(existing.text(), absent.text());
}

#[tokio::test]
async fn test_register_disabled_still_allows_first_account() {
    // With signup disabled, a fresh install must still be able to create its
    // first (admin) account — otherwise a source build is unusable. Subsequent
    // registrations stay blocked.
    let config = Config {
        ..default_test_config()
    };
    let server = create_test_server(config).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "user",
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_register_multi_user_disabled() {
    let config = Config {
        multi_user_enabled: false,
        ..default_test_config()
    };
    let server = create_test_server(config).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_login_success() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_login_rate_limited_after_five_failures() {
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

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
async fn test_throttled_response_carries_retry_after() {
    // RFC 6585 §4: a 429 must tell the client when to come back, so a
    // well-behaved one waits instead of hammering. The value is what remains
    // of the limiter's current fixed window, so it can only be bounded here,
    // not pinned to an exact number.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

    for _ in 0..5 {
        server
            .post("/api/session")
            .json(&json!({ "username": "admin", "password": "wrongpassword" }))
            .await;
    }

    let response = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "wrongpassword" }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);

    let retry_after: u64 = response
        .header(header::RETRY_AFTER)
        .to_str()
        .expect("Retry-After must be printable")
        .parse()
        .expect("Retry-After must be a delay in seconds");
    assert!(
        (1..=60).contains(&retry_after),
        "Retry-After must fall inside the 60s window and never be 0, got {retry_after}"
    );
}

#[tokio::test]
async fn test_login_rate_limit_does_not_leak_valid_credentials() {
    // Proves the rate-limit check runs before `verify_password`: once the
    // budget is exhausted, even the CORRECT password gets 429, not 200 or
    // 401. If the check ran after verification, a throttled attacker could
    // distinguish "right password, rate limited" from "wrong password,
    // rate limited" by noticing the correct guess still succeeds.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

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
            "password": "vulture-mango-77-quilt"
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
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

    for _ in 0..10 {
        let response = server
            .post("/api/session")
            .json(&json!({
                "username": "admin",
                "password": "vulture-mango-77-quilt"
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
async fn test_setup_is_rate_limited() {
    // The reservation is never released — scripted account creation is exactly
    // the abuse this limiter targets — and it is charged before the
    // "is setup still open?" check, so hammering a closed endpoint runs the
    // budget down just the same. Five attempts, then a 429.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

    for i in 0..5 {
        let response = server
            .post("/api/setup")
            .json(&json!({
                "username": format!("user{i}"),
                "password": "vulture-mango-77-quilt"
            }))
            .await;
        response.assert_status(StatusCode::FORBIDDEN);
    }

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "user5",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    response.assert_status(StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_registration_budget_exhaustion_does_not_lock_out_login() {
    // CRITICAL regression: register and login used to share a single per-IP
    // counter. Registration never releases its reservation (a successful
    // signup is exactly the abuse the limiter targets), so five
    // registrations from one IP left zero budget behind — a subsequent
    // login with the CORRECT password then got 429 instead of 200,
    // indistinguishable from a real lockout. Register and login must draw
    // from independent buckets (`Bucket::AccountSetup` / `Bucket::Login`) so a
    // registration spree can never deny a legitimate login.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;

    for i in 0..5 {
        server
            .post("/api/setup")
            .json(&json!({
                "username": format!("user{i}"),
                "password": "vulture-mango-77-quilt"
            }))
            .await
            .assert_status(StatusCode::FORBIDDEN);
    }
    // Confirm the account-setup budget for this IP is now exhausted.
    server
        .post("/api/setup")
        .json(&json!({
            "username": "user5",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::TOO_MANY_REQUESTS);

    // A login with the CORRECT password must still succeed — the login
    // bucket was never touched by the register calls above.
    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    response.assert_status_ok();
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
async fn passkey_challenge_never_discloses_registered_credentials() {
    // Regression: the sign-in challenge used to be built from every row of
    // the `passkey` table, and webauthn-rs turns the credentials it is given
    // into `allowCredentials` — so one unauthenticated POST returned the
    // credential ID of every passkey on the instance. Those IDs are stable,
    // linkable per-user identifiers; the count alone says how many accounts
    // have enrolled one. The flow is discoverable now: the challenge names no
    // credential at all.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;
    let user = rdrs::models::user::find_by_username(&db, "admin")
        .await
        .unwrap()
        .expect("the user was just created");
    // The handler never deserialises stored keys any more, so a placeholder
    // blob is enough to prove the row is not read. What matters is that the
    // credential ID below cannot appear in the response.
    rdrs::models::passkey::create_passkey(
        &db,
        user.id,
        b"secret-credential-id",
        b"{}",
        0,
        "Laptop",
        None,
    )
    .await
    .unwrap();

    let response = server.post("/api/passkey/auth/start").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let allow = &body["options"]["publicKey"]["allowCredentials"];
    assert!(
        allow.is_null() || allow.as_array().is_some_and(std::vec::Vec::is_empty),
        "the challenge must name no credentials, got {allow}"
    );

    // Belt and braces: whatever shape the options take, the ID itself must
    // not appear anywhere in the response — in any encoding of those bytes.
    let raw = response.text();
    let encoded = BASE64_URL_SAFE_NO_PAD.encode(b"secret-credential-id");
    assert!(
        !raw.contains(&encoded) && !raw.contains("secret-credential-id"),
        "a registered credential ID leaked into the challenge: {raw}"
    );
}

#[tokio::test]
async fn passkey_challenge_is_identical_with_and_without_registered_passkeys() {
    // The old handler answered "No passkeys registered" (401) for an empty
    // table and a challenge (200) otherwise — an account-existence oracle for
    // anyone who could reach the endpoint. Both states must now be
    // indistinguishable.
    let (empty_server, _empty_db) = build_server(default_test_config()).await;
    let empty = empty_server.post("/api/passkey/auth/start").await;

    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;
    let user = rdrs::models::user::find_by_username(&db, "admin")
        .await
        .unwrap()
        .expect("the user was just created");
    rdrs::models::passkey::create_passkey(&db, user.id, b"cred", b"{}", 0, "Laptop", None)
        .await
        .unwrap();
    let populated = server.post("/api/passkey/auth/start").await;

    assert_eq!(empty.status_code(), populated.status_code());
    empty.assert_status_ok();

    // The challenge bytes differ per request by design; everything else about
    // the options must match.
    let mut a: serde_json::Value = empty.json();
    let mut b: serde_json::Value = populated.json();
    a["options"]["publicKey"]["challenge"] = serde_json::Value::Null;
    b["options"]["publicKey"]["challenge"] = serde_json::Value::Null;
    assert_eq!(a, b);
}

#[tokio::test]
async fn test_get_current_user() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    common::seed_account(&db, "user1", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let started = server.post("/admin/users/2/masquerade").await;
    started.assert_status(StatusCode::SEE_OTHER);
    // Entering the masquerade rotates the session token, so the CSRF token
    // derived from it changes too — a browser re-reads both from the cookies
    // this response sets; the test server needs the header refreshed by hand.
    common::apply_csrf(&mut server, &started);

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "user1");

    let stopped = server.post("/api/admin/unmasquerade").await;
    stopped.assert_status_ok();
    common::apply_csrf(&mut server, &stopped);

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

/// The periodic renewal (OWASP "Renewal Timeout"): once a session crosses the
/// sliding-refresh threshold, the next authenticated request must come back
/// with a *new* session token, and the client must stay signed in across the
/// swap.
#[tokio::test]
async fn test_session_token_rotates_once_past_the_refresh_window() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "alice",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "alice",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(&mut server, &login);
    let original = login.cookie("session_token").value().to_string();

    // A fresh session is nowhere near the refresh window, so nothing rotates.
    let quiet = server.get("/api/user").await;
    quiet.assert_status_ok();
    assert_eq!(quiet.cookie("session_token").value(), original);

    // Age the session past half its TTL, which is what arms both the sliding
    // refresh and the rotation that rides on it.
    backdate_session(&db, &original, Duration::days(6)).await;

    let rotated_response = server.get("/api/user").await;
    rotated_response.assert_status_ok();
    let rotated = rotated_response.cookie("session_token").value().to_string();
    assert_ne!(rotated, original, "session token should have rotated");
    // The CSRF cookie is derived from the session token, so it has to move with
    // it or every later mutation would fail the synchronizer check.
    assert_ne!(rotated_response.cookie("csrf_token").value(), "");
    common::apply_csrf(&mut server, &rotated_response);

    // Still signed in, now under the new token, and settled: a second request
    // does not rotate again.
    let after = server.get("/api/user").await;
    after.assert_status_ok();
    let body: serde_json::Value = after.json();
    assert_eq!(body["username"], "alice");
    assert_eq!(after.cookie("session_token").value(), rotated);
}

/// The pre-rotation token has to keep working for the grace interval, or every
/// request already in flight when a rotation lands would be signed out.
#[tokio::test]
async fn test_pre_rotation_token_still_authenticates_during_grace() {
    let (server, db) = build_server_no_save_cookies(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "bob",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "bob",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    login.assert_status_ok();
    let original_cookie = login.cookie("session_token");

    backdate_session(&db, original_cookie.value(), Duration::days(6)).await;

    // Rotate by making one request with the original cookie...
    let rotated_response = server
        .get("/api/user")
        .add_cookie(original_cookie.clone())
        .await;
    rotated_response.assert_status_ok();
    assert_ne!(
        rotated_response.cookie("session_token").value(),
        original_cookie.value()
    );

    // ...then replay the *old* cookie, standing in for a request that was
    // already in flight. It must still authenticate.
    let in_flight = server
        .get("/api/user")
        .add_cookie(original_cookie.clone())
        .await;
    in_flight.assert_status_ok();
    let body: serde_json::Value = in_flight.json();
    assert_eq!(body["username"], "bob");
}

/// Both masquerade transitions must hand the client a *new* session cookie and
/// a matching CSRF cookie — the privilege-change renewal OWASP's Session
/// Management Cheat Sheet requires. Asserted on the wire rather than in the
/// model layer, because a rotation the handler forgets to reissue would leave
/// the browser authenticated against a row that no longer exists.
#[tokio::test]
async fn test_masquerade_rotates_session_and_csrf_cookies() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    common::seed_account(&db, "user1", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let logged_in_session = __login.cookie("session_token").value().to_string();
    let logged_in_csrf = __login.cookie("csrf_token").value().to_string();

    let started = server.post("/admin/users/2/masquerade").await;
    started.assert_status(StatusCode::SEE_OTHER);
    let masq_session = started.cookie("session_token").value().to_string();
    let masq_csrf = started.cookie("csrf_token").value().to_string();
    assert_ne!(masq_session, logged_in_session);
    assert_ne!(masq_csrf, logged_in_csrf);
    common::apply_csrf(&mut server, &started);

    let stopped = server.post("/api/admin/unmasquerade").await;
    stopped.assert_status_ok();
    let restored_session = stopped.cookie("session_token").value().to_string();
    let restored_csrf = stopped.cookie("csrf_token").value().to_string();
    assert_ne!(restored_session, masq_session);
    assert_ne!(restored_session, logged_in_session);
    assert_ne!(restored_csrf, masq_csrf);
}

#[tokio::test]
async fn test_masquerade_already_masquerading() {
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    common::seed_account(&db, "user1", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // First masquerade succeeds and lands us on /.
    let response = server.post("/admin/users/2/masquerade").await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/");
    // Refresh the CSRF header against the rotated session, so the second
    // attempt below is rejected by the already-masquerading guard rather than
    // by the synchronizer-token check.
    common::apply_csrf(&mut server, &response);

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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
async fn test_setup_page_is_served_only_while_the_instance_is_empty() {
    let (server, db) = build_server(default_test_config()).await;

    let response = server.get("/setup").await;
    response.assert_status_ok();
    assert!(response.text().contains("setup-form"));

    // Once an account exists the page has no purpose, and leaving it
    // reachable would invite the "is registration open?" question this
    // design removes. It redirects rather than rendering a disabled form.
    create_user_directly(&db, "admin", "vulture-mango-77-quilt").await;
    let closed = server.get("/setup").await;
    closed.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(closed.header(header::LOCATION), "/login");
}

#[tokio::test]
async fn test_validation_short_password() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "short"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn setup_enforces_the_password_policy() {
    // The boundaries themselves, read from the constants so this test cannot
    // drift from the policy it is guarding. Each rejection reuses the same
    // empty instance — a refused attempt creates nothing, so setup stays open.
    let server = create_test_server(default_test_config()).await;

    let one_short = "a".repeat(rdrs::auth::PASSWORD_MIN_LENGTH - 1);
    server
        .post("/api/setup")
        .json(&json!({ "username": "shorty", "password": one_short }))
        .await
        .assert_status_bad_request();

    let one_long = "a".repeat(rdrs::auth::PASSWORD_MAX_LENGTH + 1);
    server
        .post("/api/setup")
        .json(&json!({ "username": "longy", "password": one_long }))
        .await
        .assert_status_bad_request();

    // A 15-character run of one letter clears the length gate and is refused
    // by the estimator instead, which is the point of having both.
    server
        .post("/api/setup")
        .json(&json!({ "username": "repeater", "password": "a".repeat(rdrs::auth::PASSWORD_MIN_LENGTH) }))
        .await
        .assert_status_bad_request();

    // Exactly at the minimum is accepted, provided it is not guessable.
    let at_min = "vT7q!mLp2zXc9Rw";
    assert_eq!(at_min.chars().count(), rdrs::auth::PASSWORD_MIN_LENGTH);
    server
        .post("/api/setup")
        .json(&json!({ "username": "atmin", "password": at_min }))
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn registration_refuses_a_password_built_from_the_username() {
    // The estimator is given the username, so a password that is just the
    // account name with decoration is scored for what it is — something no
    // list of breached passwords could ever catch, since the string is unique
    // to this account.
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "marigoldbadger",
            "password": "marigoldbadger1"
        }))
        .await;

    response.assert_status_bad_request();
    let body: serde_json::Value = response.json();
    let error = body["error"].as_str().expect("a validation message");
    assert!(
        !error.contains("at least") && !error.contains("at most"),
        "expected a guessability message rather than a length one, got {error:?}"
    );
}

#[tokio::test]
async fn an_over_long_password_is_refused_before_it_is_hashed() {
    // A rejected password must not reach Argon2 — otherwise the length cap
    // would be decided by the request-body limit and an attacker could pick
    // how much hashing work each refused registration costs. Observable only
    // by the error being a validation failure rather than a success, but the
    // ordering is what the test pins.
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "verbose",
            "password": "a".repeat(100_000)
        }))
        .await;

    response.assert_status_bad_request();
    let body: serde_json::Value = response.json();
    assert!(
        body["error"]
            .as_str()
            .expect("a validation error carries a message")
            .contains("at most"),
        "expected the length policy to reject it, got {body}"
    );
}

#[tokio::test]
async fn a_refused_setup_does_not_hash_the_password() {
    // Once an account exists, `can_setup` must decide before `hash_password`
    // runs: Argon2 is deliberately expensive, and paying it for a request that
    // was never going to succeed is free CPU for whoever sends it. Asserted
    // through the response the ordering produces — a refusal, not a
    // validation error — plus the account genuinely not existing afterwards.
    let (server, db) = build_server(default_test_config()).await;
    create_user_directly(&db, "owner", "vulture-mango-77-quilt").await;

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "intruder",
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
    assert!(
        rdrs::models::user::find_by_username(&db, "intruder")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_validation_empty_username() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/setup")
        .json(&json!({
            "username": "",
            "password": "vulture-mango-77-quilt"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_unread_page() {
    let mut server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    common::seed_account(&db, "user1", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
    let (mut server, db) = build_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    common::seed_account(&db, "user1", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
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
        .post("/api/setup")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Password login is now forbidden.
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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

/// Log in a fresh user and return the raw session Set-Cookie value, under
/// whichever name (`session_token` or `__Host-session_token`) the server's
/// `cookie_secure` config selects.
async fn login_session_cookie(server: &TestServer) -> String {
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status_ok();
    set_cookie_headers(&res)
        .into_iter()
        .find(|s| s.starts_with("session_token=") || s.starts_with("__Host-session_token="))
        .expect("login must emit a session_token or __Host-session_token Set-Cookie")
}

#[tokio::test]
async fn test_session_cookie_secure_when_enabled() {
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let cookie = login_session_cookie(&create_test_server(config).await).await;
    // cookie_secure must select the __Host--prefixed name (OWASP Cookies:
    // "use the __Host- prefix whenever possible" — see
    // middleware::auth::SESSION_COOKIE_NAME_HOST for why this is defence in
    // depth, not a fix for an exploitable gap).
    assert!(
        cookie.starts_with("__Host-session_token="),
        "cookie_secure must select the __Host- prefixed cookie name: {cookie}"
    );
    assert!(
        cookie.contains("Secure"),
        "cookie_secure must put Secure on the session cookie: {cookie}"
    );
    // The other attributes must survive the shared builder.
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    // A __Host- cookie must never carry Domain — the browser rejects it
    // outright if it does.
    assert!(
        !cookie.to_ascii_lowercase().contains("domain="),
        "a __Host- cookie must not carry a Domain attribute: {cookie}"
    );
}

#[tokio::test]
async fn test_session_cookie_not_secure_by_default() {
    // No RDRS_PUBLIC_BASE_URL → plain-HTTP dev run. A Secure cookie would be
    // dropped by the browser and lock the developer out.
    let cookie = login_session_cookie(&create_test_server(default_test_config()).await).await;
    assert!(
        cookie.starts_with("session_token="),
        "default config must stay on the unprefixed cookie name: {cookie}"
    );
    assert!(
        !cookie.contains("Secure"),
        "session cookie must not be Secure without cookie_secure: {cookie}"
    );
}

/// An old-style unprefixed `session_token` cookie, minted before an operator
/// enables `cookie_secure`, must keep authenticating afterwards —
/// `session_token_from_jar` falls back to the unprefixed name precisely so an
/// upgrade or a config flip never silently logs everyone out.
#[tokio::test]
async fn test_unprefixed_cookie_still_authenticates_when_secure_enabled() {
    // A cookie_secure = true server only ever *writes* the __Host- prefixed
    // name, so simulate an old-style cookie (minted before an upgrade, or
    // before an operator flipped RDRS_COOKIE_SECURE) by signing a session
    // token directly with the shared secret, bypassing the cookie-minting
    // handlers entirely.
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let (server, db) = build_server(config.clone()).await;
    let user = rdrs::models::user::create_user(&db, "u", "!", rdrs::Role::User)
        .await
        .unwrap();
    let session = rdrs::models::session::create_session(&db, user.id, "test-agent", "127.0.0.1")
        .await
        .unwrap();
    let old_style_value = rdrs::secret::sign_session(&config.secret, &session.session_token);

    let res = server
        .get("/api/user")
        .add_cookie(cookie::Cookie::new("session_token", old_style_value))
        .await;
    res.assert_status_ok();
}

/// The dangerous failure mode, guarded negatively: with `cookie_secure`
/// false, the server must never emit a `__Host-` prefixed cookie — the
/// browser silently discards a `__Host-` cookie without `Secure`, so doing
/// this would make login silently impossible.
#[tokio::test]
async fn test_prefixed_cookie_is_never_emitted_without_secure() {
    let server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let res = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    res.assert_status_ok();

    let all_cookies = set_cookie_headers(&res);
    assert!(
        all_cookies.iter().all(|c| !c.starts_with("__Host-")),
        "no __Host- prefixed cookie must be emitted when cookie_secure is false: {all_cookies:?}"
    );
}

#[tokio::test]
async fn test_session_cookie_max_age_matches_sliding_ttl() {
    // The database row's expires_at is `now + SESSION_EXPIRY_DAYS` (7 days),
    // sliding forward while the session stays active. The cookie's Max-Age
    // must match that, not the 90-day absolute cap the row can eventually
    // reach — otherwise the browser holds a cookie whose backing row is long
    // gone for most of those 90 days.
    let cookie = login_session_cookie(&create_test_server(default_test_config()).await).await;
    assert!(
        cookie.contains("Max-Age=604800"),
        "session cookie Max-Age must be the 7-day sliding TTL: {cookie}"
    );
    assert!(
        !cookie.contains("Max-Age=7776000"),
        "session cookie must not carry the old 90-day absolute-cap Max-Age: {cookie}"
    );
}

#[tokio::test]
async fn test_csrf_cookie_max_age_matches_session_cookie() {
    let server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    login.assert_status_ok();

    let csrf = set_cookie_headers(&login)
        .into_iter()
        .find(|s| s.starts_with("csrf_token="))
        .expect("login must emit a csrf_token Set-Cookie");
    assert!(
        csrf.contains("Max-Age=604800"),
        "csrf cookie Max-Age must mirror the session cookie's sliding TTL: {csrf}"
    );
    assert!(
        !csrf.contains("Max-Age=7776000"),
        "csrf cookie must not carry the old 90-day absolute-cap Max-Age: {csrf}"
    );
}

#[tokio::test]
async fn test_logout_clears_cookie_with_path() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let all_cookies = set_cookie_headers(&res);

    let removal = all_cookies
        .iter()
        .find(|s| s.starts_with("session_token="))
        .expect("logout must emit a session_token Set-Cookie");
    assert!(
        removal.contains("Path=/"),
        "removal cookie must carry Path=/ to actually delete the session cookie: {removal}"
    );

    // logout emits four removal cookies total: both names (unprefixed,
    // __Host--prefixed) for both purposes (session, CSRF) — so a cookie
    // leftover from before an upgrade or a cookie_secure flip is always
    // cleared regardless of which name this deployment currently mints.
    for name in [
        "session_token=",
        "csrf_token=",
        "__Host-session_token=",
        "__Host-csrf_token=",
    ] {
        let cookie = all_cookies
            .iter()
            .find(|s| s.starts_with(name))
            .unwrap_or_else(|| {
                panic!("logout must emit a {name} removal Set-Cookie: {all_cookies:?}")
            });
        if name.starts_with("__Host-") {
            assert!(
                cookie.contains("Secure"),
                "__Host- removal cookie must carry Secure unconditionally: {cookie}"
            );
            assert!(
                cookie.contains("Path=/"),
                "__Host- removal cookie must carry Path=/: {cookie}"
            );
        }
    }
}

/// The most important test in this task: `logout` emits empty removal
/// cookies for both `session_token` and `csrf_token`, and the sliding-cookie
/// middleware — layered outside `anonymous_session` and observing this same
/// response — must yield to them rather than reissuing a live cookie on top.
/// If it didn't, logout would silently fail to clear the cookie.
#[tokio::test]
async fn test_logout_removal_cookie_is_not_overwritten_by_slide() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let all_cookies = set_cookie_headers(&res);

    let session_cookies: Vec<_> = all_cookies
        .iter()
        .filter(|s| s.starts_with("session_token="))
        .collect();
    assert_eq!(
        session_cookies.len(),
        1,
        "logout must emit exactly one session_token Set-Cookie, not a second one \
         reissued by the sliding middleware: {all_cookies:?}"
    );
    let session_value = session_cookies[0]
        .trim_start_matches("session_token=")
        .split(';')
        .next()
        .unwrap();
    assert!(
        session_value.is_empty(),
        "the sliding middleware must not overwrite logout's empty removal cookie: {}",
        session_cookies[0]
    );

    let csrf_cookies: Vec<_> = all_cookies
        .iter()
        .filter(|s| s.starts_with("csrf_token="))
        .collect();
    assert_eq!(
        csrf_cookies.len(),
        1,
        "logout must emit exactly one csrf_token Set-Cookie: {all_cookies:?}"
    );
    let csrf_value = csrf_cookies[0]
        .trim_start_matches("csrf_token=")
        .split(';')
        .next()
        .unwrap();
    assert!(
        csrf_value.is_empty(),
        "the sliding middleware must not overwrite logout's empty csrf removal cookie: {}",
        csrf_cookies[0]
    );
}

/// With `cookie_secure = true`, the live session/CSRF cookies are minted
/// under the `__Host-` names, so the sliding middleware sees those names on
/// the incoming request. Logout must still clear them: the __Host- removal
/// cookies are always appended (never gated on `jar.remove()`'s "was this
/// name in the request's Cookie header" check, see `handlers::auth::logout`),
/// so `slide_session_cookie`'s "is this purpose already covered" check must
/// recognize its own removal and must not append a live `__Host-session_token`
/// (or `__Host-csrf_token`) alongside it — that would silently undo the logout.
#[tokio::test]
async fn test_logout_clears_prefixed_cookies_when_secure_enabled() {
    let config = Config {
        cookie_secure: true,
        ..default_test_config()
    };
    let mut server = create_test_server(config).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    let token = __login.cookie("__Host-csrf_token").value().to_string();
    server.add_header("x-csrf-token", token);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let all_cookies = set_cookie_headers(&res);

    for name in ["__Host-session_token=", "__Host-csrf_token="] {
        let matches: Vec<_> = all_cookies.iter().filter(|s| s.starts_with(name)).collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one {name} Set-Cookie (the removal), not a live one \
             appended by the sliding middleware: {all_cookies:?}"
        );
        let value = matches[0]
            .trim_start_matches(name)
            .split(';')
            .next()
            .unwrap();
        assert!(
            value.is_empty(),
            "the sliding middleware must not overwrite logout's empty {name} removal: {}",
            matches[0]
        );
        assert!(matches[0].contains("Secure"), "{}", matches[0]);
    }
}

#[tokio::test]
async fn test_tampered_session_cookie_is_rejected() {
    // The cookie value is `<token>.<hmac>`. Swapping the token while keeping the
    // signature is the attack the signing defends against — guessing or leaking
    // a `session.session_token` must not be enough on its own. A server without
    // `save_cookies` lets us replay a hand-edited cookie.
    let (mut server, _db) = build_server_no_save_cookies(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    // Byte-for-byte: adding the Clear-Site-Data header must not touch the
    // JSON body the client parses `redirect_to` out of.
    assert_eq!(
        body["redirect_to"].as_str(),
        Some("https://auth.example.com/logout")
    );
    assert_eq!(body["logout_url_configured"], true);
}

#[tokio::test]
async fn test_logout_redirect_uses_relative_configured_url() {
    let mut config = default_test_config();
    config.auth_proxy_logout_url = Some("/logout".to_string());
    let mut server = create_test_server(config).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    let body: serde_json::Value = res.json();
    // Byte-for-byte: adding the Clear-Site-Data header must not touch the
    // JSON body the client parses `redirect_to` out of.
    assert_eq!(body["redirect_to"].as_str(), Some("/logout"));
    assert_eq!(body["logout_url_configured"], true);
}

/// OWASP Session Management Cheat Sheet, "Manual Session Expiration": logout
/// should instruct the browser to delete data associated with the
/// application. This pins the header value itself; the deliberate omission
/// of `"cookies"` and `"executionContexts"` is pinned separately below so a
/// regression in either direction fails loudly.
#[tokio::test]
async fn test_logout_sends_clear_site_data() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let header = res
        .headers()
        .get("clear-site-data")
        .expect("logout must send a Clear-Site-Data header")
        .to_str()
        .expect("Clear-Site-Data must be valid ASCII");
    assert_eq!(header, "\"cache\", \"storage\"");
}

/// Pins the deliberate omission of `"cookies"` (and `"executionContexts"`)
/// from the logout `Clear-Site-Data` header — see
/// `handlers::auth::LOGOUT_CLEAR_SITE_DATA`. The server already emits
/// explicit removal cookies for the session; `Clear-Site-Data` processing is
/// asynchronous relative to JS, so clearing `"cookies"` here would race the
/// `flash` cookie `rdrs-flash.js` writes right after this response lands and
/// could eat the "You have been logged out." notice. A later contributor
/// "completing" this header to include `"cookies"` would silently
/// reintroduce that flakiness, so this test must fail if that happens.
#[tokio::test]
async fn test_logout_clear_site_data_omits_cookies() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    let res = server.delete("/api/session").await;
    res.assert_status_ok();

    let header = res
        .headers()
        .get("clear-site-data")
        .expect("logout must send a Clear-Site-Data header")
        .to_str()
        .expect("Clear-Site-Data must be valid ASCII");
    assert!(
        !header.contains("cookies"),
        "\"cookies\" would race the flash cookie written after this response: {header}"
    );
    assert!(
        !header.contains("executionContexts"),
        "\"executionContexts\" would force a reload that fights the client's own redirect: {header}"
    );
}

#[tokio::test]
async fn test_login_page_redirects_authenticated_to_root() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
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
        .post("/api/setup")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
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

#[tokio::test]
async fn test_authenticated_request_reissues_session_cookie() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(&mut server, &login);

    let login_cookie = set_cookie_headers(&login)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("login must emit a session_token Set-Cookie");
    let login_value = login_cookie
        .trim_start_matches("session_token=")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // A second, ordinary authenticated request must still carry a fresh
    // session_token Set-Cookie so the browser's Max-Age keeps tracking the
    // sliding TTL — but the value must be byte-identical: only the expiry
    // advances, the token itself is never rotated by this middleware.
    let res = server.get("/api/user").await;
    res.assert_status_ok();
    let reissued_cookie = set_cookie_headers(&res)
        .into_iter()
        .find(|s| s.starts_with("session_token="))
        .expect("an ordinary authenticated request must reissue the session_token cookie");
    let reissued_value = reissued_cookie
        .trim_start_matches("session_token=")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    assert_eq!(
        reissued_value, login_value,
        "the session token must not be rotated when its cookie is slid"
    );
    assert!(
        reissued_cookie.contains("Max-Age=604800"),
        "reissued cookie must carry the refreshed Max-Age: {reissued_cookie}"
    );
}

#[tokio::test]
async fn test_static_asset_response_has_no_set_cookie() {
    let mut server = create_test_server(default_test_config()).await;
    server
        .post("/api/setup")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let login = server
        .post("/api/session")
        .json(&json!({ "username": "u", "password": "vulture-mango-77-quilt" }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(&mut server, &login);

    // The asset need not exist — a 404 from the static handler is fine, only
    // the absence of Set-Cookie on this cacheable path prefix matters.
    let res = server.get("/static/nonexistent.css").await;
    let cookies = set_cookie_headers(&res);
    assert!(
        cookies.is_empty(),
        "a static asset response must never carry Set-Cookie: {cookies:?}"
    );
}
