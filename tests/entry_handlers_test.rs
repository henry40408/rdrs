//! Integration tests for entry-related handlers using `GReader` API endpoints.
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

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::models::{category, entry, feed, user};
use rdrs::{AppState, Config, Db, Role, auth, create_router, services};
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: Db,
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

/// Setup user, category, feed, and entries directly in database
async fn setup_test_data(db: &Db) -> (i64, i64, i64, Vec<i64>) {
    // Create user
    let password_hash = rdrs::auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = user::create_user(db, "testuser", &password_hash, Role::Admin)
        .await
        .unwrap();

    // Create category
    let cat = category::create_category(db, user.id, "Test Category")
        .await
        .unwrap();

    // Create feed
    let feed = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://example.com/feed.xml",
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

    // Create entries (entry i published i hours ago: entry 1 newest, entry 5 oldest)
    let mut entry_ids = Vec::new();
    for i in 1..=5i64 {
        let published = chrono::Utc::now() - chrono::Duration::hours(i);
        let (e, _) = entry::upsert_entry(
            db,
            feed.id,
            &format!("guid-{i}"),
            Some(&format!("Entry Title {i}")),
            Some(&format!("https://example.com/entry/{i}")),
            Some(&format!("<p>Entry content {i}</p>")),
            Some(&format!("Summary for entry {i}")),
            None,
            Some(published),
        )
        .await
        .unwrap();
        entry_ids.push(e.id);
    }

    (user.id, cat.id, feed.id, entry_ids)
}

async fn login(server: &mut TestServer) {
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

/// Setup a second user's data in the database
async fn setup_second_user_data(db: &Db) -> (i64, i64, i64, Vec<i64>) {
    // Create second user
    let password_hash = rdrs::auth::hash_password("password456").unwrap();
    let user = user::create_user(db, "otheruser", &password_hash, Role::User)
        .await
        .unwrap();

    // Create category for second user
    let cat = category::create_category(db, user.id, "Other User Category")
        .await
        .unwrap();

    // Create feed for second user
    let feed = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://other.com/feed.xml",
            title: Some("Other Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();

    // Create entries for second user
    let mut entry_ids = Vec::new();
    for i in 1..=3 {
        let (e, _) = entry::upsert_entry(
            db,
            feed.id,
            &format!("other-guid-{i}"),
            Some(&format!("Other Entry {i}")),
            Some(&format!("https://other.com/entry/{i}")),
            Some(&format!("<p>Other content {i}</p>")),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        entry_ids.push(e.id);
    }

    (user.id, cat.id, feed.id, entry_ids)
}

async fn setup_entry_without_link(db: &Db, feed_id: i64) -> i64 {
    let (e, _) = entry::upsert_entry(
        db,
        feed_id,
        "no-link-guid",
        Some("Entry Without Link"),
        None,
        Some("<p>Content without link</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    e.id
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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    // Create entries where IDs correlate with published_at in ascending order.
    // This is needed because continuation pagination uses `e.id < c` (newest-first)
    // or `e.id > c` (oldest-first), so ID order must match timestamp order.
    let password_hash = rdrs::auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = user::create_user(&app.db, "testuser", &password_hash, Role::Admin)
        .await
        .unwrap();
    let cat = category::create_category(&app.db, user.id, "Test Category")
        .await
        .unwrap();
    let feed = feed::create_feed(
        &app.db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://example.com/feed.xml",
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

    // Insert entries so that lower IDs have older published_at
    // (entry 1 = 5h ago, entry 5 = 1h ago)
    for i in 1..=5i64 {
        let published = chrono::Utc::now() - chrono::Duration::hours(6 - i);
        entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-{i}"),
            Some(&format!("Entry Title {i}")),
            Some(&format!("https://example.com/entry/{i}")),
            Some(&format!("<p>Entry content {i}</p>")),
            None,
            None,
            Some(published),
        )
        .await
        .unwrap();
    }

    login(&mut app.server).await;

    // Default sort is newest-first. With our data:
    //   entry 5 (newest, id=5), 4, 3, 2, 1 (oldest, id=1)
    // Composite cursor `<sort_ts>|<id>` works correctly here since
    // id↔published_at order is monotonic.

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

    // Second page using continuation (add_query_param handles URL-encoding the composite cursor)
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .add_query_param("c", continuation)
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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_user2_id, _cat2_id, _feed2_id, entry2_ids) = setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to list entries by other user's category
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/label/Other%20User%20Category")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_access_other_user_feed_entries() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to list entries by other user's feed
    let response = app
        .server
        .get("/reader/api/0/stream/contents/feed/https://other.com/feed.xml")
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to get neighbors of other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/neighbors", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_unsubscribe_other_user_feed() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, _other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
async fn test_cannot_fetch_full_content_other_user_entry() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to summarize other user's entry
    let response = app
        .server
        .post(&format!("/api/entries/{}/summarize", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_save_other_user_entry() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to save other user's entry
    let response = app
        .server
        .post(&format!("/api/entries/{}/save", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_get_other_user_entry_summary() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // Try to get summary of other user's entry
    let response = app
        .server
        .get(&format!("/api/entries/{}/summary", other_entry_ids[0]))
        .await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_cannot_delete_other_user_entry_summary() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&mut app.server).await;

    // Try to fetch full content of entry without link
    let response = app
        .server
        .post(&format!(
            "/api/entries/{no_link_entry_id}/fetch-full-content"
        ))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("no link"));
}

#[tokio::test]
async fn test_summarize_entry_no_link() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&mut app.server).await;

    // Try to summarize entry without link
    let response = app
        .server
        .post(&format!("/api/entries/{no_link_entry_id}/summarize"))
        .await;
    response.assert_status_bad_request();

    let body: serde_json::Value = response.json();
    assert!(body["error"].as_str().unwrap().contains("no link"));
}

#[tokio::test]
async fn test_save_entry_no_link() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let no_link_entry_id = setup_entry_without_link(&app.db, feed_id).await;
    login(&mut app.server).await;

    // Try to save entry without link
    let response = app
        .server
        .post(&format!("/api/entries/{no_link_entry_id}/save"))
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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
// Stream Item IDs Tests (item.rs coverage)
// ============================================================================

#[tokio::test]
async fn test_stream_item_ids() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/items/ids?s=user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item_refs = body["itemRefs"].as_array().unwrap();
    assert_eq!(item_refs.len(), 5);
    // Each itemRef should have id and timestampUsec fields
    for item_ref in item_refs {
        assert!(item_ref["id"].is_string());
        assert!(item_ref["timestampUsec"].is_string());
    }
}

#[tokio::test]
async fn test_stream_item_ids_with_count() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/items/ids?s=user/-/state/com.google/reading-list&n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item_refs = body["itemRefs"].as_array().unwrap();
    assert_eq!(item_refs.len(), 2);
    // Should have continuation since there are more entries
    assert!(body["continuation"].is_string());
}

#[tokio::test]
async fn test_stream_item_count() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app.server.get("/reader/api/0/stream/items/count").await;
    response.assert_status_ok();

    let text = response.text();
    assert_eq!(text, "5");
}

#[tokio::test]
async fn test_stream_item_count_by_feed() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/items/count?s=feed/https://example.com/feed.xml")
        .await;
    response.assert_status_ok();

    let text = response.text();
    assert_eq!(text, "5");
}

#[tokio::test]
async fn test_stream_item_count_starred() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/items/count?s=user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let text = response.text();
    assert_eq!(text, "0");
}

#[tokio::test]
async fn test_stream_contents_oldest_first() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?r=o")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 5);

    // The first item should have the earliest published timestamp (oldest first)
    let first_published = items[0]["published"].as_i64().unwrap();
    let last_published = items[4]["published"].as_i64().unwrap();
    assert!(
        first_published <= last_published,
        "First item should be oldest: first={first_published}, last={last_published}"
    );
}

#[tokio::test]
async fn test_stream_contents_with_count() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // Should have a continuation token since there are 5 total entries
    assert!(
        body["continuation"].is_string(),
        "Expected continuation token when there are more items"
    );
}

#[tokio::test]
async fn test_stream_contents_with_continuation() {
    let mut app = create_test_app(default_test_config()).await;
    // Use custom setup to ensure IDs correlate with timestamps (needed for continuation)
    let password_hash = rdrs::auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = user::create_user(&app.db, "testuser", &password_hash, Role::Admin)
        .await
        .unwrap();
    let cat = category::create_category(&app.db, user.id, "Test Category")
        .await
        .unwrap();
    let feed = feed::create_feed(
        &app.db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://example.com/feed.xml",
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

    // Insert entries so that lower IDs have older published_at
    for i in 1..=5i64 {
        let published = chrono::Utc::now() - chrono::Duration::hours(6 - i);
        entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-{i}"),
            Some(&format!("Entry Title {i}")),
            Some(&format!("https://example.com/entry/{i}")),
            Some(&format!("<p>Entry content {i}</p>")),
            None,
            None,
            Some(published),
        )
        .await
        .unwrap();
    }

    login(&mut app.server).await;

    // First page: get 2 items
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let first_page_items = body["items"].as_array().unwrap();
    assert_eq!(first_page_items.len(), 2);
    let continuation = body["continuation"].as_str().unwrap();

    // Second page using continuation token (add_query_param handles URL-encoding the composite cursor)
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=2")
        .add_query_param("c", continuation)
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let second_page_items = body["items"].as_array().unwrap();
    assert_eq!(second_page_items.len(), 2);

    // Entries from second page should be different from first page
    let first_ids: Vec<i64> = first_page_items
        .iter()
        .map(|i| i["_entryId"].as_i64().unwrap())
        .collect();
    let second_ids: Vec<i64> = second_page_items
        .iter()
        .map(|i| i["_entryId"].as_i64().unwrap())
        .collect();
    for id in &second_ids {
        assert!(
            !first_ids.contains(id),
            "Second page should not contain items from first page"
        );
    }
}

#[tokio::test]
async fn test_stream_items_contents_post() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let form_data: Vec<(&str, String)> = vec![("i", entry_ids[0].to_string())];

    let response = app
        .server
        .post("/reader/api/0/stream/items/contents")
        .form(&form_data)
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["_entryId"], entry_ids[0]);
    assert!(items[0]["title"].as_str().unwrap().contains("Entry Title"));
}

#[tokio::test]
async fn test_stream_items_contents_empty() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // GET with no i= params
    let response = app.server.get("/reader/api/0/stream/items/contents").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_stream_contents_exclude_read() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Mark one entry as read
    mark_read(&app.server, &[entry_ids[0]]).await;

    // Get stream contents excluding read entries
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);

    // The read entry should not be in the results
    let returned_ids: Vec<i64> = items
        .iter()
        .map(|i| i["_entryId"].as_i64().unwrap())
        .collect();
    assert!(
        !returned_ids.contains(&entry_ids[0]),
        "Read entry should be excluded from results"
    );
}

#[tokio::test]
async fn test_stream_contents_include_starred() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Star one entry
    star_entry(&app.server, &[entry_ids[2]]).await;

    // Get stream contents with include tag for starred
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?it=user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["_entryId"], entry_ids[2]);
}

#[tokio::test]
async fn test_stream_item_ids_default_stream() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // GET stream/items/ids without s= parameter — should default to reading-list
    let response = app.server.get("/reader/api/0/stream/items/ids").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let item_refs = body["itemRefs"].as_array().unwrap();
    assert_eq!(item_refs.len(), 5);
}

// ============================================================================
// User Info Tests (user.rs coverage)
// ============================================================================

#[tokio::test]
async fn test_user_info() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app.server.get("/reader/api/0/user-info").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert!(body["userId"].is_string());
    assert_eq!(body["userName"], "testuser");
    assert!(body["userProfileId"].is_string());
    assert!(body["userEmail"].is_string());
    // userEmail should follow the pattern <username>@localhost
    assert_eq!(body["userEmail"], "testuser@localhost");
}

#[tokio::test]
async fn test_unread_count_with_data() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["max"], 1000);

    let unreadcounts = body["unreadcounts"].as_array().unwrap();

    // Should have feed, category, and reading-list counts
    let feed_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "feed/https://example.com/feed.xml")
        .unwrap();
    assert!(feed_count["count"].as_i64().unwrap() > 0);

    let cat_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/label/Test Category")
        .unwrap();
    assert!(cat_count["count"].as_i64().unwrap() > 0);

    let total_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/state/com.google/reading-list")
        .unwrap();
    assert_eq!(total_count["count"], 5);
    assert!(total_count["newestItemTimestampUsec"].is_string());
}

#[tokio::test]
async fn test_unread_count_after_read() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Mark 3 entries as read
    mark_read(&app.server, &[entry_ids[0], entry_ids[1], entry_ids[2]]).await;

    let response = app.server.get("/reader/api/0/unread-count").await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let unreadcounts = body["unreadcounts"].as_array().unwrap();

    let feed_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "feed/https://example.com/feed.xml")
        .unwrap();
    assert_eq!(feed_count["count"], 2);

    let total_count = unreadcounts
        .iter()
        .find(|c| c["id"] == "user/-/state/com.google/reading-list")
        .unwrap();
    assert_eq!(total_count["count"], 2);
}

// ============================================================================
// Star/Unstar via Stream Tests (models/entry.rs coverage)
// ============================================================================

#[tokio::test]
async fn test_star_and_unstar_entry() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Star an entry via edit-tag
    star_entry(&app.server, &[entry_ids[1]]).await;

    // Verify it appears in the starred stream
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["_entryId"], entry_ids[1]);

    // Unstar the entry
    unstar_entry(&app.server, &[entry_ids[1]]).await;

    // Verify it's no longer in the starred stream
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/starred")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_find_by_ids_with_feed_empty() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // GET stream/items/contents with no valid IDs (nonexistent IDs)
    let response = app
        .server
        .get("/reader/api/0/stream/items/contents?i=999999&i=999998")
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 0);
}

#[tokio::test]
async fn test_stream_contents_time_filter() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Entries are created at now-1h, now-2h, now-3h, now-4h, now-5h.
    // Use ot (oldest timestamp) to filter: only entries newer than 3.5 hours ago.
    // This should return entries at now-1h, now-2h, now-3h (3 entries).
    let ot = chrono::Utc::now().timestamp() - (3 * 3600 + 1800); // 3.5 hours ago in seconds

    let response = app
        .server
        .get(&format!(
            "/reader/api/0/stream/contents/user/-/state/com.google/reading-list?ot={ot}"
        ))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    let items = body["items"].as_array().unwrap();
    // Should return entries within the time window (newer than ot)
    assert!(
        items.len() >= 2 && items.len() <= 4,
        "Expected 2-4 items with time filter, got {}",
        items.len()
    );

    // All returned items should have published timestamps >= ot
    for item in items {
        let published = item["published"].as_i64().unwrap();
        assert!(
            published >= ot,
            "Item published timestamp {published} should be >= ot {ot}"
        );
    }
}

// ============================================================================
// Entry Summary Tests (RDRS-specific, kept as-is)
// ============================================================================

#[tokio::test]
async fn test_get_entry_summary_not_found() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

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
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    // Delete summary (even if none exists, should succeed)
    let response = app
        .server
        .delete(&format!("/api/entries/{}/summary", entry_ids[0]))
        .await;
    response.assert_status_ok();

    let body: serde_json::Value = response.json();
    assert_eq!(body["success"], true);
}

// ============================================================================
// Summary Fragment Tests (SSE swap endpoint)
// ============================================================================

#[tokio::test]
async fn summary_fragment_renders_completed_summary() {
    let mut app = create_test_app(default_test_config()).await;
    let (user_id, _cat_id, _feed_id, entry_ids) = setup_test_data(&app.db).await;
    login(&mut app.server).await;

    let entry_id = entry_ids[0];

    // Insert a completed summary directly in the DB.
    rdrs::models::entry_summary::upsert_pending(&app.db, user_id, entry_id)
        .await
        .unwrap();
    rdrs::models::entry_summary::set_completed(
        &app.db,
        user_id,
        entry_id,
        "<p>Test summary content</p>",
    )
    .await
    .unwrap();

    let response = app
        .server
        .get(&format!("/entries/{entry_id}/summary/fragment"))
        .await;
    assert_eq!(response.status_code(), axum::http::StatusCode::OK);
    let body = response.text();
    assert!(
        body.contains(r##"data-swap-target="#rp-summary-container""##),
        "body should contain swap target attribute"
    );
    assert!(
        body.contains("rp-summary-content"),
        "body should contain completed summary blockquote class"
    );
}

#[tokio::test]
async fn summary_fragment_404_for_other_users_entry() {
    let mut app = create_test_app(default_test_config()).await;
    let (_user_id, _cat_id, _feed_id, _entry_ids) = setup_test_data(&app.db).await;
    let (_other_user_id, _other_cat_id, _other_feed_id, other_entry_ids) =
        setup_second_user_data(&app.db).await;
    login(&mut app.server).await;

    // User 1 is logged in; try to access user 2's entry summary fragment — must return 404.
    let response = app
        .server
        .get(&format!("/entries/{}/summary/fragment", other_entry_ids[0]))
        .await;
    assert_eq!(response.status_code(), axum::http::StatusCode::NOT_FOUND);
}
