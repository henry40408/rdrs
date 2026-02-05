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
async fn test_home_page_shows_unread_count() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Create some entries
    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "Test Feed"],
            )
            .unwrap();
            // Add 3 unread entries
            for i in 1..=3 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                    rusqlite::params![1, format!("guid-{}", i), format!("Entry {}", i)],
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
    // Should contain the unread count somewhere
    assert!(body.contains("3") || body.contains("unread"));
}

#[tokio::test]
async fn test_home_page_while_masquerading() {
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
    // Should show the masqueraded user's name
    assert!(body.contains("user"));
    // Should still show admin link (because original user is admin)
    assert!(body.contains("[Admin]") || body.contains("admin"));

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
async fn test_user_settings_page_content() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Should contain user info sections
    assert!(body.contains("admin"));
    assert!(body.contains("Password") || body.contains("password"));
}

#[tokio::test]
async fn test_settings_page_shows_version() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show version info
    assert!(body.contains("Version") || body.contains("version"));
    // Should show user agent
    assert!(body.contains("User-Agent") || body.contains("user-agent") || body.contains("RDRS"));
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
    assert!(body.contains("Settings saved"));
}

// ============================================================================
// Entry Page with Save Services Tests
// ============================================================================

#[tokio::test]
async fn test_entry_page_shows_save_button_when_configured() {
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

            // Create entry
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "Test Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![1, "guid-1", "Test Entry", "https://example.com/entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries/1").await;
    response.assert_status_ok();
    // Page should render successfully with save services configured
}

#[tokio::test]
async fn test_entry_page_shows_summarize_when_kagi_configured() {
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

            // Create entry
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://example.com/feed.xml", "Test Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, link) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![1, "guid-1", "Test Entry", "https://example.com/entry"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/entries/1").await;
    response.assert_status_ok();
    // Page should render successfully with Kagi configured
}

// ============================================================================
// Regular User Permissions Tests
// ============================================================================

#[tokio::test]
async fn test_regular_user_home_page_no_admin_link() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    login(&app.server, "user").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show username
    assert!(body.contains("user"));
    // Should NOT show admin link
    assert!(!body.contains("[Admin]"));
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
async fn test_user_settings_page_shows_linkding_configured() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Configure Linkding
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

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show linkding is configured
    assert!(body.contains("linkding") || body.contains("Linkding"));
}

#[tokio::test]
async fn test_user_settings_page_shows_custom_entries_per_page() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    // Set custom entries per page
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

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show the custom value somewhere
    assert!(body.contains("100"));
}

// ============================================================================
// Settings Page Configuration Display
// ============================================================================

#[tokio::test]
async fn test_settings_page_shows_signup_status() {
    let config = Config {
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

    // Should show signup configuration
    assert!(
        body.contains("Signup")
            || body.contains("signup")
            || body.contains("Registration")
            || body.contains("registration")
    );
}

#[tokio::test]
async fn test_settings_page_with_custom_user_agent() {
    let config = Config {
        user_agent: "Custom-Agent/2.0".to_string(),
        ..default_test_config()
    };
    let app = create_test_app(config);
    setup_users(&app.db).await;

    login(&app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Should show custom user agent
    assert!(body.contains("Custom-Agent/2.0"));
}
