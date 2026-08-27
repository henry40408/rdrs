//! Integration tests for the Google Reader API endpoints: `ClientLogin`,
//! subscription list/edit, stream contents and item IDs, edit-tag,
//! mark-all-as-read and unread-count.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{HeaderValue, StatusCode, header};
use axum_test::TestServer;
use rdrs::models::{category, entry, feed};
use rdrs::{AppState, Config, Db, auth, create_router, services};
use serde_json::json;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct TestApp {
    server: TestServer,
    db: Db,
    state: AppState,
}

async fn create_test_app(config: Config) -> TestApp {
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

    let app = create_router(state.clone());
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db, state }
}

/// Register and login a user via session cookie. Returns `user_id`.
async fn setup_authenticated_user(app: &TestApp) -> i64 {
    app.server
        .post("/api/setup")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    app.server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status_ok();

    rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .unwrap()
        .id
}

/// Create a category and feed directly in DB. Returns (`category_id`, `feed_id`).
async fn create_test_feed(db: &Db, user_id: i64, cat_name: &str, feed_url: &str) -> (i64, i64) {
    let cat = category::create_category(db, user_id, cat_name)
        .await
        .unwrap();
    let f = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: feed_url,
            title: Some("Test Feed"),
            description: None,
            site_url: Some("https://example.com"),
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    (cat.id, f.id)
}

/// Create entries directly via DB for testing. Returns entry IDs.
async fn create_test_entries(db: &Db, feed_id: i64, count: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..count {
        let guid = format!("guid-{i}");
        let title = format!("Entry {i}");
        let link = format!("https://example.com/entry/{i}");
        let (e, _) = entry::upsert_entry(
            db,
            feed_id,
            &guid,
            Some(&title),
            Some(&link),
            Some("Content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        ids.push(e.id);
    }
    ids
}

// --- ClientLogin Tests ---

#[tokio::test]
async fn test_client_login_success() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("SID="));
    assert!(body.contains("LSID="));
    assert!(body.contains("Auth="));

    // The Auth token is an independent api_token, not the raw session_token —
    // recognisable by its rdrs_gr_ prefix.
    let token = body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap();
    assert!(
        token.starts_with(rdrs::models::api_token::API_TOKEN_PREFIX),
        "Auth token must be an api_token, got {token:?}"
    );
}

#[tokio::test]
async fn test_client_login_invalid_password() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "wrongpassword".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    assert_ne!(response.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn test_client_login_nonexistent_user() {
    let app = create_test_app(default_test_config()).await;

    let form = vec![
        ("Email", "nonexistent".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    assert_ne!(response.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn test_client_login_token_used_for_api() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    let body = response.text();
    let token = body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap();

    let response = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={token}")).unwrap(),
        )
        .await;
    response.assert_status_ok();
}

/// This is the core regression test of the whole `api_token` decoupling task:
/// a `ClientLogin` token must not be usable as a web session under any
/// disguise, even a correctly-signed one.
#[tokio::test]
async fn test_client_login_token_is_not_a_web_session() {
    let app = create_test_app(default_test_config()).await;
    // Created directly in the DB rather than through `/api/session`, so the
    // TestServer's cookie jar never picks up a real session cookie — the only
    // cookies sent in this test are the ones this test adds explicitly.
    create_user_directly(&app.db, "testuser", "vulture-mango-77-quilt").await;
    let user_id = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .unwrap()
        .id;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();
    let body = response.text();
    let token = body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap()
        .to_string();

    let sessions_before = rdrs::models::session::list_user_sessions(&app.db, user_id)
        .await
        .unwrap();
    assert!(
        sessions_before.is_empty(),
        "no web session was ever created"
    );

    // (a) sent bare as a cookie value → 401 (fails signature verification
    // before any database lookup, same as any malformed cookie).
    let bare = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_cookie(cookie::Cookie::new("session_token", token.clone()))
        .await;
    assert_eq!(bare.status_code(), StatusCode::UNAUTHORIZED);

    // (b) signed correctly with secret::sign_session and sent as a cookie →
    // still 401, because no such row exists in `session` (only in
    // `api_token`). This is the actual decoupling assertion — (a) alone would
    // pass even if the token secretly still worked as a signed session.
    let config = default_test_config();
    let signed = rdrs::secret::sign_session(&config.secret, &token);
    let signed_response = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_cookie(cookie::Cookie::new("session_token", signed))
        .await;
    assert_eq!(signed_response.status_code(), StatusCode::UNAUTHORIZED);

    // (c) the `session` table row count did not increase — ClientLogin must
    // never have written a session row for this token.
    let sessions_after = rdrs::models::session::list_user_sessions(&app.db, user_id)
        .await
        .unwrap();
    assert_eq!(sessions_before.len(), sessions_after.len());
    assert!(sessions_after.is_empty());
}

/// The mirror image of the test above, and the invariant that removing the
/// `RDRS_GREADER_LEGACY_SESSION_TOKENS` escape hatch makes unconditional: a
/// real web session token presented in the `Authorization` header is never
/// matched against `session`. Only the cookie path may carry a web session.
#[tokio::test]
async fn test_web_session_token_is_rejected_in_the_authorization_header() {
    let app = create_test_app(default_test_config()).await;
    create_user_directly(&app.db, "testuser", "vulture-mango-77-quilt").await;
    let user_id = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .unwrap()
        .id;

    // A genuine, unexpired web session — the exact value a pre-cutover
    // GReader client would have had stored.
    let session =
        rdrs::models::session::create_session(&app.db, user_id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

    // Sent bare, the way ClientLogin used to hand it out.
    let bare = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={}", session.session_token)).unwrap(),
        )
        .await;
    assert_eq!(bare.status_code(), StatusCode::UNAUTHORIZED);

    // And signed, so this cannot pass merely because the raw value failed a
    // signature check somewhere.
    let config = default_test_config();
    let signed = rdrs::secret::sign_session(&config.secret, &session.session_token);
    let signed_response = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={signed}")).unwrap(),
        )
        .await;
    assert_eq!(signed_response.status_code(), StatusCode::UNAUTHORIZED);

    // The session itself is still valid — the header path rejected it, not
    // an expiry or a cleanup sweep.
    let still_there = rdrs::models::session::find_by_token(&app.db, &session.session_token)
        .await
        .unwrap();
    assert!(
        still_there.is_some(),
        "the session must survive: it was rejected as a header credential, not deleted"
    );
}

#[tokio::test]
async fn test_web_session_cookie_still_works_for_greader() {
    let app = create_test_app(default_test_config()).await;
    // setup_authenticated_user logs in via POST /api/session; the TestServer's
    // cookie jar (save_cookies) picks up the resulting session cookie
    // automatically, so no Authorization header is sent below.
    setup_authenticated_user(&app).await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_post_token_works_for_both_credential_kinds() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    // Cookie credential.
    let cookie_token_resp = app.server.get("/reader/api/0/token").await;
    cookie_token_resp.assert_status_ok();
    let cookie_post_token = cookie_token_resp.text();

    // ApiToken credential (same underlying user).
    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let cl_response = app.server.post("/accounts/ClientLogin").form(&form).await;
    let cl_body = cl_response.text();
    let api_token = cl_body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap()
        .to_string();

    let api_token_resp = app
        .server
        .get("/reader/api/0/token")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={api_token}")).unwrap(),
        )
        .await;
    api_token_resp.assert_status_ok();
    let api_post_token = api_token_resp.text();

    assert_ne!(
        cookie_post_token, api_post_token,
        "post tokens for different credentials must not collide"
    );

    // A post token minted under one credential is not accepted under the other.
    // Cookie-authenticated requests skip the POST-token check entirely, since
    // SameSite already protects them, so the only way to observe a rejection is
    // a header-authenticated mutation submitting the mismatched token as `T`.
    let unknown_feed_form_wrong_token = vec![
        ("ac", "edit".to_string()),
        (
            "s",
            "feed/https://does-not-exist.example.com/rss".to_string(),
        ),
        ("T", cookie_post_token),
    ];
    let wrong_token_resp = app
        .server
        .post("/reader/api/0/subscription/edit")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={api_token}")).unwrap(),
        )
        .form(&unknown_feed_form_wrong_token)
        .await;
    assert_eq!(
        wrong_token_resp.status_code(),
        StatusCode::UNAUTHORIZED,
        "a cookie-credential post token must not be accepted under an ApiToken credential"
    );

    // The matching token passes the check and the request proceeds past it
    // (into "feed not found" territory, not "unauthorized").
    let unknown_feed_form_right_token = vec![
        ("ac", "edit".to_string()),
        (
            "s",
            "feed/https://does-not-exist.example.com/rss".to_string(),
        ),
        ("T", api_post_token),
    ];
    let right_token_resp = app
        .server
        .post("/reader/api/0/subscription/edit")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={api_token}")).unwrap(),
        )
        .form(&unknown_feed_form_right_token)
        .await;
    assert_eq!(right_token_resp.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_expired_api_token_is_lazily_deleted() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "vulture-mango-77-quilt".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    let body = response.text();
    let token = body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap()
        .to_string();

    rdrs::db_execute!(
        &app.db,
        "UPDATE api_token SET expires_at = datetime('now', '-1 hours') WHERE token = $1",
        &token,
    )
    .unwrap();

    let response = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={token}")).unwrap(),
        )
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

    let found = rdrs::models::api_token::find_by_token(&app.db, &token)
        .await
        .unwrap();
    assert!(found.is_none(), "expired api_token must be lazily deleted");
}

/// Create a user directly in the database, bypassing `POST /api/setup` —
/// so setup does not itself consume a slot from the client's rate-limit
/// budget (registration is a guarded, never-released endpoint; going through
/// it here would leave fewer than 5 attempts free for the test itself).
async fn create_user_directly(db: &Db, username: &str, password: &str) {
    let hash = auth::hash_password(password).unwrap();
    rdrs::models::user::create_user(db, username, &hash, rdrs::Role::User)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_client_login_rate_limited() {
    let app = create_test_app(default_test_config()).await;
    create_user_directly(&app.db, "testuser", "vulture-mango-77-quilt").await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "wrongpassword".to_string()),
    ];
    for _ in 0..5 {
        let response = app.server.post("/accounts/ClientLogin").form(&form).await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }

    // The 6th failed attempt is throttled rather than evaluated.
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    assert_eq!(response.status_code(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_client_login_rate_limit_applies_to_greader_php_prefix() {
    // The router `.nest()`s the same handler under /api/greader.php (see
    // src/lib.rs), so the two paths must share the same rate-limit bucket —
    // an attacker cannot dodge the limiter by switching prefixes mid-attack.
    let app = create_test_app(default_test_config()).await;
    create_user_directly(&app.db, "testuser", "vulture-mango-77-quilt").await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "wrongpassword".to_string()),
    ];
    for _ in 0..5 {
        let response = app
            .server
            .post("/api/greader.php/accounts/ClientLogin")
            .form(&form)
            .await;
        assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
    }

    let response = app
        .server
        .post("/api/greader.php/accounts/ClientLogin")
        .form(&form)
        .await;
    assert_eq!(response.status_code(), StatusCode::TOO_MANY_REQUESTS);
}

// --- Subscription List Tests ---

#[tokio::test]
async fn test_subscription_list_empty() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_subscription_list_with_feeds() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let subs = body["subscriptions"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert!(subs[0]["id"].as_str().unwrap().starts_with("feed/"));
    assert_eq!(subs[0]["title"].as_str().unwrap(), "Test Feed");

    let categories = subs[0]["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0]["label"].as_str().unwrap(), "Tech");
}

// --- Subscription Edit Tests ---

#[tokio::test]
async fn test_subscription_edit_unsubscribe() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = "https://example.com/tech.xml";
    create_test_feed(&app.db, user_id, "Tech", feed_url).await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);

    // Unsubscribe
    let form = vec![
        ("ac", "unsubscribe".to_string()),
        ("s", format!("feed/{feed_url}")),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    assert!(body["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_subscription_edit_rename() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = "https://example.com/feed.xml";
    create_test_feed(&app.db, user_id, "Tech", feed_url).await;

    // Edit feed title
    let form = vec![
        ("ac", "edit".to_string()),
        ("s", format!("feed/{feed_url}")),
        ("t", "New Title".to_string()),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    let subs = body["subscriptions"].as_array().unwrap();
    assert_eq!(subs[0]["title"].as_str().unwrap(), "New Title");
}

// --- Stream Contents Tests ---

#[tokio::test]
async fn test_stream_contents_reading_list() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 3).await;
    assert_eq!(entry_ids.len(), 3);

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);

    assert!(items[0]["id"].as_str().is_some());
    assert!(items[0]["title"].as_str().is_some());
}

#[tokio::test]
async fn test_stream_contents_with_limit() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 5).await;

    // Request only 2 items
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    // Should have continuation token for pagination
    assert!(body["continuation"].as_str().is_some());
}

#[tokio::test]
async fn test_stream_contents_negative_n_does_not_return_everything() {
    // Regression: a negative `n` used to make `limit = count + 1` negative, which
    // SQLite treats as unbounded (n=-2 → LIMIT -1), and `take(count as usize)`
    // wrapped to a huge value — together dumping the user's entire entry set in
    // one response. The count must clamp to 0.
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 5).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=-2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    // Clamped to 0 — not the full set of 5.
    assert_eq!(items.len(), 0, "negative n must not bypass the limit");
}

#[tokio::test]
async fn test_stream_contents_composite_cursor_no_skip_on_backdated() {
    // Regression for #164: legacy `e.id < c` cursor skipped entries with
    // high ids and old timestamps. Composite cursor must visit them.
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;
    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;

    // 5 monotonic entries
    for i in 1..=5 {
        rdrs::db_execute!(
            &app.db,
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES ($1, $2, $3, datetime('now', $4))",
            feed_id,
            format!("mono-{}", i),
            format!("M{}", i),
            format!("-{} hours", 5 - i)
        )
        .unwrap();
    }
    // 2 back-dated (new ids, old timestamps)
    for i in 1..=2 {
        rdrs::db_execute!(
            &app.db,
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES ($1, $2, $3, datetime('now', $4))",
            feed_id,
            format!("bd-{}", i),
            format!("BD{}", i),
            format!("-{} days", 30 + i)
        )
        .unwrap();
    }

    // Page 1: n=5
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=5")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let items1 = body["items"].as_array().unwrap();
    assert_eq!(items1.len(), 5);
    let cursor = body["continuation"]
        .as_str()
        .expect("continuation present")
        .to_string();
    assert!(
        cursor.contains('|'),
        "cursor must be composite format, got {cursor:?}"
    );

    // Page 2: pass cursor via add_query_param (URL-safely encodes spaces and `|`)
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .add_query_param("n", "5")
        .add_query_param("c", &cursor)
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let items2 = body["items"].as_array().unwrap();

    // Page 1 holds 5 newest, page 2 holds 2 back-dated → 7 total
    assert_eq!(items1.len() + items2.len(), 7);
}

#[tokio::test]
async fn test_stream_contents_starred_empty() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert!(items.is_empty());
}

// --- Stream Items IDs Tests ---

#[tokio::test]
async fn test_stream_items_ids() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 3).await;

    let response = app
        .server
        .get("/reader/api/0/stream/items/ids?s=user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item_refs = body["itemRefs"].as_array().unwrap();
    assert_eq!(item_refs.len(), 3);
    assert!(item_refs[0]["id"].as_str().is_some());
}

// --- Edit Tag Tests (read/star) ---

#[tokio::test]
async fn test_edit_tag_mark_read() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 1).await;
    let item_id = format!("tag:google.com,2005:reader/item/{:016x}", entry_ids[0]);

    let form = vec![
        ("i", item_id.clone()),
        ("a", "user/-/state/com.google/read".to_string()),
    ];
    let response = app.server.post("/reader/api/0/edit-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let counts = body["unreadcounts"].as_array().unwrap();
    let reading_list_count = counts
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .map_or(0, |c| c["count"].as_i64().unwrap());
    assert_eq!(reading_list_count, 0);
}

#[tokio::test]
async fn test_edit_tag_mark_read_multiple() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 3).await;

    // Mark every loaded entry as read in a single batch request — the server
    // path behind the "o" / Mark-loaded-as-read shortcut.
    let mut form: Vec<(String, String)> = entry_ids
        .iter()
        .map(|id| {
            (
                "i".to_string(),
                format!("tag:google.com,2005:reader/item/{:016x}", *id),
            )
        })
        .collect();
    form.push(("a".to_string(), "user/-/state/com.google/read".to_string()));

    let response = app.server.post("/reader/api/0/edit-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // All three are now read -> reading-list unread count is 0.
    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let counts = body["unreadcounts"].as_array().unwrap();
    let reading_list_count = counts
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .map_or(0, |c| c["count"].as_i64().unwrap());
    assert_eq!(reading_list_count, 0);
}

#[tokio::test]
async fn test_edit_tag_mark_read_rejects_unknown_entry() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 2).await;

    // A batch that mixes two owned entries with one id that does not exist.
    // The handler verifies every id up front and rejects the whole batch, so
    // no entry is marked read.
    let mut form: Vec<(String, String)> = entry_ids
        .iter()
        .map(|id| {
            (
                "i".to_string(),
                format!("tag:google.com,2005:reader/item/{:016x}", *id),
            )
        })
        .collect();
    form.push((
        "i".to_string(),
        format!("tag:google.com,2005:reader/item/{:016x}", 999_999_i64),
    ));
    form.push(("a".to_string(), "user/-/state/com.google/read".to_string()));

    let response = app.server.post("/reader/api/0/edit-tag").form(&form).await;
    response.assert_status(StatusCode::NOT_FOUND);

    // The two owned entries must remain unread (batch rolled back).
    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let counts = body["unreadcounts"].as_array().unwrap();
    let reading_list_count = counts
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .map_or(0, |c| c["count"].as_i64().unwrap());
    assert_eq!(reading_list_count, 2);
}

#[tokio::test]
async fn test_edit_tag_star_and_unstar() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 1).await;
    let item_id = format!("tag:google.com,2005:reader/item/{:016x}", entry_ids[0]);

    // Star
    let form = vec![
        ("i", item_id.clone()),
        ("a", "user/-/state/com.google/starred".to_string()),
    ];
    app.server
        .post("/reader/api/0/edit-tag")
        .form(&form)
        .await
        .assert_status_ok();

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    // Unstar
    let form = vec![
        ("i", item_id),
        ("r", "user/-/state/com.google/starred".to_string()),
    ];
    app.server
        .post("/reader/api/0/edit-tag")
        .form(&form)
        .await
        .assert_status_ok();

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

// --- Mark All As Read Tests ---

#[tokio::test]
async fn test_mark_all_as_read() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 5).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let reading_list = body["unreadcounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .unwrap();
    assert_eq!(reading_list["count"].as_i64().unwrap(), 5);

    let form = vec![("s", "user/-/state/com.google/reading-list".to_string())];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();

    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let reading_list = body["unreadcounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"));
    let count = reading_list.map_or(0, |c| c["count"].as_i64().unwrap());
    assert_eq!(count, 0);
}

/// The affected-row count rides in a header rather than the body, so a
/// `GReader` client that parses the literal `OK` keeps working while rdrs' own
/// JS can report a real number.
#[tokio::test]
async fn test_mark_all_as_read_reports_affected_count() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 5).await;

    let form = vec![("s", "user/-/state/com.google/reading-list".to_string())];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK", "GReader body contract is unchanged");
    assert_eq!(
        response.header("x-rdrs-affected"),
        "5",
        "header must carry the number of entries actually marked"
    );

    // Nothing left unread: the same call now changes zero rows and must say so
    // rather than repeating the first run's number.
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();
    assert_eq!(response.header("x-rdrs-affected"), "0");
}

/// The count is the number of rows that *changed*, not the number of ids
/// posted — the distinction the old DOM-counting flash could not make.
#[tokio::test]
async fn test_edit_tag_affected_count_excludes_already_read() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 3).await;

    let form_for = |ids: &[i64]| -> Vec<(String, String)> {
        let mut form: Vec<(String, String)> = ids
            .iter()
            .map(|id| {
                (
                    "i".to_string(),
                    format!("tag:google.com,2005:reader/item/{:016x}", *id),
                )
            })
            .collect();
        form.push(("a".to_string(), "user/-/state/com.google/read".to_string()));
        form
    };

    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_for(&entry_ids[..1]))
        .await;
    response.assert_status_ok();
    assert_eq!(response.header("x-rdrs-affected"), "1");

    // Now post all three. Only the two still unread change.
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_for(&entry_ids))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
    assert_eq!(
        response.header("x-rdrs-affected"),
        "2",
        "already-read entries must not be counted again"
    );
}

// --- Unread Count Tests ---

#[tokio::test]
async fn test_unread_count_empty() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["unreadcounts"].as_array().is_some());
}

#[tokio::test]
async fn test_unread_count_with_entries() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 3).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let counts = body["unreadcounts"].as_array().unwrap();

    // Should have counts for feed, category, and reading-list
    assert!(!counts.is_empty());

    let reading_list = counts
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .unwrap();
    assert_eq!(reading_list["count"].as_i64().unwrap(), 3);
}

// --- Unauthenticated Access Tests ---

#[tokio::test]
async fn test_unauthenticated_access_denied() {
    let config = default_test_config();
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db,
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

    let app = create_router(state);
    let server = TestServer::builder().build(app);

    // All endpoints should be unauthorized without auth
    let response = server.get("/reader/api/0/subscription/list").await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

    let response = server.get("/reader/api/0/unread-count").await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

// --- RSS fixture used by wiremock-based subscribe/quickadd tests ---

const RSS_FIXTURE: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel>
  <title>Mock Feed</title><description>D</description><link>https://e</link>
  <item><guid>m1</guid><title>One</title><link>https://e/1</link><description>c1</description></item>
</channel></rss>"#;

// --- Part A — pure validation / edit tests (no network required) ---

#[tokio::test]
async fn test_subscription_subscribe_missing_stream_id() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    // POST ac=subscribe without `s` → 400 Bad Request
    let form = vec![("ac", "subscribe".to_string())];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_subscription_subscribe_bad_prefix() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    // Stream ID must start with "feed/"; bare URL should be rejected with 400
    let form = vec![
        ("ac", "subscribe".to_string()),
        ("s", "https://example.com/feed.xml".to_string()),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_subscription_subscribe_empty_url() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    // "feed/" with nothing after the slash → empty URL → 400
    let form = vec![("ac", "subscribe".to_string()), ("s", "feed/".to_string())];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_subscription_edit_feed_not_found() {
    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    // ac=edit with a URL that doesn't exist in the DB → 404
    let form = vec![
        ("ac", "edit".to_string()),
        (
            "s",
            "feed/https://does-not-exist.example.com/rss".to_string(),
        ),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_subscription_edit_add_label_moves_category() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = "https://example.com/feed-to-move.xml";
    create_test_feed(&app.db, user_id, "OldCat", feed_url).await;

    let form = vec![
        ("ac", "edit".to_string()),
        ("s", format!("feed/{feed_url}")),
        ("a", "user/-/label/NewCat".to_string()),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    // DB: feed's category should now be "NewCat"
    let f = feed::find_by_url_for_user(&app.db, feed_url, user_id)
        .await
        .unwrap()
        .expect("feed should exist");
    let cats = category::list_by_user(&app.db, user_id).await.unwrap();
    let cat = cats
        .into_iter()
        .find(|c| c.id == f.category_id)
        .expect("category should exist");
    let cat_name = cat.name;
    assert_eq!(cat_name, "NewCat");
}

// --- Part B — wiremock subscribe / quickadd success tests ---

#[tokio::test]
async fn test_subscription_subscribe_success() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = mock_server.uri();
    let form = vec![
        ("ac", "subscribe".to_string()),
        ("s", format!("feed/{feed_url}")),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    let feed_exists = feed::find_by_url_for_user(&app.db, &feed_url, user_id)
        .await
        .unwrap()
        .is_some();
    assert!(feed_exists, "feed row should exist after subscribe");
}

#[tokio::test]
async fn test_subscription_subscribe_with_label() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = mock_server.uri();
    let form = vec![
        ("ac", "subscribe".to_string()),
        ("s", format!("feed/{feed_url}")),
        ("a", "user/-/label/Tech".to_string()),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    let f = feed::find_by_url_for_user(&app.db, &feed_url, user_id)
        .await
        .unwrap()
        .expect("feed should exist");
    let cats = category::list_by_user(&app.db, user_id).await.unwrap();
    let cat = cats
        .into_iter()
        .find(|c| c.id == f.category_id)
        .expect("category should exist");
    let cat_name = cat.name;
    assert_eq!(cat_name, "Tech");
}

#[tokio::test]
async fn test_subscription_subscribe_duplicate() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;

    // Pre-create the feed in the DB (same URL the mock server serves)
    let feed_url = mock_server.uri();
    create_test_feed(&app.db, user_id, "Existing", &feed_url).await;

    // Attempt to subscribe again → 409 CONFLICT (AppError::FeedExists)
    let form = vec![
        ("ac", "subscribe".to_string()),
        ("s", format!("feed/{feed_url}")),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_quickadd_success() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(".*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&app).await;

    let feed_url = mock_server.uri();
    let form = vec![("quickadd", feed_url.clone())];
    let response = app
        .server
        .post("/reader/api/0/subscription/quickadd")
        .form(&form)
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["numResults"].as_i64().unwrap(), 1);
    assert_eq!(
        body["streamId"].as_str().unwrap(),
        format!("feed/{feed_url}")
    );
}

// ============================================================================
// SSE: GReader writes must announce sidebar changes
//
// These mutations arrive from an external client, so no browser swap happens
// that a page could hang a refresh off. Without an `emit_sidebar` an open tab
// keeps rendering the pre-change counts until it is reloaded — busting the cache
// alone only helps the *next* request.
// ============================================================================

/// Assert the next event on `sub` is a Sidebar event for `user_id`.
async fn expect_sidebar_event(
    sub: &mut tokio::sync::broadcast::Receiver<rdrs::services::UserEvent>,
    user_id: i64,
    what: &str,
) {
    let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} must emit a sidebar event"))
        .unwrap();
    assert_eq!(ev.user_id, user_id);
    assert!(
        matches!(ev.kind, rdrs::services::EventKind::Sidebar),
        "{what} must emit Sidebar, got {:?}",
        ev.kind
    );
}

#[tokio::test]
async fn test_edit_tag_emits_sidebar_event() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;
    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 1).await;
    let mut sub = app.state.events.subscribe();

    let form = vec![
        (
            "i",
            format!("tag:google.com,2005:reader/item/{:016x}", entry_ids[0]),
        ),
        ("a", "user/-/state/com.google/read".to_string()),
    ];
    app.server
        .post("/reader/api/0/edit-tag")
        .form(&form)
        .await
        .assert_status_ok();

    expect_sidebar_event(&mut sub, user_id, "edit-tag").await;
}

#[tokio::test]
async fn test_mark_all_as_read_emits_sidebar_event() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;
    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 3).await;
    let mut sub = app.state.events.subscribe();

    let form = vec![("s", "user/-/state/com.google/reading-list".to_string())];
    app.server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await
        .assert_status_ok();

    expect_sidebar_event(&mut sub, user_id, "mark-all-as-read").await;
}

#[tokio::test]
async fn test_rename_tag_emits_sidebar_event() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;
    create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let mut sub = app.state.events.subscribe();

    let form = vec![
        ("s", "user/-/label/Tech".to_string()),
        ("dest", "user/-/label/Technology".to_string()),
    ];
    app.server
        .post("/reader/api/0/rename-tag")
        .form(&form)
        .await
        .assert_status_ok();

    expect_sidebar_event(&mut sub, user_id, "rename-tag").await;
}

#[tokio::test]
async fn test_disable_tag_emits_sidebar_event() {
    let app = create_test_app(default_test_config()).await;
    let user_id = setup_authenticated_user(&app).await;
    create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let mut sub = app.state.events.subscribe();

    let form = vec![("s", "user/-/label/Tech".to_string())];
    app.server
        .post("/reader/api/0/disable-tag")
        .form(&form)
        .await
        .assert_status_ok();

    expect_sidebar_event(&mut sub, user_id, "disable-tag").await;
}
