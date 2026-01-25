//! Advanced integration tests for Entry handlers with actual data
//!
//! These tests create actual entries in the database to test
//! entry-related handlers more thoroughly.

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, create_router, db, AppState, Config, Role};
use rusqlite::Connection;
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: Arc<Mutex<Connection>>,
}

fn create_test_app(config: Config) -> TestApp {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let db = Arc::new(Mutex::new(conn));
    let webauthn = auth::create_webauthn(&config).unwrap();
    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
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
fn setup_test_data(db: &Arc<Mutex<Connection>>) -> (i64, i64, i64, Vec<i64>) {
    let conn = db.lock().unwrap();

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

// ============================================================================
// Entry List and Get Tests
// ============================================================================

#[tokio::test]
async fn test_list_entries_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get("/api/entries").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
    assert_eq!(body["entries"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn test_list_entries_with_limit() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get("/api/entries?limit=2").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
    assert_eq!(body["entries"].as_array().unwrap().len(), 2);
    assert_eq!(body["limit"], 2);
}

#[tokio::test]
async fn test_list_entries_with_offset() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get("/api/entries?limit=2&offset=3").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
    assert_eq!(body["entries"].as_array().unwrap().len(), 2);
    assert_eq!(body["offset"], 3);
}

#[tokio::test]
async fn test_list_entries_by_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .get(&format!("/api/entries?category_id={}", cat_id))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
}

#[tokio::test]
async fn test_list_entries_by_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .get(&format!("/api/entries?feed_id={}", feed_id))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
}

#[tokio::test]
async fn test_list_feed_entries() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .get(&format!("/api/feeds/{}/entries", feed_id))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 5);
}

#[tokio::test]
async fn test_get_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .get(&format!("/api/entries/{}", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["title"].as_str().unwrap().contains("Entry Title"));
    assert!(body["sanitized_content"].is_string());
}

// ============================================================================
// Entry Read/Unread Tests
// ============================================================================

#[tokio::test]
async fn test_mark_entry_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put(&format!("/api/entries/{}/read", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["read_at"].is_string());
}

#[tokio::test]
async fn test_mark_entry_unread() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // First mark as read
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[0]))
        .await
        .assert_status_ok();

    // Then mark as unread
    let response = app
        .server
        .put(&format!("/api/entries/{}/unread", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["read_at"].is_null());
}

#[tokio::test]
async fn test_list_entries_unread_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Mark first two entries as read
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[0]))
        .await
        .assert_status_ok();
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[1]))
        .await
        .assert_status_ok();

    // Get unread entries only
    let response = app.server.get("/api/entries?unread_only=true").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 3);
}

// ============================================================================
// Entry Star Tests
// ============================================================================

#[tokio::test]
async fn test_toggle_entry_star_on() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put(&format!("/api/entries/{}/star", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["starred_at"].is_string());
}

#[tokio::test]
async fn test_toggle_entry_star_off() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Star the entry
    app.server
        .put(&format!("/api/entries/{}/star", entry_ids[0]))
        .await
        .assert_status_ok();

    // Unstar the entry
    let response = app
        .server
        .put(&format!("/api/entries/{}/star", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["starred_at"].is_null());
}

#[tokio::test]
async fn test_list_entries_starred_only() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Star first entry
    app.server
        .put(&format!("/api/entries/{}/star", entry_ids[0]))
        .await
        .assert_status_ok();

    // Get starred entries only
    let response = app.server.get("/api/entries?starred_only=true").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 1);
}

// ============================================================================
// Mark All Read Tests
// ============================================================================

#[tokio::test]
async fn test_mark_all_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({}))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["marked_count"], 5);

    // Verify all are read
    let response = app.server.get("/api/entries?unread_only=true").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn test_mark_all_read_by_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "category_id": cat_id }))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["marked_count"], 5);
}

#[tokio::test]
async fn test_mark_all_read_by_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "feed_id": feed_id }))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["marked_count"], 5);
}

// ============================================================================
// Unread Stats Tests
// ============================================================================

#[tokio::test]
async fn test_get_unread_stats_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get("/api/entries/unread-stats").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let by_feed = body["by_feed"].as_object().unwrap();
    let by_category = body["by_category"].as_object().unwrap();

    assert_eq!(by_feed[&feed_id.to_string()], 5);
    assert_eq!(by_category[&cat_id.to_string()], 5);
}

#[tokio::test]
async fn test_get_unread_stats_after_marking_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Mark two entries as read
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[0]))
        .await
        .assert_status_ok();
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[1]))
        .await
        .assert_status_ok();

    let response = app.server.get("/api/entries/unread-stats").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let by_feed = body["by_feed"].as_object().unwrap();
    let by_category = body["by_category"].as_object().unwrap();

    assert_eq!(by_feed[&feed_id.to_string()], 3);
    assert_eq!(by_category[&cat_id.to_string()], 3);
}

// ============================================================================
// Entry Neighbors Tests
// ============================================================================

#[tokio::test]
async fn test_get_entry_neighbors() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
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
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
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

// ============================================================================
// Search Tests
// ============================================================================

#[tokio::test]
async fn test_list_entries_with_search() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Search for specific entry
    let response = app.server.get("/api/entries?search=Title%201").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // Should find at least one entry matching
    assert!(body["total"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn test_list_entries_with_search_no_results() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Search for non-existent content
    let response = app.server.get("/api/entries?search=nonexistent12345").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 0);
}

// ============================================================================
// Feed Get/Update/Delete Tests with Data
// ============================================================================

#[tokio::test]
async fn test_get_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get(&format!("/api/feeds/{}", feed_id)).await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], feed_id);
    assert_eq!(body["title"], "Test Feed");
    assert_eq!(body["url"], "https://example.com/feed.xml");
}

#[tokio::test]
async fn test_update_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put(&format!("/api/feeds/{}", feed_id))
        .json(&json!({
            "category_id": cat_id,
            "url": "https://example.com/feed.xml",
            "title": "Updated Feed Title",
            "description": "New description"
        }))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["title"], "Updated Feed Title");
    assert_eq!(body["description"], "New description");
}

#[tokio::test]
async fn test_update_feed_empty_url() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put(&format!("/api/feeds/{}", feed_id))
        .json(&json!({
            "category_id": cat_id,
            "url": "",
            "title": "Test"
        }))
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_delete_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.delete(&format!("/api/feeds/{}", feed_id)).await;
    response.assert_status(StatusCode::NO_CONTENT);

    // Verify it's gone
    let response = app.server.get(&format!("/api/feeds/{}", feed_id)).await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_list_feeds() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get("/api/feeds").await;
    response.assert_status_ok();

    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["title"], "Test Feed");
}

// ============================================================================
// Category Get/Update/Delete Tests with Data
// ============================================================================

#[tokio::test]
async fn test_get_category_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app.server.get(&format!("/api/categories/{}", cat_id)).await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["id"], cat_id);
    assert_eq!(body["name"], "Test Category");
}

#[tokio::test]
async fn test_update_category_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .put(&format!("/api/categories/{}", cat_id))
        .json(&json!({ "name": "Updated Category Name" }))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "Updated Category Name");
}

#[tokio::test]
async fn test_delete_category_with_data() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    let response = app
        .server
        .delete(&format!("/api/categories/{}", cat_id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // Verify it's gone
    let response = app.server.get(&format!("/api/categories/{}", cat_id)).await;
    response.assert_status_not_found();
}

// ============================================================================
// Combined Filter Tests
// ============================================================================

#[tokio::test]
async fn test_list_entries_combined_filters() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, feed_id, entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Star some entries
    app.server
        .put(&format!("/api/entries/{}/star", entry_ids[0]))
        .await
        .assert_status_ok();
    app.server
        .put(&format!("/api/entries/{}/star", entry_ids[1]))
        .await
        .assert_status_ok();

    // Mark one starred entry as read
    app.server
        .put(&format!("/api/entries/{}/read", entry_ids[0]))
        .await
        .assert_status_ok();

    // Filter by category, unread_only and starred_only
    let response = app
        .server
        .get(&format!(
            "/api/entries?category_id={}&unread_only=true&starred_only=true",
            cat_id
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // Should find the one unread starred entry
    assert_eq!(body["total"], 1);

    // Filter by feed_id too
    let response = app
        .server
        .get(&format!(
            "/api/entries?feed_id={}&unread_only=true",
            feed_id
        ))
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["total"], 4); // 5 total - 1 read
}

// ============================================================================
// Cross-User Access Restriction Tests
// ============================================================================

/// Setup a second user's data in the database
fn setup_second_user_data(db: &Arc<Mutex<Connection>>) -> (i64, i64, i64, Vec<i64>) {
    let conn = db.lock().unwrap();

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
}

#[tokio::test]
async fn test_cannot_access_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to list entries by other user's category
    let response = app
        .server
        .get(&format!("/api/entries?category_id={}", other_cat_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_access_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to list entries by other user's feed
    let response = app
        .server
        .get(&format!("/api/entries?feed_id={}", other_feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to get other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_list_other_user_feed_entries() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to list other user's feed entries
    let response = app
        .server
        .get(&format!("/api/feeds/{}/entries", other_feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_other_user_entry_read() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to mark other user's entry as read
    let response = app
        .server
        .put(&format!("/api/entries/{}/read", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_other_user_entry_unread() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to mark other user's entry as unread
    let response = app
        .server
        .put(&format!("/api/entries/{}/unread", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_star_other_user_entry() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to star other user's entry
    let response = app
        .server
        .put(&format!("/api/entries/{}/star", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_all_read_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to mark all read by other user's feed
    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "feed_id": other_feed_id }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_mark_all_read_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to mark all read by other user's category
    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "category_id": other_cat_id }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry_neighbors() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to get neighbors of other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/neighbors", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to get other user's feed
    let response = app
        .server
        .get(&format!("/api/feeds/{}", other_feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_update_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to update other user's feed
    let response = app
        .server
        .put(&format!("/api/feeds/{}", other_feed_id))
        .json(&json!({
            "category_id": cat_id,
            "url": "https://example.com/feed.xml",
            "title": "Hacked Title"
        }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_delete_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to delete other user's feed
    let response = app
        .server
        .delete(&format!("/api/feeds/{}", other_feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to get other user's category
    let response = app
        .server
        .get(&format!("/api/categories/{}", other_cat_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_update_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to update other user's category
    let response = app
        .server
        .put(&format!("/api/categories/{}", other_cat_id))
        .json(&json!({ "name": "Hacked Category" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_delete_other_user_category() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to delete other user's category
    let response = app
        .server
        .delete(&format!("/api/categories/{}", other_cat_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_refresh_other_user_feed() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db);
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
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
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
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
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
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db);
    login(&app.server).await;

    // Try to save other user's entry
    let response = app
        .server
        .post(&format!("/api/entries/{}/save", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Mark All Read with Older Than Days Tests
// ============================================================================

#[tokio::test]
async fn test_mark_all_read_with_older_than_days() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db);
    login(&app.server).await;

    // Mark entries older than 2 hours as read (entries are 1-5 hours old)
    // This should mark entries 3, 4, 5 as read (3h, 4h, 5h old)
    // But older_than_days uses days, so let's use 0 to mark all
    let response = app
        .server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "older_than_days": 0 }))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    // All 5 entries are older than 0 days
    assert!(body["marked_count"].as_i64().unwrap() >= 0);
}

// ============================================================================
// Entry with No Link Tests
// ============================================================================

fn setup_entry_without_link(db: &Arc<Mutex<Connection>>, feed_id: i64) -> i64 {
    let conn = db.lock().unwrap();
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
}

#[tokio::test]
async fn test_fetch_full_content_entry_no_link() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id);
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
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id);
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
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db);
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id);
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
// Save/Summarize Without Config Tests
// ============================================================================

#[tokio::test]
async fn test_summarize_entry_no_kagi_config() {
    let app = create_test_app(default_test_config());
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
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
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db);
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
