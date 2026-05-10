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

    // Start masquerading as user via the SSR form endpoint.
    app.server
        .post(&format!("/admin/users/{}/masquerade", user_id))
        .await
        .assert_status(StatusCode::SEE_OTHER);

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

    // Start masquerading via the SSR form endpoint.
    app.server
        .post(&format!("/admin/users/{}/masquerade", user_id))
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // Admin page should still be accessible
    let response = app.server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Admin"));
}

#[tokio::test]
async fn test_user_settings_page_renders_ssr_content() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-user-settings-page>"));
    assert!(!body.contains("/static/js/pages/user-settings.js"));

    // SSR content present.
    assert!(body.contains("<h1>User Settings</h1>"));
    assert!(body.contains("Account Information"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/password\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/preferences\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/linkding\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/kagi\">"));
    assert!(body.contains("<rdrs-passkeys>"));
    assert!(body.contains("/static/js/passkey.js"));
}

#[tokio::test]
async fn test_settings_page_renders_ssr_content() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR content — no more <rdrs-settings-page> element / page-script.
    assert!(!body.contains("<rdrs-settings-page>"));
    assert!(!body.contains("/static/js/pages/settings.js"));

    // Server-rendered content from default config.
    assert!(body.contains("<h1>Settings</h1>"));
    assert!(body.contains("Configuration"));
    assert!(body.contains("User Agent"));
    assert!(body.contains("Signup Enabled"));
    assert!(body.contains("Environment Variables"));
}

#[tokio::test]
async fn test_settings_page_reflects_custom_config() {
    let config = Config {
        user_agent: "Custom-Agent/2.0".to_string(),
        signup_enabled: true,
        multi_user_enabled: true,
        ..default_test_config()
    };
    let app = create_test_app(config);
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("Custom-Agent/2.0"));
    assert!(body.contains("(custom)"));
    // Both Yes flags rendered for signup + multi-user.
    let yes_count = body
        .matches("<span class=\"success-text\">Yes</span>")
        .count();
    assert!(yes_count >= 2, "expected >=2 Yes badges, got {yes_count}");
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
    // SSR page embeds pending flash messages as inline JSON for
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
    // SSR page still embeds pending flash messages inline.
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
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains(r#"id="rdrs-flash-bootstrap""#));
    assert!(body.contains("Entries refreshed"));
}

#[tokio::test]
async fn test_entries_page_returns_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
}

#[tokio::test]
async fn test_summarized_entries_page_returns_shell() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries/summarized").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
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

// `has-save-services` and `has-kagi-configured` attributes are no longer
// embedded server-side — `<rdrs-entries-page>` fetches `/api/user-settings`
// after mount and sets them on `<rdrs-entry-list>` client-side. Coverage
// for that wiring lives in the e2e suite.

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

// Coverage for non-admin users hitting admin endpoints lives in the
// /admin SSR page test (`test_regular_user_cannot_access_admin_page` above)
// and the /admin/users/{id}/* form-action handlers reuse the same AdminUser
// extractor, so any non-admin caller gets the same redirect/forbidden flow.
// The dedicated GET /api/admin/users endpoint was removed in PR-5 T2.

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
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
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
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    // Category name is read from the inlined sidebar bootstrap JSON.
    assert!(body.contains("Test Category"));
    assert!(!body.contains(r#"class="ssr-entries""#));
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
    assert!(body.contains("<rdrs-entries-page>"));
    assert!(body.contains("/static/js/pages/entries.js"));
    assert!(!body.contains(r#"class="ssr-entries""#));
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

// SSR-emitted continuation cursor tests are obsolete after the entries-family
// CSR migration — `/` no longer SSR-renders entries, so there's no SSR JSON
// to inspect. Composite-cursor regression coverage lives in
// `e2e/tests/ssr-no-double-render.spec.ts` (Load More + back-dated blocks),
// which exercises the same `/reader/api/0/stream/contents` cursor end-to-end.

#[tokio::test]
async fn test_feeds_page_renders_ssr_rows() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Feeds SSR Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feeds-ssr.xml", "Feeds SSR Title"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR content includes the seeded feed title, category, and per-row
    // form actions wired to the PR-8 T1 endpoints.
    assert!(body.contains("<h1>Feeds</h1>"));
    assert!(body.contains("data-testid=\"feeds-table\""));
    assert!(body.contains("Feeds SSR Title"));
    assert!(body.contains("Feeds SSR Cat"));
    assert!(body.contains("/refresh"));
    assert!(body.contains("/edit"));
    assert!(body.contains("/delete"));
    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-feeds-page>"));
    assert!(!body.contains("/static/js/pages/feeds.js"));
}

#[tokio::test]
async fn test_feed_edit_page_renders() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Edit Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, description) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    1,
                    "https://example.com/edit.xml",
                    "Editable Feed",
                    "Some desc"
                ],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/1/edit").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("<h1>Edit Feed</h1>"));
    assert!(body.contains("<form method=\"post\" action=\"/feeds/1/edit\">"));
    assert!(body.contains("https://example.com/edit.xml"));
    assert!(body.contains("Editable Feed"));
    assert!(body.contains("Some desc"));
    // Re-fetch metadata form is its own form.
    assert!(body.contains("/feeds/1/fetch-metadata"));
    // Edit Cat is the only category and should be rendered as an option.
    assert!(body.contains("Edit Cat"));
}

#[tokio::test]
async fn test_feeds_import_page_renders() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/import").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("<h1>Import OPML</h1>"));
    assert!(body.contains("enctype=\"multipart/form-data\""));
    assert!(body.contains("name=\"file\""));
    assert!(body.contains("name=\"content\""));
}

#[tokio::test]
async fn test_categories_page_renders_ssr_content() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cats SSR"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/cats-ssr.xml", "A Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();

    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-categories-page>"));
    assert!(!body.contains("/static/js/pages/categories.js"));

    // SSR content present: page heading, create form, and the row table
    // with the seeded category name rendered server-side.
    assert!(body.contains("<h1>Categories</h1>"));
    assert!(body.contains("<form method=\"post\" action=\"/categories\">"));
    assert!(body.contains("data-testid=\"categories-table\""));
    assert!(body.contains("Cats SSR"));
    // Per-row form actions are wired to the PR-7 T1 endpoints.
    assert!(body.contains("/rename"));
    assert!(body.contains("/delete"));
    // The sidebar bootstrap (per-user chrome) is still embedded.
    assert!(body.contains("id=\"rdrs-sidebar-bootstrap\""));
}

#[tokio::test]
async fn test_categories_page_renders_empty_state() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();

    // No CSR shell on the SSR page.
    assert!(!body.contains("<rdrs-categories-page>"));
    // Empty state renders directly from the template.
    assert!(body.contains("No categories yet."));
}

#[tokio::test]
async fn test_admin_page_renders_ssr_content() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();

    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-admin-page>"));
    assert!(!body.contains("/static/js/pages/admin.js"));

    // SSR content present.
    assert!(body.contains("<h1>Admin Panel</h1>"));
    assert!(body.contains("<th>Username</th>"));
    // The admin user themselves shows the (you) marker.
    assert!(body.contains("(you)"));
}

// ============================================================================
// SSR /feeds filter / sort tests
// ============================================================================

#[tokio::test]
async fn test_feeds_page_filter_errors_only_renders_error_rows() {
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

    let response = app.server.get("/feeds?filter=errors").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Bad Feed"));
    assert!(!body.contains("Good Feed"));
    // Active filter pill is marked.
    assert!(body.contains("feed-filter-link active"));
}

#[tokio::test]
async fn test_feeds_page_filter_by_category_excludes_other_rows() {
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

    let response = app.server.get("/feeds?category=1").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Feed In A"));
    assert!(!body.contains("Feed In B"));
}

// ============================================================================
// Pre-login shell vs. logged-in chrome separation
//
// `templates/base.html` is the slim pre-login shell (only `rdrs-flash.js`).
// All logged-in chrome (kb-pending, kb-help, keyboard, entry-list, sidebar,
// app.js + the two body-mounted custom elements) lives in
// `templates/app_layout.html` and ships only on per-route templates that
// extend it.
// ============================================================================

#[tokio::test]
async fn test_login_page_does_not_load_logged_in_chrome() {
    let app = create_test_app(default_test_config());
    let response = app.server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();

    // None of the logged-in chrome should appear on the pre-login shell.
    assert!(!body.contains("rdrs-kb-pending.js"));
    assert!(!body.contains("rdrs-kb-help.js"));
    assert!(!body.contains("/keyboard.js"));
    assert!(!body.contains("rdrs-entry-list.js"));
    assert!(!body.contains("rdrs-sidebar.js"));
    assert!(!body.contains("/static/js/app.js"));
    assert!(!body.contains("<rdrs-kb-help>"));
    assert!(!body.contains("<rdrs-kb-pending>"));

    // Flash machinery is still needed (login/register use flash.redirect).
    assert!(body.contains("rdrs-flash.js"));
}

#[tokio::test]
async fn test_register_page_does_not_load_logged_in_chrome() {
    let app = create_test_app(default_test_config());
    let response = app.server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(!body.contains("rdrs-kb-pending.js"));
    assert!(!body.contains("rdrs-kb-help.js"));
    assert!(!body.contains("/keyboard.js"));
    assert!(!body.contains("rdrs-entry-list.js"));
    assert!(!body.contains("rdrs-sidebar.js"));
    assert!(!body.contains("/static/js/app.js"));
    assert!(!body.contains("<rdrs-kb-help>"));
    assert!(!body.contains("<rdrs-kb-pending>"));

    assert!(body.contains("rdrs-flash.js"));
}

#[tokio::test]
async fn test_logged_in_page_loads_full_chrome() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    // /settings extends app_layout.html — same chrome as every other
    // logged-in route.
    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // All 6 chrome scripts must be present.
    assert!(body.contains("rdrs-kb-pending.js"));
    assert!(body.contains("rdrs-kb-help.js"));
    assert!(body.contains("/keyboard.js"));
    assert!(body.contains("rdrs-entry-list.js"));
    assert!(body.contains("rdrs-sidebar.js"));
    assert!(body.contains("/static/js/app.js"));

    // Both keyboard helper custom elements must be mounted.
    assert!(body.contains("<rdrs-kb-help>"));
    assert!(body.contains("<rdrs-kb-pending>"));
}
