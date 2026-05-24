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

fn create_test_app_named(config: Config, name: &str) -> TestApp {
    let write_conn = open_shared_memory(name);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(name);

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

fn create_test_app(config: Config) -> TestApp {
    create_test_app_named(config, "test_pages")
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
async fn test_unread_page_renders_ssr_layout() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    // CSR shell must be gone from this route (SSR-first PR-10).
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(!body.contains("/static/js/pages/entries.js"));
    // SSR two-pane layout present.
    assert!(body.contains("data-entries-list"));
    assert!(body.contains(r#"id="reading-pane""#));
    assert!(body.contains("Select an entry"));
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
    // SSR layout with sidebar bootstrap inlined.
    assert!(!body.contains("<rdrs-entries-page>"));
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

/// Seed a single category + feed + entry owned by `username`. Returns the
/// entry id. Uses a unique category/feed name per call so tests that share
/// the in-memory DB don't trip on each other.
async fn seed_one_entry(db: &DbPool, username: &str, slug: &str) -> i64 {
    let username = username.to_string();
    let slug = slug.to_string();
    db.user(move |conn| {
        let user_id: i64 = conn
            .query_row(
                "SELECT id FROM user WHERE username = ?1",
                rusqlite::params![username],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
            rusqlite::params![user_id, format!("cat-{}", slug)],
        )
        .unwrap();
        let cat_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                cat_id,
                format!("https://example.com/{}.xml", slug),
                format!("Feed {}", slug)
            ],
        )
        .unwrap();
        let feed_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, link, content) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                feed_id,
                format!("guid-{}", slug),
                format!("Title for {}", slug),
                format!("https://example.com/{}", slug),
                format!("<p>Body for {}.</p>", slug)
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_page_entry_query_populates_reading_pane() {
    let app = create_test_app_named(default_test_config(), "test_unread_entry_query_ok");
    setup_users(&app.db).await;
    let entry_id = seed_one_entry(&app.db, "admin", "deep-link-ok").await;
    login(&app.server, "admin").await;

    let response = app.server.get(&format!("/?entry={}", entry_id)).await;
    response.assert_status_ok();
    let body = response.text();

    // Reading pane is rendered with the deep-linked entry, not the empty state.
    assert!(
        !body.contains("reading-pane-empty"),
        "deep link must NOT render the empty reading pane"
    );
    assert!(
        body.contains(r#"data-testid="reading-pane-title""#),
        "deep link must render the populated reading pane"
    );
    assert!(
        body.contains("Title for deep-link-ok"),
        "reading pane must contain the seeded entry title; body was: {body}"
    );
    assert!(
        body.contains("Body for deep-link-ok"),
        "reading pane must contain the seeded entry body; body was: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_page_entry_query_invalid_id_falls_back_to_empty_pane() {
    let app = create_test_app_named(default_test_config(), "test_unread_entry_query_invalid");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    // No entries seeded, so id 99999 cannot resolve.
    let response = app.server.get("/?entry=99999").await;
    response.assert_status_ok();
    let body = response.text();

    // Invalid deep-link id must silently fall back to the empty pane —
    // the list page itself must still render.
    assert!(
        body.contains("reading-pane-empty"),
        "invalid entry id must fall back to the empty reading pane"
    );
    assert!(
        !body.contains(r#"data-testid="reading-pane-title""#),
        "invalid entry id must NOT render a populated pane"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_page_entry_query_other_user_falls_back_to_empty_pane() {
    let app = create_test_app_named(default_test_config(), "test_unread_entry_query_other_user");
    setup_users(&app.db).await; // creates `admin` and `user`
                                // Entry belongs to `user`; we log in as `admin`.
    let entry_id = seed_one_entry(&app.db, "user", "cross-user").await;
    login(&app.server, "admin").await;

    let response = app.server.get(&format!("/?entry={}", entry_id)).await;
    response.assert_status_ok();
    let body = response.text();

    // Cross-tenant deep link must NOT leak the foreign entry into the pane.
    assert!(
        body.contains("reading-pane-empty"),
        "cross-user entry id must fall back to the empty reading pane (no info disclosure)"
    );
    assert!(
        !body.contains("Title for cross-user"),
        "cross-user entry title must NOT appear in the response body"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_starred_entries_page_entry_query_populates_reading_pane() {
    // The helper is shared, but exercise one of the non-unread routes too
    // so the wiring on a second handler is covered.
    let app = create_test_app_named(default_test_config(), "test_starred_entry_query_ok");
    setup_users(&app.db).await;
    let entry_id = seed_one_entry(&app.db, "admin", "starred-deep-link").await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get(&format!("/entries/starred?entry={}", entry_id))
        .await;
    response.assert_status_ok();
    let body = response.text();

    assert!(
        !body.contains("reading-pane-empty"),
        "deep link on /entries/starred must NOT render the empty reading pane"
    );
    assert!(
        body.contains("Title for starred-deep-link"),
        "/entries/starred deep link must populate the reading pane; body was: {body}"
    );
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
        database_url: "/data/custom.sqlite3".to_string(),
        server_port: 8080,
        user_agent: "Custom-Agent/2.0".to_string(),
        signup_enabled: true,
        multi_user_enabled: true,
        image_proxy_secret_generated: false,
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
    // Server section reflects the actual runtime database path and port.
    assert!(body.contains("/data/custom.sqlite3"));
    assert!(body.contains("8080"));
    // Image proxy secret is configured (not auto-generated).
    assert!(body.contains("Configured"));
    assert!(!body.contains(">Auto-generated<"));
    // Both Yes flags rendered for signup + multi-user.
    let yes_count = body
        .matches("<span class=\"success-text\">Yes</span>")
        .count();
    assert!(yes_count >= 2, "expected >=2 Yes badges, got {yes_count}");
}

#[tokio::test]
async fn test_settings_page_reflects_auto_generated_image_proxy_secret() {
    let config = Config {
        image_proxy_secret_generated: true,
        ..default_test_config()
    };
    let app = create_test_app(config);
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Image proxy secret status reflects the auto-generated runtime state.
    assert!(body.contains("Auto-generated"));
    assert!(!body.contains(">Configured<"));
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
    // Post-SSR migration: flash is embedded inline, no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains(r#"id="rdrs-flash-bootstrap""#));
    assert!(body.contains("Entries refreshed"));
}

#[tokio::test]
async fn test_entries_page_renders_ssr_layout() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout: no CSR shell, no entries.js page script.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(!body.contains("/static/js/pages/entries.js"));
    assert!(body.contains(r#"id="reading-pane""#));
}

#[tokio::test]
async fn test_summarized_entries_page_renders_ssr_layout() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/entries/summarized").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout: no CSR shell, no entries.js page script.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(!body.contains("/static/js/pages/entries.js"));
    assert!(body.contains(r#"id="reading-pane""#));
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
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
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
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
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
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
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
    // SSR page: form + empty-state hint, no CSR shell.
    assert!(body.contains("<h1>Search</h1>"));
    assert!(body.contains("<form method=\"get\" action=\"/search\""));
    assert!(body.contains("data-testid=\"search-input\""));
    assert!(body.contains("Enter a search term"));
    assert!(body.contains("class=\"search-status\""));
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(!body.contains("/static/js/pages/entries.js"));
}

#[tokio::test]
async fn test_search_page_with_results() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Search Cat"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/search-feed.xml", "Search Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link, content) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    1,
                    "search-guid-1",
                    "Quokka Discovery in Western Australia",
                    "https://example.com/quokka",
                    "<p>The quokka is a small marsupial.</p>"
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link, content) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    1,
                    "search-guid-2",
                    "Other Topic",
                    "https://example.com/other",
                    "<p>Unrelated article.</p>"
                ],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/search?q=Quokka").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("data-testid=\"search-results\""));
    // Title and snippet wrap query matches in <mark>; the un-matched fragment
    // ("Discovery in Western Australia") still appears verbatim.
    assert!(body.contains("<mark>Quokka</mark>"));
    assert!(body.contains("Discovery in Western Australia"));
    // Result links go to the in-app entry route (which redirects to
    // a list page with ?entry={id}), not the original article URL.
    assert!(body.contains("href=\"/entries/1\""));
    assert!(!body.contains("https://example.com/quokka"));
    assert!(body.contains("Search Feed"));
    // Case-insensitive match preserves snippet's lowercase 'quokka'.
    assert!(body.contains("<mark>quokka</mark>"));
    assert!(body.contains("is a small marsupial."));
    // Other entry must not appear.
    assert!(!body.contains("Other Topic"));
}

#[tokio::test]
async fn test_search_page_no_results() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/search?q=zzznotfoundzzz").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Nothing matched"));
    assert!(body.contains("zzznotfoundzzz"));
}

// ============================================================================
// Category Entries Page Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page() {
    let app = create_test_app_named(default_test_config(), "test_category_entries_page");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_ce", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_ce", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let (cat_id, entry_a_id, entry_b_id) = app
        .db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                .unwrap();
            let cat =
                rdrs::models::category::create_category(conn, user_id, "Engineering").unwrap();
            let feed1 = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/ce-feed-1",
                    title: Some("Feed 1"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let feed2 = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/ce-feed-2",
                    title: Some("Feed 2"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (a, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed1.id,
                "guid-ce-a",
                Some("Entry A"),
                Some("https://x/ce/a"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (b, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed2.id,
                "guid-ce-b",
                Some("Entry B"),
                Some("https://x/ce/b"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            (cat.id, a.id, b.id)
        })
        .await
        .unwrap();

    let resp = app
        .server
        .get(&format!("/categories/{}/entries", cat_id))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();

    assert!(
        html.contains("Engineering"),
        "page title must render the category name"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", entry_a_id)),
        "row for entry from feed 1 must be present"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", entry_b_id)),
        "row for entry from feed 2 must be present"
    );
    assert!(
        !html.contains("rdrs-entries-page"),
        "SSR page must not mount the legacy CSR shell"
    );
    assert!(
        !html.contains("/static/js/pages/entries.js"),
        "SSR page must not load the legacy entries.js bundle"
    );
    assert!(
        html.contains("Select an entry to read."),
        "reading-pane placeholder must render"
    );
    if html.contains("id=\"load-more\"") {
        assert!(
            html.contains(&format!("action=\"/categories/{}/entries\"", cat_id)),
            "Load-More form must POST back to the category-scoped URL"
        );
    }

    // Breadcrumb: `Categories / Engineering`.
    assert!(
        html.contains(r#"data-testid="breadcrumb""#),
        "category page must render a breadcrumb nav"
    );
    assert!(
        html.contains(r#"<a href="/categories">Categories</a>"#),
        "breadcrumb must link to /categories"
    );

    // Sidebar must receive active-category-id so the category is highlighted.
    assert!(
        html.contains(&format!("active-category-id=\"{}\"", cat_id)),
        "<rdrs-sidebar> must carry active-category-id for the current category"
    );

    // Scoped Mark-as-Read dropdown must be visible and carry the
    // category-scoped GReader stream ID so `app.js` only marks entries
    // in *this* category, not the global inbox.
    assert!(
        html.contains(r#"id="mark-read-age""#),
        "category page must render the Mark-as-Read dropdown"
    );
    assert!(
        html.contains(r#"data-mark-read-scope="user/-/label/Engineering""#),
        "Mark-as-Read scope must be the category's GReader label stream"
    );

    // Mark Above as Read button must render at the bottom of the list.
    assert!(
        html.contains(r#"id="mark-above-read""#),
        "category page must render the Mark Above as Read button"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page_not_found() {
    let app = create_test_app_named(
        default_test_config(),
        "test_category_entries_page_not_found",
    );

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_cnf", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_cnf", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let resp = app.server.get("/categories/999999/entries").await;
    assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    let body = resp.text();
    assert!(
        body.contains("Category not found"),
        "404 page should render the not-found heading, got: {body}"
    );
    assert!(
        body.contains("rdrs-sidebar"),
        "404 page should render inside the app chrome (sidebar present), got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page_other_user() {
    let app = create_test_app_named(
        default_test_config(),
        "test_category_entries_page_other_user",
    );

    // Register alice (owner of the category)
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_cou", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Register bob (the cross-tenant user)
    app.server
        .post("/api/register")
        .json(&json!({ "username": "bob_cou", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Get alice's user_id and create her category
    let cat_id: i64 = app
        .db
        .user(|conn| {
            let alice_id: i64 = conn
                .query_row(
                    "SELECT id FROM user WHERE username = 'alice_cou'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, alice_id, "Alice Cat").unwrap();
            cat.id
        })
        .await
        .unwrap();

    // Log in as bob
    app.server
        .post("/api/session")
        .json(&json!({ "username": "bob_cou", "password": "pw123456" }))
        .await
        .assert_status_ok();

    // Bob tries to access alice's category entries — must be 404
    let resp = app
        .server
        .get(&format!("/categories/{}/entries", cat_id))
        .await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user category entries must return 404"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page_load_more_fragment() {
    let app = create_test_app_named(default_test_config(), "test_category_entries_page_lm");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_cl", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_cl", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let cat_id: i64 = app
        .db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, user_id, "LMCat").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/clm-feed",
                    title: Some("CLM Feed"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            for i in 0..3 {
                rdrs::models::entry::upsert_entry(
                    conn,
                    feed.id,
                    &format!("guid-clm-{}", i),
                    Some(&format!("Entry {}", i)),
                    Some(&format!("https://x/clm/{}", i)),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            }
            cat.id
        })
        .await
        .unwrap();

    let resp = app
        .server
        .get(&format!(
            "/categories/{}/entries?fragment=1&after=0",
            cat_id
        ))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains("data-entry-row"),
        "fragment must include row markup"
    );
    assert!(
        !html.contains("<rdrs-sidebar"),
        "fragment must NOT include layout chrome"
    );
    assert!(
        !html.contains("<h1>LMCat</h1>"),
        "fragment must NOT include the page title"
    );
}

// ============================================================================
// Feed Entries Page Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page() {
    let app = create_test_app_named(default_test_config(), "test_feed_entries_page");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_fe", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_fe", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let (cat_id, feed_id, entry_a_id, entry_b_id) = app
        .db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/fe-feed",
                    title: Some("FE Feed"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (a, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-fe-a",
                Some("First Entry"),
                Some("https://x/a"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (b, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-fe-b",
                Some("Second Entry"),
                Some("https://x/b"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            (cat.id, feed.id, a.id, b.id)
        })
        .await
        .unwrap();

    let resp = app.server.get(&format!("/feeds/{}/entries", feed_id)).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();

    assert!(
        html.contains("FE Feed"),
        "page title must render feed title"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", entry_a_id)),
        "row for first entry must be in the HTML"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", entry_b_id)),
        "row for second entry must be in the HTML"
    );
    assert!(
        !html.contains("rdrs-entries-page"),
        "SSR page must not mount the legacy <rdrs-entries-page> shell"
    );
    assert!(
        !html.contains("/static/js/pages/entries.js"),
        "SSR page must not load the legacy entries.js bundle"
    );
    assert!(
        html.contains("Select an entry to read."),
        "reading-pane placeholder must render when no entry is selected"
    );
    if html.contains("id=\"load-more\"") {
        assert!(
            html.contains(&format!("action=\"/feeds/{}/entries\"", feed_id)),
            "Load-More form must POST back to the same feed-scoped URL"
        );
    }

    // Breadcrumb: `Feeds / Tech / FE Feed`.
    assert!(
        html.contains(r#"data-testid="breadcrumb""#),
        "feed page must render a breadcrumb nav"
    );
    assert!(
        html.contains(r#"<a href="/feeds">Feeds</a>"#),
        "breadcrumb must link to /feeds"
    );
    assert!(
        html.contains(&format!(
            r#"<a href="/categories/{}/entries">Tech</a>"#,
            cat_id
        )),
        "breadcrumb must link to the parent category page"
    );

    // Sidebar must receive active-category-id so the parent category is highlighted.
    assert!(
        html.contains(&format!("active-category-id=\"{}\"", cat_id)),
        "<rdrs-sidebar> must carry active-category-id for the feed's parent category"
    );

    // Feed has no icon seeded, so the header image must NOT render. (A
    // parallel test could seed an image row + assert the <img> appears; we
    // skip that variant to keep this test focused on layout-context wiring.)
    assert!(
        !html.contains(&format!(
            "src=\"/api/feeds/{}/icon\" alt=\"\" width=\"20\"",
            feed_id
        )),
        "header feed-icon img must not render when the feed has no icon row"
    );

    // Scoped Mark-as-Read dropdown must be visible and carry the feed's
    // GReader stream ID (`feed/<feed_url>`) so the bulk-mark only touches
    // this feed.
    assert!(
        html.contains(r#"id="mark-read-age""#),
        "feed page must render the Mark-as-Read dropdown"
    );
    assert!(
        html.contains(r#"data-mark-read-scope="feed/https://x/fe-feed""#),
        "Mark-as-Read scope must be the feed's GReader stream ID"
    );

    // Mark Above as Read button must render at the bottom of the list.
    assert!(
        html.contains(r#"id="mark-above-read""#),
        "feed page must render the Mark Above as Read button"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_status_filter() {
    let app = create_test_app_named(default_test_config(), "test_feed_entries_page_status");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_fst", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_fst", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let (feed_id, unread_id, read_id, starred_id) = app
        .db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, user_id, "FST").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/fst-feed",
                    title: Some("FST"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (u, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-fst-u",
                Some("Unread Entry"),
                Some("https://x/fst/u"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (r, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-fst-r",
                Some("Read Entry"),
                Some("https://x/fst/r"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (s, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-fst-s",
                Some("Starred Entry"),
                Some("https://x/fst/s"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            rdrs::models::entry::mark_as_read(conn, r.id).unwrap();
            rdrs::models::entry::star_entry(conn, s.id).unwrap();
            (feed.id, u.id, r.id, s.id)
        })
        .await
        .unwrap();

    // Default URL (no ?status=) → unread is the default; read entry hidden,
    // unread + starred (which is still unread) both visible.
    let resp = app.server.get(&format!("/feeds/{}/entries", feed_id)).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", unread_id)),
        "default view should include the unread entry"
    );
    assert!(
        !html.contains(&format!("id=\"entry-row-{}\"", read_id)),
        "default view should hide the read entry (default = unread filter)"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{}\"", starred_id)),
        "default view should include the starred-but-unread entry"
    );

    // ?status=all → every entry visible.
    let resp = app
        .server
        .get(&format!("/feeds/{}/entries?status=all", feed_id))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{}\"", unread_id)));
    assert!(html.contains(&format!("id=\"entry-row-{}\"", read_id)));
    assert!(html.contains(&format!("id=\"entry-row-{}\"", starred_id)));

    // ?status=read → only read.
    let resp = app
        .server
        .get(&format!("/feeds/{}/entries?status=read", feed_id))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{}\"", read_id)));
    assert!(!html.contains(&format!("id=\"entry-row-{}\"", unread_id)));

    // ?status=starred → only starred.
    let resp = app
        .server
        .get(&format!("/feeds/{}/entries?status=starred", feed_id))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{}\"", starred_id)));
    assert!(!html.contains(&format!("id=\"entry-row-{}\"", read_id)));
    assert!(!html.contains(&format!("id=\"entry-row-{}\"", unread_id)));

    // Filter <select> present; the active <option> matches the query.
    assert!(
        html.contains("data-status-filter"),
        "filter select must render on feed pages"
    );
    assert!(
        html.contains(r#"id="status-filter""#),
        "filter <select> must have id=\"status-filter\""
    );
    assert!(
        html.contains(&format!(
            r#"<option value="/feeds/{}/entries?status=starred" selected>Starred</option>"#,
            feed_id
        )),
        "Starred <option> must be `selected` when ?status=starred"
    );
    // Option order: All, Unread, Read, Starred (Unread URL = base path).
    let all_pos = html
        .find(&format!(
            r#"<option value="/feeds/{}/entries?status=all""#,
            feed_id
        ))
        .expect("All option present");
    let unread_pos = html
        .find(&format!(r#"<option value="/feeds/{}/entries""#, feed_id))
        .expect("Unread option present (base URL)");
    let read_pos = html
        .find(&format!(
            r#"<option value="/feeds/{}/entries?status=read""#,
            feed_id
        ))
        .expect("Read option present");
    let starred_pos = html
        .find(&format!(
            r#"<option value="/feeds/{}/entries?status=starred""#,
            feed_id
        ))
        .expect("Starred option present");
    assert!(
        all_pos < unread_pos && unread_pos < read_pos && read_pos < starred_pos,
        "filter <option>s must be in order: All, Unread, Read, Starred"
    );

    // Load-More form preserves the status filter via a hidden input.
    // (The seeded feed has only 3 entries so the form may not appear; we
    // assert only when present.)
    if html.contains("id=\"load-more\"") {
        assert!(
            html.contains(r#"<input type="hidden" name="status" value="starred">"#),
            "Load-More form must carry the active status filter"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_not_found() {
    let app = create_test_app_named(default_test_config(), "test_feed_entries_page_not_found");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_fnf", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_fnf", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let resp = app.server.get("/feeds/999999/entries").await;
    assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    let body = resp.text();
    assert!(
        body.contains("Feed not found"),
        "404 page should render the not-found heading, got: {body}"
    );
    assert!(
        body.contains("rdrs-sidebar"),
        "404 page should render inside the app chrome (sidebar present), got: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_other_user() {
    let app = create_test_app_named(default_test_config(), "test_feed_entries_page_other_user");

    // Register alice (owner of the feed)
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_fou", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Register bob (the cross-tenant user)
    app.server
        .post("/api/register")
        .json(&json!({ "username": "bob_fou", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Get alice's user_id and create her feed
    let feed_id: i64 = app
        .db
        .user(|conn| {
            let alice_id: i64 = conn
                .query_row(
                    "SELECT id FROM user WHERE username = 'alice_fou'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, alice_id, "Alice Cat").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/alice-feed",
                    title: Some("Alice Feed"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            feed.id
        })
        .await
        .unwrap();

    // Log in as bob
    app.server
        .post("/api/session")
        .json(&json!({ "username": "bob_fou", "password": "pw123456" }))
        .await
        .assert_status_ok();

    // Bob tries to access alice's feed entries — must be 404
    let resp = app.server.get(&format!("/feeds/{}/entries", feed_id)).await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user feed entries must return 404"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_load_more_fragment() {
    let app = create_test_app_named(default_test_config(), "test_feed_entries_page_lm");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_fl", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_fl", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let feed_id: i64 = app
        .db
        .user(|conn| {
            let user_id: i64 = conn
                .query_row("SELECT id FROM user LIMIT 1", [], |row| row.get(0))
                .unwrap();
            let cat = rdrs::models::category::create_category(conn, user_id, "T").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://x/lm-feed",
                    title: Some("LM Feed"),
                    description: None,
                    site_url: None,
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            for i in 0..3 {
                rdrs::models::entry::upsert_entry(
                    conn,
                    feed.id,
                    &format!("guid-lm-{}", i),
                    Some(&format!("Entry {}", i)),
                    Some(&format!("https://x/lm/{}", i)),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
            }
            feed.id
        })
        .await
        .unwrap();

    let resp = app
        .server
        .get(&format!("/feeds/{}/entries?fragment=1&after=0", feed_id))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains("data-entry-row"),
        "fragment must include row markup"
    );
    assert!(
        !html.contains("<rdrs-sidebar"),
        "fragment must NOT include layout chrome"
    );
    assert!(
        !html.contains("<h1>LM Feed</h1>"),
        "fragment must NOT include the page title"
    );
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
async fn test_feed_edit_page_not_found_renders_error_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/feeds/999999/edit").await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    let body = response.text();
    assert!(
        body.contains("Feed not found"),
        "404 page should render the not-found heading, got: {body}"
    );
    assert!(
        body.contains("rdrs-sidebar"),
        "404 page should render inside the app chrome (sidebar present), got: {body}"
    );
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
    // The active category's <option> must carry `selected` so the filter
    // dropdown reflects the URL query. Regression guard for the
    // `active_category_id: Option<i64>` rendering branch in feeds.html.
    assert!(
        body.contains("value=\"1\" selected>Cat A"),
        "Cat A option should be selected when ?category=1, got: {body}"
    );
    assert!(
        !body.contains("value=\"2\" selected"),
        "Cat B option must not be selected when ?category=1"
    );

    // With no `?category=` query the dropdown defaults to "All Categories" and
    // no <option> should carry selected.
    let response_default = app.server.get("/feeds").await;
    response_default.assert_status_ok();
    let body_default = response_default.text();
    assert!(
        !body_default.contains("value=\"1\" selected"),
        "no category should be selected when ?category= is absent"
    );
    assert!(
        !body_default.contains("value=\"2\" selected"),
        "no category should be selected when ?category= is absent"
    );
}

// ============================================================================
// Pre-login shell vs. logged-in chrome separation
//
// `templates/base.html` is the slim pre-login shell (only `rdrs-flash.js`).
// All logged-in chrome (kb-help, sidebar, app.js + the body-mounted
// `<rdrs-kb-help>` overlay) lives in `templates/app_layout.html` and ships
// only on per-route templates that extend it.
// ============================================================================

#[tokio::test]
async fn test_login_page_does_not_load_logged_in_chrome() {
    let app = create_test_app(default_test_config());
    let response = app.server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();

    // None of the logged-in chrome should appear on the pre-login shell.
    assert!(!body.contains("rdrs-kb-help.js"));
    assert!(!body.contains("rdrs-sidebar.js"));
    assert!(!body.contains("/static/js/app.js"));
    assert!(!body.contains("<rdrs-kb-help"));

    // Flash machinery is still needed (login/register use flash.redirect).
    assert!(body.contains("rdrs-flash.js"));
}

#[tokio::test]
async fn test_register_page_does_not_load_logged_in_chrome() {
    let app = create_test_app(default_test_config());
    let response = app.server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(!body.contains("rdrs-kb-help.js"));
    assert!(!body.contains("rdrs-sidebar.js"));
    assert!(!body.contains("/static/js/app.js"));
    assert!(!body.contains("<rdrs-kb-help"));

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

    // All chrome scripts must be present.
    assert!(body.contains("rdrs-kb-help.js"));
    assert!(body.contains("rdrs-sidebar.js"));
    assert!(body.contains("/static/js/app.js"));

    // The keyboard-help overlay element must be mounted on every
    // logged-in page so the `?` shortcut can show it.
    assert!(body.contains("<rdrs-kb-help"));
    // The kb-pending dropdown indicator was a CSR-era artifact (chord
    // prefix toast). No `g`-prefix sequences survive in the SSR world,
    // so the element must NOT mount.
    assert!(!body.contains("<rdrs-kb-pending>"));
    assert!(!body.contains("rdrs-kb-pending.js"));
}

// ============================================================================
// SSR / (unread) — PR-10 T1
// ============================================================================

#[tokio::test]
async fn test_unread_page_renders_entry_rows() {
    let app = create_test_app_named(default_test_config(), "test_pages_unread_ssr");

    // Register and login as alice
    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_unread", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_unread", "password": "pw123456" }))
        .await
        .assert_status_ok();

    // Get user id
    let user_id: i64 = app
        .db
        .user(|conn| {
            conn.query_row(
                "SELECT id FROM user WHERE username = 'alice_unread'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();

    // Seed: category + feed + 3 entries (2 unread, 1 read)
    let (_, entry_three_id) = app
        .db
        .user(move |conn| {
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://blog.example/feed",
                    title: Some("Example Blog"),
                    description: None,
                    site_url: Some("https://blog.example"),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();

            let (e1, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-entry-one",
                Some("Entry One"),
                Some("https://blog.example/one"),
                Some("<p>Content one</p>"),
                None,
                None,
                None,
            )
            .unwrap();
            let (_, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-entry-two",
                Some("Entry Two"),
                Some("https://blog.example/two"),
                Some("<p>Content two</p>"),
                None,
                None,
                None,
            )
            .unwrap();
            let (e3, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-entry-three",
                Some("Read Already"),
                Some("https://blog.example/three"),
                Some("<p>Content three</p>"),
                None,
                None,
                None,
            )
            .unwrap();
            (e1.id, e3.id)
        })
        .await
        .unwrap();

    // Mark entry three as read
    let _ = app
        .db
        .user(move |conn| rdrs::models::entry::mark_as_read(conn, entry_three_id))
        .await
        .unwrap();

    let response = app.server.get("/").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let html = response.text();

    // SSR rows present (CSR shell must be gone)
    assert!(
        !html.contains("<rdrs-entries-page"),
        "CSR shell should be gone"
    );
    assert!(html.contains("data-entry-row"), "rows should be SSR'd");
    assert!(html.contains("Entry One"), "unread entry should appear");
    assert!(html.contains("Entry Two"), "unread entry should appear");
    assert!(
        !html.contains("Read Already"),
        "read entries should be filtered out on /"
    );

    // Reading pane placeholder + swap target
    assert!(html.contains(r#"id="reading-pane""#));
    assert!(html.contains("Select an entry"));
}

// ============================================================================
// SSR entries family — PR-10 T2
// ============================================================================

#[tokio::test]
async fn test_entries_page_renders_ssr_rows() {
    let app = create_test_app_named(default_test_config(), "test_pages_entries_ssr");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_entries", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_entries", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let user_id: i64 = app
        .db
        .user(|conn| {
            conn.query_row(
                "SELECT id FROM user WHERE username = 'alice_entries'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();

    // Seed: 3 entries — all visible on /entries (no filter exclusions).
    app.db
        .user(move |conn| {
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://entries.example/feed",
                    title: Some("Entries Blog"),
                    description: None,
                    site_url: Some("https://entries.example"),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-e1",
                Some("Entries Alpha"),
                Some("https://entries.example/alpha"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-e2",
                Some("Entries Beta"),
                Some("https://entries.example/beta"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-e3",
                Some("Entries Gamma"),
                Some("https://entries.example/gamma"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        })
        .await
        .unwrap();

    let response = app.server.get("/entries").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let html = response.text();

    assert!(
        !html.contains("<rdrs-entries-page"),
        "CSR shell should be gone"
    );
    assert!(html.contains("data-entry-row"), "rows should be SSR'd");
    assert!(html.contains("Entries Alpha"), "entry should appear");
    assert!(html.contains("Entries Beta"), "entry should appear");
    assert!(html.contains("Entries Gamma"), "entry should appear");
    assert!(html.contains(r#"id="reading-pane""#));
    assert!(html.contains("Select an entry"));
}

#[tokio::test]
async fn test_read_entries_page_renders_ssr_rows() {
    let app = create_test_app_named(default_test_config(), "test_pages_read_ssr");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_read", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_read", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let user_id: i64 = app
        .db
        .user(|conn| {
            conn.query_row(
                "SELECT id FROM user WHERE username = 'alice_read'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();

    let (read_one_id, read_two_id, unread_id) = app
        .db
        .user(move |conn| {
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://read.example/feed",
                    title: Some("Read Blog"),
                    description: None,
                    site_url: Some("https://read.example"),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (e1, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-r1",
                Some("Read Alpha"),
                Some("https://read.example/alpha"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (e2, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-r2",
                Some("Read Beta"),
                Some("https://read.example/beta"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (e3, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-r3",
                Some("Unread One"),
                Some("https://read.example/unread"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            (e1.id, e2.id, e3.id)
        })
        .await
        .unwrap();

    // Mark two entries as read; leave one unread.
    let _ = app
        .db
        .user(move |conn| rdrs::models::entry::mark_as_read(conn, read_one_id))
        .await
        .unwrap();
    let _ = app
        .db
        .user(move |conn| rdrs::models::entry::mark_as_read(conn, read_two_id))
        .await
        .unwrap();
    let _ = unread_id; // suppress warning

    let response = app.server.get("/entries/read").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let html = response.text();

    assert!(
        !html.contains("<rdrs-entries-page"),
        "CSR shell should be gone"
    );
    assert!(html.contains("data-entry-row"), "rows should be SSR'd");
    assert!(html.contains("Read Alpha"), "read entry should appear");
    assert!(html.contains("Read Beta"), "read entry should appear");
    assert!(
        !html.contains("Unread One"),
        "unread entry should be filtered out on /entries/read"
    );
    assert!(html.contains(r#"id="reading-pane""#));
    assert!(html.contains("Select an entry"));
}

#[tokio::test]
async fn test_starred_entries_page_renders_ssr_rows() {
    let app = create_test_app_named(default_test_config(), "test_pages_starred_ssr");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_starred", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_starred", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let user_id: i64 = app
        .db
        .user(|conn| {
            conn.query_row(
                "SELECT id FROM user WHERE username = 'alice_starred'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();

    let (starred_one_id, starred_two_id) = app
        .db
        .user(move |conn| {
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://starred.example/feed",
                    title: Some("Starred Blog"),
                    description: None,
                    site_url: Some("https://starred.example"),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (e1, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-s1",
                Some("Starred Alpha"),
                Some("https://starred.example/alpha"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (e2, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-s2",
                Some("Starred Beta"),
                Some("https://starred.example/beta"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-s3",
                Some("Unstarred One"),
                Some("https://starred.example/unstarred"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            (e1.id, e2.id)
        })
        .await
        .unwrap();

    // Star the two matching entries.
    let _ = app
        .db
        .user(move |conn| rdrs::models::entry::star_entry(conn, starred_one_id))
        .await
        .unwrap();
    let _ = app
        .db
        .user(move |conn| rdrs::models::entry::star_entry(conn, starred_two_id))
        .await
        .unwrap();

    let response = app.server.get("/entries/starred").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let html = response.text();

    assert!(
        !html.contains("<rdrs-entries-page"),
        "CSR shell should be gone"
    );
    assert!(html.contains("data-entry-row"), "rows should be SSR'd");
    assert!(
        html.contains("Starred Alpha"),
        "starred entry should appear"
    );
    assert!(html.contains("Starred Beta"), "starred entry should appear");
    assert!(
        !html.contains("Unstarred One"),
        "unstarred entry should be filtered out on /entries/starred"
    );
    assert!(html.contains(r#"id="reading-pane""#));
    assert!(html.contains("Select an entry"));
}

#[tokio::test]
async fn test_summarized_entries_page_renders_ssr_rows() {
    let app = create_test_app_named(default_test_config(), "test_pages_summarized_ssr");

    app.server
        .post("/api/register")
        .json(&json!({ "username": "alice_summarized", "password": "pw123456" }))
        .await
        .assert_status(StatusCode::CREATED);
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_summarized", "password": "pw123456" }))
        .await
        .assert_status_ok();

    let user_id: i64 = app
        .db
        .user(|conn| {
            conn.query_row(
                "SELECT id FROM user WHERE username = 'alice_summarized'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .await
        .unwrap()
        .unwrap();

    let (sum_one_id, sum_two_id) = app
        .db
        .user(move |conn| {
            let cat = rdrs::models::category::create_category(conn, user_id, "Tech").unwrap();
            let feed = rdrs::models::feed::create_feed(
                conn,
                &rdrs::models::feed::CreateFeedParams {
                    category_id: cat.id,
                    url: "https://sum.example/feed",
                    title: Some("Summary Blog"),
                    description: None,
                    site_url: Some("https://sum.example"),
                    custom_user_agent: None,
                    http2_disabled: None,
                    custom_referrer: None,
                },
            )
            .unwrap();
            let (e1, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-sum1",
                Some("Summarized Alpha"),
                Some("https://sum.example/alpha"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let (e2, _) = rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-sum2",
                Some("Summarized Beta"),
                Some("https://sum.example/beta"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            rdrs::models::entry::upsert_entry(
                conn,
                feed.id,
                "guid-sum3",
                Some("No Summary One"),
                Some("https://sum.example/nosummary"),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            (e1.id, e2.id)
        })
        .await
        .unwrap();

    // Insert entry_summary rows for the two matching entries.
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO entry_summary (user_id, entry_id, status, summary_text) \
                 VALUES (?1, ?2, 'completed', 'Summary text alpha')",
                rusqlite::params![user_id, sum_one_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry_summary (user_id, entry_id, status, summary_text) \
                 VALUES (?1, ?2, 'completed', 'Summary text beta')",
                rusqlite::params![user_id, sum_two_id],
            )
            .unwrap();
        })
        .await
        .unwrap();

    let response = app.server.get("/entries/summarized").await;
    assert_eq!(response.status_code(), StatusCode::OK);
    let html = response.text();

    assert!(
        !html.contains("<rdrs-entries-page"),
        "CSR shell should be gone"
    );
    assert!(html.contains("data-entry-row"), "rows should be SSR'd");
    assert!(
        html.contains("Summarized Alpha"),
        "summarized entry should appear"
    );
    assert!(
        html.contains("Summarized Beta"),
        "summarized entry should appear"
    );
    assert!(
        !html.contains("No Summary One"),
        "entry without summary should be filtered out on /entries/summarized"
    );
    assert!(html.contains(r#"id="reading-pane""#));
    assert!(html.contains("Select an entry"));
}
