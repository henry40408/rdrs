//! Integration tests for Category, Feed, Entry handlers
//!
//! This test file covers:
//! - handlers/category.rs (CRUD operations)
//! - handlers/feed.rs (CRUD operations, OPML import/export)
//! - handlers/entry.rs (listing, reading, marking read/unread/starred)
//! - handlers/user.rs (settings management)
//! - handlers/pages.rs (page rendering)

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{create_router, db, AppState, Config};
use rusqlite::Connection;
use serde_json::json;

fn create_test_server(config: Config) -> TestServer {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let state = AppState {
        db: Arc::new(Mutex::new(conn)),
        config: Arc::new(config),
    };

    let app = create_router(state);
    TestServer::builder().save_cookies().build(app).unwrap()
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
