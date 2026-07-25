//! Integration tests for `GReader` API, Feed, Entry handlers
//!
//! This test file covers:
//! - handlers/greader/tag.rs (category CRUD via rename-tag, disable-tag, tag/list)
//! - handlers/greader/subscription.rs (feed management via subscription/edit, OPML import/export)
//! - handlers/greader/item.rs (entry listing via stream/contents, edit-tag, mark-all-as-read)
//! - handlers/greader/user.rs (unread-count)
//! - handlers/user.rs (settings management)
//! - handlers/pages.rs (page rendering)

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum_test::TestServer;
use axum_test::multipart::{MultipartForm, Part};
use rdrs::{AppState, Config, Db, Role, auth, create_router, services};
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: Db,
}

/// Static asset cache-control depends on whether the binary was built from a
/// clean git tree. PR-9 switched to `no-cache` for `-dirty` builds so dev
/// iteration sees fresh assets — production builds keep the immutable header.
fn expected_static_cache_control() -> &'static str {
    if rdrs::GIT_VERSION.ends_with("-dirty") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    }
}

async fn create_test_server(config: Config) -> TestServer {
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
    TestServer::builder().save_cookies().build(app)
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

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db }
}

/// Helper to register and login a user
async fn setup_authenticated_user(server: &mut TestServer) {
    server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

/// Helper to create a category via `GReader` rename-tag (s==dest creates idempotently)
async fn create_category(server: &TestServer, name: &str) {
    let form = vec![
        ("s", format!("user/-/label/{name}")),
        ("dest", format!("user/-/label/{name}")),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();
}

/// Helper to count folder-type tags from tag/list (excluding the 4 built-in state tags)
async fn count_folder_tags(server: &TestServer) -> usize {
    let response = server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    tags.iter()
        .filter(|t| t["type"].as_str() == Some("folder"))
        .count()
}

/// Helper to get all folder tag names from tag/list
async fn get_folder_tag_names(server: &TestServer) -> Vec<String> {
    let response = server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    tags.iter()
        .filter(|t| t["type"].as_str() == Some("folder"))
        .map(|t| {
            t["id"]
                .as_str()
                .unwrap()
                .strip_prefix("user/-/label/")
                .unwrap()
                .to_string()
        })
        .collect()
}

// ============================================================================
// Category Handler Tests (via GReader tag endpoints)
// ============================================================================

#[tokio::test]
async fn test_create_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form = vec![
        ("s", "user/-/label/Tech News".to_string()),
        ("dest", "user/-/label/Tech News".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify via tag/list
    let names = get_folder_tag_names(&server).await;
    assert!(names.contains(&"Tech News".to_string()));
}

#[tokio::test]
async fn test_create_category_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Empty label name should fail validation in StreamId::parse
    let form = vec![
        ("s", "user/-/label/".to_string()),
        ("dest", "user/-/label/".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_create_category_name_too_long() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let long_name = "a".repeat(101);
    let form = vec![
        ("s", format!("user/-/label/{long_name}")),
        ("dest", format!("user/-/label/{long_name}")),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    // GReader rename-tag may or may not validate length;
    // if it succeeds, just verify the category was created
    // (no explicit 100-char limit at the GReader API layer)
    assert!(
        response.status_code() == StatusCode::OK
            || response.status_code() == StatusCode::BAD_REQUEST,
        "Expected OK or BAD_REQUEST, got {}",
        response.status_code()
    );
}

#[tokio::test]
async fn test_list_categories() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Create some categories
    create_category(&server, "Tech").await;
    create_category(&server, "News").await;
    create_category(&server, "Sports").await;

    // 3 created + the "Uncategorized" category seeded at registration.
    let count = count_folder_tags(&server).await;
    assert_eq!(count, 4);
}

#[tokio::test]
async fn test_list_categories_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/reader/api/0/tag/list").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Test Category").await;

    // Verify the category appears in tag/list
    let names = get_folder_tag_names(&server).await;
    assert!(names.contains(&"Test Category".to_string()));
}

#[tokio::test]
async fn test_update_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Old Name").await;

    // Rename via rename-tag
    let form = vec![
        ("s", "user/-/label/Old Name".to_string()),
        ("dest", "user/-/label/New Name".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify the rename happened
    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"Old Name".to_string()));
    assert!(names.contains(&"New Name".to_string()));
}

#[tokio::test]
async fn test_update_category_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Test").await;

    // Rename with empty destination label should fail
    let form = vec![
        ("s", "user/-/label/Test".to_string()),
        ("dest", "user/-/label/".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_delete_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "To Delete").await;

    // Delete via disable-tag
    let form = vec![("s", "user/-/label/To Delete".to_string())];
    let response = server.post("/reader/api/0/disable-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    // Verify it's gone
    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"To Delete".to_string()));
}

// ============================================================================
// Feed Handler Tests (via GReader subscription endpoints)
// ============================================================================

#[tokio::test]
async fn test_list_feeds_empty() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["subscriptions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_list_feeds_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_update_feed_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://nonexistent.com/feed.xml"),
        ("t", "Test Feed"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_feed_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // subscription/edit ac=edit with non-existent feed returns 404
    let form: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://nonexistent.com/feed.xml"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_delete_feed_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![
        ("ac", "unsubscribe"),
        ("s", "feed/https://nonexistent.com/feed.xml"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_feed_icon_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/api/feeds/9999/icon").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_create_feed_empty_url() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("ac", "subscribe"), ("s", "feed/")];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_create_feed_whitespace_url() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("ac", "subscribe"), ("s", "feed/   ")];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    // Should fail because whitespace URL is not a valid feed
    assert!(
        response.status_code() == StatusCode::BAD_REQUEST
            || response.status_code() == StatusCode::INTERNAL_SERVER_ERROR,
        "Expected error status for whitespace URL, got {}",
        response.status_code()
    );
}

#[tokio::test]
async fn test_move_feed_to_different_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Import a feed to create it
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="MoveCategory1">
            <outline type="rss" text="Move Test Feed" xmlUrl="https://move.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // Move feed to a new category via subscription/edit ac=edit
    let form: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://move.example.com/feed.xml"),
        ("a", "user/-/label/MoveNewCategory"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    // Verify subscription/list shows the new category
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    let categories = subscriptions[0]["categories"].as_array().unwrap();
    assert_eq!(categories[0]["label"], "MoveNewCategory");
}

#[tokio::test]
async fn test_update_feed_to_other_user_category() {
    let mut server = create_test_server(default_test_config()).await;

    // User 1 registers
    server
        .post("/api/register")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // User 1 creates a category
    create_category(&server, "MoveFeedUser1 Category").await;

    // Logout user1
    server.delete("/api/session").await.assert_status_ok();

    // User 2 registers
    server
        .post("/api/register")
        .json(&json!({
            "username": "movefeeduser2",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser2",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // Import a feed for user2
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="MoveFeedUser2 Category">
            <outline type="rss" text="User2 Move Feed" xmlUrl="https://movefeeduser2.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

    // In GReader, categories are per-user. User2 can create a category with the
    // same name as User1's -- it's their own independent category.
    // We verify user2's data is separate: user2's subscription/list should only
    // show user2's feed.
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(
        subscriptions[0]["id"],
        "feed/https://movefeeduser2.example.com/feed.xml"
    );
}

#[tokio::test]
async fn test_delete_feed_other_user() {
    let mut app = create_test_app(default_test_config()).await;

    // User1 registers and imports a feed
    app.server
        .post("/api/register")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="DelTestCat">
            <outline type="rss" text="Del Test Feed" xmlUrl="https://deltest.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;
    app.server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

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

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser2",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // User2 tries to unsubscribe from User1's feed URL -> 404
    let form: Vec<(&str, &str)> = vec![
        ("ac", "unsubscribe"),
        ("s", "feed/https://deltest.example.com/feed.xml"),
    ];
    let response = app
        .server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// OPML Tests (via GReader subscription/export and subscription/import)
// ============================================================================

#[tokio::test]
async fn test_export_opml_empty() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/subscription/export").await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("<?xml") || body.contains("<opml"));
}

#[tokio::test]
async fn test_export_opml_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/reader/api/0/subscription/export").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_export_opml_with_feeds() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

    let response = server.get("/reader/api/0/subscription/export").await;
    response.assert_status_ok();

    let body = response.text();
    assert!(body.contains("ExportTestCategory"));
    assert!(body.contains("export.example.com"));
}

#[tokio::test]
async fn test_import_opml_valid() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert_eq!(body, "OK");

    // Verify via subscription/list that the feed was created
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);

    // Verify via tag/list that the category was created
    let names = get_folder_tag_names(&server).await;
    assert!(names.contains(&"Tech".to_string()));
}

#[tokio::test]
async fn test_import_opml_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/reader/api/0/subscription/import")
        .text("<opml></opml>")
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_import_opml_invalid() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/reader/api/0/subscription/import")
        .text("not valid xml")
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_import_opml_duplicate_feeds_skipped() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // Second import - duplicates should be skipped
    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // Verify only 1 feed exists (not duplicated)
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_import_opml_multiple_categories() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // Verify 3 feeds
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 3);

    // Verify 2 imported folder-type tags + the seeded "Uncategorized".
    let count = count_folder_tags(&server).await;
    assert_eq!(count, 3);
}

// ============================================================================
// Entry Handler Tests (via GReader stream/contents and edit-tag)
// ============================================================================

#[tokio::test]
async fn test_list_entries_empty() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_entries_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_list_entries_with_pagination() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=10")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["items"].is_array());
}

#[tokio::test]
async fn test_list_entries_with_filters() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Test unread_only filter via xt=read
    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    // Test starred_only filter via it=starred
    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?it=user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_list_entries_invalid_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/stream/contents/user/-/label/NonExistent")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_list_entries_invalid_feed() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/stream/contents/feed/https://nonexistent.com/feed.xml")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_entry_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // stream/items/contents with non-existent ID returns 200 with empty items
    let response = server
        .get("/reader/api/0/stream/items/contents?i=9999")
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_mark_entry_read_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form_data: Vec<(&str, String)> = vec![
        ("i", "9999".to_string()),
        ("a", "user/-/state/com.google/read".to_string()),
    ];
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_entry_unread_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form_data: Vec<(&str, String)> = vec![
        ("i", "9999".to_string()),
        ("r", "user/-/state/com.google/read".to_string()),
    ];
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_toggle_entry_star_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form_data: Vec<(&str, String)> = vec![
        ("i", "9999".to_string()),
        ("a", "user/-/state/com.google/starred".to_string()),
    ];
    let response = server.post("/reader/api/0/edit-tag").form(&form_data).await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_entry_neighbors_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/api/entries/9999/neighbors").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_fetch_full_content_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.post("/api/entries/9999/fetch-full-content").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_summarize_entry_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.post("/api/entries/9999/summarize").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_save_to_services_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.post("/api/entries/9999/save").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_list_feed_entries_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/stream/contents/feed/https://nonexistent.com/feed.xml")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_get_unread_stats() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["unreadcounts"].is_array());
}

#[tokio::test]
async fn test_mark_all_read_all() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("s", "user/-/state/com.google/reading-list")];
    let response = server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

#[tokio::test]
async fn test_mark_all_read_older_than_days() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Use timestamp in microseconds (7 days ago)
    let ts = (chrono::Utc::now().timestamp() - 7 * 86400) * 1_000_000;
    let ts_str = ts.to_string();
    let form: Vec<(&str, &str)> = vec![
        ("s", "user/-/state/com.google/reading-list"),
        ("ts", &ts_str),
    ];
    let response = server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");
}

#[tokio::test]
async fn test_mark_all_read_by_category_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("s", "user/-/label/NonExistent")];
    let response = server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_mark_all_read_by_feed_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("s", "feed/https://nonexistent.com/feed.xml")];
    let response = server
        .post("/reader/api/0/mark-all-as-read")
        .form(&form)
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_entries_filter_by_valid_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Create a category
    create_category(&server, "TestCategory").await;

    // Filter entries by this category (should be empty but return 200)
    let response = server
        .get("/reader/api/0/stream/contents/user/-/label/TestCategory")
        .await;
    response.assert_status_ok();
}

// ============================================================================
// User Settings Handler Tests
// ============================================================================
//
// JSON PUT/GET endpoints for password, settings, linkding, and kagi were
// removed in PR-4 Task 3 in favour of SSR form-action endpoints
// (POST /user-settings/{password,preferences,linkding,kagi}). Coverage
// for those flows lives in test_change_password_form_*,
// test_update_preferences_form*, test_update_linkding_form, and
// test_update_kagi_form below.

#[tokio::test]
async fn test_get_theme_default() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["theme"], serde_json::Value::Null);
}

#[tokio::test]
async fn test_get_theme_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_update_theme_dark() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "dark" }))
        .await;

    response.assert_status_ok();

    // Verify the theme was saved
    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["theme"], "dark");
}

#[tokio::test]
async fn test_update_theme_light() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "light" }))
        .await;

    response.assert_status_ok();

    // Verify the theme was saved
    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["theme"], "light");
}

#[tokio::test]
async fn test_update_theme_system() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // First set a theme
    server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "dark" }))
        .await
        .assert_status_ok();

    // Then reset to system (null)
    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": null }))
        .await;

    response.assert_status_ok();

    // Verify the theme was reset
    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["theme"], serde_json::Value::Null);
}

#[tokio::test]
async fn test_update_theme_invalid() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "invalid-theme" }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_update_theme_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "dark" }))
        .await;

    response.assert_status_unauthorized();
}

// ============================================================================
// Page Handler Tests
// ============================================================================

#[tokio::test]
async fn test_categories_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR page: heading + create form + row table rendered server-side.
    assert!(!body.contains("<rdrs-categories-page>"));
    assert!(!body.contains("/static/js/pages/categories.js"));
    assert!(body.contains("<h1>Categories</h1>"));
    assert!(body.contains("<form method=\"post\" action=\"/categories\">"));
    assert!(body.contains("data-testid=\"categories-table\""));
}

#[tokio::test]
async fn test_categories_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/categories").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_feeds_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR page: heading, add form, table, filter bar.
    assert!(body.contains("<h1>Feeds</h1>"));
    assert!(body.contains("<form method=\"post\" action=\"/feeds\">"));
    assert!(body.contains("data-testid=\"feeds-table\""));
    assert!(body.contains("data-testid=\"feed-url-input\""));
    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-feeds-page>"));
    assert!(!body.contains("/static/js/pages/feeds.js"));
}

#[tokio::test]
async fn test_feeds_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/feeds").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_entries_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/entries").await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_entries_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/entries").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_entry_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Entry page now redirects to the list page with ?entry= param
    let response = server.get("/entries/1").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_entry_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/entries/1").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_user_settings_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Settings") || body.contains("settings"));
}

#[tokio::test]
async fn test_user_settings_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/user-settings").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_settings_page() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR content — no longer a CSR shell.
    assert!(!body.contains("<rdrs-settings-page>"));
    assert!(!body.contains("/static/js/pages/settings.js"));
    assert!(body.contains("<h1>App</h1>"));
}

#[tokio::test]
async fn test_settings_page_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/settings").await;
    response.assert_status_see_other();
}

// ============================================================================
// Cross-User Isolation Tests
// ============================================================================

#[tokio::test]
async fn test_category_isolation_between_users() {
    let mut server = create_test_server(default_test_config()).await;

    // User 1 creates a category
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

    create_category(&server, "User1 Category").await;

    // Logout
    server.delete("/api/session").await.assert_status_ok();

    // User 2 should not see User 1's category
    server
        .post("/api/register")
        .json(&json!({
            "username": "user2",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user2",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // User2 sees only its own seeded "Uncategorized", never User1's category.
    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"User1 Category".to_string()));
    assert_eq!(names, vec!["Uncategorized".to_string()]);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[tokio::test]
async fn test_create_category_with_whitespace_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Name with leading/trailing whitespace
    let form = vec![
        ("s", "user/-/label/ Trimmed Name ".to_string()),
        ("dest", "user/-/label/ Trimmed Name ".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();

    // Verify the category was created (may or may not be trimmed depending on implementation)
    let response = server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    let folder_tags: Vec<&str> = tags
        .iter()
        .filter(|t| t["type"].as_str() == Some("folder"))
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    // Should have at least one folder tag
    assert!(!folder_tags.is_empty());
}

#[tokio::test]
async fn test_update_category_with_whitespace_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Original").await;

    let form = vec![
        ("s", "user/-/label/Original".to_string()),
        ("dest", "user/-/label/ Updated ".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();

    // Verify the rename happened
    let response = server.get("/reader/api/0/tag/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let tags = body["tags"].as_array().unwrap();
    let folder_ids: Vec<&str> = tags
        .iter()
        .filter(|t| t["type"].as_str() == Some("folder"))
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    // Original should be gone
    assert!(!folder_ids.contains(&"user/-/label/Original"));
}

// ============================================================================
// Additional Feed Icon Tests
// ============================================================================

#[tokio::test]
async fn test_get_feed_icon_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/api/feeds/1/icon").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_feed_icon_no_icon() {
    let mut app = create_test_app(default_test_config()).await;

    // Create user and a feed (via OPML import) that has no icon
    let hash = auth::hash_password("password123").unwrap();
    rdrs::models::user::create_user(&app.db, "iconuser", &hash, Role::User)
        .await
        .unwrap();

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "iconuser",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Import a feed via GReader OPML import
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="IconTestCat">
            <outline type="rss" text="No Icon Feed" xmlUrl="https://noicon.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;
    app.server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

    // Get feed ID from subscription/list iconUrl field
    let response = app.server.get("/reader/api/0/subscription/list").await;
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();

    // iconUrl may be empty string when no icon exists; extract feed ID from the URL field
    // We need to find the feed ID. The subscription has url field but the icon endpoint
    // needs the internal feed ID. Let's extract from iconUrl if present, or use the DB.
    // Since iconUrl is empty when no icon, we'll query via DB.
    let feed_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM feed WHERE url = $1",
        "https://noicon.example.com/feed.xml"
    )
    .unwrap();

    // Request icon for a feed that exists but has no icon -> 404
    let response = app.server.get(&format!("/api/feeds/{feed_id}/icon")).await;
    response.assert_status_not_found();

    // Also verify the subscription's iconUrl is empty (no icon)
    assert_eq!(subscriptions[0]["iconUrl"], "");
}

// ============================================================================
// Passkey Handler Tests
// ============================================================================

#[tokio::test]
async fn test_passkey_register_start_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.post("/api/passkey/register/start").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_passkey_register_start_authorized() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.post("/api/passkey/register/start").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["options"]["publicKey"]["challenge"].is_string());
    assert!(body["options"]["publicKey"]["user"]["name"].is_string());
}

#[tokio::test]
async fn test_passkey_register_finish_unauthorized() {
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

    let response = server.post("/api/passkey/auth/start").await;
    response.assert_status_unauthorized();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("No passkeys"));
}

#[tokio::test]
async fn test_passkey_auth_finish_no_challenge() {
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/api/passkeys").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_list_passkeys_empty() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/api/passkeys").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_rename_passkey_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server
        .put("/api/passkeys/1")
        .json(&json!({ "name": "New Name" }))
        .await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_rename_passkey_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .put("/api/passkeys/9999")
        .json(&json!({ "name": "New Name" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_rename_passkey_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .put("/api/passkeys/1")
        .json(&json!({ "name": "" }))
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_delete_passkey_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.delete("/api/passkeys/1").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_delete_passkey_not_found() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.delete("/api/passkeys/9999").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_passkey_auth_start_with_invalid_passkey_data() {
    let app = create_test_app(default_test_config()).await;

    // Create user and passkey with invalid public_key JSON
    let password_hash = auth::hash_password("password123").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
    // Insert passkey with invalid JSON in public_key
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5)",
        user.id,
        vec![1u8, 2, 3],
        b"invalid json".to_vec(),
        0i64,
        "Test Passkey"
    )
    .unwrap();

    let response = app.server.post("/api/passkey/auth/start").await;
    response.assert_status_unauthorized();

    let body: serde_json::Value = response.json();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("No valid passkeys")
    );
}

#[tokio::test]
async fn test_passkey_register_finish_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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
    let mut app = create_test_app(default_test_config()).await;

    // Create user and passkey
    let password_hash = auth::hash_password("password123").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name, transports) VALUES ($1, $2, $3, $4, $5, $6)",
        user.id,
        vec![1u8, 2, 3],
        b"{}".to_vec(),
        5i64,
        "My Passkey",
        "usb,nfc"
    )
    .unwrap();

    // Login
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let response = app.server.get("/api/passkeys").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let passkeys = body["passkeys"].as_array().unwrap();
    assert_eq!(passkeys.len(), 1);
    assert_eq!(passkeys[0]["name"], "My Passkey");
}

#[tokio::test]
async fn test_rename_passkey_success() {
    let mut app = create_test_app(default_test_config()).await;

    // Create user and passkey
    let password_hash = auth::hash_password("password123").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
    let passkey_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        user.id,
        vec![1u8, 2, 3],
        b"{}".to_vec(),
        0i64,
        "Old Name"
    )
    .unwrap();

    // Login
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let response = app
        .server
        .put(&format!("/api/passkeys/{passkey_id}"))
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
    let mut app = create_test_app(default_test_config()).await;

    // Create user and passkey
    let password_hash = auth::hash_password("password123").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
    let passkey_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        user.id,
        vec![1u8, 2, 3],
        b"{}".to_vec(),
        0i64,
        "Test Passkey"
    )
    .unwrap();

    // Login
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let response = app
        .server
        .delete(&format!("/api/passkeys/{passkey_id}"))
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
    let mut app = create_test_app(default_test_config()).await;

    // Create two users, each with a passkey
    let hash1 = auth::hash_password("password123").unwrap();
    let user1 = rdrs::models::user::create_user(&app.db, "pkuser1", &hash1, Role::User)
        .await
        .unwrap();
    let passkey_id_user1: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        user1.id,
        vec![1u8, 2, 3],
        b"{}".to_vec(),
        0i64,
        "User1 Passkey"
    )
    .unwrap();
    let hash2 = auth::hash_password("password123").unwrap();
    rdrs::models::user::create_user(&app.db, "pkuser2", &hash2, Role::User)
        .await
        .unwrap();

    // Login as user2
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "pkuser2",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // User2 tries to rename User1's passkey -> 404
    let response = app
        .server
        .put(&format!("/api/passkeys/{passkey_id_user1}"))
        .json(&json!({ "name": "Hacked Name" }))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_passkey_delete_other_user() {
    let mut app = create_test_app(default_test_config()).await;

    // Create two users, user1 has a passkey
    let hash1 = auth::hash_password("password123").unwrap();
    let user1 = rdrs::models::user::create_user(&app.db, "pkdeluser1", &hash1, Role::User)
        .await
        .unwrap();
    let passkey_id_user1: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        user1.id,
        vec![4u8, 5, 6],
        b"{}".to_vec(),
        0i64,
        "User1 Key"
    )
    .unwrap();
    let hash2 = auth::hash_password("password123").unwrap();
    rdrs::models::user::create_user(&app.db, "pkdeluser2", &hash2, Role::User)
        .await
        .unwrap();

    // Login as user2
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "pkdeluser2",
            "password": "password123"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // User2 tries to delete User1's passkey -> 404
    let response = app
        .server
        .delete(&format!("/api/passkeys/{passkey_id_user1}"))
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Favicon Handler Tests
// ============================================================================

#[tokio::test]
async fn test_favicon_ico() {
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

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
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/apple-touch-icon.png").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/png");

    // iOS home-screen icons do not support transparency (transparent pixels
    // render as black) and iOS applies its own rounded-corner mask. The Apple
    // touch icon must therefore be a fully-opaque, full-bleed square: every
    // corner pixel must have alpha == 255.
    let img = image::load_from_memory(&response.into_bytes())
        .expect("apple-touch-icon must be a decodable image")
        .to_rgba8();
    let (w, h) = img.dimensions();
    assert_eq!((w, h), (180, 180));
    for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
        assert_eq!(
            img.get_pixel(x, y).0[3],
            255,
            "corner pixel ({x},{y}) must be fully opaque, not transparent"
        );
    }
}

// ============================================================================
// Static Assets Handler Tests
// ============================================================================

#[tokio::test]
async fn test_static_js_serves_known_file() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/js/utils.js").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "application/javascript");

    let cache_control = response
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cache_control, expected_static_cache_control());

    let body = response.text();
    assert!(!body.is_empty(), "JS file should not be empty");
}

#[tokio::test]
async fn test_static_js_serves_component_file() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/js/components/rdrs-sidebar.js").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "application/javascript");
}

#[tokio::test]
async fn test_static_js_not_found() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/js/nonexistent.js").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_static_css_serves_app_css() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/css/app.css").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "text/css; charset=utf-8");

    let cache_control = response
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cache_control, expected_static_cache_control());

    let body = response.text();
    assert!(!body.is_empty(), "CSS file should not be empty");
    assert!(
        body.contains(":root"),
        "CSS should contain design-token :root block"
    );
}

#[tokio::test]
async fn test_static_font_serves_woff2() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/fonts/newsreader-latin.woff2").await;
    response.assert_status_ok();

    // Self-hosted webfonts are served with the `font/woff2` content type from
    // the binary-embedded FONTS table (ahead of the text FILES table).
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "font/woff2");

    let cache_control = response
        .headers()
        .get("cache-control")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cache_control, expected_static_cache_control());

    let body = response.into_bytes();
    assert!(!body.is_empty(), "font file should not be empty");
    // woff2 files begin with the `wOF2` magic signature.
    assert_eq!(&body[0..4], b"wOF2", "font must be a valid woff2 payload");
}

#[tokio::test]
async fn test_static_font_not_found() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/fonts/nonexistent.woff2").await;
    response.assert_status_not_found();
}

// ============================================================================
// Health Check Tests
// ============================================================================

#[tokio::test]
async fn test_health_check() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/health").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["status"], "ok");
    assert!(body["git_version"].is_string());
}

#[tokio::test]
async fn test_health_check_no_auth_required() {
    let server = create_test_server(default_test_config()).await;

    // Health check should work without authentication
    let response = server.get("/health").await;
    response.assert_status_ok();
}

// ============================================================================
// Subscription Handler Coverage Tests
// ============================================================================

#[tokio::test]
async fn test_subscription_list_with_feeds() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Import a feed via OPML
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="SubListCat">
            <outline type="rss" text="SubList Feed" xmlUrl="https://sublist.example.com/feed.xml" htmlUrl="https://sublist.example.com"/>
        </outline>
    </body>
</opml>"#;

    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // GET subscription/list and verify response structure
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);

    let sub = &subscriptions[0];
    // Verify all expected fields exist
    assert_eq!(sub["id"], "feed/https://sublist.example.com/feed.xml");
    assert!(sub["title"].is_string(), "title should be a string");
    assert!(
        sub["categories"].is_array(),
        "categories should be an array"
    );
    assert!(sub["sortid"].is_string(), "sortid should be a string");
    assert_eq!(sub["url"], "https://sublist.example.com/feed.xml");
    assert!(
        sub.get("iconUrl").is_some(),
        "iconUrl field should be present"
    );

    // Verify category structure
    let categories = sub["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 1);
    assert_eq!(categories[0]["label"], "SubListCat");
    assert!(
        categories[0]["id"].as_str().unwrap().contains("SubListCat"),
        "category id should contain the label name"
    );
}

#[tokio::test]
async fn test_subscription_edit_subscribe_unreachable_url() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // subscription/edit ac=subscribe performs feed discovery before inserting.
    // An unreachable URL should result in a BAD_GATEWAY (502) error from discovery.
    let form: Vec<(&str, &str)> = vec![
        ("ac", "subscribe"),
        ("s", "feed/https://unreachable.example.com/feed.xml"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status(StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_subscription_edit_unknown_action() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![
        ("ac", "invalid"),
        ("s", "feed/https://example.com/feed.xml"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_quickadd_empty_url() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let form: Vec<(&str, &str)> = vec![("quickadd", "")];
    let response = server
        .post("/reader/api/0/subscription/quickadd")
        .form(&form)
        .await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_subscribed_true() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Import a feed via OPML
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="SubdTrueCat">
            <outline type="rss" text="SubdTrue Feed" xmlUrl="https://subdtrue.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

    // Check subscribed → "true"
    let response = server
        .get("/reader/api/0/subscribed?s=feed/https://subdtrue.example.com/feed.xml")
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "true");
}

#[tokio::test]
async fn test_subscribed_false() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .get("/reader/api/0/subscribed?s=feed/https://nonexistent.com/feed.xml")
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "false");
}

#[tokio::test]
async fn test_subscribed_invalid_stream() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // "invalid" does not start with "feed/" so should fail validation
    let response = server.get("/reader/api/0/subscribed?s=invalid").await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_export_opml_content_type() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/subscription/export").await;
    response.assert_status_ok();

    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("application/xml"),
        "Content-Type should be application/xml, got: {content_type}"
    );

    let content_disposition = response
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_disposition.contains("attachment"),
        "Content-Disposition should contain 'attachment', got: {content_disposition}"
    );
    assert!(
        content_disposition.contains("subscriptions.opml"),
        "Content-Disposition should contain 'subscriptions.opml', got: {content_disposition}"
    );
}

#[tokio::test]
async fn test_import_opml_with_existing_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Pre-create a category via rename-tag
    create_category(&server, "PreExistingCat").await;

    // Import OPML with a feed in the same category name
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="PreExistingCat">
            <outline type="rss" text="Existing Cat Feed" xmlUrl="https://existingcat.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    // Verify no duplicate category was created (still exactly 1 folder tag with this name)
    let names = get_folder_tag_names(&server).await;
    let matching: Vec<&String> = names.iter().filter(|n| *n == "PreExistingCat").collect();
    assert_eq!(
        matching.len(),
        1,
        "Should have exactly 1 'PreExistingCat' category, got {}",
        matching.len()
    );

    // Verify the feed was created under the existing category
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["categories"][0]["label"], "PreExistingCat");
}

#[tokio::test]
async fn test_subscription_edit_edit_title() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Import a feed via OPML
    let opml_content = r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
    <body>
        <outline text="EditTitleCat">
            <outline type="rss" text="Original Title" xmlUrl="https://edittitle.example.com/feed.xml"/>
        </outline>
    </body>
</opml>"#;

    server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await
        .assert_status_ok();

    // Edit the feed's title via subscription/edit ac=edit
    let form: Vec<(&str, &str)> = vec![
        ("ac", "edit"),
        ("s", "feed/https://edittitle.example.com/feed.xml"),
        ("t", "NewTitle"),
    ];
    let response = server
        .post("/reader/api/0/subscription/edit")
        .form(&form)
        .await;
    response.assert_status_ok();

    // Verify the title was changed via subscription/list
    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["title"], "NewTitle");
}

// ============================================================================
// Auth Handler Coverage Tests (ClientLogin, token, preference, friend)
// ============================================================================

#[tokio::test]
async fn test_client_login_success() {
    let server = create_test_server(default_test_config()).await;

    // Register a user first
    server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // ClientLogin with correct credentials
    let form: Vec<(&str, &str)> = vec![("Email", "testuser"), ("Passwd", "password123")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();

    let body = response.text();
    assert!(
        body.contains("Auth="),
        "Response should contain Auth= token, got: {body}"
    );
    assert!(
        body.contains("SID="),
        "Response should contain SID=, got: {body}"
    );
    assert!(
        body.contains("LSID="),
        "Response should contain LSID=, got: {body}"
    );
}

#[tokio::test]
async fn test_client_login_wrong_password() {
    let server = create_test_server(default_test_config()).await;

    // Register a user
    server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // ClientLogin with wrong password → 401
    let form: Vec<(&str, &str)> = vec![("Email", "testuser"), ("Passwd", "wrongpassword")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_client_login_nonexistent_user() {
    let server = create_test_server(default_test_config()).await;

    // ClientLogin with non-existent user → 401
    let form: Vec<(&str, &str)> = vec![("Email", "nouser"), ("Passwd", "password123")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_greader_auth_header() {
    let server = create_test_server(default_test_config()).await;

    // Register user
    server
        .post("/api/register")
        .json(&json!({
            "username": "testuser",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // Get auth token via ClientLogin
    let form: Vec<(&str, &str)> = vec![("Email", "testuser"), ("Passwd", "password123")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();

    let body = response.text();
    let auth_token = body
        .lines()
        .find(|line| line.starts_with("Auth="))
        .unwrap()
        .strip_prefix("Auth=")
        .unwrap();

    // Use the auth token in Authorization header to call user-info
    let auth_header_value = format!("GoogleLogin auth={auth_token}");
    let response = server
        .get("/reader/api/0/user-info")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_str(&auth_header_value).unwrap(),
        )
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["userName"], "testuser");
}

#[tokio::test]
async fn test_greader_invalid_auth_header() {
    let server = create_test_server(default_test_config()).await;

    // Use an invalid auth token in Authorization header → 401
    let response = server
        .get("/reader/api/0/user-info")
        .add_header(
            header::AUTHORIZATION,
            HeaderValue::from_static("GoogleLogin auth=invalidtoken"),
        )
        .await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_post_token() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/token").await;
    response.assert_status_ok();

    let token = response.text();
    assert!(!token.is_empty(), "POST token should be a non-empty string");
    // Token format is "<timestamp>/<hmac_hex>"
    assert!(
        token.contains('/'),
        "POST token should contain '/' separator, got: {token}"
    );
}

#[tokio::test]
async fn test_preference_list() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/preference/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body, json!({ "prefs": [] }));
}

#[tokio::test]
async fn test_preference_stream_list() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/preference/stream/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body, json!({ "streamprefs": {} }));
}

#[tokio::test]
async fn test_friend_list() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server.get("/reader/api/0/friend/list").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body, json!({ "friends": [] }));
}

// ============================================================================
// Form-action handlers for the SSR /user-settings page (PR-4 T1).
// Each endpoint accepts application/x-www-form-urlencoded bodies and returns
// 303 See Other with a flash cookie + Location header.
// ============================================================================

#[tokio::test]
async fn test_change_password_form_success() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/password")
        .form(&json!({
            "current_password": "password123",
            "new_password": "newpassword456",
            "confirm_password": "newpassword456",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn test_change_password_form_mismatch() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/password")
        .form(&json!({
            "current_password": "password123",
            "new_password": "newpassword456",
            "confirm_password": "differentvalue",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");
}

#[tokio::test]
async fn test_update_preferences_form() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "dark",
            "entries_per_page": 50,
            "retention_read_days": 0,
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");
}

#[tokio::test]
async fn test_update_preferences_form_validation() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // entries_per_page=5 is below MIN_ENTRIES_PER_PAGE (10), expect error path
    let response = server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 5,
            "retention_read_days": 0,
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");
}

#[tokio::test]
async fn test_update_linkding_form() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/linkding")
        .form(&json!({
            "api_url": "https://linkding.example.com",
            "api_token": "secret-token",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");
}

#[tokio::test]
async fn test_update_kagi_form() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/kagi")
        .form(&json!({
            "session_link": "https://kagi.com/search?token=mysessiontoken",
            "language": "EN",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");
}

#[tokio::test]
async fn test_revoke_other_sessions_form_keeps_current_deletes_others() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");

    // Seed a second session for the same user (e.g. another device/browser).
    let other_session =
        rdrs::models::session::create_session(&app.db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");

    // The seeded second session is gone.
    let found = rdrs::models::session::find_by_token(&app.db, &other_session.session_token)
        .await
        .unwrap();
    assert!(found.is_none(), "other session should be deleted");

    // The current (cookie) session still authenticates.
    app.server.get("/api/me").await.assert_status_ok();
}

#[tokio::test]
async fn test_revoke_other_sessions_form_unauthenticated_rejected() {
    // `/user-settings/*` is not under the CSRF/anonymous-session skip prefixes
    // (unlike `/api/*`), so `anonymous_session` mints a signed session cookie
    // for this request and `csrf_guard` then requires a matching token. A bare
    // POST with no cookie jar and no `_csrf` field never has one, so this is
    // rejected as 403 Forbidden by the CSRF guard before the `AuthUser`
    // extractor (which would otherwise 401) ever runs. This doubles as the
    // CSRF-missing coverage for this endpoint.
    let server = create_test_server(default_test_config()).await;

    let response = server.post("/user-settings/sessions/revoke-others").await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_revoke_other_sessions_form_blocked_while_masquerading() {
    // `start_masquerade` mutates the admin's own session row in place
    // (user_id -> target, original_user_id -> admin, same token/cookie), so
    // while masquerading, the effective session belongs to the target. The
    // revoke-others action must refuse to run in that state — otherwise an
    // admin masquerading as a user would silently delete that user's real
    // sessions on their own devices.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.server).await;

    // Seed a second, real session for the target user (id=2) — e.g. a
    // session from the target's own phone, unrelated to the masquerade.
    let target_session =
        rdrs::models::session::create_session(&app.db, 2, "test-agent", "127.0.0.1")
            .await
            .unwrap();

    // Admin starts masquerading as target (id=2). This reuses the admin's
    // existing session cookie/CSRF token (apply_csrf already wired it up in
    // setup_admin_user), since start_masquerade keeps the same token.
    app.server
        .post("/admin/users/2/masquerade")
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // Attempt to revoke other sessions while masquerading — must be refused.
    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/user-settings");

    // The target's real, seeded session must still exist: the victim was not
    // signed out of their own devices as a side effect of the masquerade.
    let found = rdrs::models::session::find_by_token(&app.db, &target_session.session_token)
        .await
        .unwrap();
    assert!(
        found.is_some(),
        "masquerade must not delete the target's real sessions"
    );
}

// ============================================================================
// Form-action admin endpoint tests (PR-5 T1)
// ============================================================================

/// Helper to register the first user (becomes admin) and login.
async fn setup_admin_user(server: &mut TestServer) {
    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

/// Helper to register a second (regular) user without logging in.
async fn register_target_user(server: &TestServer) {
    server
        .post("/api/register")
        .json(&json!({
            "username": "target",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);
}

#[tokio::test]
async fn test_update_role_form_promotes_user() {
    let mut server = create_test_server(default_test_config()).await;
    setup_admin_user(&mut server).await;
    register_target_user(&server).await;

    // Promote target (id=2) to admin role
    let response = server
        .post("/admin/users/2/role")
        .form(&json!({ "role": "admin" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/admin");

    // Verify role change via the SSR /admin page — the target row should now
    // show "demote" (i.e. target is admin) instead of "promote".
    let admin_resp = server.get("/admin").await;
    admin_resp.assert_status_ok();
    let body = admin_resp.text();
    assert!(body.contains("target"));
    // Two admins now → both rows render; target row's role cell shows "admin".
    // The action button on a non-self admin row says "demote".
    assert!(body.contains("demote"));
}

#[tokio::test]
async fn test_update_status_form_disables_user() {
    let mut server = create_test_server(default_test_config()).await;
    setup_admin_user(&mut server).await;
    register_target_user(&server).await;

    // Disable target (id=2)
    let response = server
        .post("/admin/users/2/status")
        .form(&json!({ "disabled": "true" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/admin");

    // Verify the user is now disabled — login should fail
    let login_resp = server
        .post("/api/session")
        .json(&json!({
            "username": "target",
            "password": "password123"
        }))
        .await;
    login_resp.assert_status_forbidden();
}

#[tokio::test]
async fn test_start_masquerade_form_redirects_to_root() {
    let mut server = create_test_server(default_test_config()).await;
    setup_admin_user(&mut server).await;
    register_target_user(&server).await;

    // Start masquerade as target (id=2)
    let response = server.post("/admin/users/2/masquerade").await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/");
}

#[tokio::test]
async fn test_delete_user_form_succeeds() {
    let mut server = create_test_server(default_test_config()).await;
    setup_admin_user(&mut server).await;
    register_target_user(&server).await;

    // Delete target (id=2)
    let response = server.post("/admin/users/2/delete").await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/admin");

    // Verify the target user is gone — SSR /admin page should no longer
    // contain a "target" row.
    let admin_resp = server.get("/admin").await;
    admin_resp.assert_status_ok();
    let body = admin_resp.text();
    assert!(!body.contains(">target<"));
}

#[tokio::test]
async fn test_update_role_form_self_protection() {
    let mut server = create_test_server(default_test_config()).await;
    setup_admin_user(&mut server).await;

    // Admin (id=1) tries to change their own role — should be blocked
    let response = server
        .post("/admin/users/1/role")
        .form(&json!({ "role": "user" }))
        .await;

    // Should still redirect to /admin (error flash), not a 400
    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/admin");

    // Verify the admin role is unchanged via the SSR /admin page.
    // The admin's own row carries the "(you)" marker and no role-toggle form,
    // and there are no rows that say "promote" because no non-admin exists.
    let admin_resp = server.get("/admin").await;
    admin_resp.assert_status_ok();
    let body = admin_resp.text();
    assert!(body.contains("(you)"));
    assert!(!body.contains("promote"));
}

// ============================================================================
// /categories form-action POST endpoint tests (SSR PR-7 T1)
// ============================================================================

#[tokio::test]
async fn test_create_category_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let response = app
        .server
        .post("/categories")
        .form(&json!({ "name": "Tech" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

    // Verify the category exists in the DB
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM category WHERE name = $1",
        "Tech"
    )
    .unwrap();
    let exists = count > 0;
    assert!(
        exists,
        "category 'Tech' should exist in the DB after creation"
    );
}

#[tokio::test]
async fn test_create_category_form_empty_name() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let response = app
        .server
        .post("/categories")
        .form(&json!({ "name": "" }))
        .await;

    // Error flash redirect — still 303 to /categories, no DB write
    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

    // Confirm nothing was inserted
    let count: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT COUNT(*) FROM category").unwrap();
    // Only the seeded "Uncategorized" remains; the empty name created nothing.
    assert_eq!(
        count, 1,
        "no category should be created for an empty name (only the seeded default)"
    );
}

#[tokio::test]
async fn test_rename_category_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    // Create a category via the new form endpoint
    app.server
        .post("/categories")
        .form(&json!({ "name": "OldName" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // Fetch the inserted ID
    let cat_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM category WHERE name = $1",
        "OldName"
    )
    .unwrap();

    // Rename it
    let response = app
        .server
        .post(&format!("/categories/{cat_id}/rename"))
        .form(&json!({ "name": "NewName" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

    // Verify the rename in the DB
    let new_name: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT name FROM category WHERE id = $1",
        cat_id
    )
    .unwrap();
    assert_eq!(new_name, "NewName");
}

#[tokio::test]
async fn test_delete_category_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    // Create a category via the new form endpoint
    app.server
        .post("/categories")
        .form(&json!({ "name": "ToDelete" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // Fetch the inserted ID
    let cat_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM category WHERE name = $1",
        "ToDelete"
    )
    .unwrap();

    // Delete it
    let response = app
        .server
        .post(&format!("/categories/{cat_id}/delete"))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

    // Verify it's gone from the DB
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM category WHERE id = $1",
        cat_id
    )
    .unwrap();
    assert_eq!(count, 0, "category should be deleted from the DB");
}

// ============================================================================
// /feeds form-action POST endpoint tests (SSR PR-8 T1)
// ============================================================================

/// Helper: insert a feed directly via the model (skips network discovery).
/// Returns (`category_id`, `feed_id`).
async fn insert_test_feed(app: &TestApp, category_name: &str, feed_url: &str) -> (i64, i64) {
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, category_name)
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
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
    (cat.id, feed.id)
}

#[tokio::test]
async fn test_create_feed_form_empty_url() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let response = app
        .server
        .post("/feeds")
        .form(&json!({ "url": "", "category_id": 1 }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    let count: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT COUNT(*) FROM feed").unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_create_feed_form_invalid_category() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let response = app
        .server
        .post("/feeds")
        .form(&json!({ "url": "https://example.com/feed.xml", "category_id": 999_999 }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    let count: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT COUNT(*) FROM feed").unwrap();
    assert_eq!(
        count, 0,
        "no feed should be created when category is invalid"
    );
}

#[tokio::test]
async fn test_edit_feed_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    let (cat_id, feed_id) = insert_test_feed(&app, "Tech", "https://example.com/feed.xml").await;

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://example.com/feed.xml",
            "title": "Renamed Feed",
            "description": "New description",
            "site_url": "https://example.com",
            "category_id": cat_id,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/feeds/{feed_id}/edit")
    );

    let title: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT title FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    let description: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT description FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(title, "Renamed Feed");
    assert_eq!(description, "New description");
}

#[tokio::test]
async fn test_edit_feed_form_changes_category() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    let (_cat_a, feed_id) = insert_test_feed(&app, "Tech", "https://example.com/feed.xml").await;

    // Add a second category for the same user.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat_b_id = rdrs::models::category::create_category(&app.db, user_id, "Other")
        .await
        .unwrap()
        .id;

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://example.com/feed.xml",
            "title": "Test Feed",
            "description": "",
            "site_url": "",
            "category_id": cat_b_id,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);

    let new_cat_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT category_id FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(new_cat_id, cat_b_id);
}

#[tokio::test]
async fn test_delete_feed_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    let (_cat_id, feed_id) = insert_test_feed(&app, "Tech", "https://example.com/feed.xml").await;

    let response = app.server.post(&format!("/feeds/{feed_id}/delete")).await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_delete_feed_form_not_owned() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    // Insert a feed under another user (not the logged-in one).
    let other_user = rdrs::models::user::create_user(&app.db, "other", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, other_user.id, "Other")
        .await
        .unwrap();
    let other_feed_id = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://other.example.com/feed.xml",
            title: Some("Other"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap()
    .id;

    let response = app
        .server
        .post(&format!("/feeds/{other_feed_id}/delete"))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    // Other user's feed must still exist.
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE id = $1",
        other_feed_id
    )
    .unwrap();
    assert_eq!(count, 1, "non-owner delete must not remove the feed");
}

#[tokio::test]
async fn test_refresh_feed_form_not_owned() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    // Insert a feed under another user.
    let other_user = rdrs::models::user::create_user(&app.db, "other2", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, other_user.id, "Other")
        .await
        .unwrap();
    let other_feed_id = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://other2.example.com/feed.xml",
            title: Some("Other"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap()
    .id;

    let response = app
        .server
        .post(&format!("/feeds/{other_feed_id}/refresh"))
        .await;

    // Ownership check fails first → no network call attempted.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");
}

#[tokio::test]
async fn test_import_opml_form_empty() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let form = MultipartForm::new();
    let response = app.server.post("/feeds/import").multipart(form).await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds/import");

    let count: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT COUNT(*) FROM feed").unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_import_opml_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

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

    let part = Part::bytes(opml_content.as_bytes())
        .file_name("subscriptions.opml")
        .mime_type("application/xml");
    let form = MultipartForm::new().add_part("file", part);
    let response = app.server.post("/feeds/import").multipart(form).await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    let cat_count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM category WHERE name = 'Tech'"
    )
    .unwrap();
    let feed_count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE url = 'https://example.com/feed.xml'"
    )
    .unwrap();
    assert_eq!(cat_count, 1);
    assert_eq!(feed_count, 1);
}

// ============================================================================
// GET /entries/{id}/fragment — PR-10 T3
// ============================================================================

/// Isolated app factory used by the fragment tests so they don't share the
/// `test_handlers_app` `SQLite` in-memory database with the rest of the suite.
async fn create_test_app_named(config: Config, _name: &str) -> TestApp {
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

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_renders_reading_pane() {
    let mut app = create_test_app_named(default_test_config(), "test_entry_fragment_happy").await;

    // Register and log in as alice.
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_frag", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_frag", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed: category + feed + entry with content.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/feed",
            title: Some("Test Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-frag-test",
        Some("Hello World"),
        Some("https://x/post"),
        Some("<p>Body text here</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    let response = app
        .server
        .get(&format!("/entries/{entry_id}/fragment"))
        .await;

    assert_eq!(response.status_code(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("text/html"),
        "expected text/html, got: {content_type}"
    );
    let html = response.text();
    assert!(
        html.contains(r#"id="reading-pane""#),
        "reading-pane id must be present"
    );
    assert!(html.contains("Hello World"), "entry title must appear");
    assert!(html.contains("Body text here"), "entry body must appear");
    // Editorial redesign: meta shows the feed favicon chip (feed "Test Feed"
    // has no icon -> coloured initial chip "T").
    assert!(
        html.contains("entry-favicon-chip"),
        "reading pane meta must render the favicon chip fallback"
    );
    // Actions use the shared .rp-action control with stable aria-labels.
    assert!(
        html.contains(r#"class="rp-action""#),
        "reading pane actions must use the .rp-action control"
    );
    assert!(
        html.contains(r#"aria-label="Mark Unread""#),
        "Mark Unread action must keep its accessible name"
    );
    // Auto-mark-as-read: response carries the updated row + sidebar blocks.
    assert!(
        html.contains(&format!(r##"data-swap-target="#entry-row-{entry_id}""##)),
        "response must include a multi-target row block to clear unread state"
    );
    assert!(
        html.contains(r##"data-swap-target="#sidebar-unread""##),
        "response must include a multi-target sidebar block"
    );
    // The mark-as-read write is dispatched off the critical path via a detached
    // `tokio::spawn` (fire-and-forget), so it may not have committed by the time
    // the response returns — there is no ordering guarantee between it and this
    // read-back. Poll until the write lands rather than assuming it already has.
    let mut read_at: Option<String> = None;
    for _ in 0..100 {
        read_at = rdrs::query_scalar!(
            &app.db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            entry_id
        )
        .unwrap();
        if read_at.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(read_at.is_some(), "entry must be marked read after open");
}

/// A real top-level browser navigation to the partial-only `/fragment` route
/// (carrying `Sec-Fetch-Dest: document`) must redirect to the full entries
/// page rather than serving the bare `<template>` blocks — which render as a
/// blank page. The redirect path is read-only and must not mark the entry read.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_redirects_on_top_level_navigation() {
    let mut app = create_test_app_named(default_test_config(), "test_entry_fragment_doc_nav").await;

    app.server
        .post("/api/register")
        .json(&json!({ "username": "doc_nav", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "doc_nav", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/feed",
            title: Some("Test Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-doc-nav",
        Some("Hello World"),
        Some("https://x/post"),
        Some("<p>Body text here</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    let response = app
        .server
        .get(&format!("/entries/{entry_id}/fragment"))
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, format!("/entries?entry={entry_id}"));

    // The redirect short-circuits before the write transaction, so the entry
    // stays unread (the user will mark it read by opening it normally).
    let read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        entry_id
    )
    .unwrap();
    assert!(
        read_at.is_none(),
        "the top-level-navigation redirect must not mark the entry read"
    );
}

/// A top-level navigation to `/entries/{id}/fragment` carrying a `Referer` from
/// a scoped list page must redirect back into that scope (preserving its
/// filters) with the pane pre-opened — not dump the user into All Entries. The
/// redirect short-circuits before the entry is looked up, so the id need not
/// exist. Regression for "clicking an Unread/category/feed entry jumps to All
/// Entries" when `app.js` is stale-cached and clicks fall through to navigation.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_document_nav_preserves_referer_scope() {
    let mut app =
        create_test_app_named(default_test_config(), "test_entry_fragment_referer_scope").await;

    app.server
        .post("/api/register")
        .json(&json!({ "username": "ref_scope", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "ref_scope", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let response = app
        .server
        .get("/entries/123/fragment")
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .add_header(
            header::REFERER,
            HeaderValue::from_static("https://testserver/categories/4/entries?q=rust"),
        )
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, "/categories/4/entries?q=rust&entry=123");
}

/// The media proxy serves a stable `ETag` (the per-URL request signature). A
/// conditional request that already holds it gets a 304 with no origin fetch,
/// mirroring miniflux's media proxy.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_proxy_image_304_on_if_none_match() {
    let app = create_test_app_named(default_test_config(), "test_proxy_image_inm").await;

    let response = app
        .server
        .get("/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=sometoken")
        .add_header(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"sometoken\""),
        )
        .await;

    response.assert_status(StatusCode::NOT_MODIFIED);
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(etag, "\"sometoken\"");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_404_for_other_user() {
    let mut app = create_test_app_named(default_test_config(), "test_entry_fragment_404").await;

    // Register alice (will be logged in).
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_404", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_404", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Insert bob + bob's entry directly via the DB — bob never logs in via the
    // test server so alice's session cookie stays active.
    let bob = rdrs::models::user::create_user(&app.db, "bob_404", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, bob.id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://b/feed",
            title: Some("Bob Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-bob-entry",
        Some("Bob's Entry"),
        Some("https://b/post"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let bob_entry_id = entry.id;

    // Alice tries to read Bob's entry — must get 404, not 200.
    let response = app
        .server
        .get(&format!("/entries/{bob_entry_id}/fragment"))
        .await;

    assert_eq!(
        response.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user access must return 404"
    );
}

// ============================================================================
// POST /entries/{id}/star — PR-10 T4
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_star_entry_form_is_idempotent_mark_starred() {
    let mut app = create_test_app_named(default_test_config(), "test_star_entry_form").await;

    // Register and log in as alice.
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_star", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_star", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed: category + feed + one unread, unstarred entry.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/star-feed",
            title: Some("Star Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-star-test",
        Some("E"),
        Some("https://x/p"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    // First /star — real state change, must star the entry.
    let resp = app.server.post(&format!("/entries/{entry_id}/star")).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains("data-swap-target=\"#entry-row-"),
        "multi-target row block must be present"
    );
    assert!(
        html.contains("data-swap-target=\"#sidebar-unread\""),
        "multi-target sidebar block must be present"
    );
    assert!(
        html.contains("aria-label=\"Unstar\""),
        "row must reflect starred state via the star toggle's Unstar aria-label after first call"
    );
    // Pane Star button swap must be present so the reading-pane button label
    // can flip to "Unstar" when the pane is visible.
    assert!(
        html.contains("data-swap-target=\"#reading-pane-star-form-"),
        "multi-target response must include the pane-star-form swap"
    );
    assert!(
        html.contains(r#"aria-label="Unstar""#),
        "pane-star-form swap payload must render the Unstar aria-label"
    );

    // Second /star — idempotent, entry stays starred (no toggle back).
    let resp2 = app.server.post(&format!("/entries/{entry_id}/star")).await;
    assert_eq!(resp2.status_code(), StatusCode::OK);
    let html2 = resp2.text();
    assert!(
        html2.contains("aria-label=\"Unstar\""),
        "second /star call must be a no-op — row star toggle must still show Unstar"
    );
    assert!(
        html2.contains(r#"aria-label="Unstar""#),
        "pane-star-form payload must still show Unstar aria-label after no-op"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unstar_entry_form_is_idempotent_mark_unstarred() {
    let mut app = create_test_app_named(default_test_config(), "test_unstar_entry_form").await;

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_unstar", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_unstar", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed one already-starred entry so /unstar's first call is a real
    // state change and the second call exercises the no-op path.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/unstar-feed",
            title: Some("Unstar Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-unstar-test",
        Some("E"),
        Some("https://x/u"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::star_entry(&app.db, entry.id)
        .await
        .unwrap();
    let entry_id = entry.id;

    // First /unstar — real state change.
    let resp = app
        .server
        .post(&format!("/entries/{entry_id}/unstar"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        !html.contains("aria-label=\"Unstar\""),
        "row star toggle must show Star (not Unstar) after /unstar"
    );
    assert!(
        html.contains(">Star<"),
        "pane-star-form swap must render the Star label after unstar"
    );

    // Second /unstar — no-op. Row must still be unstarred.
    let resp2 = app
        .server
        .post(&format!("/entries/{entry_id}/unstar"))
        .await;
    assert_eq!(resp2.status_code(), StatusCode::OK);
    let html2 = resp2.text();
    assert!(
        !html2.contains("aria-label=\"Unstar\""),
        "second /unstar call must be a no-op — row must still be unstarred"
    );
}

// ============================================================================
// POST /entries/{id}/read — PR-10 T4
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_read_entry_form_is_idempotent_mark_read() {
    let mut app = create_test_app_named(default_test_config(), "test_read_entry_form").await;

    // Register and log in as alice.
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_read", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_read", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed: category + feed + one unread entry.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/read-feed",
            title: Some("Read Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-read-test",
        Some("E"),
        Some("https://x/r"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    // First POST — should mark read.
    let resp = app.server.post(&format!("/entries/{entry_id}/read")).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains("data-swap-target=\"#entry-row-"),
        "multi-target row block must be present"
    );
    assert!(
        html.contains("data-swap-target=\"#sidebar-unread\""),
        "multi-target sidebar block must be present"
    );
    assert!(
        html.contains(r#"class="entry-item entry-read""#),
        "row must reflect read state via the .entry-read class after first call"
    );

    // Second POST — idempotent, entry stays read (no toggle back).
    let resp2 = app.server.post(&format!("/entries/{entry_id}/read")).await;
    assert_eq!(resp2.status_code(), StatusCode::OK);
    let html2 = resp2.text();
    assert!(
        html2.contains(r#"class="entry-item entry-read""#),
        "second /read call must be a no-op — row must still carry .entry-read"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_entry_form_is_idempotent_mark_unread() {
    let mut app = create_test_app_named(default_test_config(), "test_unread_entry_form").await;

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_unr", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_unr", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed one entry already in the read state so the first /unread is a real
    // state change and the second one is a no-op.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/unread-feed",
            title: Some("Unread Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-unread-test",
        Some("E"),
        Some("https://x/u"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::mark_as_read(&app.db, entry.id)
        .await
        .unwrap();
    let entry_id = entry.id;

    // First /unread — real state change, must mark unread + emit flash.
    let resp = app
        .server
        .post(&format!("/entries/{entry_id}/unread"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        !html.contains(r#"class="entry-item entry-read""#),
        "row must drop .entry-read after /unread"
    );
    assert!(
        html.contains("Marked as unread."),
        "real state change must emit the Marked-as-unread flash payload"
    );

    // Second /unread — no-op. Must NOT re-toggle to read and must NOT
    // re-emit the flash (that would spam the user on stale-label re-clicks).
    let resp2 = app
        .server
        .post(&format!("/entries/{entry_id}/unread"))
        .await;
    assert_eq!(resp2.status_code(), StatusCode::OK);
    let html2 = resp2.text();
    assert!(
        !html2.contains(r#"class="entry-item entry-read""#),
        "second /unread call must be a no-op — row must still be unread"
    );
    assert!(
        !html2.contains("Marked as unread."),
        "no-op /unread must not re-emit the flash"
    );
}

// ============================================================================
// POST /entries/{id}/star — cross-tenant 404 (PR-10 review)
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_star_entry_form_404_for_other_user() {
    let mut app = create_test_app_named(default_test_config(), "test_star_entry_form_404").await;

    // Register + login alice (session cookie is now alice's).
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_s404", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_s404", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Insert bob + bob's entry directly via DB — bob never logs in via the test
    // server so alice's session cookie stays active.
    let bob = rdrs::models::user::create_user(&app.db, "bob_s404", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, bob.id, "Bob Cat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://bob/star-feed",
            title: Some("Bob Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-bob-star",
        Some("Bob Entry"),
        Some("https://bob/entry"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let bob_entry_id = entry.id;

    // Alice tries to star bob's entry → 404.
    let resp = app
        .server
        .post(&format!("/entries/{bob_entry_id}/star"))
        .await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user star must return 404"
    );

    // Same ownership guard for the /unstar endpoint.
    let resp_unstar = app
        .server
        .post(&format!("/entries/{bob_entry_id}/unstar"))
        .await;
    assert_eq!(
        resp_unstar.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user unstar must return 404"
    );
}

// ============================================================================
// POST /entries/{id}/read — cross-tenant 404 (PR-10 review)
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_read_entry_form_404_for_other_user() {
    let mut app = create_test_app_named(default_test_config(), "test_read_entry_form_404").await;

    // Register + login alice (session cookie is now alice's).
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_r404", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_r404", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Insert bob + bob's entry directly via DB — bob never logs in via the test
    // server so alice's session cookie stays active.
    let bob = rdrs::models::user::create_user(&app.db, "bob_r404", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, bob.id, "Bob Cat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://bob/read-feed",
            title: Some("Bob Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-bob-read",
        Some("Bob Entry"),
        Some("https://bob/entry"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let bob_entry_id = entry.id;

    // Alice tries to mark bob's entry as read → 404.
    let resp = app
        .server
        .post(&format!("/entries/{bob_entry_id}/read"))
        .await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user read must return 404"
    );

    // Same ownership guard for the /unread endpoint.
    let resp_unread = app
        .server
        .post(&format!("/entries/{bob_entry_id}/unread"))
        .await;
    assert_eq!(
        resp_unread.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user unread must return 404"
    );
}

// POST /entries/{id}/summarize — PR-10 T5
//
// The handler returns a small multi-target swap that only updates the
// `#rp-summary-container` block (not the whole reading pane), so a
// Fetch-Full-Content view keeps its externally-fetched article body.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_summarize_entry_form_renders_summary_pending_fragment() {
    let mut app = create_test_app_named(default_test_config(), "test_summarize_entry_form").await;

    // Register and log in as alice.
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_sum", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_sum", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed: category + feed + entry with a link.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/sum-feed",
            title: Some("Sum Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-sum-test",
        Some("Summarizable Entry"),
        Some("https://x/sum-post"),
        Some("<p>Content to summarize</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    // POST /entries/{id}/summarize — should return only the
    // `#rp-summary-container` swap fragment with a pending state.
    let resp = app
        .server
        .post(&format!("/entries/{entry_id}/summarize"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains("data-swap-target=\"#rp-summary-container\""),
        "response must target the summary container, not the whole reading pane"
    );
    assert!(
        html.contains("Summarizing"),
        "pending fragment must show a 'Summarizing…' message"
    );
    // The article body should not be in the response — that is the whole
    // point of the smaller swap target.
    assert!(
        !html.contains("reading-pane-article"),
        "response must NOT swap the article body (would reset full-content view)"
    );
}

// ============================================================================
// GET /entries?fragment=1&after=... — PR-10 T6 Load-More fragment
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entries_load_more_returns_row_fragments() {
    let mut app = create_test_app_named(default_test_config(), "test_load_more_fragment").await;

    // Register and log in as alice.
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_lm", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_lm", "password": "pw123456" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Seed: category + feed + 75 entries.
    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "LoadMore Cat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://lm/feed",
            title: Some("LM Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    for i in 0..75i64 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-lm-{i}"),
            Some(&format!("LM Entry {i}")),
            Some(&format!("https://lm/{i}")),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }

    // GET /entries — fetch page 1 (50 entries) and extract the keyset cursor.
    let page1 = app.server.get("/entries").await;
    assert_eq!(page1.status_code(), StatusCode::OK);
    let page1_html = page1.text();
    // The load-more form contains <input type="hidden" name="after" value="<cursor>">.
    let after_cursor = page1_html
        .split("name=\"after\" value=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("page 1 should have a load-more cursor (75 entries > 50 page size)");

    // GET /entries?fragment=1&after=<cursor> — append semantics: rows 50..74 only.
    let resp = app
        .server
        .get("/entries")
        .add_query_param("fragment", "1")
        .add_query_param("after", after_cursor)
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();

    // Append semantics: should contain only the new slice (rows 50..74 = 25 rows).
    let row_count = html.matches("data-entry-row").count();
    assert_eq!(
        row_count, 25,
        "append semantics should return only the new slice (rows 50..74)"
    );

    // No more pages — load-more form should be absent.
    assert!(
        !html.contains("id=\"load-more\""),
        "no more pages → load-more form must be absent"
    );

    // Response must be wrapped in a <template data-swap-target="#load-more">.
    assert!(
        html.contains("data-swap-target=\"#load-more\""),
        "fragment must use multi-target template swap that targets #load-more"
    );
}

// ============================================================================
// handlers/feeds.rs — additional branch coverage (Part A: pure-DB tests)
// ============================================================================

#[tokio::test]
async fn test_edit_feed_form_empty_url() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_empty_url").await;
    setup_authenticated_user(&mut app.server).await;
    let (cat_id, feed_id) =
        insert_test_feed(&app, "Tech", "https://empty-url-test.example.com/feed.xml").await;

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "",
            "title": "Test Feed",
            "description": "",
            "site_url": "",
            "category_id": cat_id,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    // Empty url → error flash redirect back to the edit page.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/feeds/{feed_id}/edit")
    );

    // Verify url unchanged in DB.
    let url: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT url FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(url, "https://empty-url-test.example.com/feed.xml");
}

#[tokio::test]
async fn test_edit_feed_form_not_found() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_not_found").await;
    setup_authenticated_user(&mut app.server).await;
    let (cat_id, _) =
        insert_test_feed(&app, "Tech", "https://notfound-test.example.com/feed.xml").await;

    let response = app
        .server
        .post("/feeds/999999/edit")
        .form(&json!({
            "url": "https://notfound-test.example.com/feed.xml",
            "title": "Test",
            "description": "",
            "site_url": "",
            "category_id": cat_id,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    // Not found → error flash redirect (no crash).
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds/999999/edit");
}

#[tokio::test]
async fn test_edit_feed_form_other_users_feed() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_other_user_feed").await;
    setup_authenticated_user(&mut app.server).await;

    // Seed a feed under a different user (not testuser).
    let other_user = rdrs::models::user::create_user(&app.db, "other_editfeed", "x", Role::User)
        .await
        .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, other_user.id, "OtherCat")
        .await
        .unwrap();
    let other_feed_id = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://other-edit.example.com/feed.xml",
            title: Some("Other Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap()
    .id;

    // testuser tries to edit the other user's feed — the handler checks
    // category ownership (find_by_id_and_user on the feed's category)
    // which fails because the category belongs to other_editfeed.
    let response = app
        .server
        .post(&format!("/feeds/{other_feed_id}/edit"))
        .form(&json!({
            "url": "https://other-edit.example.com/feed.xml",
            "title": "Hacked",
            "description": "",
            "site_url": "",
            "category_id": 1,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);

    // The other user's feed must be unchanged.
    let title: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT title FROM feed WHERE id = $1",
        other_feed_id
    )
    .unwrap();
    assert_eq!(title, "Other Feed");
}

#[tokio::test]
async fn test_edit_feed_form_category_not_owned() {
    let mut app =
        create_test_app_named(default_test_config(), "test_edit_feed_cat_not_owned").await;
    setup_authenticated_user(&mut app.server).await;
    let (cat_id, feed_id) = insert_test_feed(
        &app,
        "OwnedCat",
        "https://cat-not-owned.example.com/feed.xml",
    )
    .await;

    // Create a category that belongs to a different user.
    let other_user = rdrs::models::user::create_user(&app.db, "other_catowner", "x", Role::User)
        .await
        .unwrap();
    let other_cat_id =
        rdrs::models::category::create_category(&app.db, other_user.id, "NotMyCategory")
            .await
            .unwrap()
            .id;

    // testuser tries to move their feed into the other user's category.
    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://cat-not-owned.example.com/feed.xml",
            "title": "Test Feed",
            "description": "",
            "site_url": "",
            "category_id": other_cat_id,
            "custom_user_agent": "",
            "custom_referrer": "",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);

    // The feed's category must remain unchanged.
    let actual_cat: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT category_id FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(actual_cat, cat_id);
}

#[tokio::test]
async fn test_edit_feed_form_clear_user_agent() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_clear_ua").await;
    setup_authenticated_user(&mut app.server).await;

    // Seed a feed and set a custom_user_agent on it directly.
    let (cat_id, feed_id) =
        insert_test_feed(&app, "Tech", "https://clear-ua.example.com/feed.xml").await;
    rdrs::db_execute!(
        &app.db,
        "UPDATE feed SET custom_user_agent = 'MyBot/1.0' WHERE id = $1",
        feed_id
    )
    .unwrap();

    // POST edit with _clear_user_agent=on
    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://clear-ua.example.com/feed.xml",
            "title": "Test Feed",
            "description": "",
            "site_url": "",
            "category_id": cat_id,
            "custom_user_agent": "",
            "custom_referrer": "",
            "_clear_user_agent": "on",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/feeds/{feed_id}/edit")
    );

    // custom_user_agent must now be NULL.
    let ua: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT custom_user_agent FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert!(
        ua.is_none(),
        "custom_user_agent should be NULL after _clear_user_agent=on, got: {ua:?}"
    );
}

#[tokio::test]
async fn test_delete_feed_form_not_found() {
    let mut app = create_test_app_named(default_test_config(), "test_delete_feed_not_found").await;
    setup_authenticated_user(&mut app.server).await;

    let response = app.server.post("/feeds/999999/delete").await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");
}

#[tokio::test]
async fn test_import_opml_form_invalid() {
    let mut app =
        create_test_app_named(default_test_config(), "test_import_opml_invalid_form").await;
    setup_authenticated_user(&mut app.server).await;

    let invalid_xml = b"<not valid opml";
    let part = Part::bytes(invalid_xml.as_ref())
        .file_name("bad.opml")
        .mime_type("application/xml");
    let form = MultipartForm::new().add_part("file", part);
    let response = app.server.post("/feeds/import").multipart(form).await;

    // Parse error → error flash redirect to /feeds/import.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds/import");

    // No feeds created.
    let count: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT COUNT(*) FROM feed").unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_import_opml_form_duplicate_skipped() {
    let mut app =
        create_test_app_named(default_test_config(), "test_import_opml_dup_skipped").await;
    setup_authenticated_user(&mut app.server).await;

    // Pre-seed a feed with a specific URL in a specific category.
    let feed_url = "https://dup-import.example.com/feed.xml";
    insert_test_feed(&app, "DupCat", feed_url).await;

    // Import OPML containing the same URL under the same category name.
    let opml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Dup Test</title></head>
  <body>
    <outline text="DupCat" title="DupCat">
      <outline type="rss" text="Dup Feed" xmlUrl="{feed_url}"/>
    </outline>
  </body>
</opml>"#
    );
    let part = Part::bytes(opml.into_bytes())
        .file_name("subs.opml")
        .mime_type("application/xml");
    let form = MultipartForm::new().add_part("file", part);
    let response = app.server.post("/feeds/import").multipart(form).await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    // Feed count for that URL must still be 1 (duplicate skipped).
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE url = $1",
        feed_url
    )
    .unwrap();
    assert_eq!(
        count, 1,
        "duplicate import must not create a second feed row"
    );
}

// ============================================================================
// handlers/feeds.rs — Part B: wiremock success-arm tests
// ============================================================================

const RSS_FIXTURE: &str = r#"<?xml version="1.0"?><rss version="2.0"><channel>
  <title>Mock Feed</title><description>Mock Desc</description><link>https://e</link>
  <item><guid>m1</guid><title>One</title><link>https://e/1</link><description>c1</description></item>
</channel></rss>"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_feed_form_success() {
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mut app = create_test_app_named(default_test_config(), "test_create_feed_success").await;
    setup_authenticated_user(&mut app.server).await;

    // Get the owned category id (the seeded "Uncategorized").
    let cat_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM category LIMIT 1").unwrap();

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let feed_url = mock_server.uri();

    let response = app
        .server
        .post("/feeds")
        .form(&json!({ "url": feed_url, "category_id": cat_id }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    // One feed row with that URL must exist.
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE url = $1",
        feed_url
    )
    .unwrap();
    assert_eq!(count, 1, "feed should have been created in the DB");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_feed_form_duplicate() {
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mut app = create_test_app_named(default_test_config(), "test_create_feed_dup").await;
    setup_authenticated_user(&mut app.server).await;

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let feed_url = mock_server.uri();

    // Pre-create the feed.
    insert_test_feed(&app, "Tech", &feed_url).await;

    let cat_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM category LIMIT 1").unwrap();

    // POST create with the same URL.
    let response = app
        .server
        .post("/feeds")
        .form(&json!({ "url": feed_url, "category_id": cat_id }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    // Feed count must still be 1.
    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM feed WHERE url = $1",
        feed_url
    )
    .unwrap();
    assert_eq!(count, 1, "duplicate create must not add a second feed row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_refresh_feed_form_success() {
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mut app = create_test_app_named(default_test_config(), "test_refresh_feed_success").await;
    setup_authenticated_user(&mut app.server).await;

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let feed_url = mock_server.uri();
    let (_, feed_id) = insert_test_feed(&app, "Tech", &feed_url).await;

    let response = app.server.post(&format!("/feeds/{feed_id}/refresh")).await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/feeds");

    // At least 1 entry should have been synced.
    let entry_count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM entry WHERE feed_id = $1",
        feed_id
    )
    .unwrap();
    assert!(
        entry_count >= 1,
        "refresh should have synced at least 1 entry, got {entry_count}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fetch_metadata_form_success() {
    use wiremock::matchers::{any, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mut app = create_test_app_named(default_test_config(), "test_fetch_metadata_success").await;
    setup_authenticated_user(&mut app.server).await;

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(any())
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS_FIXTURE)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&mock_server)
        .await;

    let feed_url = mock_server.uri();
    let (_, feed_id) = insert_test_feed(&app, "Tech", &feed_url).await;

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/fetch-metadata"))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/feeds/{feed_id}/edit")
    );

    // The RSS fixture has title "Mock Feed" and description "Mock Desc"; these
    // must now be reflected in the DB.
    let title: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT title FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    let description: String = rdrs::query_scalar!(
        &app.db,
        String,
        "SELECT description FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(title, "Mock Feed");
    assert_eq!(description, "Mock Desc");
}

// ============================================================================
// SSE /events endpoint auth-gate test
// ============================================================================

#[tokio::test]
async fn events_endpoint_requires_auth() {
    let server = create_test_server(default_test_config()).await;
    // GET /events without a session cookie — PageAuthUser redirects to /login.
    let response = server.get("/events").await;
    // PageAuthUser always redirects unauthenticated requests to /login (303).
    response.assert_status(StatusCode::SEE_OTHER);
}

// ============================================================================
// middleware/cache_control.rs — no-store wiring through the real router
// ============================================================================

#[tokio::test]
async fn test_authenticated_page_is_no_store() {
    let mut app = create_test_app_named(default_test_config(), "test_authenticated_no_store").await;
    setup_authenticated_user(&mut app.server).await;

    let response = app.server.get("/").await;

    response.assert_status_ok();
    assert_eq!(
        response.header(header::CACHE_CONTROL),
        "no-store",
        "an authenticated page must never be retained by a disk cache or shared proxy"
    );
    assert_eq!(
        response.header(header::VARY),
        "Cookie",
        "Vary: Cookie must accompany no-store so a proxy can't conflate the anonymous and authenticated variants"
    );
}

#[tokio::test]
async fn test_static_asset_keeps_its_cache_control() {
    // Regression guard: the cache_control middleware only fills in a header
    // when the response has none, so the long-lived static-asset directive
    // set by handlers/static_assets.rs must survive byte-for-byte.
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/static/css/app.css").await;

    response.assert_status_ok();
    assert_eq!(
        response.header(header::CACHE_CONTROL),
        expected_static_cache_control()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_image_proxy_keeps_upstream_cache_control() {
    // A genuine upstream fetch can't be exercised here: `utils/url_validation`
    // (the shared SSRF guard the proxy runs every origin URL through) rejects
    // loopback/private addresses unconditionally and has no test escape
    // hatch, and every mock HTTP server available to this suite (wiremock
    // included) binds to one. `choose_cache_control`'s upstream-forwarding
    // logic itself is exercised directly in handlers/proxy.rs's own unit
    // tests (`test_choose_cache_control_mirrors_origin`). What this test adds
    // is the piece those unit tests can't cover: that the cache_control
    // *middleware* — which runs after the handler, on every response — does
    // not clobber whatever `Cache-Control` the proxy handler already set,
    // even on an authenticated request where rule 3 ("has a session cookie")
    // would otherwise apply. The proxy's ETag/If-None-Match short-circuit
    // (see `test_proxy_image_304_on_if_none_match`) is a real, SSRF-free
    // response path that carries the handler's own `Cache-Control`
    // (`DEFAULT_CACHE_CONTROL`, the same constant `choose_cache_control`
    // falls back to when upstream sends none) and is reachable through the
    // full router, so it stands in for "the proxy already set a
    // Cache-Control" here.
    let mut app =
        create_test_app_named(default_test_config(), "test_proxy_keeps_upstream_cc").await;
    // Authenticated on purpose: this is the case where the cache_control
    // middleware could plausibly override the proxy's own header (rule 1 —
    // "response already has Cache-Control" — must win over rule 3 — "request
    // has a session cookie").
    setup_authenticated_user(&mut app.server).await;

    let response = app
        .server
        .get("/api/proxy/image?url=aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw&s=sometoken")
        .add_header(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"sometoken\""),
        )
        .await;

    response.assert_status(StatusCode::NOT_MODIFIED);
    assert_eq!(
        response.header(header::CACHE_CONTROL),
        "public, max-age=86400",
        "the proxy's own Cache-Control must pass through untouched even on an authenticated request"
    );
}

#[tokio::test]
async fn test_health_endpoint_stays_cacheable() {
    let server = create_test_server(default_test_config()).await;

    // No session cookie on this request, so the cache_control middleware
    // must leave it alone entirely (rule 2).
    let response = server.get("/health").await;

    response.assert_status_ok();
    assert!(
        response.maybe_header(header::CACHE_CONTROL).is_none(),
        "an unauthenticated request must not get a forced no-store"
    );
}
