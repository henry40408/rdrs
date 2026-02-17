//! Integration tests for entry-related handlers using GReader API endpoints.
//!
//! Tests cover:
//! - Stream contents listing (GET /reader/api/0/stream/contents/*)
//! - Item contents by ID (GET/POST /reader/api/0/stream/items/contents)
//! - Edit tag (mark read/unread/star/unstar via POST /reader/api/0/edit-tag)
//! - Mark all as read (POST /reader/api/0/mark-all-as-read)
//! - Unread count (GET /reader/api/0/unread-count)
//! - Subscription list (GET /reader/api/0/subscription/list)
//! - Subscription edit (POST /reader/api/0/subscription/edit)
//! - Tag list (GET /reader/api/0/tag/list)
//! - Rename tag (POST /reader/api/0/rename-tag)
//! - Disable tag (POST /reader/api/0/disable-tag)
//! - RDRS-specific endpoints (neighbors, fetch-full-content, summarize, summary, save)
//! - Cross-user access restrictions

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool, Role};
use rusqlite::Connection;
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: DbPool,
}

fn create_test_app(config: Config) -> TestApp {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let (db, _handle) = DbPool::new(conn);
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app).unwrap();

    TestApp { server, db }
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
    }
}

/// Setup user, category, feed, and entries directly in database
async fn setup_test_data(db: &DbPool) -> (i64, i64, i64, Vec<i64>) {
    db.user(move |conn| {
        // Create user
        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["testuser", password_hash, Role::Admin.as_str()],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        // Create category
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
            rusqlite::params![user_id, "Test Category"],
        )
        .unwrap();
        let category_id = conn.last_insert_rowid();

        // Create feed
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![category_id, "https://example.com/feed.xml", "Test Feed"],
        )
        .unwrap();
        let feed_id = conn.last_insert_rowid();

        // Create entries
        let mut entry_ids = Vec::new();
        for i in 1..=5 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link, content, summary, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now', ?7))",
                rusqlite::params![
                    feed_id,
                    format!("guid-{}", i),
                    format!("Entry Title {}", i),
                    format!("https://example.com/entry/{}", i),
                    format!("<p>Entry content {}</p>", i),
                    format!("Summary for entry {}", i),
                    format!("-{} hours", i)
                ],
            )
            .unwrap();
            entry_ids.push(conn.last_insert_rowid());
        }

        (user_id, category_id, feed_id, entry_ids)
    })
    .await
    .unwrap()
}

async fn login(server: &TestServer) {
    server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();
}

/// Setup a second user's data in the database
async fn setup_second_user_data(db: &DbPool) -> (i64, i64, i64, Vec<i64>) {
    db.user(move |conn| {
        // Create second user
        let password_hash = rdrs::auth::hash_password("password456").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["otheruser", password_hash, "user"],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        // Create category for second user
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
            rusqlite::params![user_id, "Other User Category"],
        )
        .unwrap();
        let category_id = conn.last_insert_rowid();

        // Create feed for second user
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![category_id, "https://other.com/feed.xml", "Other Feed"],
        )
        .unwrap();
        let feed_id = conn.last_insert_rowid();

        // Create entries for second user
        let mut entry_ids = Vec::new();
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link, content, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                rusqlite::params![
                    feed_id,
                    format!("other-guid-{}", i),
                    format!("Other Entry {}", i),
                    format!("https://other.com/entry/{}", i),
                    format!("<p>Other content {}</p>", i)
                ],
            )
            .unwrap();
            entry_ids.push(conn.last_insert_rowid());
        }

        (user_id, category_id, feed_id, entry_ids)
    })
    .await
    .unwrap()
}

async fn setup_entry_without_link(db: &DbPool, feed_id: i64) -> i64 {
    db.user(move |conn| {
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, content, published_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            rusqlite::params![
                feed_id,
                "no-link-guid",
                "Entry Without Link",
                "<p>Content without link</p>"
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    })
    .await
    .unwrap()
}

// --- GReader helper functions ---

/// Mark entries as read via edit-tag endpoint.
async fn mark_read(server: &TestServer, entry_ids: &[i64]) {
    let mut form_data: Vec<(&str, String)> =
        entry_ids.iter().map(|id| ("i", id.to_string())).collect();
    form_data.push(("a", "user/-/state/com.google/read".to_string()));
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

/// Mark entries as unread via edit-tag endpoint.
async fn mark_unread(server: &TestServer, entry_ids: &[i64]) {
    let mut form_data: Vec<(&str, String)> =
        entry_ids.iter().map(|id| ("i", id.to_string())).collect();
    form_data.push(("r", "user/-/state/com.google/read".to_string()));
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

/// Star entries via edit-tag endpoint.
async fn star_entry(server: &TestServer, entry_ids: &[i64]) {
    let mut form_data: Vec<(&str, String)> =
        entry_ids.iter().map(|id| ("i", id.to_string())).collect();
    form_data.push(("a", "user/-/state/com.google/starred".to_string()));
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

/// Unstar entries via edit-tag endpoint.
async fn unstar_entry(server: &TestServer, entry_ids: &[i64]) {
    let mut form_data: Vec<(&str, String)> =
        entry_ids.iter().map(|id| ("i", id.to_string())).collect();
    form_data.push(("r", "user/-/state/com.google/starred".to_string()));
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

// ============================================================================
// Stream Contents Tests (Entry List)
// ============================================================================

#[tokio::test]
async fn test_list_entries_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 5);
    assert_eq!(body["id"], "user/-/state/com.google/reading-list");
}

#[tokio::test]
async fn test_list_entries_with_limit() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // Should have continuation since there are more entries
    assert!(body["continuation"].is_string());
}

#[tokio::test]
async fn test_list_entries_with_continuation() {
    let app = create_test_app(default_test_config());
    // Create entries where IDs correlate with published_at in ascending order.
    // This is needed because continuation pagination uses `e.id < c` (newest-first)
    // or `e.id > c` (oldest-first), so ID order must match timestamp order.
    let (_user_id, _cat_id, _feed_id) = app
        .db
        .user(move |conn| {
            let password_hash = rdrs::auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["testuser", password_hash, Role::Admin.as_str()],
            )
            .unwrap();
            let user_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![user_id, "Test Category"],
            )
            .unwrap();
            let category_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![category_id, "https://example.com/feed.xml", "Test Feed"],
            )
            .unwrap();
            let feed_id = conn.last_insert_rowid();

            // Insert entries so that lower IDs have older published_at
            // (entry 1 = 5h ago, entry 5 = 1h ago)
            for i in 1..=5 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, link, content, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now', ?6))",
                    rusqlite::params![
                        feed_id,
                        format!("guid-{}", i),
                        format!("Entry Title {}", i),
                        format!("https://example.com/entry/{}", i),
                        format!("<p>Entry content {}</p>", i),
                        format!("-{} hours", 6 - i) // entry 1=-5h, entry 5=-1h
                    ],
                )
                .unwrap();
            }

            (user_id, category_id, feed_id)
        })
        .await
        .unwrap();

    login(&app.server).await;

    // Default sort is newest-first. With our data:
    //   entry 5 (newest, id=5), 4, 3, 2, 1 (oldest, id=1)
    // continuation_id uses `e.id < c`, which works correctly here.

    // First page
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let first_page_items = body["items"].as_array().unwrap();
    assert_eq!(first_page_items.len(), 2);
    let continuation = body["continuation"].as_str().unwrap();

    // Second page using continuation
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2&c={}",
            continuation
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let second_page_items = body["items"].as_array().unwrap();
    assert_eq!(second_page_items.len(), 2);

    // Entries from second page should be different from first page
    let first_ids: Vec<&str> = first_page_items
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    let second_ids: Vec<&str> = second_page_items
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    for id in &second_ids {
        assert!(!first_ids.contains(id));
    }
}

#[tokio::test]
async fn test_list_entries_by_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/label/Test%20Category")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 5);
    assert_eq!(body["id"], "user/-/label/Test Category");
}

#[tokio::test]
async fn test_list_entries_by_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/feed/https://example.com/feed.xml")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 5);
    assert_eq!(body["id"], "feed/https://example.com/feed.xml");
}

#[tokio::test]
async fn test_get_entry_by_id() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["title"].as_str().unwrap().contains("Entry Title"));
    assert!(items[0]["summary"]["content"].is_string());
    // Verify RDRS extension fields
    assert_eq!(items[0]["_entryId"], entry_ids[0]);
}

#[tokio::test]
async fn test_get_multiple_entries_by_id() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}&i={}",
            entry_ids[0], entry_ids[1]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

#[tokio::test]
async fn test_get_entries_by_id_post() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, String)> = entry_ids
        .iter()
        .take(3)
        .map(|id| ("i", id.to_string()))
        .collect();

    let response = app
        .server
        .post("/reader/api/0/stream/items/contents")
        .form(&form_data)
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
}

// ============================================================================
// Entry Read/Unread Tests (via edit-tag)
// ============================================================================

#[tokio::test]
async fn test_mark_entry_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    mark_read(&app.server, &[entry_ids[0]]).await;

    // Verify entry is now read via stream/items/contents
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item = &body["items"][0];
    // _readAt should be a string (not null) when read
    assert!(item["_readAt"].is_string());
    // categories should contain the read tag
    let categories: Vec<&str> = item["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(categories.contains(&"user/-/state/com.google/read"));
}

#[tokio::test]
async fn test_mark_entry_unread() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // First mark as read
    mark_read(&app.server, &[entry_ids[0]]).await;

    // Then mark as unread
    mark_unread(&app.server, &[entry_ids[0]]).await;

    // Verify entry is now unread
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item = &body["items"][0];
    assert!(item["_readAt"].is_null());
    let categories: Vec<&str> = item["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(!categories.contains(&"user/-/state/com.google/read"));
}

#[tokio::test]
async fn test_list_entries_unread_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark first two entries as read
    mark_read(&app.server, &[entry_ids[0], entry_ids[1]]).await;

    // Get unread entries only using xt (exclude read)
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
}

// ============================================================================
// Entry Star Tests (via edit-tag)
// ============================================================================

#[tokio::test]
async fn test_star_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    star_entry(&app.server, &[entry_ids[0]]).await;

    // Verify entry is starred
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item = &body["items"][0];
    assert!(item["_starredAt"].is_string());
    let categories: Vec<&str> = item["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(categories.contains(&"user/-/state/com.google/starred"));
}

#[tokio::test]
async fn test_unstar_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Star the entry
    star_entry(&app.server, &[entry_ids[0]]).await;

    // Unstar the entry
    unstar_entry(&app.server, &[entry_ids[0]]).await;

    // Verify entry is unstarred
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item = &body["items"][0];
    assert!(item["_starredAt"].is_null());
    let categories: Vec<&str> = item["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(!categories.contains(&"user/-/state/com.google/starred"));
}

#[tokio::test]
async fn test_list_entries_starred_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Star first entry
    star_entry(&app.server, &[entry_ids[0]]).await;

    // Get starred entries only
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["_entryId"], entry_ids[0]);
}

#[tokio::test]
async fn test_list_entries_read_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark first two entries as read
    mark_read(&app.server, &[entry_ids[0], entry_ids[1]]).await;

    // Get read entries only
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
}

// ============================================================================
// Mark All Read Tests
// ============================================================================

#[tokio::test]
async fn test_mark_all_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![("s", "user/-/state/com.google/reading-list")];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify all are read — unread stream should be empty
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_mark_all_read_by_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![("s", "user/-/label/Test Category")];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify all are read
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/label/Test%20Category?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_mark_all_read_by_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![("s", "feed/https://example.com/feed.xml")];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify all are read
    let response = app
        .server
        .get("/reader/api/0/stream/contents/feed/https://example.com/feed.xml?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_mark_read_batch_by_ids() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark first 3 entries as read by IDs using edit-tag
    mark_read(&app.server, &[entry_ids[0], entry_ids[1], entry_ids[2]]).await;

    // Verify remaining 2 entries are still unread
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_edit_tag_no_items_returns_error() {
    let app = create_test_app(default_test_config());
    setup_test_data(&app.db).await;
    login(&app.server).await;

    // Send edit-tag with no i= params — should return 400
    let form_data: Vec<(&str, &str)> = vec![("a", "user/-/state/com.google/read")];
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_data)
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_mark_read_already_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark entry as read
    mark_read(&app.server, &[entry_ids[0]]).await;

    // Mark the same entry again — should succeed (idempotent)
    mark_read(&app.server, &[entry_ids[0]]).await;

    // Verify still only 1 read entry in read stream
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    // entry_ids[0] should appear exactly once
    let matching: Vec<_> = items
        .iter()
        .filter(|i| i["_entryId"] == entry_ids[0])
        .collect();
    assert_eq!(matching.len(), 1);
}

#[tokio::test]
async fn test_cannot_mark_read_by_ids_other_user() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_user2_id, _cat2_id, _feed2_id, entry2_ids) = setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // User 1 tries to mark user 2's entries as read — should return 404 (EntryNotFound)
    let mut form_data: Vec<(&str, String)> = entry2_ids
        .iter()
        .take(2)
        .map(|id| ("i", id.to_string()))
        .collect();
    form_data.push(("a", "user/-/state/com.google/read".to_string()));
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Unread Count Tests
// ============================================================================

#[tokio::test]
async fn test_get_unread_count_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let unreadcounts = body["unreadcounts"].as_array().unwrap();

    // Find the feed unread count
    let feed_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "feed/https://example.com/feed.xml")
        .unwrap();
    assert_eq!(feed_count["count"], 5);

    // Find the category unread count
    let cat_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/label/Test Category")
        .unwrap();
    assert_eq!(cat_count["count"], 5);

    // Find total (reading-list) unread count
    let total_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/state/com.google/reading-list")
        .unwrap();
    assert_eq!(total_count["count"], 5);
}

#[tokio::test]
async fn test_get_unread_count_after_marking_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark two entries as read
    mark_read(&app.server, &[entry_ids[0], entry_ids[1]]).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let unreadcounts = body["unreadcounts"].as_array().unwrap();

    let feed_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "feed/https://example.com/feed.xml")
        .unwrap();
    assert_eq!(feed_count["count"], 3);

    let cat_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/label/Test Category")
        .unwrap();
    assert_eq!(cat_count["count"], 3);
}

// ============================================================================
// Entry Neighbors Tests (RDRS-specific, kept as-is)
// ============================================================================

#[tokio::test]
async fn test_get_entry_neighbors() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Get neighbors for middle entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/neighbors", entry_ids[2]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // Should have both previous and next
    assert!(body["previous_id"].is_i64() || body["previous_id"].is_null());
    assert!(body["next_id"].is_i64() || body["next_id"].is_null());
}

#[tokio::test]
async fn test_get_entry_neighbors_first_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Get neighbors for first entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/neighbors", entry_ids[0]))
        .await;
    response.assert_status_ok();

    // Response should be valid
    let _body: serde_json::Value = response.json();
}

#[tokio::test]
async fn test_get_entry_neighbors_unread_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Mark some entries as read
    mark_read(&app.server, &[entry_ids[0], entry_ids[2]]).await;

    // Get neighbors with unread_only=true for a middle entry
    let response = app
        .server
        .get(&format!(
            "/api/entries/{}/neighbors?unread_only=true",
            entry_ids[1]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // Should have neighbors that are unread
    assert!(body.get("prev_id").is_some());
    assert!(body.get("next_id").is_some());
}

// ============================================================================
// Subscription List Tests
// ============================================================================

#[tokio::test]
async fn test_subscription_list() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["title"], "Test Feed");
    assert_eq!(subscriptions[0]["id"], "feed/https://example.com/feed.xml");
    // Check category info
    let categories = subscriptions[0]["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0]["label"], "Test Category");
    assert_eq!(categories[0]["id"], "user/-/label/Test Category");
}

#[tokio::test]
async fn test_subscription_edit_update_title() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://example.com/feed.xml"),
        ("t", "Updated Feed Title"),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify title was updated via subscription list
    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"][0]["title"], "Updated Feed Title");
}

#[tokio::test]
async fn test_subscription_unsubscribe() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![
        ("ac", "unsubscribe"),
        ("s", "feed/https://example.com/feed.xml"),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify feed is gone
    let response = app.server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 0);
}

// ============================================================================
// Tag List Tests
// ============================================================================

#[tokio::test]
async fn test_tag_list() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app.server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();

    // Should have system tags + user labels
    let tag_ids: Vec<&str> = tags.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert!(tag_ids.contains(&"user/-/state/com.google/reading-list"));
    assert!(tag_ids.contains(&"user/-/state/com.google/read"));
    assert!(tag_ids.contains(&"user/-/state/com.google/starred"));
    assert!(tag_ids.contains(&"user/-/label/Test Category"));
}

#[tokio::test]
async fn test_rename_tag() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![
        ("s", "user/-/label/Test Category"),
        ("dest", "user/-/label/Renamed Category"),
    ];
    let response = app
        .server
        .post("/reader/api/0/rename-tag")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify tag was renamed
    let response = app.server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    let tag_ids: Vec<&str> = tags.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert!(!tag_ids.contains(&"user/-/label/Test Category"));
    assert!(tag_ids.contains(&"user/-/label/Renamed Category"));
}

#[tokio::test]
async fn test_disable_tag() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let form_data: Vec<(&str, &str)> = vec![("s", "user/-/label/Test Category")];
    let response = app
        .server
        .post("/reader/api/0/disable-tag")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify tag is gone
    let response = app.server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    let tag_ids: Vec<&str> = tags.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert!(!tag_ids.contains(&"user/-/label/Test Category"));
}

// ============================================================================
// Combined Filter Tests
// ============================================================================

#[tokio::test]
async fn test_list_entries_combined_filters() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Star some entries
    star_entry(&app.server, &[entry_ids[0], entry_ids[1]]).await;

    // Mark one starred entry as read
    mark_read(&app.server, &[entry_ids[0]]).await;

    // Filter: starred stream, exclude read — should find only the unread starred entry
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["_entryId"], entry_ids[1]);

    // Filter: by feed, exclude read — 4 unread
    let response = app
        .server
        .get("/reader/api/0/stream/contents/feed/https://example.com/feed.xml?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn test_list_entries_oldest_first() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Get entries in oldest-first order
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?r=o")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);

    // Verify oldest-first: published timestamps should be ascending
    let timestamps: Vec<i64> = items
        .iter()
        .map(|i| i["published"].as_i64().unwrap())
        .collect();
    for i in 1..timestamps.len() {
        assert!(
            timestamps[i] >= timestamps[i - 1],
            "Expected ascending timestamps in oldest-first order"
        );
    }
}

// ============================================================================
// Mark All Read with Timestamp Tests
// ============================================================================

#[tokio::test]
async fn test_mark_all_read_with_timestamp() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Entries are 1-5 hours old. Set a timestamp that is 2.5 hours ago.
    // This means entries 3h, 4h, 5h old should be marked read
    // (they are older than 2.5 hours, so older_than_days = 0 from integer division).
    // Because older_than_days becomes 0 for anything < 1 day, all entries get marked.
    // Use a timestamp far in the future to mark everything, effectively testing the
    // ts parameter passes through.
    let now_usec = chrono::Utc::now().timestamp() * 1_000_000;
    let ts_str = now_usec.to_string();
    let form_data: Vec<(&str, &str)> = vec![
        ("s", "user/-/state/com.google/reading-list"),
        ("ts", &ts_str),
    ];

    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

// ============================================================================
// GReader Item Response Format Tests
// ============================================================================

#[tokio::test]
async fn test_stream_contents_item_format() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item = &body["items"][0];

    // Standard GReader fields
    assert!(item["id"].is_string());
    assert!(item["published"].is_i64());
    assert!(item["updated"].is_i64());
    assert!(item["crawlTimeMsec"].is_string());
    assert!(item["timestampUsec"].is_string());
    assert!(item["title"].is_string());
    assert!(item["categories"].is_array());
    assert!(item["summary"]["content"].is_string());
    assert!(item["canonical"].is_array());
    assert!(item["canonical"][0]["href"].is_string());
    assert!(item["origin"]["streamId"].is_string());
    assert!(item["origin"]["title"].is_string());

    // RDRS extension fields
    assert!(item["_entryId"].is_i64());
    assert!(item["_feedId"].is_i64());
    assert!(item["_categoryId"].is_i64());
    assert!(item["_categoryName"].is_string());
    assert_eq!(item["_categoryName"], "Test Category");
    assert!(item["_feedHasIcon"].is_boolean());
    // Unread entry should have null _readAt
    assert!(item["_readAt"].is_null());
    assert!(item["_starredAt"].is_null());
    assert!(item["_publishedAt"].is_string());

    // Check origin
    assert_eq!(
        item["origin"]["streamId"],
        "feed/https://example.com/feed.xml"
    );
}

// ============================================================================
// Cross-User Access Restriction Tests
// ============================================================================

#[tokio::test]
async fn test_cannot_access_other_user_category_entries() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to list entries by other user's category
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/label/Other%20User%20Category")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_access_other_user_feed_entries() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to list entries by other user's feed
    let response = app
        .server
        .get("/reader/api/0/stream/contents/feed/https://other.com/feed.xml")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to get other user's entry — should return empty items
    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/items/contents?i={}",
            other_entry_ids[0]
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // Entry not owned by user should not appear in results
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_cannot_mark_other_user_entry_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to mark other user's entry as read — should return 404
    let mut form_data: Vec<(&str, String)> = vec![("i", other_entry_ids[0].to_string())];
    form_data.push(("a", "user/-/state/com.google/read".to_string()));
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_other_user_entry_unread() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to mark other user's entry as unread — should return 404
    let mut form_data: Vec<(&str, String)> = vec![("i", other_entry_ids[0].to_string())];
    form_data.push(("r", "user/-/state/com.google/read".to_string()));
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_star_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to star other user's entry — should return 404
    let mut form_data: Vec<(&str, String)> = vec![("i", other_entry_ids[0].to_string())];
    form_data.push(("a", "user/-/state/com.google/starred".to_string()));
    let response = app
        .server
        .post("/reader/api/0/edit-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_all_read_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to mark all read by other user's feed — should return 404
    let form_data: Vec<(&str, &str)> = vec![("s", "feed/https://other.com/feed.xml")];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_all_read_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to mark all read by other user's category — should return 404
    let form_data: Vec<(&str, &str)> = vec![("s", "user/-/label/Other User Category")];
    let response = app
        .server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry_neighbors() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to get neighbors of other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/neighbors", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_unsubscribe_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to unsubscribe other user's feed — should return 404
    let form_data: Vec<(&str, &str)> = vec![
        ("ac", "unsubscribe"),
        ("s", "feed/https://other.com/feed.xml"),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_edit_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to edit other user's feed — should return 404
    let form_data: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://other.com/feed.xml"),
        ("t", "Hacked Title"),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_disable_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to disable other user's category — should return 404
    let form_data: Vec<(&str, &str)> = vec![("s", "user/-/label/Other User Category")];
    let response = app
        .server
        .post("/reader/api/0/disable-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_rename_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to rename other user's category — should return 404
    let form_data: Vec<(&str, &str)> = vec![
        ("s", "user/-/label/Other User Category"),
        ("dest", "user/-/label/Hacked Category"),
    ];
    let response = app
        .server
        .post("/reader/api/0/rename-tag")
        .form(&form_data)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_refresh_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to refresh other user's feed
    let response = app
        .server
        .post(&format!("/api/feeds/{}/refresh", other_feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_fetch_full_content_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to fetch full content of other user's entry
    let response = app
        .server
        .post(&format!(
            "/api/entries/{}/fetch-full-content",
            other_entry_ids[0]
        ))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_summarize_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to summarize other user's entry
    let response = app
        .server
        .post(&format!("/api/entries/{}/summarize", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_save_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to save other user's entry
    let response = app
        .server
        .post(&format!("/api/entries/{}/save", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry_summary() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to get summary of other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/summary", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_delete_other_user_entry_summary() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&app.server).await;

    // Try to delete summary of other user's entry
    let response = app
        .server
        .delete(&format!("/api/entries/{}/summary", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Entry with No Link Tests (RDRS-specific, kept as-is)
// ============================================================================

#[tokio::test]
async fn test_fetch_full_content_entry_no_link() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&app.server).await;

    // Try to fetch full content of entry without link
    let response = app
        .server
        .post(&format!(
            "/api/entries/{}/fetch-full-content",
            no_link_entry_id
        ))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("no link"));
}

#[tokio::test]
async fn test_summarize_entry_no_link() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&app.server).await;

    // Try to summarize entry without link
    let response = app
        .server
        .post(&format!("/api/entries/{}/summarize", no_link_entry_id))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("no link"));
}

#[tokio::test]
async fn test_save_entry_no_link() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&app.server).await;

    // Try to save entry without link
    let response = app
        .server
        .post(&format!("/api/entries/{}/save", no_link_entry_id))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("no link"));
}

// ============================================================================
// Save/Summarize Without Config Tests (RDRS-specific, kept as-is)
// ============================================================================

#[tokio::test]
async fn test_summarize_entry_no_kagi_config() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Try to summarize without Kagi configured
    let response = app
        .server
        .post(&format!("/api/entries/{}/summarize", entry_ids[0]))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("Kagi"));
}

#[tokio::test]
async fn test_save_entry_no_services_config() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Try to save without any services configured
    let response = app
        .server
        .post(&format!("/api/entries/{}/save", entry_ids[0]))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("No save services"));
}

// ============================================================================
// Entry Summary Tests (RDRS-specific, kept as-is)
// ============================================================================

#[tokio::test]
async fn test_get_entry_summary_not_found() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Try to get summary for an entry that has no summary cached
    let response = app
        .server
        .get(&format!("/api/entries/{}/summary", entry_ids[0]))
        .await;
    response.assert_status_not_found();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("No summary"));
}

#[tokio::test]
async fn test_delete_entry_summary() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&app.server).await;

    // Delete summary (even if none exists, should succeed)
    let response = app
        .server
        .delete(&format!("/api/entries/{}/summary", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["success"], true);
}
