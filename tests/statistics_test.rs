//! Integration tests for the statistics CSR shell + JSON API.

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool, Role};
use rusqlite::Connection;
use serde_json::{json, Value};

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

fn create_test_app(name: &str) -> TestApp {
    let write_conn = open_shared_memory(name);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(name);

    let (db, _handle) = DbPool::new(write_conn, read_conn);
    let config = Config {
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
    };
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

async fn setup_users(db: &DbPool) -> (i64, i64) {
    db.user(move |conn| {
        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["admin", password_hash, Role::Admin.as_str()],
        )
        .unwrap();
        let admin_id = conn.last_insert_rowid();

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

async fn seed_entries(db: &DbPool, admin_id: i64) {
    db.user(move |conn| {
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'Tech')",
            rusqlite::params![admin_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (1, 'https://example.com/feed', 'Test Feed')",
            [],
        )
        .unwrap();
        for i in 1..=5 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, ?1, ?2, '2026-03-15T10:00:00Z')",
                rusqlite::params![format!("guid-{}", i), format!("Entry {}", i)],
            )
            .unwrap();
        }
        // Mark 3 as read
        conn.execute(
            "UPDATE entry SET read_at = '2026-03-15T12:00:00Z' WHERE id IN (1, 2, 3)",
            [],
        )
        .unwrap();
        // Star 1
        conn.execute(
            "UPDATE entry SET starred_at = '2026-03-15T14:00:00Z' WHERE id = 1",
            [],
        )
        .unwrap();
    })
    .await
    .unwrap();
}

// ----- Shell route -----

#[tokio::test]
async fn test_statistics_page_requires_login() {
    let app = create_test_app("test_stats_auth");
    let response = app.server.get("/statistics").await;
    assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn test_statistics_page_shell_renders_for_user() {
    let app = create_test_app("test_stats_shell");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // Shell should reference the page custom element + module path,
    // not the SSR statistics content.
    assert!(body.contains("<rdrs-statistics-page>"));
    assert!(body.contains("/static/js/pages/statistics.js"));
    // SSR content (period buttons, stats cards) should NOT be in the shell.
    assert!(!body.contains("stats-period-btn"));
    assert!(!body.contains("Total Entries"));
}

#[tokio::test]
async fn test_statistics_shell_embeds_sidebar_bootstrap() {
    let app = create_test_app("test_stats_shell_bootstrap");
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // The shell embeds the sidebar payload inline so the sidebar paints
    // without a round trip on first visit.
    assert!(body.contains("id=\"rdrs-sidebar-bootstrap\""));
    assert!(body.contains("\"username\":\"admin\""));
    assert!(body.contains("\"is_admin\":true"));
    // Categories from seed appear in the bootstrap payload.
    assert!(body.contains("\"name\":\"Tech\""));
}

// ----- /api/statistics -----

#[tokio::test]
async fn test_api_statistics_requires_login() {
    let app = create_test_app("test_api_stats_auth");
    let response = app.server.get("/api/statistics").await;
    assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_api_statistics_returns_user_data() {
    let app = create_test_app("test_api_stats_user");
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/statistics?period=all").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "all");
    assert_eq!(body["overview"]["total_entries"], 5);
    assert_eq!(body["overview"]["read_entries"], 3);
    assert_eq!(body["overview"]["starred_entries"], 1);
    assert!(body["categories"].is_array());
    assert!(body["top_feeds"].is_array());
}

#[tokio::test]
async fn test_api_statistics_default_period_is_7d() {
    let app = create_test_app("test_api_stats_default");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/statistics").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "7d");
}

#[tokio::test]
async fn test_api_statistics_period_30d() {
    let app = create_test_app("test_api_stats_30d");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/statistics?period=30d").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "30d");
}

#[tokio::test]
async fn test_api_statistics_invalid_period_falls_back() {
    let app = create_test_app("test_api_stats_invalid");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/statistics?period=invalid").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "7d");
}

#[tokio::test]
async fn test_api_statistics_admin_sees_sitewide() {
    let app = create_test_app("test_api_stats_admin");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/statistics").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert!(body["admin"].is_object());
    assert!(body["admin"]["total_users"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn test_api_statistics_user_no_sitewide() {
    let app = create_test_app("test_api_stats_nonadmin");
    setup_users(&app.db).await;
    login(&app.server, "user").await;

    let response = app.server.get("/api/statistics").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert!(body["admin"].is_null());
}

#[tokio::test]
async fn test_api_statistics_custom_period() {
    let app = create_test_app("test_api_stats_custom");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/api/statistics?period=custom&from=2026-03-01&to=2026-03-31")
        .await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "custom");
    assert_eq!(body["custom_from"], "2026-03-01");
    assert_eq!(body["custom_to"], "2026-03-31");
}

#[tokio::test]
async fn test_api_statistics_invalid_custom_range_falls_back() {
    let app = create_test_app("test_api_stats_bad_custom");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/api/statistics?period=custom&from=2026-12-01&to=2026-01-01")
        .await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["active_period"], "7d");
}

#[tokio::test]
async fn test_api_statistics_masquerade_hides_admin_section() {
    let app = create_test_app("test_api_stats_masq");
    let (_admin_id, user_id) = setup_users(&app.db).await;
    login(&app.server, "admin").await;

    app.server
        .post(&format!("/api/admin/masquerade/{}", user_id))
        .await
        .assert_status_ok();

    let response = app.server.get("/api/statistics").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert!(body["admin"].is_null());
}

// ----- /api/me + /api/sidebar -----

#[tokio::test]
async fn test_api_me_returns_role_and_flags() {
    let app = create_test_app("test_api_me");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/me").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
    assert_eq!(body["is_admin"], true);
    assert_eq!(body["is_masquerading"], false);
}

#[tokio::test]
async fn test_api_me_masquerade_flag_set() {
    let app = create_test_app("test_api_me_masq");
    let (_admin_id, user_id) = setup_users(&app.db).await;
    login(&app.server, "admin").await;
    app.server
        .post(&format!("/api/admin/masquerade/{}", user_id))
        .await
        .assert_status_ok();

    let response = app.server.get("/api/me").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["username"], "user");
    assert_eq!(body["is_masquerading"], true);
    // Original user is admin → is_admin remains true under masquerade.
    assert_eq!(body["is_admin"], true);
}

#[tokio::test]
async fn test_api_sidebar_returns_categories_with_unread() {
    let app = create_test_app("test_api_sidebar");
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/api/sidebar").await;
    response.assert_status_ok();
    let body: Value = response.json();
    assert_eq!(body["username"], "admin");
    let cats = body["categories"].as_array().unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0]["name"], "Tech");
    // 5 seeded, 3 marked read → 2 unread.
    assert_eq!(cats[0]["unread_count"], 2);
    assert_eq!(body["total_unread"], 2);
}
