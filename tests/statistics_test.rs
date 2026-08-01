//! Integration tests for the SSR `/statistics` page and the
//! shared `/api/me` + `/api/sidebar` endpoints used by the chrome.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::models::{category, entry, feed, user};
use rdrs::{AppState, Config, Db, Role, auth, create_router, services};
use serde_json::{Value, json};

struct TestApp {
    server: TestServer,
    db: Db,
}

async fn create_test_app(_name: &str) -> TestApp {
    let db = Db::connect_in_memory().await.unwrap();
    let config = Config {
        database_url: ":memory:".to_string(),
        server_bind: "127.0.0.1:8080".parse().unwrap(),
        signup_enabled: true,
        multi_user_enabled: true,
        secret: vec![0u8; 32],
        secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:8080".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
        cookie_secure: false,
        auth_proxy_header: String::new(),
        trusted_proxy_networks: Vec::new(),
        auth_proxy_user_creation: false,
        disable_local_auth: false,
        auth_proxy_groups_header: String::new(),
        auth_proxy_admin_group: String::new(),
        auth_proxy_logout_url: None,
        login_rate_limit_attempts: rdrs::middleware::rate_limit::LOGIN_MAX_ATTEMPTS,
        login_rate_limit_window_secs: rdrs::middleware::rate_limit::LOGIN_WINDOW_SECS,
        hsts: false,
        hsts_max_age: 31_536_000,
        hsts_include_subdomains: true,
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

async fn setup_users(db: &Db) -> (i64, i64) {
    let password_hash = rdrs::auth::hash_password("password123456789").unwrap();
    let admin = user::create_user(db, "admin", &password_hash, Role::Admin)
        .await
        .unwrap();

    let password_hash = rdrs::auth::hash_password("password123456789").unwrap();
    let user = user::create_user(db, "user", &password_hash, Role::User)
        .await
        .unwrap();

    (admin.id, user.id)
}

async fn login(server: &mut TestServer, username: &str) {
    let login = server
        .post("/api/session")
        .json(&json!({
            "username": username,
            "password": "password123456789"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

async fn seed_entries(db: &Db, admin_id: i64) {
    let cat = category::create_category(db, admin_id, "Tech")
        .await
        .unwrap();
    let feed = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://example.com/feed",
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

    let published: chrono::DateTime<chrono::Utc> = "2026-03-15T10:00:00Z".parse().unwrap();
    let mut entry_ids = Vec::new();
    for i in 1..=5 {
        let (e, _) = entry::upsert_entry(
            db,
            feed.id,
            &format!("guid-{i}"),
            Some(&format!("Entry {i}")),
            None,
            None,
            None,
            None,
            Some(published),
        )
        .await
        .unwrap();
        entry_ids.push(e.id);
    }
    // Mark 3 as read
    for id in &entry_ids[..3] {
        rdrs::db_execute!(
            db,
            "UPDATE entry SET read_at = '2026-03-15T12:00:00Z' WHERE id = $1",
            *id
        )
        .unwrap();
    }
    // Star 1
    rdrs::db_execute!(
        db,
        "UPDATE entry SET starred_at = '2026-03-15T14:00:00Z' WHERE id = $1",
        entry_ids[0]
    )
    .unwrap();
}

// ----- SSR /statistics -----

#[tokio::test]
async fn test_statistics_page_requires_login() {
    let app = create_test_app("test_stats_auth").await;
    let response = app.server.get("/statistics").await;
    assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn test_statistics_page_renders_ssr_content() {
    let mut app = create_test_app("test_stats_ssr").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics?period=all").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR content is present — period buttons, stats cards, headings.
    assert!(body.contains("stats-period-btn"));
    assert!(body.contains("Total Entries"));
    assert!(body.contains("Daily Read Articles"));
    assert!(body.contains("Entries by Category"));
    assert!(body.contains("Top Feeds"));
    // The "all" period button is marked active.
    assert!(body.contains("class=\"stats-period-btn active\">All"));

    // Legacy CSR markers must be gone.
    assert!(!body.contains("<rdrs-statistics-page>"));
    assert!(!body.contains("/static/js/pages/statistics.js"));
}

#[tokio::test]
async fn test_statistics_page_default_period_is_7d() {
    let mut app = create_test_app("test_stats_default_period").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // Default period is 7d — that button is marked active.
    assert!(body.contains("class=\"stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_page_period_30d() {
    let mut app = create_test_app("test_stats_period_30d").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics?period=30d").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("class=\"stats-period-btn active\">30d"));
}

#[tokio::test]
async fn test_statistics_page_invalid_period_falls_back_to_7d() {
    let mut app = create_test_app("test_stats_invalid_period").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics?period=invalid").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("class=\"stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_page_admin_sees_sitewide() {
    let mut app = create_test_app("test_stats_admin").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // The admin section heading is rendered for non-masquerading admins.
    assert!(body.contains("Site-wide Statistics"));
    assert!(body.contains("Total Users"));
    // SQLite can measure free space (freelist PRAGMAs), so the Reclaimable card
    // is present here. It is omitted on PostgreSQL, which reports `None` rather
    // than a zero — see `models::statistics::get_admin_database_stats` and the
    // `reclaimable.is_none()` assertion in `tests/postgres_test.rs`.
    assert!(
        body.contains("Reclaimable"),
        "SQLite must render the Reclaimable card"
    );
}

#[tokio::test]
async fn test_statistics_page_user_no_sitewide() {
    let mut app = create_test_app("test_stats_user_no_sitewide").await;
    setup_users(&app.db).await;
    login(&mut app.server, "user").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("Site-wide Statistics"));
}

#[tokio::test]
async fn test_statistics_page_custom_period() {
    let mut app = create_test_app("test_stats_custom").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app
        .server
        .get("/statistics?period=custom&from=2026-03-01&to=2026-03-31")
        .await;
    response.assert_status_ok();
    let body = response.text();
    // Custom dates are reflected in the form's date inputs.
    assert!(body.contains("value=\"2026-03-01\""));
    assert!(body.contains("value=\"2026-03-31\""));
}

#[tokio::test]
async fn test_statistics_page_invalid_custom_range_falls_back() {
    let mut app = create_test_app("test_stats_bad_custom").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app
        .server
        .get("/statistics?period=custom&from=2026-12-01&to=2026-01-01")
        .await;
    response.assert_status_ok();
    let body = response.text();
    // Falls back to 7d.
    assert!(body.contains("class=\"stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_page_masquerade_hides_admin_section() {
    let mut app = create_test_app("test_stats_masq").await;
    let (_admin_id, user_id) = setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    app.server
        .post(&format!("/admin/users/{user_id}/masquerade"))
        .await
        .assert_status(axum::http::StatusCode::SEE_OTHER);

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("Site-wide Statistics"));
}

#[tokio::test]
async fn test_statistics_page_embeds_sidebar_bootstrap() {
    let mut app = create_test_app("test_stats_sidebar_bootstrap").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // The page embeds the sidebar payload inline so the sidebar paints
    // without a round trip on first visit.
    assert!(body.contains("id=\"rdrs-sidebar-bootstrap\""));
    assert!(body.contains("\"username\":\"admin\""));
    assert!(body.contains("\"is_admin\":true"));
    // Categories from seed appear in the bootstrap payload.
    assert!(body.contains("\"name\":\"Tech\""));
}

#[tokio::test]
async fn test_statistics_page_renders_overview_counts() {
    let mut app = create_test_app("test_stats_overview").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/statistics?period=all").await;
    response.assert_status_ok();
    let body = response.text();
    // Seeded data: 5 total entries, 3 read, 1 starred.
    assert!(body.contains("Total Entries"));
    // Quick sanity check — the seeded values appear in the page.
    assert!(body.contains(">5</div>"));
    assert!(body.contains(">3</div>"));
    assert!(body.contains(">1</div>"));
}

#[tokio::test]
async fn test_statistics_page_direct_labels_single_max_day() {
    let mut app = create_test_app("test_stats_is_max").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    // seed_entries marks entries 1..=3 read at 2026-03-15, so the daily-read
    // chart has a single busiest bucket within a custom window covering that
    // date. Only that bucket reaches `daily_max`, so exactly one column gets
    // the direct-labeled `stats-bar-value` (ties must not spam a number on
    // every column).
    seed_entries(&app.db, admin_id).await;
    login(&mut app.server, "admin").await;

    let response = app
        .server
        .get("/statistics?period=custom&from=2026-03-01&to=2026-03-31")
        .await;
    response.assert_status_ok();
    let body = response.text();

    let value_labels = body.matches("stats-bar-value").count();
    assert_eq!(
        value_labels, 1,
        "exactly one busiest column should be direct-labeled"
    );
    // The direct label shows the peak count (3 entries read that day).
    assert!(body.contains("<span class=\"stats-bar-value\">3</span>"));
}

// ----- /api/me + /api/sidebar -----

#[tokio::test]
async fn test_api_me_returns_role_and_flags() {
    let mut app = create_test_app("test_api_me").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app("test_api_me_masq").await;
    let (_admin_id, user_id) = setup_users(&app.db).await;
    login(&mut app.server, "admin").await;
    app.server
        .post(&format!("/admin/users/{user_id}/masquerade"))
        .await
        .assert_status(axum::http::StatusCode::SEE_OTHER);

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
    let mut app = create_test_app("test_api_sidebar").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&mut app.server, "admin").await;

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

#[tokio::test]
async fn test_api_sidebar_total_summarized() {
    let mut app = create_test_app("test_api_sidebar_total_summarized").await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;

    // Seed a completed summary for the first entry (belongs to admin_id).
    // upsert_pending first — set_completed is a no-op without an existing row.
    // Do this BEFORE hitting /api/sidebar so the cache is cold when we read.
    let entry_id: i64 =
        rdrs::query_scalar!(&app.db, i64, "SELECT id FROM entry ORDER BY id LIMIT 1").unwrap();
    rdrs::models::entry_summary::upsert_pending(&app.db, admin_id, entry_id)
        .await
        .unwrap();
    rdrs::models::entry_summary::set_completed(&app.db, admin_id, entry_id, "summary text")
        .await
        .unwrap();

    login(&mut app.server, "admin").await;

    let body: Value = app.server.get("/api/sidebar").await.json();
    assert_eq!(body["total_summarized"], 1);
}
