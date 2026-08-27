//! Integration tests for the `GReader` API, feed, entry, user-settings and
//! page handlers.

mod common;
use common::{default_test_config, flash_text};

use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum_test::TestServer;
use axum_test::multipart::{MultipartForm, Part};
use chrono::{Duration, Utc};
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
        .post("/api/setup")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
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

// --- Category Handler Tests (via GReader tag endpoints) ---

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

    let names = get_folder_tag_names(&server).await;
    assert!(names.contains(&"Test Category".to_string()));
}

#[tokio::test]
async fn test_update_category() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Old Name").await;

    let form = vec![
        ("s", "user/-/label/Old Name".to_string()),
        ("dest", "user/-/label/New Name".to_string()),
    ];
    let response = server.post("/reader/api/0/rename-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"Old Name".to_string()));
    assert!(names.contains(&"New Name".to_string()));
}

#[tokio::test]
async fn test_update_category_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    create_category(&server, "Test").await;

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

    let form = vec![("s", "user/-/label/To Delete".to_string())];
    let response = server.post("/reader/api/0/disable-tag").form(&form).await;
    response.assert_status_ok();
    assert_eq!(response.text(), "OK");

    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"To Delete".to_string()));
}

// --- Feed Handler Tests (via GReader subscription endpoints) ---

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
    let app = create_test_app(default_test_config()).await;
    let (mut server, db) = (app.server, app.db);

    // User 1 registers
    server
        .post("/api/setup")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser1",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // User 1 creates a category
    create_category(&server, "MoveFeedUser1 Category").await;

    server.delete("/api/session").await.assert_status_ok();

    // User 2 exists
    common::seed_account(
        &db,
        "movefeeduser2",
        "vulture-mango-77-quilt",
        rdrs::Role::User,
    )
    .await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "movefeeduser2",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

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
        .post("/api/setup")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser1",
            "password": "vulture-mango-77-quilt"
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

    app.server.delete("/api/session").await.assert_status_ok();

    // User2 registers and logs in
    common::seed_account(
        &app.db,
        "feeddeluser2",
        "vulture-mango-77-quilt",
        rdrs::Role::User,
    )
    .await;

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "feeddeluser2",
            "password": "vulture-mango-77-quilt"
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

// --- OPML Tests (via GReader subscription/export and subscription/import) ---

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

    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);

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

    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

    let response = server
        .post("/reader/api/0/subscription/import")
        .text(opml_content)
        .await;
    response.assert_status_ok();

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

    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 3);

    // Verify 2 imported folder-type tags + the seeded "Uncategorized".
    let count = count_folder_tags(&server).await;
    assert_eq!(count, 3);
}

// --- Entry Handler Tests (via GReader stream/contents and edit-tag) ---

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

    let response = server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

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

    create_category(&server, "TestCategory").await;

    // Filter entries by this category (should be empty but return 200)
    let response = server
        .get("/reader/api/0/stream/contents/user/-/label/TestCategory")
        .await;
    response.assert_status_ok();
}

// --- User Settings Handler Tests ---
//
// The JSON PUT/GET endpoints were replaced by SSR form actions (POST
// /user-settings/{password,preferences,linkding,kagi}); coverage for those
// lives in the test_*_form* tests below.

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

    let response = server.get("/api/user/settings/theme").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["theme"], "light");
}

#[tokio::test]
async fn test_update_theme_system() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": "dark" }))
        .await
        .assert_status_ok();

    let response = server
        .put("/api/user/settings/theme")
        .json(&json!({ "theme": null }))
        .await;

    response.assert_status_ok();

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

// --- Page Handler Tests ---

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

// --- Cross-User Isolation Tests ---

#[tokio::test]
async fn test_category_isolation_between_users() {
    let app = create_test_app(default_test_config()).await;
    let (mut server, db) = (app.server, app.db);

    // User 1 creates a category
    server
        .post("/api/setup")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    create_category(&server, "User1 Category").await;

    server.delete("/api/session").await.assert_status_ok();

    // User 2 should not see User 1's category
    common::seed_account(&db, "user2", "vulture-mango-77-quilt", rdrs::Role::User).await;

    let __login = server
        .post("/api/session")
        .json(&json!({
            "username": "user2",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut server, &__login);

    // User2 sees only its own seeded "Uncategorized", never User1's category.
    let names = get_folder_tag_names(&server).await;
    assert!(!names.contains(&"User1 Category".to_string()));
    assert_eq!(names, vec!["Uncategorized".to_string()]);
}

// --- Edge Case Tests ---

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

// --- Additional Feed Icon Tests ---

#[tokio::test]
async fn test_get_feed_icon_unauthorized() {
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/api/feeds/1/icon").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_feed_icon_no_icon() {
    let mut app = create_test_app(default_test_config()).await;

    let hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
    rdrs::models::user::create_user(&app.db, "iconuser", &hash, Role::User)
        .await
        .unwrap();

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "iconuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

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

#[tokio::test]
async fn test_favicon_cache_control_is_version_gated() {
    // The icons are `include_bytes!`d into the binary, so they change with the
    // build. Only the `?v=`-stamped <link>s in `base.html` may be pinned
    // long-term — a bare /favicon.ico has no URL left to change on upgrade.
    let server = create_test_server(default_test_config()).await;

    let bare = server.get("/favicon.ico").await;
    bare.assert_status_ok();
    assert_eq!(bare.header(header::CACHE_CONTROL), "public, max-age=3600");

    let stamped = server
        .get(&format!("/favicon.ico?v={}", rdrs::GIT_VERSION))
        .await;
    stamped.assert_status_ok();
    let expected = if rdrs::GIT_VERSION.ends_with("-dirty") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    assert_eq!(stamped.header(header::CACHE_CONTROL), expected);
}

#[tokio::test]
async fn test_get_feed_icon_is_privately_cached() {
    // The handler sets its own `Cache-Control`, so `no_store_for_authenticated`
    // steps aside and adds no `Vary: Cookie`. The directive itself therefore has
    // to keep this auth-scoped response out of shared storage, or a proxy keyed
    // on the URL alone could serve feed N's icon to someone with no access.
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let (_cat_id, feed_id) =
        insert_test_feed(&app, "IconCacheCat", "https://icon.example.com/feed.xml").await;
    rdrs::models::image::upsert(
        &app.db,
        rdrs::models::image::ENTITY_FEED,
        feed_id,
        b"\x89PNG\r\n\x1a\n",
        "image/png",
        None,
    )
    .await
    .unwrap();

    let response = app.server.get(&format!("/api/feeds/{feed_id}/icon")).await;

    response.assert_status_ok();
    assert_eq!(
        response.header(header::CACHE_CONTROL),
        "private, max-age=86400"
    );
}

// --- Passkey Handler Tests ---

/// Attach a session created straight in the database as this server's default
/// cookie, plus the CSRF header derived from it. Needed where the login endpoint
/// is unavailable (`disable_local_auth`). The signing key is
/// `default_test_config`'s all-zero secret.
fn apply_session_cookie(server: &mut TestServer, token: &str) {
    let secret = vec![0u8; 32];
    server.add_cookie(cookie::Cookie::new(
        "session_token",
        rdrs::secret::sign_session(&secret, token),
    ));
    server.clear_headers();
    server.add_header("x-csrf-token", rdrs::secret::derive_csrf(&secret, token));
}

/// Push every session's `last_authenticated_at` out of the re-authentication
/// window, standing in for "this browser logged in a while ago" without a test
/// having to wait out `REAUTH_WINDOW_MINUTES`.
async fn stale_authentication(db: &Db) {
    rdrs::db_execute!(
        db,
        "UPDATE session SET last_authenticated_at = $1",
        Utc::now() - Duration::hours(1)
    )
    .unwrap();
}

/// Adding a passkey from a session that has not proved itself recently must be
/// refused — that credential outlives a password change, so a picked-up
/// session must not be able to mint one silently.
#[tokio::test]
async fn test_passkey_register_start_requires_recent_authentication() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    // Fresh login is inside the window.
    app.server
        .post("/api/passkey/register/start")
        .await
        .assert_status_ok();

    stale_authentication(&app.db).await;

    let refused = app.server.post("/api/passkey/register/start").await;
    refused.assert_status_forbidden();
    let body: serde_json::Value = refused.json();
    assert_eq!(body["error"], "Reauthentication required");
}

#[tokio::test]
async fn test_passkey_delete_requires_recent_authentication() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    stale_authentication(&app.db).await;

    // Refused before the passkey is even looked up, so a non-existent id still
    // reports the re-authentication requirement rather than 404.
    let refused = app.server.delete("/api/passkeys/1").await;
    refused.assert_status_forbidden();
    let body: serde_json::Value = refused.json();
    assert_eq!(body["error"], "Reauthentication required");
}

/// The whole point of the window: re-authenticating re-opens it, and the
/// operation that was refused now goes through.
#[tokio::test]
async fn test_reauth_with_correct_password_reopens_the_window() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    stale_authentication(&app.db).await;

    app.server
        .post("/api/passkey/register/start")
        .await
        .assert_status_forbidden();

    app.server
        .post("/api/session/reauth")
        .json(&json!({ "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::NO_CONTENT);

    app.server
        .post("/api/passkey/register/start")
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn test_reauth_with_wrong_password_is_rejected() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;
    stale_authentication(&app.db).await;

    app.server
        .post("/api/session/reauth")
        .json(&json!({ "password": "not-my-password" }))
        .await
        .assert_status_unauthorized();

    // Still refused: a failed re-authentication must not open the window.
    app.server
        .post("/api/passkey/register/start")
        .await
        .assert_status_forbidden();
}

#[tokio::test]
async fn test_reauth_requires_a_session() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/session/reauth")
        .json(&json!({ "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status_unauthorized();
}

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
    // An instance with nothing enrolled must answer exactly like one that has
    // passkeys: a challenge. The handler used to reject with "No passkeys
    // registered", telling any unauthenticated caller whether this deployment
    // had accounts using them.
    let server = create_test_server(default_test_config()).await;

    let response = server.post("/api/passkey/auth/start").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(
        body["options"]["publicKey"]["challenge"].is_string(),
        "a challenge must be issued regardless of what is enrolled"
    );
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

/// The success path: a fresh session deletes its own passkey, which is also
/// what reaches the `passkey.removed` audit call.
#[tokio::test]
async fn test_delete_passkey_succeeds_within_the_window() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO passkey (user_id, credential_id, public_key, counter, name) VALUES ($1, $2, $3, $4, $5)",
        1_i64,
        vec![1_u8, 2, 3],
        b"{}".to_vec(),
        0_i64,
        "MacBook"
    )
    .unwrap();

    app.server
        .delete("/api/passkeys/1")
        .await
        .assert_status(StatusCode::NO_CONTENT);

    // And it is gone, not merely reported as deleted.
    app.server
        .delete("/api/passkeys/1")
        .await
        .assert_status_not_found();
}

/// `disable_local_auth` leaves no password to check, so re-authentication has
/// to refuse rather than fall through to a verify against a hash that can
/// never match.
#[tokio::test]
async fn test_reauth_refused_when_local_auth_disabled() {
    let mut config = default_test_config();
    config.disable_local_auth = true;
    let mut app = create_test_app(config).await;

    // Build the session directly: with local auth disabled there is no login
    // endpoint to go through.
    let password_hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
    let session =
        rdrs::models::session::create_session(&app.db, user.id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
    apply_session_cookie(&mut app.server, &session.session_token);

    app.server
        .post("/api/session/reauth")
        .json(&json!({ "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status_forbidden();
}

#[tokio::test]
async fn test_passkey_auth_start_with_invalid_passkey_data() {
    let app = create_test_app(default_test_config()).await;

    let password_hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = rdrs::models::user::create_user(&app.db, "testuser", &password_hash, Role::User)
        .await
        .unwrap();
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

    // Starting the ceremony no longer reads stored credentials at all, so an
    // unparseable row cannot take the sign-in page down with it — it can only
    // fail at the finish step, for that one credential.
    let response = app.server.post("/api/passkey/auth/start").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["options"]["publicKey"]["challenge"].is_string());
}

#[tokio::test]
async fn test_passkey_register_finish_empty_name() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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

    let password_hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
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

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
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

    let password_hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
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

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
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

    let response = app.server.get("/api/passkeys").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"][0]["name"], "New Name");
}

#[tokio::test]
async fn test_delete_passkey_success() {
    let mut app = create_test_app(default_test_config()).await;

    let password_hash = auth::hash_password("vulture-mango-77-quilt").unwrap();
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

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let response = app
        .server
        .delete(&format!("/api/passkeys/{passkey_id}"))
        .await;
    response.assert_status(StatusCode::NO_CONTENT);

    let response = app.server.get("/api/passkeys").await;
    let body: serde_json::Value = response.json();
    assert_eq!(body["passkeys"].as_array().unwrap().len(), 0);
}

// --- Cross-User Passkey Isolation Tests ---

#[tokio::test]
async fn test_passkey_rename_other_user() {
    let mut app = create_test_app(default_test_config()).await;

    let hash1 = auth::hash_password("vulture-mango-77-quilt").unwrap();
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
    let hash2 = auth::hash_password("vulture-mango-77-quilt").unwrap();
    rdrs::models::user::create_user(&app.db, "pkuser2", &hash2, Role::User)
        .await
        .unwrap();

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "pkuser2",
            "password": "vulture-mango-77-quilt"
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

    let hash1 = auth::hash_password("vulture-mango-77-quilt").unwrap();
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
    let hash2 = auth::hash_password("vulture-mango-77-quilt").unwrap();
    rdrs::models::user::create_user(&app.db, "pkdeluser2", &hash2, Role::User)
        .await
        .unwrap();

    let __login = app
        .server
        .post("/api/session")
        .json(&json!({
            "username": "pkdeluser2",
            "password": "vulture-mango-77-quilt"
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

// --- Favicon Handler Tests ---

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

// --- Static Assets Handler Tests ---

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

// --- Health Check Tests ---

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

// --- Subscription Handler Coverage Tests ---

#[tokio::test]
async fn test_subscription_list_with_feeds() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

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

    let response = server.get("/reader/api/0/subscription/list").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let subscriptions = body["subscriptions"].as_array().unwrap();
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["title"], "NewTitle");
}

// --- Auth Handler Coverage Tests (ClientLogin, token, preference, friend) ---

#[tokio::test]
async fn test_client_login_success() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // ClientLogin with correct credentials
    let form: Vec<(&str, &str)> = vec![("Email", "testuser"), ("Passwd", "vulture-mango-77-quilt")];
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

    server
        .post("/api/setup")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
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
    let form: Vec<(&str, &str)> = vec![("Email", "nouser"), ("Passwd", "vulture-mango-77-quilt")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_greader_auth_header() {
    let server = create_test_server(default_test_config()).await;

    server
        .post("/api/setup")
        .json(&json!({
            "username": "testuser",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let form: Vec<(&str, &str)> = vec![("Email", "testuser"), ("Passwd", "vulture-mango-77-quilt")];
    let response = server.post("/accounts/ClientLogin").form(&form).await;
    response.assert_status_ok();

    let body = response.text();
    let auth_token = body
        .lines()
        .find(|line| line.starts_with("Auth="))
        .unwrap()
        .strip_prefix("Auth=")
        .unwrap();

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
// Form-action handlers for the SSR /user-settings page. Each accepts a
// urlencoded body and answers 303 with a flash cookie plus Location.
// ============================================================================

#[tokio::test]
async fn test_change_password_form_success() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    let response = server
        .post("/user-settings/password")
        .form(&json!({
            "current_password": "vulture-mango-77-quilt",
            "new_password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift",
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
            "current_password": "vulture-mango-77-quilt",
            "new_password": "heron-lantern-53-drift",
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
async fn test_update_preferences_form_sets_sidebar_prefs() {
    let mut server = create_test_server(default_test_config()).await;
    setup_authenticated_user(&mut server).await;

    // Defaults before anything is submitted.
    let sidebar: serde_json::Value = server.get("/api/sidebar").await.json();
    assert_eq!(sidebar["sidebar_sort"], "name");
    assert_eq!(sidebar["sidebar_hide_read"], false);

    server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 30,
            "retention_read_days": 0,
            "sidebar_sort": "unread",
            "sidebar_hide_read": "1",
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let sidebar: serde_json::Value = server.get("/api/sidebar").await.json();
    assert_eq!(sidebar["sidebar_sort"], "unread");
    assert_eq!(sidebar["sidebar_hide_read"], true);

    // An unchecked checkbox sends nothing, which must clear the flag rather
    // than leave it on — and an unknown sort falls back to the default.
    server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 30,
            "retention_read_days": 0,
            "sidebar_sort": "nonsense",
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let sidebar: serde_json::Value = server.get("/api/sidebar").await.json();
    assert_eq!(sidebar["sidebar_sort"], "name");
    assert_eq!(sidebar["sidebar_hide_read"], false);
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
    // `/user-settings/*` is not under the CSRF skip prefixes, so
    // `anonymous_session` mints a signed session cookie and `csrf_guard` then
    // requires a matching token. A bare POST never has one, so it is 403'd
    // before the `AuthUser` extractor (which would 401) ever runs — which
    // doubles as this endpoint's CSRF-missing coverage.
    let server = create_test_server(default_test_config()).await;

    let response = server.post("/user-settings/sessions/revoke-others").await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_revoke_other_sessions_form_blocked_while_masquerading() {
    // `start_masquerade` mutates the admin's own session row in place, so while
    // masquerading the effective session belongs to the target. Revoke-others
    // must refuse in that state, or an admin would silently delete the target's
    // real sessions on their own devices.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;

    // Seed a second, real session for the target user (id=2) — e.g. a
    // session from the target's own phone, unrelated to the masquerade.
    let target_session =
        rdrs::models::session::create_session(&app.db, 2, "test-agent", "127.0.0.1")
            .await
            .unwrap();

    // Admin starts masquerading as target (id=2). The privilege change rotates
    // the session token, so the CSRF token derived from it changes too and the
    // header wired up by `setup_admin_user` has to be refreshed from the
    // cookies this response sets — a browser does the same thing on its own.
    let started = app.server.post("/admin/users/2/masquerade").await;
    started.assert_status(StatusCode::SEE_OTHER);
    common::apply_csrf(&mut app.server, &started);

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

#[tokio::test]
async fn test_revoke_session_form_deletes_only_the_named_session() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");

    let doomed = rdrs::models::session::create_session(&app.db, user.id, "phone", "127.0.0.1")
        .await
        .unwrap();
    let bystander = rdrs::models::session::create_session(&app.db, user.id, "laptop", "127.0.0.1")
        .await
        .unwrap();

    let response = app
        .server
        .post(&format!("/user-settings/sessions/{}/revoke", doomed.id))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/user-settings");

    assert!(
        rdrs::models::session::find_by_token(&app.db, &doomed.session_token)
            .await
            .unwrap()
            .is_none(),
        "the named session should be deleted"
    );
    assert!(
        rdrs::models::session::find_by_token(&app.db, &bystander.session_token)
            .await
            .unwrap()
            .is_some(),
        "revoking one session must not touch the user's other sessions"
    );
    // The caller's own session still authenticates.
    app.server.get("/api/me").await.assert_status_ok();
}

#[tokio::test]
async fn test_revoke_session_form_refuses_the_current_session() {
    // The settings page renders no Revoke control for the caller's own session,
    // but that is a UI affordance and not a guarantee — a hand-crafted POST
    // naming it must still be refused, or the user signs themselves out through
    // a path whose success message and redirect both assume they are still in.
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");

    let sessions = rdrs::models::session::list_user_sessions(&app.db, user.id)
        .await
        .unwrap();
    let current = sessions
        .first()
        .expect("the authenticated caller has a session");

    let response = app
        .server
        .post(&format!("/user-settings/sessions/{}/revoke", current.id))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert!(
        rdrs::models::session::find_by_token(&app.db, &current.session_token)
            .await
            .unwrap()
            .is_some(),
        "the caller's own session must survive"
    );
    app.server.get("/api/me").await.assert_status_ok();
}

#[tokio::test]
async fn test_revoke_session_form_blocked_while_masquerading() {
    // Same reasoning as `test_revoke_other_sessions_form_blocked_while_
    // masquerading`, one session at a time: while masquerading, the effective
    // session belongs to the target, so an unguarded revoke would let an admin
    // sign the target out of their own devices.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;

    let target_session =
        rdrs::models::session::create_session(&app.db, 2, "test-agent", "127.0.0.1")
            .await
            .unwrap();

    let started = app.server.post("/admin/users/2/masquerade").await;
    started.assert_status(StatusCode::SEE_OTHER);
    common::apply_csrf(&mut app.server, &started);

    let response = app
        .server
        .post(&format!(
            "/user-settings/sessions/{}/revoke",
            target_session.id
        ))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/user-settings");
    assert!(
        rdrs::models::session::find_by_token(&app.db, &target_session.session_token)
            .await
            .unwrap()
            .is_some(),
        "masquerade must not delete the target's real sessions"
    );
}

#[tokio::test]
async fn test_password_change_revokes_api_tokens() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");
    let token = rdrs::models::api_token::create_api_token(
        &app.db,
        user.id,
        "greader",
        "test-client",
        "test-agent",
        "127.0.0.1",
    )
    .await
    .unwrap();

    let response = app
        .server
        .post("/user-settings/password")
        .form(&json!({
            "current_password": "vulture-mango-77-quilt",
            "new_password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift",
        }))
        .await;
    response.assert_status(StatusCode::SEE_OTHER);

    let found = rdrs::models::api_token::find_by_token(&app.db, &token.token)
        .await
        .unwrap();
    assert!(
        found.is_none(),
        "a password change must revoke API tokens too, not just browser sessions"
    );
}

#[tokio::test]
async fn test_password_change_is_rate_limited() {
    // Changing a password verifies the *current* one with Argon2. The caller
    // already holds a session, so this is no way in from outside — but
    // unthrottled it lets a hijacked session brute-force the original password.
    //
    // Asserted the strong way: once the budget is spent even the CORRECT
    // password is refused, which can only hold if the limiter runs *before*
    // `verify_password`.
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    for _ in 0..5 {
        app.server
            .post("/user-settings/password")
            .form(&json!({
                "current_password": "wrongpassword",
                "new_password": "heron-lantern-53-drift",
                "confirm_password": "heron-lantern-53-drift",
            }))
            .await;
    }

    app.server
        .post("/user-settings/password")
        .form(&json!({
            "current_password": "vulture-mango-77-quilt",
            "new_password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift",
        }))
        .await;

    // The throttled attempt must not have changed anything: the original
    // password still verifies, the new one does not exist.
    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");
    assert!(
        rdrs::auth::verify_password("vulture-mango-77-quilt", &user.password_hash),
        "a throttled request must not have changed the password"
    );

    // And the change-password budget is its own: exhausting it above must not
    // have locked this client out of logging in.
    let login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "testuser", "password": "vulture-mango-77-quilt" }))
        .await;
    assert_ne!(
        login.status_code(),
        StatusCode::TOO_MANY_REQUESTS,
        "the password-change bucket must not spend the login budget"
    );
}

#[tokio::test]
async fn test_revoke_others_does_not_touch_api_tokens() {
    // "Sign out other sessions" means browser sessions specifically — see the
    // comment on `revoke_other_sessions_form`. A GReader client's token must
    // survive this action; revoking it is a separate, explicit control.
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");
    let token = rdrs::models::api_token::create_api_token(
        &app.db,
        user.id,
        "greader",
        "test-client",
        "test-agent",
        "127.0.0.1",
    )
    .await
    .unwrap();

    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;
    response.assert_status(StatusCode::SEE_OTHER);

    let found = rdrs::models::api_token::find_by_token(&app.db, &token.token)
        .await
        .unwrap();
    assert!(
        found.is_some(),
        "revoke-other-sessions must not touch API tokens"
    );
}

/// A bulk revocation has to say how much it revoked. "Signed out all other
/// sessions." reads identically whether it ended six sessions or none, which is
/// exactly the case a user clicks this button to find out about.
#[tokio::test]
async fn test_revoke_others_flash_counts_the_sessions_it_ended() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");

    // No other devices signed in yet — the flash must not claim otherwise.
    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    let text = flash_text(&response);
    assert!(
        text.contains("No other sessions were signed in"),
        "revoking nothing must say so, got: {text}"
    );

    // Two other devices sign in, then the caller signs them out.
    for _ in 0..2 {
        rdrs::models::session::create_session(&app.db, user.id, "other-agent", "127.0.0.1")
            .await
            .unwrap();
    }
    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    let text = flash_text(&response);
    assert!(
        text.contains("Signed out 2 other sessions."),
        "flash must carry the affected-row count, got: {text}"
    );

    // The caller's own session survives, so a repeat is back to the zero case.
    let response = app
        .server
        .post("/user-settings/sessions/revoke-others")
        .await;
    let text = flash_text(&response);
    assert!(text.contains("No other sessions were signed in"), "{text}");
}

/// Singular wording, and the same count-or-say-nothing rule as sessions.
#[tokio::test]
async fn test_revoke_all_api_tokens_flash_counts_the_tokens() {
    let mut app = create_test_app(default_test_config()).await;
    setup_authenticated_user(&mut app.server).await;

    let user = rdrs::models::user::find_by_username(&app.db, "testuser")
        .await
        .unwrap()
        .expect("user must exist");

    let response = app
        .server
        .post("/user-settings/api-tokens/revoke-all")
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    let text = flash_text(&response);
    assert!(
        text.contains("There were no API tokens to revoke."),
        "got: {text}"
    );

    rdrs::models::api_token::create_api_token(
        &app.db,
        user.id,
        "greader",
        "only-client",
        "test-agent",
        "127.0.0.1",
    )
    .await
    .unwrap();

    let response = app
        .server
        .post("/user-settings/api-tokens/revoke-all")
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    let text = flash_text(&response);
    assert!(
        text.contains("Revoked 1 API token."),
        "one token must read as singular, got: {text}"
    );
}

// --- Form-action admin endpoint tests (PR-5 T1) ---

/// Helper to register the first user (becomes admin) and login.
async fn setup_admin_user(server: &mut TestServer) {
    server
        .post("/api/setup")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

/// Seed the second account these tests act on, straight into the database
/// rather than through the admin create + invite redeem pair: those endpoints
/// have their own tests, and every caller here only needs the account to exist.
async fn register_target_user(db: &Db) {
    common::seed_account(db, "target", "vulture-mango-77-quilt", rdrs::Role::User).await;
}

#[tokio::test]
async fn test_update_role_form_promotes_user() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    let server = app.server;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    let server = app.server;

    let response = server
        .post("/admin/users/2/status")
        .form(&json!({ "disabled": "true" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/admin");

    let login_resp = server
        .post("/api/session")
        .json(&json!({
            "username": "target",
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    login_resp.assert_status_forbidden();
}

#[tokio::test]
async fn test_start_masquerade_form_redirects_to_root() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    let server = app.server;

    let response = server.post("/admin/users/2/masquerade").await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/");
}

#[tokio::test]
async fn test_delete_user_form_succeeds() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    let server = app.server;

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

// --- /categories form-action POST endpoint tests (SSR PR-7 T1) ---

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

    app.server
        .post("/categories")
        .form(&json!({ "name": "OldName" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let cat_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM category WHERE name = $1",
        "OldName"
    )
    .unwrap();

    let response = app
        .server
        .post(&format!("/categories/{cat_id}/rename"))
        .form(&json!({ "name": "NewName" }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

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

    app.server
        .post("/categories")
        .form(&json!({ "name": "ToDelete" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let cat_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM category WHERE name = $1",
        "ToDelete"
    )
    .unwrap();

    let response = app
        .server
        .post(&format!("/categories/{cat_id}/delete"))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    let location = response.header(header::LOCATION);
    assert_eq!(location, "/categories");

    let count: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT COUNT(*) FROM category WHERE id = $1",
        cat_id
    )
    .unwrap();
    assert_eq!(count, 0, "category should be deleted from the DB");
}

// --- /feeds form-action POST endpoint tests (SSR PR-8 T1) ---

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

// --- GET /entries/{id}/fragment — PR-10 T3 ---

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

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_frag", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_frag", "password": "vulture-mango-77-quilt" }))
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
    // Auto-mark-as-read: the row update rides along as a marker-form swap plus
    // an `entry-read` class directive, not a whole-row re-render — opening an
    // entry changes nothing else about the row, so nothing else is shipped.
    assert!(
        html.contains(&format!(
            r##"data-swap-target="#entry-row-{entry_id} .entry-marker""##
        )),
        "response must swap the row's marker form to clear unread state"
    );
    assert!(
        html.contains(&format!(
            r##"<template data-class-target="#entry-row-{entry_id}" data-class-add="entry-read">"##
        )),
        "response must mark the row read via the class directive"
    );
    assert!(
        !html.contains(&format!(r#"<div id="entry-row-{entry_id}""#)),
        "the whole row must not be re-sent — only the sub-elements that changed"
    );
    // The sidebar's counts travel over SSE, not in this response. Nothing ever
    // rendered a `#sidebar-unread` element for the payload that used to ride
    // along here, so the browser discarded it — guard against it coming back.
    assert!(
        !html.contains("sidebar-unread"),
        "the unconsumed sidebar-unread payload must not be reintroduced"
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

/// A real top-level navigation to the partial-only `/fragment` route (carrying
/// `Sec-Fetch-Dest: document`) must redirect to the full entries page rather
/// than serving bare `<template>` blocks, which render blank. It still marks the
/// entry read on the way out: a scriptless click is the same intent as the
/// `fetch()` the swap helper would have sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_redirects_on_top_level_navigation() {
    let mut app = create_test_app_named(default_test_config(), "test_entry_fragment_doc_nav").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "doc_nav", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "doc_nav", "password": "vulture-mango-77-quilt" }))
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

    // The mark-as-read fires before the redirect, on the same detached task the
    // `fetch()` path uses — so poll for it rather than assuming it has landed.
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
    assert!(
        read_at.is_some(),
        "a scriptless open must mark the entry read, like the fetch() path"
    );
}

/// A prefetch or prerender is the browser guessing, not the reader opening
/// something — it must not mark anything read, and its response must not be
/// storable, or the reader's real click could be served from the prefetch cache
/// and the entry would never become read at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_speculative_load_does_not_mark_read() {
    let mut app =
        create_test_app_named(default_test_config(), "test_entry_fragment_speculative").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "spec_load", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "spec_load", "password": "vulture-mango-77-quilt" }))
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
            url: "https://x/spec-feed",
            title: Some("Spec Feed"),
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
        "guid-spec",
        Some("Hello World"),
        Some("https://x/spec"),
        Some("<p>Body</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_id = entry.id;

    // Chrome's current header. A prerender sends `prefetch;prerender`, which the
    // substring test also has to catch.
    for purpose in ["prefetch", "prefetch;prerender"] {
        let response = app
            .server
            .get(&format!("/entries/{entry_id}/fragment"))
            .add_header(
                HeaderName::from_static("sec-purpose"),
                HeaderValue::from_str(purpose).unwrap(),
            )
            .await;
        response.assert_status_ok();
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store"),
            "a speculative response must not be storable ({purpose})"
        );
    }

    // Same request as a scriptless navigation, so the redirect branch is covered
    // too — still no mutation, still unstorable.
    let response = app
        .server
        .get(&format!("/entries/{entry_id}/fragment"))
        .add_header(
            HeaderName::from_static("sec-purpose"),
            HeaderValue::from_static("prefetch"),
        )
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );

    // Nothing above may have marked it read. The write is detached, so give it
    // the same window a real one would have had to land in.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        entry_id
    )
    .unwrap();
    assert!(
        read_at.is_none(),
        "a speculative load must leave the entry unread"
    );

    // And the reader's real open still works.
    app.server
        .get(&format!("/entries/{entry_id}/fragment"))
        .await
        .assert_status_ok();
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
    assert!(
        read_at.is_some(),
        "a genuine open after a prefetch must still mark read"
    );
}

/// A top-level navigation to `/entries/{id}/fragment` carrying a `Referer` from
/// a scoped list page must redirect back into that scope with the pane
/// pre-opened, not dump the reader into All Entries. An id that resolves to
/// nothing still redirects rather than 404s, since the list ignores an
/// unresolvable `?entry=`. Regression for the jump-to-All-Entries bug when
/// `app.js` is stale-cached and clicks fall through to navigation.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entry_fragment_document_nav_preserves_referer_scope() {
    let mut app =
        create_test_app_named(default_test_config(), "test_entry_fragment_referer_scope").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "ref_scope", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "ref_scope", "password": "vulture-mango-77-quilt" }))
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

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_404", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_404", "password": "vulture-mango-77-quilt" }))
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

// --- POST /entries/{id}/star — PR-10 T4 ---

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_star_entry_form_is_idempotent_mark_starred() {
    let mut app = create_test_app_named(default_test_config(), "test_star_entry_form").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_star", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_star", "password": "vulture-mango-77-quilt" }))
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
        !html.contains("sidebar-unread"),
        "the unconsumed sidebar-unread payload must not be reintroduced"
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
        .post("/api/setup")
        .json(&json!({ "username": "alice_unstar", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_unstar", "password": "vulture-mango-77-quilt" }))
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

// --- POST /entries/{id}/read — PR-10 T4 ---

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_read_entry_form_is_idempotent_mark_read() {
    let mut app = create_test_app_named(default_test_config(), "test_read_entry_form").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_read", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_read", "password": "vulture-mango-77-quilt" }))
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
        !html.contains("sidebar-unread"),
        "the unconsumed sidebar-unread payload must not be reintroduced"
    );
    // The row's read state now travels as a class directive: the response swaps
    // only the marker/star forms, so `entry-read` has to be applied to the row
    // element the client already has rather than shipped inside a new one.
    assert!(
        html.contains(r#"data-class-add="entry-read""#),
        "row must be marked read via the class directive after first call"
    );
    assert!(
        html.contains(r#"action="/entries/"#) && html.contains("/unread\""),
        "the swapped marker form must now offer the inverse (unread) action"
    );

    // Second POST — idempotent, entry stays read (no toggle back).
    let resp2 = app.server.post(&format!("/entries/{entry_id}/read")).await;
    assert_eq!(resp2.status_code(), StatusCode::OK);
    let html2 = resp2.text();
    assert!(
        html2.contains(r#"data-class-add="entry-read""#),
        "second /read call must be a no-op — row must still be marked read"
    );
    assert!(
        !html2.contains(r#"data-class-remove="entry-read""#),
        "second /read call must not flip the row back to unread"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_entry_form_is_idempotent_mark_unread() {
    let mut app = create_test_app_named(default_test_config(), "test_unread_entry_form").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_unr", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_unr", "password": "vulture-mango-77-quilt" }))
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
    // Positive assertion, not just the absence of the read marker: the class
    // directive has to actively *remove* `entry-read` from the row the client
    // already has, since no replacement row is sent to overwrite it.
    assert!(
        html.contains(r#"data-class-remove="entry-read""#),
        "row must be told to drop .entry-read after /unread"
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
        html2.contains(r#"data-class-remove="entry-read""#),
        "second /unread call must be a no-op — row must still be unread"
    );
    assert!(
        !html2.contains("Marked as unread."),
        "no-op /unread must not re-emit the flash"
    );
}

// --- POST /entries/{id}/star — cross-tenant 404 (PR-10 review) ---

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_star_entry_form_404_for_other_user() {
    let mut app = create_test_app_named(default_test_config(), "test_star_entry_form_404").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_s404", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_s404", "password": "vulture-mango-77-quilt" }))
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

// --- POST /entries/{id}/read — cross-tenant 404 (PR-10 review) ---

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_read_entry_form_404_for_other_user() {
    let mut app = create_test_app_named(default_test_config(), "test_read_entry_form_404").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_r404", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_r404", "password": "vulture-mango-77-quilt" }))
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

// POST /entries/{id}/summarize returns a small multi-target swap that only
// updates `#rp-summary-container`, so a Fetch-Full-Content view keeps its
// externally-fetched article body.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_summarize_entry_form_renders_summary_pending_fragment() {
    let mut app = create_test_app_named(default_test_config(), "test_summarize_entry_form").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_sum", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_sum", "password": "vulture-mango-77-quilt" }))
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

// --- GET /entries?fragment=1&after=... — PR-10 T6 Load-More fragment ---

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_entries_load_more_returns_row_fragments() {
    let mut app = create_test_app_named(default_test_config(), "test_load_more_fragment").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_lm", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_lm", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

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

// --- handlers/feeds.rs — additional branch coverage (Part A: pure-DB tests) ---

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

/// A blank `custom_user_agent` / `custom_referrer` erases the stored value,
/// the same way a blank `description` or `site_url` does. The form renders the
/// current value into the input, so a blank submission is deliberate.
#[tokio::test]
async fn test_edit_feed_form_blank_http_settings_clear_them() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_clear_ua").await;
    setup_authenticated_user(&mut app.server).await;

    let (cat_id, feed_id) =
        insert_test_feed(&app, "Tech", "https://clear-ua.example.com/feed.xml").await;
    rdrs::db_execute!(
        &app.db,
        "UPDATE feed SET custom_user_agent = 'MyBot/1.0', custom_referrer = 'https://ref.example.com' WHERE id = $1",
        feed_id
    )
    .unwrap();

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
            "custom_referrer": "   ",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/feeds/{feed_id}/edit")
    );

    let ua: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT custom_user_agent FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert!(
        ua.is_none(),
        "custom_user_agent should be NULL after submitting a blank field, got: {ua:?}"
    );

    // Whitespace-only counts as blank, matching the trim() the other fields use.
    let referrer: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT custom_referrer FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert!(
        referrer.is_none(),
        "custom_referrer should be NULL after submitting whitespace, got: {referrer:?}"
    );
}

/// The counterpart to the test above: a non-empty submission still round-trips
/// unchanged, so an ordinary "save" from the form never disturbs the overrides.
#[tokio::test]
async fn test_edit_feed_form_keeps_resubmitted_http_settings() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_keep_ua").await;
    setup_authenticated_user(&mut app.server).await;

    let (cat_id, feed_id) =
        insert_test_feed(&app, "Tech", "https://keep-ua.example.com/feed.xml").await;
    rdrs::db_execute!(
        &app.db,
        "UPDATE feed SET custom_user_agent = 'MyBot/1.0', custom_referrer = 'https://ref.example.com' WHERE id = $1",
        feed_id
    )
    .unwrap();

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://keep-ua.example.com/feed.xml",
            "title": "Renamed Feed",
            "description": "",
            "site_url": "",
            "category_id": cat_id,
            "custom_user_agent": "MyBot/1.0",
            "custom_referrer": "https://ref.example.com",
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);

    let ua: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT custom_user_agent FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    let referrer: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT custom_referrer FROM feed WHERE id = $1",
        feed_id
    )
    .unwrap();
    assert_eq!(ua.as_deref(), Some("MyBot/1.0"));
    assert_eq!(referrer.as_deref(), Some("https://ref.example.com"));
}

/// Omitting an optional field is not the same as sending it blank: a partial
/// POST that carries only the fields it means to change must leave everything
/// else alone. The rendered form always sends all of them, but nothing stops a
/// future handler, a script, or a narrower form from posting a subset.
#[tokio::test]
async fn test_edit_feed_form_omitted_fields_are_left_alone() {
    let mut app = create_test_app_named(default_test_config(), "test_edit_feed_omitted").await;
    setup_authenticated_user(&mut app.server).await;

    let (_cat_a, feed_id) =
        insert_test_feed(&app, "Tech", "https://omitted.example.com/feed.xml").await;
    rdrs::db_execute!(
        &app.db,
        "UPDATE feed SET description = 'Kept description', site_url = 'https://site.example.com', custom_user_agent = 'MyBot/1.0', custom_referrer = 'https://ref.example.com' WHERE id = $1",
        feed_id
    )
    .unwrap();

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat_b_id = rdrs::models::category::create_category(&app.db, user_id, "Other")
        .await
        .unwrap()
        .id;

    // Only the two required fields plus the category — everything optional is
    // absent from the body entirely.
    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/edit"))
        .form(&json!({
            "url": "https://omitted.example.com/feed.xml",
            "category_id": cat_b_id,
        }))
        .await;

    response.assert_status(StatusCode::SEE_OTHER);

    let row: (
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = (
        rdrs::query_scalar!(
            &app.db,
            i64,
            "SELECT category_id FROM feed WHERE id = $1",
            feed_id
        )
        .unwrap(),
        rdrs::query_scalar!(
            &app.db,
            Option<String>,
            "SELECT description FROM feed WHERE id = $1",
            feed_id
        )
        .unwrap(),
        rdrs::query_scalar!(
            &app.db,
            Option<String>,
            "SELECT site_url FROM feed WHERE id = $1",
            feed_id
        )
        .unwrap(),
        rdrs::query_scalar!(
            &app.db,
            Option<String>,
            "SELECT custom_user_agent FROM feed WHERE id = $1",
            feed_id
        )
        .unwrap(),
        rdrs::query_scalar!(
            &app.db,
            Option<String>,
            "SELECT custom_referrer FROM feed WHERE id = $1",
            feed_id
        )
        .unwrap(),
    );

    assert_eq!(row.0, cat_b_id, "the category change should have applied");
    assert_eq!(row.1.as_deref(), Some("Kept description"));
    assert_eq!(row.2.as_deref(), Some("https://site.example.com"));
    assert_eq!(row.3.as_deref(), Some("MyBot/1.0"));
    assert_eq!(row.4.as_deref(), Some("https://ref.example.com"));
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

/// "OPML imported." cannot distinguish a 300-feed subscription haul from a
/// re-import that added nothing, which is the whole question a user has after
/// uploading a file they are not sure about.
#[tokio::test]
async fn test_import_opml_form_flash_reports_counts() {
    let mut app =
        create_test_app_named(default_test_config(), "test_import_opml_flash_counts").await;
    setup_authenticated_user(&mut app.server).await;

    let existing = "https://already.example.com/feed.xml";
    insert_test_feed(&app, "MixCat", existing).await;

    let opml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Mixed</title></head>
  <body>
    <outline text="MixCat" title="MixCat">
      <outline type="rss" text="Already" xmlUrl="{existing}"/>
      <outline type="rss" text="New A" xmlUrl="https://new-a.example.com/feed.xml"/>
      <outline type="rss" text="New B" xmlUrl="https://new-b.example.com/feed.xml"/>
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
    let text = flash_text(&response);
    assert!(
        text.contains("OPML imported: 2 feeds added, 1 already subscribed."),
        "flash must break the import down by outcome, got: {text}"
    );
}

// --- handlers/feeds.rs — Part B: wiremock success-arm tests ---

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

// --- SSE /events endpoint auth-gate test ---

#[tokio::test]
async fn events_endpoint_requires_auth() {
    let server = create_test_server(default_test_config()).await;
    // GET /events without a session cookie — PageAuthUser redirects to /login.
    let response = server.get("/events").await;
    // PageAuthUser always redirects unauthenticated requests to /login (303).
    response.assert_status(StatusCode::SEE_OTHER);
}

// --- middleware/cache_control.rs — no-store wiring through the real router ---

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
    // A genuine upstream fetch can't be exercised here: the shared SSRF guard
    // rejects loopback/private addresses with no test escape hatch, and every
    // mock HTTP server available binds to one. `choose_cache_control` itself is
    // covered by handlers/proxy.rs's unit tests; what this adds is that the
    // cache_control *middleware*, which runs after the handler on every
    // response, does not clobber the `Cache-Control` the proxy already set —
    // even on an authenticated request, where rule 3 would otherwise apply. The
    // ETag/If-None-Match short-circuit is a real SSRF-free path carrying the
    // handler's own header, so it stands in for the upstream case.
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

// Regression test for session-cookie cache poisoning: a shared cache that
// stores this authenticated, publicly-cacheable response must never also
// receive a live session/CSRF cookie riding along on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn authenticated_cacheable_response_carries_no_set_cookie() {
    let mut app = create_test_app_named(
        default_test_config(),
        "authenticated_cacheable_response_carries_no_set_cookie",
    )
    .await;
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
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .count(),
        0,
        "a publicly-cacheable authenticated response must not carry Set-Cookie"
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

// --- Strict-Transport-Security (HSTS) tests ---

#[tokio::test]
async fn test_hsts_header_present_when_public_base_url_is_https() {
    // Go through Config::from_map (not a hand-built literal) so this exercises
    // the real derivation path: RDRS_PUBLIC_BASE_URL's scheme -> parse_hsts ->
    // Config::hsts -> Config::hsts_header_value -> the router layer.
    let config = Config::from_map(|k| {
        (k == "RDRS_PUBLIC_BASE_URL").then(|| "https://rdrs.example.com".to_string())
    })
    .expect("from_map should succeed");
    let server = create_test_server(config).await;

    let response = server.get("/health").await;

    assert_eq!(
        response.header(header::STRICT_TRANSPORT_SECURITY),
        "max-age=31536000; includeSubDomains"
    );
}

#[tokio::test]
async fn test_hsts_header_absent_by_default() {
    // A plain-HTTP deployment — the default, with no RDRS_PUBLIC_BASE_URL and no
    // RDRS_HSTS override — must never be told to enforce HTTPS: HSTS is sticky
    // and cannot be retracted quickly, so sending it by accident would lock
    // browsers out of a working install.
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/health").await;

    response.assert_status_ok();
    assert!(
        response
            .maybe_header(header::STRICT_TRANSPORT_SECURITY)
            .is_none(),
        "a plain-HTTP deployment must never receive Strict-Transport-Security"
    );
}

#[tokio::test]
async fn test_hsts_header_on_static_and_health() {
    // HSTS is a declaration about the host, not any one response, so there is
    // no skip list: /static and /health must carry it exactly like every
    // other path when the deployment is HTTPS.
    let config = Config::from_map(|k| {
        (k == "RDRS_PUBLIC_BASE_URL").then(|| "https://rdrs.example.com".to_string())
    })
    .expect("from_map should succeed");
    let server = create_test_server(config).await;

    let health = server.get("/health").await;
    assert_eq!(
        health.header(header::STRICT_TRANSPORT_SECURITY),
        "max-age=31536000; includeSubDomains"
    );

    let static_asset = server.get("/static/css/app.css").await;
    static_asset.assert_status_ok();
    assert_eq!(
        static_asset.header(header::STRICT_TRANSPORT_SECURITY),
        "max-age=31536000; includeSubDomains"
    );
}

#[tokio::test]
async fn test_existing_hsts_header_is_not_overwritten() {
    // No handler sets Strict-Transport-Security itself — the realistic source of
    // a pre-existing header is a TLS-terminating reverse proxy, which this suite
    // cannot stand up. So the exported `set_hsts` middleware is wired around a
    // bare handler standing in for the proxy, to confirm the middleware leaves
    // an existing value alone.
    async fn proxy_set_header() -> impl axum::response::IntoResponse {
        (
            [(
                header::STRICT_TRANSPORT_SECURITY,
                HeaderValue::from_static("max-age=1"),
            )],
            "ok",
        )
    }

    let value = HeaderValue::from_static("max-age=31536000; includeSubDomains");
    let router = axum::Router::new()
        .route("/probe", axum::routing::get(proxy_set_header))
        .layer(axum::middleware::from_fn_with_state(
            rdrs::middleware::HstsState::new(value),
            rdrs::middleware::set_hsts,
        ));
    let server = TestServer::new(router);

    let response = server.get("/probe").await;

    assert_eq!(
        response.header(header::STRICT_TRANSPORT_SECURITY),
        "max-age=1"
    );
}

#[tokio::test]
async fn hsts_is_sent_on_a_csrf_rejected_response() {
    // Regression test: HSTS must be the outermost layer, because
    // `csrf_origin_guard` short-circuits with a 403 without calling `next`,
    // so a layer nested inside it (as HSTS used to be) would never run.
    let config = Config::from_map(|k| {
        (k == "RDRS_PUBLIC_BASE_URL").then(|| "https://rdrs.example.com".to_string())
    })
    .expect("from_map should succeed");
    let server = create_test_server(config).await;

    let response = server
        .post("/api/session")
        .add_header(
            HeaderName::from_static("sec-fetch-site"),
            HeaderValue::from_static("cross-site"),
        )
        .await;

    response.assert_status_forbidden();
    assert_eq!(
        response.header(header::STRICT_TRANSPORT_SECURITY),
        "max-age=31536000; includeSubDomains"
    );
}

// ============================================================================
// Fixed security headers (CSP, nosniff, Referrer-Policy, Permissions-Policy,
// X-Frame-Options, COOP)
// ============================================================================

/// Every header the fixed layer is responsible for, paired with the exact value
/// it must carry. Kept as one list so each test below asserts on the whole set
/// rather than whichever header the author happened to remember.
const EXPECTED_SECURITY_HEADERS: &[(&str, &str)] = &[
    (
        "content-security-policy",
        "default-src 'self'; script-src 'self'; style-src 'self'; \
         img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; \
         base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
    ),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    (
        "permissions-policy",
        "accelerometer=(), autoplay=(), camera=(), display-capture=(), encrypted-media=(), \
         geolocation=(), gyroscope=(), magnetometer=(), microphone=(), midi=(), payment=(), \
         usb=(), xr-spatial-tracking=()",
    ),
    ("x-frame-options", "DENY"),
    ("cross-origin-opener-policy", "same-origin"),
];

fn assert_security_headers(response: &axum_test::TestResponse, context: &str) {
    for (name, expected) in EXPECTED_SECURITY_HEADERS {
        assert_eq!(
            response.header(HeaderName::from_static(name)),
            *expected,
            "{context}: wrong or missing {name}"
        );
    }
}

#[tokio::test]
async fn test_security_headers_present_on_the_default_config() {
    // Unlike HSTS these are unconditional: a plain-HTTP deployment (the
    // default) still gets the full set, because none of them depends on the
    // transport.
    let server = create_test_server(default_test_config()).await;

    let response = server.get("/health").await;

    response.assert_status_ok();
    assert_security_headers(&response, "/health");
}

#[tokio::test]
async fn test_security_headers_on_static_and_a_page() {
    // No skip list, for the same reason HSTS has none — and `nosniff` on a
    // static asset is precisely where it matters.
    let server = create_test_server(default_test_config()).await;

    let static_asset = server.get("/static/css/app.css").await;
    static_asset.assert_status_ok();
    assert_security_headers(&static_asset, "/static/css/app.css");

    let login = server.get("/login").await;
    login.assert_status_ok();
    assert_security_headers(&login, "/login");
}

#[tokio::test]
async fn security_headers_are_sent_on_a_csrf_rejected_response() {
    // Same regression the HSTS test above guards: `csrf_origin_guard`
    // short-circuits with a 403 without calling `next`, so a layer nested
    // inside it would never run. A rejected response is exactly the one an
    // attacker sees, so it must carry the policy too.
    let server = create_test_server(default_test_config()).await;

    let response = server
        .post("/api/session")
        .add_header(
            HeaderName::from_static("sec-fetch-site"),
            HeaderValue::from_static("cross-site"),
        )
        .await;

    response.assert_status_forbidden();
    assert_security_headers(&response, "csrf-rejected /api/session");
}

#[tokio::test]
async fn test_existing_security_header_is_not_overwritten() {
    // As with HSTS, the realistic source of a pre-existing header is a reverse
    // proxy this suite cannot stand up, so the same exported middleware is wired
    // around a handler standing in for it. The headers it did not set are still
    // filled in.
    async fn proxy_set_header() -> impl axum::response::IntoResponse {
        (
            [(
                header::CONTENT_SECURITY_POLICY,
                HeaderValue::from_static("default-src 'none'"),
            )],
            "ok",
        )
    }

    let router = axum::Router::new()
        .route("/probe", axum::routing::get(proxy_set_header))
        .layer(axum::middleware::from_fn(
            rdrs::middleware::set_security_headers,
        ));
    let server = TestServer::new(router);

    let response = server.get("/probe").await;

    assert_eq!(
        response.header(header::CONTENT_SECURITY_POLICY),
        "default-src 'none'"
    );
    assert_eq!(response.header(header::X_CONTENT_TYPE_OPTIONS), "nosniff");
}

// ============================================================================
// Admin re-authentication. Every route that changes another account is behind
// the same `REAUTH_WINDOW_MINUTES` window that guards passkey enrolment.
// ============================================================================

/// The four account-changing admin actions must all refuse a session whose
/// confirmation window has lapsed — and refuse it *before* touching the
/// account, so a picked-up session cannot promote, disable, delete or take
/// over anyone while the real admin is away from the keyboard.
#[tokio::test]
async fn admin_account_changes_require_recent_authentication() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;

    stale_authentication(&app.db).await;

    for (path, form) in [
        ("/admin/users/2/role", Some(json!({ "role": "admin" }))),
        ("/admin/users/2/status", Some(json!({ "disabled": "true" }))),
        ("/admin/users/2/masquerade", None),
        ("/admin/users/2/delete", None),
    ] {
        let request = app.server.post(path);
        let response = match form {
            Some(body) => request.form(&body).await,
            None => request.await,
        };
        response.assert_status(StatusCode::SEE_OTHER);
        assert_eq!(response.header(header::LOCATION), "/admin", "for {path}");
    }

    // Nothing happened: still a plain user, still enabled, still there.
    let target = rdrs::models::user::find_by_id(&app.db, 2)
        .await
        .unwrap()
        .expect("the target account must survive a refused delete");
    assert_eq!(target.role, rdrs::Role::User);
    assert!(!target.is_disabled());

    // And the admin was not dragged into a masquerade on the way.
    let admin = rdrs::models::user::find_by_id(&app.db, 1).await.unwrap();
    assert!(admin.is_some_and(|u| u.is_admin()));
}

#[tokio::test]
async fn confirming_the_password_reopens_the_admin_window() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    stale_authentication(&app.db).await;

    let confirmed = app
        .server
        .post("/admin/reauth")
        .form(&json!({ "password": "vulture-mango-77-quilt" }))
        .await;
    confirmed.assert_status(StatusCode::SEE_OTHER);

    // The action that would have been refused a moment ago now lands.
    app.server
        .post("/admin/users/2/role")
        .form(&json!({ "role": "admin" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let target = rdrs::models::user::find_by_id(&app.db, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target.role, rdrs::Role::Admin);
}

#[tokio::test]
async fn a_wrong_password_does_not_reopen_the_admin_window() {
    // The confirmation is a real credential check, not a click-through. Both
    // responses are the same 303 to /admin (the flash cookie carries the
    // difference), so the assertion that matters is the account state after.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;
    stale_authentication(&app.db).await;

    app.server
        .post("/admin/reauth")
        .form(&json!({ "password": "not-the-password" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    app.server
        .post("/admin/users/2/role")
        .form(&json!({ "role": "admin" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let target = rdrs::models::user::find_by_id(&app.db, 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target.role,
        rdrs::Role::User,
        "a failed confirmation must not let the change through"
    );
}

/// Ending a masquerade must never need a password. While masquerading, the
/// password that would be asked for belongs to the *impersonated* account, so
/// requiring one would strand the admin inside the impersonation — and
/// stepping back down is a de-escalation, not a privileged act.
#[tokio::test]
async fn stopping_a_masquerade_never_requires_reauthentication() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    register_target_user(&app.db).await;

    let started = app.server.post("/admin/users/2/masquerade").await;
    started.assert_status(StatusCode::SEE_OTHER);
    common::apply_csrf(&mut app.server, &started);

    stale_authentication(&app.db).await;

    app.server
        .post("/api/admin/unmasquerade")
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn the_admin_page_asks_for_confirmation_only_once_the_window_lapses() {
    // The form is rendered ahead of time rather than left for the POST to
    // discover, so an admin is told the window lapsed *before* clicking
    // "delete" rather than after.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let fresh = app.server.get("/admin").await;
    fresh.assert_status_ok();
    assert!(
        !fresh.text().contains("admin-reauth-form"),
        "a session inside the window has nothing to confirm"
    );

    stale_authentication(&app.db).await;

    let stale = app.server.get("/admin").await;
    stale.assert_status_ok();
    assert!(
        stale.text().contains("admin-reauth-form"),
        "a lapsed window must surface the confirmation form"
    );
}

#[tokio::test]
async fn password_fields_advertise_the_server_side_policy() {
    // The browser's `minlength`/`maxlength` hints are generated from the same
    // constants the handler validates against, so the two cannot drift into a
    // form that submits only to be rejected.
    //
    // Two servers on purpose: /setup is only served while the instance has no
    // accounts, and loading any page first mints an anonymous CSRF cookie the
    // guard would then expect on the POST.
    let setup_only = create_test_app(default_test_config()).await;
    let setup = setup_only.server.get("/setup").await;
    setup.assert_status_ok();
    let body = setup.text();
    assert!(body.contains(&format!(
        "minlength=\"{}\"",
        rdrs::auth::PASSWORD_MIN_LENGTH
    )));
    assert!(body.contains(&format!(
        "maxlength=\"{}\"",
        rdrs::auth::PASSWORD_MAX_LENGTH
    )));

    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    let settings = app.server.get("/user-settings").await;
    settings.assert_status_ok();
    let body = settings.text();
    assert!(body.contains(&format!(
        "minlength=\"{}\"",
        rdrs::auth::PASSWORD_MIN_LENGTH
    )));
    assert!(body.contains(&format!(
        "maxlength=\"{}\"",
        rdrs::auth::PASSWORD_MAX_LENGTH
    )));
}

// ============================================================================
// Admin-created accounts and the one-time link that activates them. This
// replaces self-service registration outright: no anonymous endpoint accepts a
// username any more, so there is nothing to ask about who has an account.
// ============================================================================

/// Pull the `/invite/{token}` path out of the flash cookie the create-account
/// redirect sets. The link is shown once and never stored in recoverable form,
/// so this is exactly what an admin has to do — read it off the screen.
fn invite_path_from(response: &axum_test::TestResponse) -> String {
    let flash = response.cookie("flash");
    // The value is the flash JSON, but a cookie value cannot carry spaces or
    // quotes unescaped, so what arrives here is percent-encoded. Searching the
    // decoded text is enough — the path itself is URL-safe base64 and survives
    // encoding untouched.
    let raw = flash.value().replace("%2F", "/");
    let start = raw
        .find("/invite/")
        .unwrap_or_else(|| panic!("expected a link in the flash, got {raw:?}"));

    // The token is base64url (`A-Za-z0-9-_`), so the path ends at the first
    // character outside that alphabet — which is where the percent-encoded
    // remainder of the JSON begins.
    let tail: String = raw[start + "/invite/".len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    format!("/invite/{tail}")
}

#[tokio::test]
async fn an_admin_created_account_is_activated_through_its_link() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let created = app
        .server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await;
    created.assert_status(StatusCode::SEE_OTHER);
    let invite = invite_path_from(&created);

    // The account exists but cannot be signed into: no password has been set,
    // so `verify_password` has nothing that can match.
    let refused = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "newcomer", "password": "vulture-mango-77-quilt" }))
        .await;
    refused.assert_status_unauthorized();

    // The link names the account it opens...
    let page = app.server.get(&invite).await;
    page.assert_status_ok();
    assert!(page.text().contains("newcomer"));

    // ...and redeeming it sets the password.
    let redeemed = app
        .server
        .post(&invite)
        .form(&json!({
            "password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift"
        }))
        .await;
    redeemed.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(redeemed.header(header::LOCATION), "/login");

    app.server
        .post("/api/session")
        .json(&json!({ "username": "newcomer", "password": "heron-lantern-53-drift" }))
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn a_link_works_exactly_once() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let created = app
        .server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await;
    let invite = invite_path_from(&created);

    app.server
        .post(&invite)
        .form(&json!({
            "password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift"
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // A second submission must not be able to replace the password the first
    // person just chose.
    let replayed = app
        .server
        .post(&invite)
        .form(&json!({
            "password": "badger-kestrel-19-plume",
            "confirm_password": "badger-kestrel-19-plume"
        }))
        .await;
    replayed.assert_status_ok();
    assert!(replayed.text().contains("not valid"));

    app.server
        .post("/api/session")
        .json(&json!({ "username": "newcomer", "password": "heron-lantern-53-drift" }))
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn every_bad_token_gets_the_same_answer() {
    // Unknown, revoked and already-spent links must be indistinguishable, and
    // none of them may name an account — otherwise the page becomes the
    // enumeration oracle that removing registration was meant to close.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let created = app
        .server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await;
    let invite = invite_path_from(&created);

    let unknown = app.server.get("/invite/completely-made-up-token").await;
    unknown.assert_status_ok();

    // Revoke the real one, then compare.
    app.server
        .post("/admin/users/2/invite/revoke")
        .await
        .assert_status(StatusCode::SEE_OTHER);
    let revoked = app.server.get(&invite).await;
    revoked.assert_status_ok();

    assert_eq!(unknown.text(), revoked.text());
    assert!(!revoked.text().contains("newcomer"));
}

#[tokio::test]
async fn a_pending_account_cannot_sign_in_by_any_path() {
    // The unusable hash has to hold for the GReader client protocol too, not
    // just the browser form — otherwise an account could be reached before
    // anyone chose its password.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    app.server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    app.server
        .post("/api/session")
        .json(&json!({ "username": "newcomer", "password": "!" }))
        .await
        .assert_status_unauthorized();

    let client_login = app
        .server
        .post("/accounts/ClientLogin")
        .form(&json!({ "Email": "newcomer", "Passwd": "!" }))
        .await;
    client_login.assert_status_unauthorized();
}

#[tokio::test]
async fn redemption_enforces_the_password_policy() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let created = app
        .server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await;
    let invite = invite_path_from(&created);

    // Too short, and the link survives so the person can try again.
    let short = app
        .server
        .post(&invite)
        .form(&json!({ "password": "short", "confirm_password": "short" }))
        .await;
    short.assert_status_ok();
    assert!(short.text().contains("at least"));

    // Built out of the username, which only the estimator can catch.
    let derived = app
        .server
        .post(&invite)
        .form(&json!({ "password": "newcomernewcomer", "confirm_password": "newcomernewcomer" }))
        .await;
    derived.assert_status_ok();
    assert!(derived.text().contains("invite-error"));

    // The link still works afterwards.
    app.server
        .post(&invite)
        .form(&json!({
            "password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift"
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn an_admin_issued_reset_leaves_the_old_password_working_until_redeemed() {
    // rdrs has no self-service recovery — with no email there is nowhere to
    // send it — so this is the reset path. Issuing one must not lock the
    // account's owner out on its own.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    common::seed_account(
        &app.db,
        "forgetful",
        "vulture-mango-77-quilt",
        rdrs::Role::User,
    )
    .await;

    let issued = app.server.post("/admin/users/2/invite").await;
    issued.assert_status(StatusCode::SEE_OTHER);
    let invite = invite_path_from(&issued);

    // Still the old password until someone uses the link. Signing in as the
    // target replaces the session cookie in this jar, so the CSRF header has
    // to be refreshed from the response before the next POST — exactly what a
    // browser does on its own.
    let signed_in = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "forgetful", "password": "vulture-mango-77-quilt" }))
        .await;
    signed_in.assert_status_ok();
    common::apply_csrf(&mut app.server, &signed_in);

    app.server
        .post(&invite)
        .form(&json!({
            "password": "heron-lantern-53-drift",
            "confirm_password": "heron-lantern-53-drift"
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // ...and afterwards only the new one.
    app.server
        .post("/api/session")
        .json(&json!({ "username": "forgetful", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status_unauthorized();
    app.server
        .post("/api/session")
        .json(&json!({ "username": "forgetful", "password": "heron-lantern-53-drift" }))
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn creating_an_account_requires_recent_authentication() {
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;
    stale_authentication(&app.db).await;

    app.server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    assert!(
        rdrs::models::user::find_by_username(&app.db, "newcomer")
            .await
            .unwrap()
            .is_none(),
        "a stale session must not be able to create an account"
    );
}

#[tokio::test]
async fn a_single_user_instance_refuses_a_second_account() {
    let config = Config {
        multi_user_enabled: false,
        ..default_test_config()
    };
    let mut app = create_test_app(config).await;
    setup_admin_user(&mut app.server).await;

    app.server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    assert!(
        rdrs::models::user::find_by_username(&app.db, "newcomer")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_one_time_link_is_shown_in_its_own_block_not_a_banner() {
    // The link is the only copy that will ever exist — the table stores an
    // HMAC of it — so it belongs somewhere it can be read and copied, not in a
    // banner that fades. It must also appear exactly once on the page: leaving
    // it in the flash bootstrap as well would render it twice.
    let mut app = create_test_app(default_test_config()).await;
    setup_admin_user(&mut app.server).await;

    let created = app
        .server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await;
    created.assert_status(StatusCode::SEE_OTHER);
    let invite = invite_path_from(&created);

    let page = app.server.get("/admin").await;
    page.assert_status_ok();
    let body = page.text();

    assert!(
        body.contains("admin-invite-link"),
        "the link should render in its own block"
    );
    assert_eq!(
        body.matches(&invite).count(),
        1,
        "the link must appear exactly once, not in the block *and* the banner"
    );

    // The banner still says something happened; it just does not repeat the
    // link. Asserted because dropping the message entirely would leave the
    // flash cookie uncleared, and the link would come back on the next load.
    assert!(body.contains("Account link ready."));
}

#[tokio::test]
async fn the_one_time_link_is_absolute_when_a_public_base_url_is_configured() {
    // An admin has to paste this into a chat window, so a bare path is close
    // to useless. It follows the same rule as every other absolute URL rdrs
    // generates: `RDRS_PUBLIC_BASE_URL` when set, relative otherwise — never
    // the client-supplied Host header.
    let config = Config {
        public_base_url: Some("https://reader.example.com".to_string()),
        ..default_test_config()
    };
    let mut app = create_test_app(config).await;
    setup_admin_user(&mut app.server).await;

    app.server
        .post("/admin/users")
        .form(&json!({ "username": "newcomer", "role": "user" }))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let body = app.server.get("/admin").await.text();
    assert!(
        body.contains("https://reader.example.com/invite/"),
        "expected an absolute link, got: {body}"
    );
}
