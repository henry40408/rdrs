//! Integration tests for Google Reader API endpoints.
//!
//! Tests cover:
//! - POST /accounts/ClientLogin (authentication)
//! - GET /reader/api/0/subscription/list (subscription listing)
//! - POST /reader/api/0/subscription/edit (edit/remove subscriptions)
//! - GET /reader/api/0/stream/contents/{stream} (stream contents)
//! - GET /reader/api/0/stream/items/ids (item IDs)
//! - POST /reader/api/0/edit-tag (read/star tags)
//! - POST /reader/api/0/mark-all-as-read (mark all read)
//! - GET /reader/api/0/unread-count (unread counts)

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{header, HeaderValue, StatusCode};
use axum_test::TestServer;
use rdrs::models::{category, entry, feed};
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: DbPool,
}

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

fn create_test_app(config: Config) -> TestApp {
    let write_conn = open_shared_memory("test_greader");
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory("test_greader");

    let (db, _handle) = DbPool::new(write_conn, read_conn);
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
    };

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db }
}

/// Register and login a user via session cookie. Returns user_id.
async fn setup_authenticated_user(app: &TestApp) -> i64 {
    app.server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    app.server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // Get user_id from DB
    app.db
        .user(|conn| {
            rdrs::models::user::find_by_username(conn, "testuser")
                .unwrap()
                .unwrap()
                .id
        })
        .await
        .unwrap()
}

/// Create a category and feed directly in DB. Returns (category_id, feed_id).
async fn create_test_feed(db: &DbPool, user_id: i64, cat_name: &str, feed_url: &str) -> (i64, i64) {
    let cat_name = cat_name.to_string();
    let feed_url = feed_url.to_string();
    db.user(move |conn| {
        let cat = category::create_category(conn, user_id, &cat_name).unwrap();
        let f = feed::create_feed(
            conn,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url: &feed_url,
                title: Some("Test Feed"),
                description: None,
                site_url: Some("https://example.com"),
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .unwrap();
        (cat.id, f.id)
    })
    .await
    .unwrap()
}

/// Create entries directly via DB for testing. Returns entry IDs.
async fn create_test_entries(db: &DbPool, feed_id: i64, count: usize) -> Vec<i64> {
    let mut ids = Vec::new();
    for i in 0..count {
        let guid = format!("guid-{}", i);
        let title = format!("Entry {}", i);
        let link = format!("https://example.com/entry/{}", i);
        let id = db
            .user(move |conn| {
                let (e, _) = entry::upsert_entry(
                    conn,
                    feed_id,
                    &guid,
                    Some(&title),
                    Some(&link),
                    Some("Content"),
                    None,
                    None,
                    None,
                )
                .unwrap();
                e.id
            })
            .await
            .unwrap();
        ids.push(id);
    }
    ids
}

// ============================================================================
// ClientLogin Tests
// ============================================================================

#[tokio::test]
async fn test_client_login_success() {
    let app = create_test_app(default_test_config());
    setup_authenticated_user(&app).await;

    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "password123".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("SID="));
    assert!(body.contains("LSID="));
    assert!(body.contains("Auth="));
}

#[tokio::test]
async fn test_client_login_invalid_password() {
    let app = create_test_app(default_test_config());
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
    let app = create_test_app(default_test_config());

    let form = vec![
        ("Email", "nonexistent".to_string()),
        ("Passwd", "password123".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    assert_ne!(response.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn test_client_login_token_used_for_api() {
    let app = create_test_app(default_test_config());
    setup_authenticated_user(&app).await;

    // Get auth token via ClientLogin
    let form = vec![
        ("Email", "testuser".to_string()),
        ("Passwd", "password123".to_string()),
    ];
    let response = app.server.post("/accounts/ClientLogin").form(&form).await;
    let body = response.text();
    let token = body
        .lines()
        .find_map(|line| line.strip_prefix("Auth="))
        .unwrap();

    // Use the token to access a protected endpoint via the Authorization header
    let response = app
        .server
        .get("/reader/api/0/subscription/list")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("GoogleLogin auth={}", token)).unwrap(),
        )
        .await;
    response.assert_status_ok();
}

// ============================================================================
// Subscription List Tests
// ============================================================================

#[tokio::test]
async fn test_subscription_list_empty() {
    let app = create_test_app(default_test_config());
    setup_authenticated_user(&app).await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_subscription_list_with_feeds() {
    let app = create_test_app(default_test_config());
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

// ============================================================================
// Subscription Edit Tests
// ============================================================================

#[tokio::test]
async fn test_subscription_edit_unsubscribe() {
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = "https://example.com/tech.xml";
    create_test_feed(&app.db, user_id, "Tech", feed_url).await;

    // Verify feed exists
    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);

    // Unsubscribe
    let form = vec![
        ("ac", "unsubscribe".to_string()),
        ("s", format!("feed/{}", feed_url)),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    // Verify empty
    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    assert!(body["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_subscription_edit_rename() {
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;

    let feed_url = "https://example.com/feed.xml";
    create_test_feed(&app.db, user_id, "Tech", feed_url).await;

    // Edit feed title
    let form = vec![
        ("ac", "edit".to_string()),
        ("s", format!("feed/{}", feed_url)),
        ("t", "New Title".to_string()),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    // Verify title changed
    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    let subs = body["subscriptions"].as_array().unwrap();
    assert_eq!(subs[0]["title"].as_str().unwrap(), "New Title");
}

// ============================================================================
// Stream Contents Tests
// ============================================================================

#[tokio::test]
async fn test_stream_contents_reading_list() {
    let app = create_test_app(default_test_config());
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

    // Verify item structure
    assert!(items[0]["id"].as_str().is_some());
    assert!(items[0]["title"].as_str().is_some());
}

#[tokio::test]
async fn test_stream_contents_with_limit() {
    let app = create_test_app(default_test_config());
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
async fn test_stream_contents_composite_cursor_no_skip_on_backdated() {
    // Regression for #164: legacy `e.id < c` cursor skipped entries with
    // high ids and old timestamps. Composite cursor must visit them.
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;
    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;

    app.db
        .user(move |conn| {
            // 5 monotonic entries
            for i in 1..=5 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        feed_id,
                        format!("mono-{}", i),
                        format!("M{}", i),
                        format!("-{} hours", 5 - i)
                    ],
                )
                .unwrap();
            }
            // 2 back-dated (new ids, old timestamps)
            for i in 1..=2 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        feed_id,
                        format!("bd-{}", i),
                        format!("BD{}", i),
                        format!("-{} days", 30 + i)
                    ],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

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
        "cursor must be composite format, got {:?}",
        cursor
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
    let app = create_test_app(default_test_config());
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

// ============================================================================
// Stream Items IDs Tests
// ============================================================================

#[tokio::test]
async fn test_stream_items_ids() {
    let app = create_test_app(default_test_config());
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

// ============================================================================
// Edit Tag Tests (read/star)
// ============================================================================

#[tokio::test]
async fn test_edit_tag_mark_read() {
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    let entry_ids = create_test_entries(&app.db, feed_id, 1).await;
    let item_id = format!("tag:google.com,2005:reader/item/{:016x}", entry_ids[0]);

    // Mark as read
    let form = vec![
        ("i", item_id.clone()),
        ("a", "user/-/state/com.google/read".to_string()),
    ];
    let response = app.server.post("/reader/api/0/edit-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify unread count is 0
    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let counts = body["unreadcounts"].as_array().unwrap();
    let reading_list_count = counts
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"))
        .map(|c| c["count"].as_i64().unwrap())
        .unwrap_or(0);
    assert_eq!(reading_list_count, 0);
}

#[tokio::test]
async fn test_edit_tag_mark_read_multiple() {
    let app = create_test_app(default_test_config());
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
        .map(|c| c["count"].as_i64().unwrap())
        .unwrap_or(0);
    assert_eq!(reading_list_count, 0);
}

#[tokio::test]
async fn test_edit_tag_mark_read_rejects_unknown_entry() {
    let app = create_test_app(default_test_config());
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
        .map(|c| c["count"].as_i64().unwrap())
        .unwrap_or(0);
    assert_eq!(reading_list_count, 2);
}

#[tokio::test]
async fn test_edit_tag_star_and_unstar() {
    let app = create_test_app(default_test_config());
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

    // Verify starred stream has 1 item
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

    // Verify starred stream is empty
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

// ============================================================================
// Mark All As Read Tests
// ============================================================================

#[tokio::test]
async fn test_mark_all_as_read() {
    let app = create_test_app(default_test_config());
    let user_id = setup_authenticated_user(&app).await;

    let (_cat_id, feed_id) =
        create_test_feed(&app.db, user_id, "Tech", "https://example.com/feed.xml").await;
    create_test_entries(&app.db, feed_id, 5).await;

    // Verify we have 5 unread entries
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

    // Mark all as read
    let form = vec![("s", "user/-/state/com.google/reading-list".to_string())];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();

    // Verify all are read (0 unread)
    let response = app.server.get("/reader/api/0/unread-count").await;
    let body: serde_json::Value = response.json();
    let reading_list = body["unreadcounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"].as_str().unwrap().contains("reading-list"));
    let count = reading_list
        .map(|c| c["count"].as_i64().unwrap())
        .unwrap_or(0);
    assert_eq!(count, 0);
}

// ============================================================================
// Unread Count Tests
// ============================================================================

#[tokio::test]
async fn test_unread_count_empty() {
    let app = create_test_app(default_test_config());
    setup_authenticated_user(&app).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["unreadcounts"].as_array().is_some());
}

#[tokio::test]
async fn test_unread_count_with_entries() {
    let app = create_test_app(default_test_config());
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

// ============================================================================
// Unauthenticated Access Tests
// ============================================================================

#[tokio::test]
async fn test_unauthenticated_access_denied() {
    let config = default_test_config();
    let write_conn = open_shared_memory("test_unauth");
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory("test_unauth");

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
        sidebar_cache: Arc::new(services::SidebarCache::default()),
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
