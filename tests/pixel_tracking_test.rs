//! The open-tracking pixel end to end: what the endpoint records, what it
//! refuses to record, and the property the whole feature rests on — that the
//! pixel survives the sanitiser and stays same-origin.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::{AppState, Config, Db, Role, auth, create_router, query_scalar, services};
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
        fetcher: rdrs::services::Fetcher::new(config.fetch_allow_private.clone()).unwrap(),
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

    TestApp {
        server: TestServer::builder()
            .save_cookies()
            .build(create_router(state)),
        db,
    }
}

async fn login(server: &mut TestServer, username: &str) {
    let login = server
        .post("/api/session")
        .json(&json!({ "username": username, "password": "vulture-mango-77-quilt" }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

async fn seed_users(db: &Db) {
    common::seed_account(db, "reader", "vulture-mango-77-quilt", Role::Admin).await;
    common::seed_account(db, "other", "vulture-mango-77-quilt", Role::User).await;
}

async fn user_id(db: &Db, username: &str) -> i64 {
    rdrs::models::user::find_by_username(db, username)
        .await
        .unwrap()
        .unwrap()
        .id
}

/// A feed named `label` for `username` with `count` entries, returning their ids.
async fn seed_feed(db: &Db, username: &str, label: &str, count: usize) -> Vec<i64> {
    let uid = user_id(db, username).await;
    let cat = rdrs::models::category::create_category(db, uid, &format!("cat-{username}-{label}"))
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: &format!("https://example.com/{username}-{label}.xml"),
            title: Some(label),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();

    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let (entry, _) = rdrs::models::entry::upsert_entry(
            db,
            feed.id,
            &format!("guid-{username}-{label}-{i}"),
            Some(&format!("Entry {i}")),
            Some(&format!("https://example.com/{username}/{label}/{i}")),
            Some(&format!("<p>Body {i}.</p>")),
            None,
            None,
            Some(chrono::Utc::now()),
        )
        .await
        .unwrap();
        ids.push(entry.id);
    }
    ids
}

/// The default feed most tests need.
async fn seed_entries(db: &Db, username: &str, count: usize) -> Vec<i64> {
    seed_feed(db, username, "Feed", count).await
}

/// A second feed, for the comparisons the ranking is about.
async fn seed_second_feed(db: &Db, username: &str, count: usize) -> Vec<i64> {
    seed_feed(db, username, "Ignored", count).await
}

async fn enable_tracking(db: &Db, username: &str) {
    let uid = user_id(db, username).await;
    rdrs::models::user_settings::update_pixel_tracking(db, uid, true)
        .await
        .unwrap();
}

async fn open_count(db: &Db) -> i64 {
    query_scalar!(db, i64, "SELECT COUNT(*) FROM entry_open").unwrap()
}

/// The pixel URL the render paths would embed, built the same way they do.
fn pixel_path(secret: &[u8], uid: i64, entry_id: i64) -> String {
    let sig = rdrs::secret::pixel_sig(secret, uid, entry_id);
    format!("/p/{uid}-{entry_id}-{sig}.gif")
}

const TEST_SECRET: &[u8] = &[0u8; 32];

// --- The endpoint ---

#[tokio::test]
async fn a_hit_records_one_open_and_is_idempotent() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;

    let path = pixel_path(TEST_SECRET, uid, entries[0]);
    app.server.get(&path).await.assert_status_ok();
    assert_eq!(open_count(&app.db).await, 1);

    // A second client, a re-render, or a proxy retry must not inflate the
    // count — the metric is "entries opened", not "requests served".
    app.server.get(&path).await.assert_status_ok();
    assert_eq!(open_count(&app.db).await, 1);
}

#[tokio::test]
async fn the_response_is_an_uncacheable_gif() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;

    let response = app
        .server
        .get(&pixel_path(TEST_SECRET, uid, entries[0]))
        .await;
    response.assert_status_ok();
    assert_eq!(
        response.header("content-type").to_str().unwrap(),
        "image/gif"
    );
    // A cached pixel is a pixel that never reports again.
    assert!(
        response
            .header("cache-control")
            .to_str()
            .unwrap()
            .contains("no-store")
    );
    assert_eq!(response.as_bytes().len(), 43);
}

#[tokio::test]
async fn a_bad_signature_records_nothing_and_still_returns_the_gif() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;

    // Answering 200 either way is deliberate: a 403 would tell whoever is
    // probing which tokens are real.
    for path in [
        format!("/p/{uid}-{}-deadbeef.gif", entries[0]),
        format!("/p/{uid}-{}-.gif", entries[0]),
        "/p/nonsense.gif".to_string(),
    ] {
        let response = app.server.get(&path).await;
        response.assert_status_ok();
        assert_eq!(response.as_bytes().len(), 43);
    }
    assert_eq!(open_count(&app.db).await, 0);
}

#[tokio::test]
async fn an_opted_out_reader_records_nothing() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    let uid = user_id(&app.db, "reader").await;

    // A correctly signed token, but tracking was never turned on. This is also
    // the shape of a hit on HTML that was rendered before the reader opted out.
    app.server
        .get(&pixel_path(TEST_SECRET, uid, entries[0]))
        .await
        .assert_status_ok();
    assert_eq!(open_count(&app.db).await, 0);
}

#[tokio::test]
async fn entries_predating_the_opt_in_are_not_recorded() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    let uid = user_id(&app.db, "reader").await;

    // Push the baseline past the entry, as opting in tomorrow would.
    rdrs::db_execute!(
        &app.db,
        "UPDATE user_settings SET pixel_tracking_enabled_at = datetime('now', '+1 day') WHERE user_id = $1",
        uid
    )
    .unwrap();

    app.server
        .get(&pixel_path(TEST_SECRET, uid, entries[0]))
        .await
        .assert_status_ok();
    assert_eq!(
        open_count(&app.db).await,
        0,
        "the backlog is outside the denominator, so it must not count"
    );
}

#[tokio::test]
async fn a_token_cannot_record_an_open_on_another_readers_entry() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let other_entries = seed_entries(&app.db, "other", 1).await;
    seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    enable_tracking(&app.db, "other").await;
    let reader = user_id(&app.db, "reader").await;

    // Correctly signed for `reader`, but the entry belongs to `other`. The
    // ownership join is what stops one account writing into another's counts.
    app.server
        .get(&pixel_path(TEST_SECRET, reader, other_entries[0]))
        .await
        .assert_status_ok();
    assert_eq!(open_count(&app.db).await, 0);
}

// --- The render path ---

#[tokio::test]
async fn the_reading_pane_carries_an_unproxied_same_origin_pixel() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .get(&format!("/entries/{}/fragment", entries[0]))
        .await;
    response.assert_status_ok();
    let body = response.text();

    // The regression this feature is shaped around: `sanitize_html` strips 1x1
    // images and rewrites every `<img src>` through the image proxy, so a pixel
    // injected before it would be gone, and one that survived would no longer be
    // same-origin. Both are only true because injection runs after sanitising.
    let expected = pixel_path(TEST_SECRET, uid, entries[0]);
    assert!(
        body.contains(&expected),
        "reading pane should carry {expected}"
    );
    assert!(
        !body.contains(&format!("/api/proxy/image?url=%2Fp%2F{uid}")),
        "the pixel must not be routed through the image proxy"
    );
}

#[tokio::test]
async fn an_opted_out_reading_pane_carries_no_pixel() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .get(&format!("/entries/{}/fragment", entries[0]))
        .await;
    response.assert_status_ok();
    assert!(!response.text().contains("/p/"));
}

#[tokio::test]
async fn greader_content_carries_an_absolute_pixel() {
    let mut config = default_test_config();
    // External clients render the content off-origin, so a root-relative URL
    // would resolve against their own host and never reach us.
    config.public_base_url = Some("https://rdrs.example.com".to_string());
    let mut app = create_test_app(config).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
        .await;
    response.assert_status_ok();
    let body = response.text();
    let sig = rdrs::secret::pixel_sig(TEST_SECRET, uid, entries[0]);
    assert!(
        body.contains(&format!(
            "https://rdrs.example.com/p/{uid}-{}-{sig}.gif",
            entries[0]
        )),
        "greader item content should carry an absolute pixel: {body}"
    );
}

#[tokio::test]
async fn the_offline_mirror_fragment_carries_no_pixel() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 1).await;
    enable_tracking(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    // `?offline=1` is the service worker mirroring the queue, not a reader
    // opening anything — the same request that must not mark the entry read.
    // `offline.js` fetches every same-origin `<img src>` in what it caches, so a
    // pixel here would report an open for every entry merely queued.
    let response = app
        .server
        .get(&format!("/entries/{}/fragment?offline=1", entries[0]))
        .await;
    response.assert_status_ok();
    assert!(
        !response.text().contains("/p/"),
        "the offline mirror must not carry a pixel"
    );

    // The same holds for a browser prefetching or prerendering the fragment.
    let response = app
        .server
        .get(&format!("/entries/{}/fragment", entries[0]))
        .add_header("sec-purpose", "prefetch")
        .await;
    response.assert_status_ok();
    assert!(
        !response.text().contains("/p/"),
        "a speculative fetch must not carry a pixel"
    );

    // A real open still does, or the feature would record nothing at all.
    let response = app
        .server
        .get(&format!("/entries/{}/fragment", entries[0]))
        .await;
    assert!(response.text().contains("/p/"));
}

// --- The read-side UI ---

#[tokio::test]
async fn the_feeds_column_appears_only_once_tracking_is_on() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 1).await;
    login(&mut app.server, "reader").await;

    // Opted out: a column of dashes would be worse than no column.
    let body = app.server.get("/feeds").await.text();
    assert!(!body.contains("Open Rate"));

    enable_tracking(&app.db, "reader").await;
    let body = app.server.get("/feeds").await.text();
    assert!(body.contains("Open Rate"));
}

#[tokio::test]
async fn the_feeds_column_suppresses_a_rate_it_cannot_stand_behind() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let floor = rdrs::models::entry_open::MIN_TRACKED_FOR_RATE as usize;
    let entries = seed_entries(&app.db, "reader", floor - 1).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    app.server
        .get(&pixel_path(TEST_SECRET, uid, entries[0]))
        .await;

    // One open out of four is 25%, but on four entries that number is noise.
    let body = app.server.get("/feeds").await.text();
    assert!(
        body.contains("Needs at least"),
        "a sub-floor sample must render the dash, not a percentage"
    );
    assert!(!body.contains("25%"));
}

#[tokio::test]
async fn the_feeds_column_reports_the_rate_and_its_counts() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let entries = seed_entries(&app.db, "reader", 10).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    for id in entries.iter().take(4) {
        app.server.get(&pixel_path(TEST_SECRET, uid, *id)).await;
    }

    // The raw counts ride along: 40% of ten is a very different claim from 40%
    // of five.
    let body = app.server.get("/feeds").await.text();
    assert!(body.contains("40% (4/10)"), "{body}");
}

#[tokio::test]
async fn the_statistics_section_ranks_the_least_opened_feed_first() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let popular = seed_entries(&app.db, "reader", 10).await;
    let ignored = seed_second_feed(&app.db, "reader", 10).await;
    enable_tracking(&app.db, "reader").await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    for id in popular.iter().take(8) {
        app.server.get(&pixel_path(TEST_SECRET, uid, *id)).await;
    }
    for id in ignored.iter().take(1) {
        app.server.get(&pixel_path(TEST_SECRET, uid, *id)).await;
    }

    let body = app.server.get("/statistics").await.text();
    assert!(body.contains("Feeds by Open Rate"), "{body}");
    assert!(body.contains("Tracked since"));
    let ignored_at = body.find("10% (1/10)").expect("the ignored feed is listed");
    let popular_at = body.find("80% (8/10)").expect("the popular feed is listed");
    assert!(
        ignored_at < popular_at,
        "the unsubscribe candidate belongs at the top"
    );
}

#[tokio::test]
async fn the_statistics_section_is_absent_while_opted_out() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 10).await;
    login(&mut app.server, "reader").await;

    assert!(
        !app.server
            .get("/statistics")
            .await
            .text()
            .contains("Feeds by Open Rate")
    );
}

// --- The settings toggle ---

#[tokio::test]
async fn the_preferences_form_toggles_tracking_without_moving_the_baseline() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let uid = user_id(&app.db, "reader").await;
    login(&mut app.server, "reader").await;

    let form = |tracking: bool| {
        let mut fields = vec![
            ("theme", "system".to_string()),
            ("entries_per_page", "30".to_string()),
            ("retention_read_days", "0".to_string()),
            ("sidebar_sort", "name".to_string()),
            ("offline_keep", "0".to_string()),
        ];
        if tracking {
            fields.push(("pixel_tracking", "1".to_string()));
        }
        fields
    };

    app.server
        .post("/user-settings/preferences")
        .form(&form(true))
        .await;
    let first = rdrs::models::user_settings::get_pixel_tracking_enabled_at(&app.db, uid)
        .await
        .unwrap()
        .expect("the checkbox turns tracking on");

    // Saving any other preference re-submits this form. The baseline must not
    // move, or the denominator silently resets.
    app.server
        .post("/user-settings/preferences")
        .form(&form(true))
        .await;
    assert_eq!(
        rdrs::models::user_settings::get_pixel_tracking_enabled_at(&app.db, uid)
            .await
            .unwrap(),
        Some(first)
    );

    // An unchecked checkbox sends no field at all.
    app.server
        .post("/user-settings/preferences")
        .form(&form(false))
        .await;
    assert_eq!(
        rdrs::models::user_settings::get_pixel_tracking_enabled_at(&app.db, uid)
            .await
            .unwrap(),
        None
    );
}
