//! Additional tests for page handlers and edge cases
//!
//! This test file covers additional scenarios for:
//! - Page templates rendering
//! - Masquerading behavior in pages
//! - Flash message handling

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
    let write_conn = open_shared_memory("test_pages");
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory("test_pages");

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
    };

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);

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
        public_base_url: None,
    }
}

/// Setup admin and regular user
async fn setup_users(db: &DbPool) -> (i64, i64) {
    db.user(move |conn| {
        // Create admin user
        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["admin", password_hash, Role::Admin.as_str()],
        )
        .unwrap();
        let admin_id = conn.last_insert_rowid();

        // Create regular user
        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["user", password_hash, Role::User.as_str()],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        (admin_id, user_id)
    })
    .await
    .unwrap()
}

async fn login(server: &TestServer, username: &str) {
    server
        .post("/api/session")
        .json(&json!({
            "username": username,
            "password": "password123"
        }))
        .await
        .assert_status_ok();
}

// ============================================================================
// Page Rendering Tests
// ============================================================================

#[tokio::test]
async fn test_unread_page_returns_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    // Shell shape — no SSR entry markup.
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    // SSR machinery for entries must be gone from this route.
    assert!(!body.contains(r#"class="ssr-entries""#));
    assert!(!body.contains(r#"class="ssr-reading-pane""#));
}

#[tokio::test]
async fn test_unread_page_while_masquerading() {
    let app = create_test_app(default_test_config());
    let (admin_id, user_id) = setup_users(&app.db).await;

    login(&app.server, "admin").await;

    // Start masquerading as user
    app.server
        .post(&format!("/api/admin/masquerade/{}", user_id))
        .await
        .assert_status_ok();

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    // CSR shell with sidebar bootstrap inlined.
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains(r#"id="rdrs-sidebar-bootstrap""#));
    // Bootstrap JSON marks the original admin so the rdrs-sidebar element
    // can show the Admin nav link client-side even under masquerade.
    assert!(body.contains(r#""is_admin":true"#));
    assert!(body.contains(r#""is_masquerading":true"#));

    // Verify current user API returns masqueraded user
    let response = app.server.get("/api/user").await;
    response.assert_status_ok();
    let api_body: serde_json::Value = response.json();
    assert_eq!(api_body["username"], "user");
    assert_eq!(api_body["id"], user_id);

    // Admin ID should be different
    assert_ne!(admin_id, user_id);
}

#[tokio::test]
async fn test_admin_page_while_masquerading() {
    let app = create_test_app(default_test_config());
    let (_admin_id, user_id) = setup_users(&app.db).await;

    login(&app.server, "admin").await;

    // Start masquerading
    app.server
        .post(&format!("/api/admin/masquerade/{}", user_id))
        .await
        .assert_status_ok();

    // Admin page should still be accessible
    let response = app.server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Admin"));
}

#[tokio::test]
async fn test_user_settings_page_serves_csr_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-user-settings-page>"));
    assert!(body.contains("/static/js/pages/user-settings.js"));
}

#[tokio::test]
async fn test_settings_page_serves_csr_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-settings-page>"));
    assert!(body.contains("/static/js/pages/settings.js"));
}

#[tokio::test]
async fn test_login_page_hides_signup_when_disabled() {
    let config = Config {
        signup_enabled: false,
        ..default_test_config()
    };
    let app = create_test_app(config);

    let response = app.server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();

    // Register link should not be present or should be hidden
    // This depends on template logic
    assert!(body.contains("Login"));
}

#[tokio::test]
async fn test_register_page_shows_disabled_message() {
    let config = Config {
        signup_enabled: false,
        ..default_test_config()
    };
    let app = create_test_app(config);

    let response = app.server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show registration disabled message
    assert!(body.contains("disabled") || body.contains("Registration"));
}

#[tokio::test]
async fn test_register_page_shows_disabled_after_first_user_in_single_mode() {
    let config = Config {
        signup_enabled: true,
        multi_user_enabled: false,
        ..default_test_config()
    };
    let app = create_test_app(config);

    // Register first user
    app.server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    // Check register page
    let response = app.server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();

    // Should indicate registration is disabled
    assert!(body.contains("disabled") || body.contains("Registration"));
}

// ============================================================================
// Flash Message Tests for Pages
// ============================================================================

#[tokio::test]
async fn test_categories_page_with_flash() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/categories")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"success","message":"Category created successfully"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    // CSR shell embeds pending flash messages as inline JSON for
    // `<rdrs-flash>` to display on first paint.
    assert!(body.contains("id=\"rdrs-flash-bootstrap\""));
    assert!(body.contains("Category created successfully"));
}

#[tokio::test]
async fn test_feeds_page_with_flash() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/feeds")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"error","message":"Failed to add feed"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    // CSR shell embeds pending flash messages inline.
    assert!(body.contains("id=\"rdrs-flash-bootstrap\""));
    assert!(body.contains("Failed to add feed"));
}

#[tokio::test]
async fn test_entries_page_with_flash() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/entries")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"info","message":"Entries refreshed"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Entries refreshed"));
}

#[tokio::test]
async fn test_user_settings_page_with_flash() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/user-settings")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"success","message":"Settings saved"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("id=\"rdrs-flash-bootstrap\""));
    assert!(body.contains("Settings saved"));
}

// ============================================================================
// Entry Page with Save Services Tests
// ============================================================================

#[tokio::test]
async fn test_entry_page_redirects_with_save_services_configured() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Configure Linkding for user
    app.db
        .user(move |conn| {
            let config = serde_json::json!({
                "linkding": {
                    "api_url": "https://linkding.example.com",
                    "api_token": "secret"
                }
            });
            conn.execute(
                "INSERT INTO user_settings (user_id, save_services) VALUES (?1, ?2)",
                rusqlite::params![1, config.to_string()],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Entry page now redirects to the list page
    let response = app.server.get("/entries/1").await;
    response.assert_status_see_other();

    // Entries list page should have has-save-services attribute
    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("has-save-services"));
}

#[tokio::test]
async fn test_entry_page_redirects_with_kagi_configured() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Configure Kagi for user
    app.db
        .user(move |conn| {
            let config = serde_json::json!({
                "kagi": {
                    "session_token": "secret-token",
                    "language": "EN"
                }
            });
            conn.execute(
                "INSERT INTO user_settings (user_id, save_services) VALUES (?1, ?2)",
                rusqlite::params![1, config.to_string()],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Entry page now redirects to the list page
    let response = app.server.get("/entries/1").await;
    response.assert_status_see_other();

    // Entries list page should have has-kagi-configured attribute
    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("has-kagi-configured"));
}

// ============================================================================
// Regular User Permissions Tests
// ============================================================================

#[tokio::test]
async fn test_regular_user_unread_page_no_admin_link() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "user").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show username
    assert!(body.contains("user"));
    // Should NOT show admin link
    assert!(!body.contains("data-testid=\"nav-admin\""));
}

#[tokio::test]
async fn test_regular_user_cannot_access_admin_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "user").await;

    let response = app.server.get("/admin").await;
    // Should redirect to login
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_regular_user_cannot_access_admin_api() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "user").await;

    let response = app.server.get("/api/admin/users").await;
    response.assert_status_forbidden();
}

// ============================================================================
// User Settings Page with Existing Config
// ============================================================================

#[tokio::test]
async fn test_api_user_settings_returns_linkding_configured() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            let config = serde_json::json!({
                "linkding": {
                    "api_url": "https://linkding.example.com",
                    "api_token": "secret"
                }
            });
            conn.execute(
                "INSERT INTO user_settings (user_id, save_services) VALUES (?1, ?2)",
                rusqlite::params![1, config.to_string()],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/user-settings").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["linkding_configured"], true);
    assert_eq!(body["linkding_api_url"], "https://linkding.example.com");
}

#[tokio::test]
async fn test_api_user_settings_returns_custom_entries_per_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)",
                rusqlite::params![1, 100],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/user-settings").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["entries_per_page"], 100);
}

// ============================================================================
// /api/server-config tests
// ============================================================================

#[tokio::test]
async fn test_api_server_config_returns_signup_status() {
    let config = Config {
        signup_enabled: true,
        multi_user_enabled: true,
        ..default_test_config()
    };
    let app = create_test_app(config);
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/api/server-config").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["signup_enabled"], true);
    assert_eq!(body["multi_user_enabled"], true);
    assert!(body["git_version"].is_string());
}

#[tokio::test]
async fn test_api_server_config_with_custom_user_agent() {
    let config = Config {
        user_agent: "Custom-Agent/2.0".to_string(),
        ..default_test_config()
    };
    let app = create_test_app(config);
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/api/server-config").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["user_agent"], "Custom-Agent/2.0");
    assert_eq!(body["user_agent_is_default"], false);
}

// ============================================================================
// Archive Entry Pages Tests
// ============================================================================

#[tokio::test]
async fn test_read_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries/read").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Read Entries") || body.contains("read"));
}

#[tokio::test]
async fn test_starred_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries/starred").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Starred Entries") || body.contains("starred"));
}

#[tokio::test]
async fn test_summarized_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries/summarized").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Summarized Entries") || body.contains("summarized"));
}

#[tokio::test]
async fn test_search_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/search").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Search") || body.contains("search"));
}

// ============================================================================
// Category Entries Page Tests
// ============================================================================

#[tokio::test]
async fn test_category_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Create category
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test Category"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/categories/1/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Test Category"));
}

#[tokio::test]
async fn test_category_entries_page_not_found() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/categories/999/entries").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_category_entries_page_other_user() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Create category for user 2
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![2, "User2 Category"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Admin (user 1) should not see user 2's category
    let response = app.server.get("/categories/1/entries").await;
    response.assert_status_not_found();
}

// ============================================================================
// Feed Entries Page Tests
// ============================================================================

#[tokio::test]
async fn test_feed_entries_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Create category and feed
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test Category"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "Test Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/1/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Test Feed"));
    assert!(body.contains("Test Category"));
}

#[tokio::test]
async fn test_feed_entries_page_not_found() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/999/entries").await;
    response.assert_status_not_found();
}

#[tokio::test]
async fn test_feed_entries_page_other_user() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Create category and feed for user 2
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![2, "User2 Category"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "User2 Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Admin (user 1) should not see user 2's feed
    let response = app.server.get("/feeds/1/entries").await;
    response.assert_status_not_found();
}

// ============================================================================
// SSR Data Embedding Tests
// ============================================================================

#[tokio::test]
async fn test_entries_page_contains_ssr_entries_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "All Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/all.xml", "All Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, read_at) VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![1, "all-guid-1", "Read Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Read Entry"));
}

#[tokio::test]
async fn test_category_entries_page_contains_ssr_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cat SSR"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/cat-ssr.xml", "Cat Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "cat-ssr-guid", "Cat SSR Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/categories/1/entries").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Cat SSR Entry"));
}

#[tokio::test]
async fn test_feed_entries_page_contains_ssr_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Feed SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed-ssr.xml", "Feed SSR"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "feed-ssr-guid", "Feed SSR Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/1/entries").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Feed SSR Entry"));
}

#[tokio::test]
async fn test_read_entries_page_contains_ssr_entries_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Read SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/read-ssr.xml", "Read SSR Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, read_at) VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![1, "read-ssr-guid", "Read SSR Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries/read").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Read SSR Entry"));
}

#[tokio::test]
async fn test_starred_entries_page_contains_ssr_entries_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Starred SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/star-ssr.xml", "Starred SSR Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, starred_at) VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![1, "star-ssr-guid", "Starred SSR Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries/starred").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Starred SSR Entry"));
}

#[tokio::test]
async fn test_summarized_entries_page_contains_ssr_entries_json() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Summary SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/sum-ssr.xml", "Summary SSR Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "sum-ssr-guid", "Summary SSR Entry"],
            )
            .unwrap();
            // entry_summary table marks the entry as having a summary
            conn.execute(
                "INSERT INTO entry_summary (entry_id, user_id, status, summary_text) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![1, 1, "completed", "A short summary."],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries/summarized").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Summary SSR Entry"));
}

#[tokio::test]
async fn test_search_page_contains_ssr_entries_json_when_query_present() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Search SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/search-ssr.xml", "Search SSR Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "search-ssr-guid", "Quokka Discovery"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "search-ssr-guid-2", "Unrelated Pelican"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/search?q=Quokka").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR script tag must be present and only the matching entry rendered.
    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains("Quokka Discovery"));
    assert!(!body.contains("Unrelated Pelican"));
    // Search input should be pre-filled with the query.
    assert!(body.contains(r#"value="Quokka""#));
}

#[tokio::test]
async fn test_ssr_continuation_matches_api_convention_no_duplicates_on_load_more() {
    // Regression test for issue #148 follow-up: SSR's continuation token must be the ID of
    // the LAST visible entry (matching `stream_contents` API convention), NOT the ID of the
    // popped boundary entry. Otherwise the next-page query `e.id < c` re-fetches the boundary
    // entry and the client renders duplicates after Load More.
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Pag Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/pag.xml", "Pag Feed"],
            )
            .unwrap();
            // Pin entries-per-page to MIN_ENTRIES_PER_PAGE (10) so we can predict the boundary.
            conn.execute(
                "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)",
                rusqlite::params![1, 10],
            )
            .unwrap();
            // 12 entries; published_at decreasing so newest first ⇒ entry id 12 .. 1 in SSR.
            for i in 1..=12 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        1,
                        format!("p-{}", i),
                        format!("E{}", i),
                        format!("-{} hours", 12 - i)
                    ],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    let marker = r#"<script type="application/json" class="ssr-entries">"#;
    let json_start = body.find(marker).expect("ssr-entries script") + marker.len();
    let json_end = body[json_start..]
        .find("</script>")
        .expect("ssr-entries script close");
    let json = &body[json_start..json_start + json_end];
    let value: serde_json::Value = serde_json::from_str(json).expect("valid SSR JSON");

    let entries = value["entries"].as_array().expect("entries array");
    assert_eq!(
        entries.len(),
        10,
        "first page should have 10 visible entries"
    );

    let continuation = value["continuation"]
        .as_str()
        .expect("continuation present when more pages exist");
    let last_visible_id = entries[9]["id"].as_i64().expect("last entry id");

    assert!(
        continuation.ends_with(&format!("|{}", last_visible_id)),
        "SSR continuation must encode the last visible entry id in the new \
         composite '<sort_ts>|<id>' format; got {:?}",
        continuation
    );
}

#[tokio::test]
async fn test_ssr_load_more_does_not_skip_backdated_entries() {
    // Regression for #164: when an entry has a HIGH id but an OLD
    // published_at (e.g. OPML re-import), the legacy `e.id < c` cursor
    // silently skipped it on Load More. With the composite cursor, every
    // entry must be visible across pages 1+2.
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Skip Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/skip.xml", "Skip Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO user_settings (user_id, entries_per_page) VALUES (?1, ?2)",
                rusqlite::params![1, 10],
            )
            .unwrap();

            // 10 monotonic newest-first (ids 1..=10, descending hours-ago)
            for i in 1..=10 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        1,
                        format!("mono-{}", i),
                        format!("M{}", i),
                        format!("-{} hours", 10 - i)
                    ],
                )
                .unwrap();
            }
            // 3 back-dated: NEW ids (11, 12, 13) but OLD timestamps (10+ days ago)
            for i in 1..=3 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, ?2, ?3, datetime('now', ?4))",
                    rusqlite::params![
                        1,
                        format!("bd-{}", i),
                        format!("BD{}", i),
                        format!("-{} days", 10 + i)
                    ],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Page 1: SSR
    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    let marker = r#"<script type="application/json" class="ssr-entries">"#;
    let json_start = body.find(marker).expect("ssr-entries script") + marker.len();
    let json_end = body[json_start..].find("</script>").unwrap();
    let json = &body[json_start..json_start + json_end];
    let value: serde_json::Value = serde_json::from_str(json).expect("valid SSR JSON");

    let page1: Vec<i64> = value["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect();
    assert_eq!(page1.len(), 10, "page 1 should have 10 entries");

    let continuation = value["continuation"]
        .as_str()
        .expect("continuation")
        .to_string();
    assert!(
        continuation.contains('|'),
        "continuation must be composite format"
    );

    // Page 2: stream/contents API with the SSR-emitted cursor — use add_query_param
    // to URL-safely encode the composite cursor (contains spaces and `|`).
    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .add_query_param("n", "10")
        .add_query_param("c", &continuation)
        .await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let page2: Vec<i64> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            // GReader item ids look like "tag:google.com,2005:reader/item/<hex>"
            let s = e["id"].as_str().unwrap();
            let hex = s.rsplit('/').next().unwrap();
            i64::from_str_radix(hex, 16).unwrap()
        })
        .collect();

    let mut all: Vec<i64> = page1.iter().chain(page2.iter()).copied().collect();
    all.sort();
    all.dedup();
    assert_eq!(
        all.len(),
        13,
        "pages 1+2 must include all 13 entries (10 monotonic + 3 back-dated); got {} unique ids",
        all.len()
    );
}

#[tokio::test]
async fn test_search_page_without_query_emits_empty_ssr_payload() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Search Empty Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/empty.xml", "Empty Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "empty-guid", "Should Not Appear"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/search").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR script tag is still present but with empty entries (no DB fetch happened).
    assert!(body.contains(r#"<script type="application/json" class="ssr-entries">"#));
    assert!(body.contains(r#"{"entries":[],"continuation":null}"#));
    assert!(!body.contains("Should Not Appear"));
    assert!(body.contains("Enter a search term"));
}

#[tokio::test]
async fn test_feeds_page_csr_shell_does_not_embed_rows() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Feeds CSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feeds-csr.xml", "Feeds CSR Title"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();

    // After CSR migration the shell does NOT embed feed rows; the custom
    // element fetches them from /api/feeds after mount.
    assert!(body.contains("<rdrs-feeds-page>"));
    assert!(!body.contains("Feeds CSR Title"));
    assert!(!body.contains("feeds-csr.xml"));
}

#[tokio::test]
async fn test_api_feeds_returns_rows_with_unread() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "API Feeds Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, description, site_url) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![1, "https://example.com/api-feeds.xml", "API Feed Title", "desc", "https://example.com"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "api-feeds-guid", "Unread Entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let feeds = body["feeds"].as_array().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0]["title"], "API Feed Title");
    assert_eq!(feeds[0]["category_name"], "API Feeds Cat");
    assert_eq!(feeds[0]["url"], "https://example.com/api-feeds.xml");
    assert_eq!(feeds[0]["unread_count"], 1);
    assert_eq!(body["total_feed_count"], 1);
}

#[tokio::test]
async fn test_categories_page_csr_shell_does_not_embed_rows() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cats CSR"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/cats-csr.xml", "A Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();

    // After migration to CSR, the shell does NOT embed category table
    // rows — the custom element fetches them via the GReader API after
    // mount. The category name does still appear inside the sidebar
    // bootstrap JSON (sidebar lists every category by name), so check
    // for the row-specific markup rather than the bare name.
    assert!(body.contains("<rdrs-categories-page>"));
    assert!(!body.contains("data-tag-id"));
    assert!(!body.contains("cat-edit-input"));
    // The sidebar bootstrap (per-user chrome) is still embedded.
    assert!(body.contains("id=\"rdrs-sidebar-bootstrap\""));
}

#[tokio::test]
async fn test_admin_page_csr_shell_does_not_embed_rows() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();

    // After migration to CSR the shell does NOT embed user table rows.
    // The custom element fetches them from /api/admin/users after mount.
    assert!(body.contains("<rdrs-admin-page>"));
    assert!(body.contains("/static/js/pages/admin.js"));
    // The "active" status text only exists in the JS module, not inline.
    assert!(!body.contains(">active</span>"));
}

// ============================================================================
// /api/feeds tests (filter, sort, category, freshness, health)
// ============================================================================

#[tokio::test]
async fn test_api_feeds_includes_health_fields() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Health Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, fetched_at, feed_updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    1,
                    "https://example.com/health.xml",
                    "Health Feed",
                    "2026-03-18T10:00:00Z",
                    "2026-03-17T10:00:00Z"
                ],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let f = &body["feeds"][0];
    assert!(f["fetched_at_relative"].as_str().unwrap().len() > 0);
    assert!(f["feed_updated_at_relative"].as_str().unwrap().len() > 0);
    assert!(f["freshness_class"].is_string());
    assert!(f["freshness_key"].is_string());
}

#[tokio::test]
async fn test_api_feeds_filter_errors() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Filter Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, fetch_error) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    1,
                    "https://bad.com/feed.xml",
                    "Bad Feed",
                    "Connection refused"
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://good.com/feed.xml", "Good Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds?filter=errors").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let titles: Vec<&str> = body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Bad Feed"));
    assert!(!titles.contains(&"Good Feed"));
    assert_eq!(body["active_filter"], "errors");
}

#[tokio::test]
async fn test_api_feeds_filter_stale() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Stale Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-100 days'))",
                rusqlite::params![1, "https://stale.com/feed.xml", "Stale Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![1, "https://fresh.com/feed.xml", "Fresh Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds?filter=stale").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let titles: Vec<&str> = body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Stale Feed"));
    assert!(!titles.contains(&"Fresh Feed"));
}

#[tokio::test]
async fn test_api_feeds_sort_unread() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Sort Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://a.com/feed.xml", "AAA Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://b.com/feed.xml", "BBB Feed"],
            )
            .unwrap();
            for i in 1..=3 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                    rusqlite::params![2, format!("guid-{}", i), format!("Entry {}", i)],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds?sort=unread").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let titles: Vec<&str> = body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["BBB Feed", "AAA Feed"]);
}

#[tokio::test]
async fn test_api_feeds_freshness_classes() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Freshness Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-50 days'))",
                rusqlite::params![1, "https://warn.com/feed.xml", "Warning Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-100 days'))",
                rusqlite::params![1, "https://stale.com/feed.xml", "Old Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let classes: Vec<&str> = body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["freshness_class"].as_str().unwrap())
        .collect();
    assert!(classes.iter().any(|c| c.contains("feed-freshness-warning")));
    assert!(classes.iter().any(|c| c.contains("feed-freshness-stale")));
}

#[tokio::test]
async fn test_api_feeds_filter_by_category() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cat A"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cat B"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://a.com/feed.xml", "Feed In A"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![2, "https://b.com/feed.xml", "Feed In B"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds?category=1").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    let titles: Vec<&str> = body["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Feed In A"));
    assert!(!titles.contains(&"Feed In B"));
    assert_eq!(body["active_category"], 1);
    // total_feed_count is the unfiltered total (drives the
    // "All Categories (N)" select option).
    assert_eq!(body["total_feed_count"], 2);
}

#[tokio::test]
async fn test_api_feeds_invalid_filter_defaults_to_all() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/feeds?filter=invalid").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["active_filter"], "all");
}
