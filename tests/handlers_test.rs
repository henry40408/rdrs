//! Integration tests for Category, Feed, Entry handlers
//!
//! This test file covers:
//! - handlers/category.rs (CRUD operations)
//! - handlers/feed.rs (CRUD operations, OPML import/export)
//! - handlers/entry.rs (listing, reading, marking read/unread/starred)
//! - handlers/user.rs (settings management)
//! - handlers/pages.rs (page rendering)

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool, Role};
use rusqlite::Connection;
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: DbPool,
}

fn create_test_server(config: Config) -> TestServer {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let (db, _handle) = DbPool::new(conn);
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    let app = create_router(state);
    TestServer::builder().save_cookies().build(app).unwrap()
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

/// Helper to register and login a user
async fn setup_authenticated_user(server: &TestServer) {
    server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();
}

/// Helper to create a category
async fn create_category(server: &TestServer, name: &str) -> i64 {
    let response = server
        .post("/api/categories")
        .json(&json!({ "name": name }))
        .await;
    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    body["id"].as_i64().unwrap()
}

// ============================================================================
// Category Handler Tests
// ============================================================================

#[tokio::test]
async fn test_create_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/categories")
        .json(&json!({ "name": "Tech News" }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "Tech News");
    assert!(body["id"].as_i64().is_some());
}

#[tokio::test]
async fn test_create_category_empty_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/categories")
        .json(&json!({ "name": "" }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_create_category_name_too_long() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let long_name = "a".repeat(101);
    let response = server
        .post("/api/categories")
        .json(&json!({ "name": long_name }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_list_categories() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Create some categories
    create_category(&server, "Tech").await;
    create_category(&server, "News").await;
    create_category(&server, "Sports").await;

    let response = server.get("/api/categories").await;
    response.assert_status_ok();

    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 3);
}

#[tokio::test]
async fn test_list_categories_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/categories").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Test Category").await;

    let response = server.get(&format!("/api/categories/{}", cat_id)).await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "Test Category");
    assert_eq!(body["id"], cat_id);
}

#[tokio::test]
async fn test_get_category_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/categories/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_update_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Old Name").await;

    let response = server
        .put(&format!("/api/categories/{}", cat_id))
        .json(&json!({ "name": "New Name" }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "New Name");
}

#[tokio::test]
async fn test_update_category_empty_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Test").await;

    let response = server
        .put(&format!("/api/categories/{}", cat_id))
        .json(&json!({ "name": "  " }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_delete_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "To Delete").await;

    let response = server.delete(&format!("/api/categories/{}", cat_id)).await;
    response.assert_status(StatusCode::NO_CONTENT);

    // Verify it's gone
    let response = server.get(&format!("/api/categories/{}", cat_id)).await;
    response.assert_status_not_found();
}

// ============================================================================
// Feed Handler Tests
// ============================================================================

#[tokio::test]
async fn test_list_feeds_empty() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/feeds").await;
    response.assert_status_ok();

    let body: Vec<serde_json::Value> = response.json();
    assert!(body.is_empty());
}

#[tokio::test]
async fn test_list_feeds_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/feeds").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_feed_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/feeds/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_update_feed_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Create a category first
    let cat_id = create_category(&server, "Test").await;

    let response = server
        .put("/api/feeds/9999")
        .json(&json!({
            "category_id": cat_id,
            "url": "https://example.com/feed.xml",
            "title": "Test Feed"
        }))
        .await;

    response.assert_status_not_found();
}

#[tokio::test]
async fn test_delete_feed_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.delete("/api/feeds/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_feed_icon_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/feeds/9999/icon").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_export_opml_empty() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/opml/export").await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("<?xml"));
    assert!(body.contains("opml"));
}

#[tokio::test]
async fn test_import_opml_valid() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test Subscriptions</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Example Feed" title="Example Feed"
               xmlUrl="https://example.com/feed.xml" htmlUrl="https://example.com"/>
    </outline>
  </body>
</opml>"#;

    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["categories_created"], 1);
    assert_eq!(body["feeds_created"], 1);
    assert_eq!(body["feeds_skipped"], 0);
}

#[tokio::test]
async fn test_import_opml_invalid() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": "not valid xml" }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_import_opml_duplicate_feeds_skipped() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Feed" title="Feed"
               xmlUrl="https://example.com/feed.xml"/>
    </outline>
  </body>
</opml>"#;

    // First import
    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await;
    response.assert_status_ok();

    // Second import - should skip the duplicate
    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["categories_created"], 0);
    assert_eq!(body["feeds_created"], 0);
    assert_eq!(body["feeds_skipped"], 1);
}

// ============================================================================
// Entry Handler Tests
// ============================================================================

#[tokio::test]
async fn test_list_entries_empty() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
    assert_eq!(body["total"], 0);
}

#[tokio::test]
async fn test_list_entries_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/entries").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_list_entries_with_pagination() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries?limit=10&offset=0").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["limit"], 10);
    assert_eq!(body["offset"], 0);
}

#[tokio::test]
async fn test_list_entries_with_filters() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Test unread_only filter
    let response = server.get("/api/entries?unread_only=true").await;
    response.assert_status_ok();

    // Test starred_only filter
    let response = server.get("/api/entries?starred_only=true").await;
    response.assert_status_ok();

    // Test search filter
    let response = server.get("/api/entries?search=test").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_list_entries_invalid_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries?category_id=9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_list_entries_invalid_feed() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries?feed_id=9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_entry_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_entry_read_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.put("/api/entries/9999/read").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_entry_unread_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.put("/api/entries/9999/unread").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_toggle_entry_star_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.put("/api/entries/9999/star").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_entry_neighbors_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries/9999/neighbors").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_fetch_full_content_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.post("/api/entries/9999/fetch-full-content").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_summarize_entry_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.post("/api/entries/9999/summarize").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_save_to_services_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.post("/api/entries/9999/save").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_list_feed_entries_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/feeds/9999/entries").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_refresh_feed_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.post("/api/feeds/9999/refresh").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_unread_stats() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/entries/unread-stats").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["by_feed"].is_object());
    assert!(body["by_category"].is_object());
}

#[tokio::test]
async fn test_mark_all_read_all() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/entries/mark-all-read")
        .json(&json!({}))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert!(body["marked_count"].is_i64());
}

#[tokio::test]
async fn test_mark_all_read_by_category_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "category_id": 9999 }))
        .await;

    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_all_read_by_feed_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "feed_id": 9999 }))
        .await;

    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_all_read_older_than_days() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/entries/mark-all-read")
        .json(&json!({ "older_than_days": 7 }))
        .await;

    response.assert_status_ok();
}

// ============================================================================
// User Settings Handler Tests
// ============================================================================

#[tokio::test]
async fn test_update_user_settings() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/user/settings")
        .json(&json!({ "entries_per_page": 25 }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["entries_per_page"], 25);
}

#[tokio::test]
async fn test_get_linkding_settings() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/user/settings/linkding").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["configured"], false);
}

#[tokio::test]
async fn test_update_linkding_settings() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/user/settings/linkding")
        .json(&json!({
            "api_url": "https://linkding.example.com",
            "api_token": "secret-token"
        }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["configured"], true);
    assert_eq!(body["api_url"], "https://linkding.example.com");
}

#[tokio::test]
async fn test_update_linkding_settings_clear() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // First configure
    server
        .put("/api/user/settings/linkding")
        .json(&json!({
            "api_url": "https://linkding.example.com",
            "api_token": "secret-token"
        }))
        .await
        .assert_status_ok();

    // Then clear
    let response = server
        .put("/api/user/settings/linkding")
        .json(&json!({
            "api_url": "",
            "api_token": ""
        }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["configured"], false);
}

#[tokio::test]
async fn test_get_kagi_settings() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/user/settings/kagi").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["configured"], false);
}

#[tokio::test]
async fn test_update_kagi_settings() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/user/settings/kagi")
        .json(&json!({
            "session_link": "https://kagi.com/summarizer/index.html?token=abc123",
            "language": "EN"
        }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["configured"], true);
    assert_eq!(body["language"], "EN");
}

#[tokio::test]
async fn test_update_kagi_settings_invalid_session_link() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/user/settings/kagi")
        .json(&json!({
            "session_link": "not-a-valid-url"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_update_kagi_settings_no_token() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/user/settings/kagi")
        .json(&json!({
            "session_link": "https://kagi.com/summarizer/index.html"
        }))
        .await;

    response.assert_status_bad_request();
}

// ============================================================================
// Page Handler Tests
// ============================================================================

#[tokio::test]
async fn test_categories_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Categories") || body.contains("categories"));
}

#[tokio::test]
async fn test_categories_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/categories").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_feeds_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Feeds") || body.contains("feeds"));
}

#[tokio::test]
async fn test_feeds_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/feeds").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_entries_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/entries").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_entries_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/entries").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_entry_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Entry page should render even for non-existent entries
    // (the frontend handles the 404 when fetching entry data)
    let response = server.get("/entries/1").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_entry_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/entries/1").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_user_settings_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Settings") || body.contains("settings"));
}

#[tokio::test]
async fn test_user_settings_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/user-settings").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_settings_page() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();
    // Should show version information
    assert!(body.contains("Version") || body.contains("version"));
}

#[tokio::test]
async fn test_settings_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/settings").await;
    response.assert_status_see_other();
}

// ============================================================================
// Cross-User Isolation Tests
// ============================================================================

#[tokio::test]
async fn test_category_isolation_between_users() {
    let server = create_test_server(default_test_config());

    // User 1 creates a category
    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let user1_cat_id = create_category(&server, "User1 Category").await;

    // Logout
    server.delete("/api/session").await.assert_status_ok();

    // User 2 tries to access User 1's category
    server
        .post("/api/register")
        .json(&json!({
            "username": "user2",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "user2",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // Should not find user1's category
    let response = server
        .get(&format!("/api/categories/{}", user1_cat_id))
        .await;
    response.assert_status_not_found();

    // User2's category list should be empty
    let response = server.get("/api/categories").await;
    response.assert_status_ok();
    let body: Vec<serde_json::Value> = response.json();
    assert!(body.is_empty());
}

#[tokio::test]
async fn test_entries_filter_by_valid_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Create a category
    let cat_id = create_category(&server, "Test Category").await;

    // Filter entries by this category
    let response = server
        .get(&format!("/api/entries?category_id={}", cat_id))
        .await;
    response.assert_status_ok();
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[tokio::test]
async fn test_create_category_with_whitespace_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Name with leading/trailing whitespace should be trimmed
    let response = server
        .post("/api/categories")
        .json(&json!({ "name": "  Trimmed Name  " }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "Trimmed Name");
}

#[tokio::test]
async fn test_update_category_with_whitespace_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Original").await;

    let response = server
        .put(&format!("/api/categories/{}", cat_id))
        .json(&json!({ "name": "  Updated  " }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["name"], "Updated");
}

// ============================================================================
// Additional OPML Tests
// ============================================================================

#[tokio::test]
async fn test_export_opml_with_feeds() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Import feeds to create data
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="ExportTestCategory">
            <outline type="rss" text="Export Test Feed" xmlUrl="https://export.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await
        .assert_status_ok();

    let response = server.get("/api/opml/export").await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("ExportTestCategory"));
    assert!(body.contains("export.example.com"));
}

#[tokio::test]
async fn test_export_opml_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/opml/export").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_import_opml_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": "<opml></opml>" }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_import_opml_multiple_categories() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <head><title>Subscriptions</title></head>
    <body>
        <outline text="MultiTech" title="MultiTech">
            <outline type="rss" text="Feed 1" xmlUrl="https://multi.example.com/feed1.xml"/>
        </outline>
        <outline text="MultiNews" title="MultiNews">
            <outline type="rss" text="Feed 2" xmlUrl="https://multi.example.com/feed2.xml"/>
            <outline type="rss" text="Feed 3" xmlUrl="https://multi.example.com/feed3.xml"/>
        </outline>
    </body>
</opml>"#;

    let response = server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["categories_created"], 2);
    assert_eq!(body["feeds_created"], 3);
}

// ============================================================================
// Additional Feed Icon Tests
// ============================================================================

#[tokio::test]
async fn test_get_feed_icon_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/feeds/1/icon").await;
    response.assert_status_unauthorized();
}

// ============================================================================
// Move Feed Between Categories Tests
// ============================================================================

#[tokio::test]
async fn test_move_feed_to_different_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Import a feed to create it
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="MoveCategory1">
            <outline type="rss" text="Move Test Feed" xmlUrl="https://move.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await
        .assert_status_ok();

    // Get feeds to find the feed ID
    let response = server.get("/api/feeds").await;
    let feeds: Vec<serde_json::Value> = response.json();
    let feed_id = feeds[0]["id"].as_i64().unwrap();
    let original_cat_id = feeds[0]["category_id"].as_i64().unwrap();

    // Create new category
    let new_cat_id = create_category(&server, "MoveNewCategory").await;

    // Update feed to move to new category
    let response = server
        .put(&format!("/api/feeds/{}", feed_id))
        .json(&json!({
            "category_id": new_cat_id,
            "url": "https://move.example.com/feed.xml",
            "title": "Move Test Feed"
        }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["category_id"], new_cat_id);
    assert_ne!(body["category_id"], original_cat_id);
}

#[tokio::test]
async fn test_update_feed_to_other_user_category() {
    let server = create_test_server(default_test_config());

    // User 1 creates a category
    server
        .post("/api/register")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let user1_cat_id = create_category(&server, "MoveFeedUser1 Category").await;

    // Logout user1
    server.delete("/api/session").await.assert_status_ok();

    // User 2 creates a category
    server
        .post("/api/register")
        .json(&json!({
            "username": "movefeeduser2",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser2",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let _user2_cat_id = create_category(&server, "MoveFeedUser2 Category").await;

    // Import a feed for user2
    server
        .post("/api/opml/import")
        .json(&json!({ "content": r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="MoveFeedUser2 Category">
            <outline type="rss" text="User2 Move Feed" xmlUrl="https://movefeeduser2.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"# }))
        .await
        .assert_status_ok();

    // Get user2's feed ID
    let response = server.get("/api/feeds").await;
    let feeds: Vec<serde_json::Value> = response.json();
    let feed_id = feeds[0]["id"].as_i64().unwrap();

    // User 2 tries to move their feed to User 1's category - should fail
    let response = server
        .put(&format!("/api/feeds/{}", feed_id))
        .json(&json!({
            "category_id": user1_cat_id,
            "url": "https://movefeeduser2.example.com/feed.xml",
            "title": "User2 Move Feed"
        }))
        .await;

    response.assert_status_not_found();
}

// ============================================================================
// Fetch Metadata Tests
// ============================================================================

#[tokio::test]
async fn test_fetch_metadata_empty_url() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/feeds/fetch-metadata")
        .json(&json!({ "url": "" }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_fetch_metadata_whitespace_url() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/feeds/fetch-metadata")
        .json(&json!({ "url": "   " }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_fetch_metadata_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/feeds/fetch-metadata")
        .json(&json!({ "url": "https://example.com" }))
        .await;

    response.assert_status_unauthorized();
}

// ============================================================================
// Create Feed Validation Tests
// ============================================================================

#[tokio::test]
async fn test_create_feed_empty_url() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Test").await;

    let response = server
        .post("/api/feeds")
        .json(&json!({
            "url": "",
            "category_id": cat_id
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_create_feed_whitespace_url() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let cat_id = create_category(&server, "Test").await;

    let response = server
        .post("/api/feeds")
        .json(&json!({
            "url": "   ",
            "category_id": cat_id
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_create_feed_invalid_category() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .post("/api/feeds")
        .json(&json!({
            "url": "https://example.com/feed.xml",
            "category_id": 9999
        }))
        .await;

    response.assert_status_not_found();
}

// ============================================================================
// Passkey Handler Tests
// ============================================================================

#[tokio::test]
async fn test_passkey_register_start_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.post("/api/passkey/register/start").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_passkey_register_start_authorized() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.post("/api/passkey/register/start").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["options"]["publicKey"]["challenge"].is_string());
    assert!(body["options"]["publicKey"]["user"]["name"].is_string());
}

#[tokio::test]
async fn test_passkey_register_finish_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/passkey/register/finish")
        .json(&json!({
            "name": "Test Passkey",
            "credential": {}
        }))
        .await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_passkey_auth_start_no_passkeys() {
    let server = create_test_server(default_test_config());

    let response = server.post("/api/passkey/auth/start").await;
    response.assert_status_unauthorized();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("No passkeys"));
}

#[tokio::test]
async fn test_passkey_auth_finish_no_challenge() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/passkey/auth/finish")
        .json(&json!({
            "credential": {
                "id": "dGVzdA",
                "rawId": "dGVzdA",
                "type": "public-key",
                "response": {
                    "authenticatorData": "dGVzdA",
                    "clientDataJSON": "dGVzdA",
                    "signature": "dGVzdA"
                }
            }
        }))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("Challenge"));
}

#[tokio::test]
async fn test_list_passkeys_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/passkeys").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_list_passkeys_empty() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.get("/api/passkeys").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_rename_passkey_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server
        .put("/api/passkeys/1")
        .json(&json!({ "name": "New Name" }))
        .await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_rename_passkey_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/passkeys/9999")
        .json(&json!({ "name": "New Name" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_rename_passkey_empty_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server
        .put("/api/passkeys/1")
        .json(&json!({ "name": "" }))
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_delete_passkey_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.delete("/api/passkeys/1").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_delete_passkey_not_found() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    let response = server.delete("/api/passkeys/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_passkey_auth_start_with_invalid_passkey_data() {
    let app = create_test_app(default_test_config());

    // Create user and passkey with invalid public_key JSON
    app.db
        .user(move |conn| {
            let password_hash = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["testuser", password_hash, Role::User.as_str()],
            )
            .unwrap();
            let user_id = conn.last_insert_rowid();

            // Insert passkey with invalid JSON in public_key
            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![user_id, vec![1u8, 2, 3], b"invalid json", 0, "Test Passkey"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    let response = app.server.post("/api/passkey/auth/start").await;
    response.assert_status_unauthorized();

    let body: serde_json::Value = response.json();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("No valid passkeys"));
}

#[tokio::test]
async fn test_passkey_register_finish_empty_name() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // First start registration to create a challenge
    server.post("/api/passkey/register/start").await;

    // Try to finish with empty name - this should fail validation before checking credential
    let response = server
        .post("/api/passkey/register/finish")
        .json(&json!({
            "name": "",
            "credential": {
                "id": "dGVzdA",
                "rawId": "dGVzdA",
                "type": "public-key",
                "response": {
                    "attestationObject": "dGVzdA",
                    "clientDataJSON": "dGVzdA"
                }
            }
        }))
        .await;

    response.assert_status_bad_request();
    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("name"));
}

#[tokio::test]
async fn test_passkey_register_finish_no_challenge() {
    let server = create_test_server(default_test_config());
    setup_authenticated_user(&server).await;

    // Try to finish registration without starting (no challenge exists)
    let response = server
        .post("/api/passkey/register/finish")
        .json(&json!({
            "name": "Test Passkey",
            "credential": {
                "id": "dGVzdA",
                "rawId": "dGVzdA",
                "type": "public-key",
                "response": {
                    "attestationObject": "dGVzdA",
                    "clientDataJSON": "dGVzdA"
                }
            }
        }))
        .await;

    response.assert_status_bad_request();
    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("Challenge"));
}

#[tokio::test]
async fn test_list_passkeys_with_data() {
    let app = create_test_app(default_test_config());

    // Create user and passkey
    app.db
        .user(move |conn| {
            let password_hash = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["testuser", password_hash, Role::User.as_str()],
            )
            .unwrap();
            let user_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![user_id, vec![1u8, 2, 3], b"{}", 5, "My Passkey", "usb,nfc"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    // Login
    app.server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = app.server.get("/api/passkeys").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let passkeys = body["passkeys"].as_array().unwrap();
    assert_eq!(passkeys.len(), 1);
    assert_eq!(passkeys[0]["name"], "My Passkey");
}

#[tokio::test]
async fn test_rename_passkey_success() {
    let app = create_test_app(default_test_config());

    // Create user and passkey
    let passkey_id: i64 = app
        .db
        .user(move |conn| {
            let password_hash = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["testuser", password_hash, Role::User.as_str()],
            )
            .unwrap();
            let user_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![user_id, vec![1u8, 2, 3], b"{}", 0, "Old Name"],
            )
            .unwrap();
            conn.last_insert_rowid()
        })
        .await
        .unwrap();

    // Login
    app.server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = app
        .server
        .put(&format!("/api/passkeys/{}", passkey_id))
        .json(&json!({ "name": "New Name" }))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // Verify rename
    let response = app.server.get("/api/passkeys").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"][0]["name"], "New Name");
}

#[tokio::test]
async fn test_delete_passkey_success() {
    let app = create_test_app(default_test_config());

    // Create user and passkey
    let passkey_id: i64 = app
        .db
        .user(move |conn| {
            let password_hash = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["testuser", password_hash, Role::User.as_str()],
            )
            .unwrap();
            let user_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![user_id, vec![1u8, 2, 3], b"{}", 0, "Test Passkey"],
            )
            .unwrap();
            conn.last_insert_rowid()
        })
        .await
        .unwrap();

    // Login
    app.server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = app
        .server
        .delete(&format!("/api/passkeys/{}", passkey_id))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    // Verify deletion
    let response = app.server.get("/api/passkeys").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"].as_array().unwrap().len(), 0);
}

// ============================================================================
// Cross-User Passkey Isolation Tests
// ============================================================================

#[tokio::test]
async fn test_passkey_rename_other_user() {
    let app = create_test_app(default_test_config());

    // Create two users, each with a passkey
    let (passkey_id_user1,) = app
        .db
        .user(move |conn| {
            let hash1 = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["pkuser1", hash1, Role::User.as_str()],
            )
            .unwrap();
            let user1_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![user1_id, vec![1u8, 2, 3], b"{}", 0, "User1 Passkey"],
            )
            .unwrap();
            let pk_id = conn.last_insert_rowid();

            let hash2 = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["pkuser2", hash2, Role::User.as_str()],
            )
            .unwrap();

            (pk_id,)
        })
        .await
        .unwrap();

    // Login as user2
    app.server
        .post("/api/session")
        .json(&json!({
            "username": "pkuser2",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // User2 tries to rename User1's passkey → 404
    let response = app
        .server
        .put(&format!("/api/passkeys/{}", passkey_id_user1))
        .json(&json!({ "name": "Hacked Name" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_passkey_delete_other_user() {
    let app = create_test_app(default_test_config());

    // Create two users, user1 has a passkey
    let (passkey_id_user1,) = app
        .db
        .user(move |conn| {
            let hash1 = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["pkdeluser1", hash1, Role::User.as_str()],
            )
            .unwrap();
            let user1_id = conn.last_insert_rowid();

            conn.execute(
                "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![user1_id, vec![4u8, 5, 6], b"{}", 0, "User1 Key"],
            )
            .unwrap();
            let pk_id = conn.last_insert_rowid();

            let hash2 = auth::hash_password("password123").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["pkdeluser2", hash2, Role::User.as_str()],
            )
            .unwrap();

            (pk_id,)
        })
        .await
        .unwrap();

    // Login as user2
    app.server
        .post("/api/session")
        .json(&json!({
            "username": "pkdeluser2",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // User2 tries to delete User1's passkey → 404
    let response = app
        .server
        .delete(&format!("/api/passkeys/{}", passkey_id_user1))
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Feed Icon & Cross-User Feed Tests
// ============================================================================

#[tokio::test]
async fn test_get_feed_icon_no_icon() {
    let app = create_test_app(default_test_config());

    // Create user and a feed (via OPML import) that has no icon
    let hash = auth::hash_password("password123").unwrap();
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["iconuser", hash, Role::User.as_str()],
            )
            .unwrap();
        })
        .await
        .unwrap();

    app.server
        .post("/api/session")
        .json(&json!({
            "username": "iconuser",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // Import a feed
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="IconTestCat">
            <outline type="rss" text="No Icon Feed" xmlUrl="https://noicon.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;
    app.server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await
        .assert_status_ok();

    // Get feed ID
    let response = app.server.get("/api/feeds").await;
    let feeds: Vec<serde_json::Value> = response.json();
    let feed_id = feeds[0]["id"].as_i64().unwrap();

    // Request icon for a feed that exists but has no icon → 404
    let response = app
        .server
        .get(&format!("/api/feeds/{}/icon", feed_id))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_delete_feed_other_user() {
    let app = create_test_app(default_test_config());

    // User1 registers and imports a feed
    app.server
        .post("/api/register")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    app.server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="DelTestCat">
            <outline type="rss" text="Del Test Feed" xmlUrl="https://deltest.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;
    app.server
        .post("/api/opml/import")
        .json(&json!({ "content": opml_content }))
        .await
        .assert_status_ok();

    // Get user1's feed ID
    let response = app.server.get("/api/feeds").await;
    let feeds: Vec<serde_json::Value> = response.json();
    let feed_id = feeds[0]["id"].as_i64().unwrap();

    // Logout user1
    app.server.delete("/api/session").await.assert_status_ok();

    // User2 registers and logs in
    app.server
        .post("/api/register")
        .json(&json!({
            "username": "feeddeluser2",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    app.server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser2",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // User2 tries to delete User1's feed → 404
    let response = app.server.delete(&format!("/api/feeds/{}", feed_id)).await;
    response.assert_status_not_found();
}

// ============================================================================
// Favicon Handler Tests
// ============================================================================

#[tokio::test]
async fn test_favicon_ico() {
    let server = create_test_server(default_test_config());

    let response = server.get("/favicon.ico").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/x-icon");
}

#[tokio::test]
async fn test_favicon_svg() {
    let server = create_test_server(default_test_config());

    let response = server.get("/favicon.svg").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/svg+xml");
}

#[tokio::test]
async fn test_favicon_16() {
    let server = create_test_server(default_test_config());

    let response = server.get("/favicon-16x16.png").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/png");
}

#[tokio::test]
async fn test_favicon_32() {
    let server = create_test_server(default_test_config());

    let response = server.get("/favicon-32x32.png").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/png");
}

#[tokio::test]
async fn test_apple_touch_icon() {
    let server = create_test_server(default_test_config());

    let response = server.get("/apple-touch-icon.png").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/png");
}
