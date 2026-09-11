//! Integration tests for the SSR `/statistics` page and the
//! shared `/api/me` + `/api/sidebar` endpoints used by the chrome.

mod common;
use common::{app_signed_in_as, create_test_app, default_test_config, login, setup_users};

use axum::http::StatusCode;
use rdrs::Db;
use rdrs::models::{category, entry, feed};
use serde_json::Value;

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
            ..Default::default()
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
    let app = create_test_app(default_test_config()).await;
    let response = app.server.get("/statistics").await;
    assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn test_statistics_page_renders_ssr_content() {
    let mut app = create_test_app(default_test_config()).await;
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
async fn test_statistics_page_marks_the_selected_period_active() {
    let (app, _) = app_signed_in_as("admin").await;

    // No period and an unknown one both fall back to the 7d default.
    for (query, active) in [
        ("", "7d"),
        ("?period=30d", "30d"),
        ("?period=invalid", "7d"),
    ] {
        let response = app.server.get(&format!("/statistics{query}")).await;
        response.assert_status_ok();
        assert!(
            response
                .text()
                .contains(&format!("class=\"stats-period-btn active\">{active}")),
            "{query}"
        );
    }
}

#[tokio::test]
async fn test_statistics_page_admin_sees_sitewide() {
    let (app, _) = app_signed_in_as("admin").await;

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

/// The site-wide database figures are memoized, so a second render inside the
/// TTL must not re-run the `COUNT(*)`s behind them. Staleness is the observable
/// side of that: entries added between the two renders stay hidden until the
/// slot is dropped.
#[tokio::test]
async fn test_admin_database_stats_are_served_from_the_cache() {
    let (app, (admin_id, _user_id)) = app_signed_in_as("admin").await;

    let first = app.server.get("/statistics").await;
    first.assert_status_ok();
    assert!(
        first
            .text()
            .contains("data-testid=\"stat-db-total-entries\">0<"),
        "no entries seeded yet"
    );

    seed_entries(&app.db, admin_id).await;

    let cached = app.server.get("/statistics").await;
    cached.assert_status_ok();
    assert!(
        cached
            .text()
            .contains("data-testid=\"stat-db-total-entries\">0<"),
        "the cached slot must still be serving the pre-seed count"
    );

    app.state.admin_db_stats_cache.invalidate_all();
    app.state.admin_db_stats_cache.run_pending_tasks();

    let fresh = app.server.get("/statistics").await;
    fresh.assert_status_ok();
    assert!(
        fresh
            .text()
            .contains("data-testid=\"stat-db-total-entries\">5<"),
        "dropping the slot must recompute, body was:\n{}",
        fresh.text()
    );
}

#[tokio::test]
async fn test_statistics_page_user_no_sitewide() {
    let (app, _) = app_signed_in_as("user").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("Site-wide Statistics"));
}

#[tokio::test]
async fn test_statistics_page_custom_period() {
    let (app, _) = app_signed_in_as("admin").await;

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
    let (app, _) = app_signed_in_as("admin").await;

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
    let (app, (_admin_id, user_id)) = app_signed_in_as("admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
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
    let mut app = create_test_app(default_test_config()).await;
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
    let mut app = create_test_app(default_test_config()).await;
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
    let (app, _) = app_signed_in_as("admin").await;

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
    let (app, (_admin_id, user_id)) = app_signed_in_as("admin").await;
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
    let mut app = create_test_app(default_test_config()).await;
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
    let mut app = create_test_app(default_test_config()).await;
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
