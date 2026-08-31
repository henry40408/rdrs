//! Offline reading: the sync ledger, the library page, and the one property the
//! whole feature rests on — that mirroring a reader's queue does not consume it.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
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

/// One feed for `username`, and `count` entries on it, oldest first.
async fn seed_entries(db: &Db, username: &str, count: usize) -> Vec<i64> {
    let user = rdrs::models::user::find_by_username(db, username)
        .await
        .unwrap()
        .unwrap();
    let cat = rdrs::models::category::create_category(db, user.id, &format!("cat-{username}"))
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: &format!("https://example.com/{username}.xml"),
            title: Some("Feed"),
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
            &format!("guid-{username}-{i}"),
            Some(&format!("Entry {i}")),
            Some(&format!("https://example.com/{username}/{i}")),
            Some(&format!("<p>Body {i}.</p>")),
            None,
            None,
            // Ascending published_at, so the last seeded entry is the newest.
            Some(chrono::Utc::now() - chrono::Duration::minutes((count - i) as i64)),
        )
        .await
        .unwrap();
        ids.push(entry.id);
    }
    ids
}

async fn set_keep(db: &Db, username: &str, keep: i64) {
    let user = rdrs::models::user::find_by_username(db, username)
        .await
        .unwrap()
        .unwrap();
    rdrs::models::user_settings::update_offline_keep(db, user.id, keep)
        .await
        .unwrap();
}

async fn seed_users(db: &Db) {
    common::seed_account(db, "reader", "vulture-mango-77-quilt", Role::Admin).await;
    common::seed_account(db, "other", "vulture-mango-77-quilt", Role::User).await;
}

// --- The manifest ---

#[tokio::test]
async fn manifest_requires_a_session() {
    let app = create_test_app(default_test_config()).await;
    let response = app.server.get("/api/offline/manifest").await;
    assert_ne!(response.status_code(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn manifest_is_empty_until_the_reader_opts_in() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 3).await;
    login(&mut app.server, "reader").await;

    let body: serde_json::Value = app.server.get("/api/offline/manifest").await.json();

    assert_eq!(body["keep"], 0);
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn manifest_holds_the_newest_unread_entries_up_to_the_budget() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let ids = seed_entries(&app.db, "reader", 5).await;
    set_keep(&app.db, "reader", 2).await;
    login(&mut app.server, "reader").await;

    let body: serde_json::Value = app.server.get("/api/offline/manifest").await.json();
    let got: Vec<i64> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect();

    // Newest first, and only as many as the reader asked for.
    assert_eq!(got, vec![ids[4], ids[3]]);
}

#[tokio::test]
async fn manifest_never_names_another_readers_entries() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let mine = seed_entries(&app.db, "reader", 2).await;
    let theirs = seed_entries(&app.db, "other", 2).await;
    set_keep(&app.db, "reader", 50).await;
    login(&mut app.server, "reader").await;

    let body: serde_json::Value = app.server.get("/api/offline/manifest").await.json();
    let got: Vec<i64> = body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_i64().unwrap())
        .collect();

    for id in &mine {
        assert!(got.contains(id), "own entry {id} missing from the manifest");
    }
    for id in &theirs {
        assert!(!got.contains(id), "another reader's entry {id} leaked");
    }
}

#[tokio::test]
async fn manifest_cache_key_is_opaque_and_differs_per_reader() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;

    login(&mut app.server, "reader").await;
    let mine: serde_json::Value = app.server.get("/api/offline/manifest").await.json();
    let mine_key = mine["cache_key"].as_str().unwrap().to_string();

    login(&mut app.server, "other").await;
    let theirs: serde_json::Value = app.server.get("/api/offline/manifest").await.json();
    let theirs_key = theirs["cache_key"].as_str().unwrap().to_string();

    assert_ne!(mine_key, theirs_key);
    // The key names a cache in a place JavaScript can read. A user id there
    // would tell every page how many accounts the deployment has and let one
    // reader guess another's cache name.
    let reader = rdrs::models::user::find_by_username(&app.db, "reader")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(mine_key, reader.id.to_string());
    assert_eq!(mine_key.len(), 16, "expected a truncated hex tag");
    assert!(mine_key.chars().all(|c| c.is_ascii_hexdigit()));
}

// --- Prefetch must not consume the queue ---

/// `read_at` once the detached mark-read write has had a chance to land, or
/// `None` if it never does within `attempts`.
///
/// The write is dispatched off the critical path (`dispatch_mark_read_on_open`),
/// so a bare read straight after the response proves nothing: it races the task
/// and passes whether or not the entry was going to be marked. Both directions
/// of this property therefore wait the same way — the negative assertion is
/// only worth anything if the positive one would have been observed by then.
async fn read_at_within(
    db: &Db,
    id: i64,
    attempts: usize,
) -> Option<chrono::DateTime<chrono::Utc>> {
    for _ in 0..attempts {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let read_at = rdrs::models::entry::find_by_id(db, id)
            .await
            .unwrap()
            .unwrap()
            .read_at;
        if read_at.is_some() {
            return read_at;
        }
    }
    None
}

#[tokio::test]
async fn prefetching_a_fragment_leaves_the_entry_unread() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let ids = seed_entries(&app.db, "reader", 1).await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .get(&format!("/entries/{}/fragment?offline=1", ids[0]))
        .add_header("sec-fetch-dest", "empty")
        .await;
    response.assert_status_ok();

    assert!(
        read_at_within(&app.db, ids[0], 50).await.is_none(),
        "a sync opens every entry in the queue; if that marks them read the \
         reader's whole backlog disappears the moment offline reading is on"
    );
}

#[tokio::test]
async fn opening_a_fragment_normally_still_marks_it_read() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let ids = seed_entries(&app.db, "reader", 1).await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .get(&format!("/entries/{}/fragment", ids[0]))
        .add_header("sec-fetch-dest", "empty")
        .await;
    response.assert_status_ok();

    assert!(
        read_at_within(&app.db, ids[0], 50).await.is_some(),
        "opening an entry must still mark it read"
    );
}

// --- The library page ---

#[tokio::test]
async fn library_lists_what_the_browser_should_be_holding() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let ids = seed_entries(&app.db, "reader", 3).await;
    set_keep(&app.db, "reader", 2).await;
    login(&mut app.server, "reader").await;

    let response = app.server.get("/entries/offline").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains(&format!("entry-row-{}", ids[2])));
    assert!(body.contains(&format!("entry-row-{}", ids[1])));
    assert!(
        !body.contains(&format!("entry-row-{}", ids[0])),
        "the oldest entry is outside the budget and must not be listed"
    );
    // Nothing on this page may need the network to be useful.
    assert!(!body.contains("id=\"load-more\""));
    assert!(!body.contains("data-entries-search"));
}

#[tokio::test]
async fn library_says_so_when_offline_reading_is_off() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 2).await;
    login(&mut app.server, "reader").await;

    let body = app.server.get("/entries/offline").await.text();

    assert!(body.contains("Offline reading is off"));
    assert!(body.contains("Settings"));
}

/// The reason this page exists at all. Every other list stops at a page of 50
/// and offers Load More, which reaches the server — so with the connection gone
/// a reader could otherwise see only the first page of a library their own
/// browser is holding in full.
#[tokio::test]
async fn library_lists_the_whole_budget_past_the_first_page_of_a_list() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let ids = seed_entries(&app.db, "reader", 55).await;
    set_keep(&app.db, "reader", 200).await;
    login(&mut app.server, "reader").await;

    let inbox = app.server.get("/").await.text();
    assert!(
        inbox.contains("id=\"load-more\""),
        "sanity: 55 entries is expected to leave a second page of the inbox"
    );

    let body = app.server.get("/entries/offline").await.text();
    for id in &ids {
        assert!(
            body.contains(&format!("entry-row-{id}")),
            "entry {id} is saved but the library page does not list it"
        );
    }
    assert!(!body.contains("id=\"load-more\""));
}

#[tokio::test]
async fn library_requires_a_session() {
    let app = create_test_app(default_test_config()).await;
    let response = app.server.get("/entries/offline").await;
    assert_ne!(response.status_code(), axum::http::StatusCode::OK);
}

// --- The setting ---

#[tokio::test]
async fn the_preferences_form_saves_the_budget() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 30,
            "retention_read_days": 0,
            "offline_keep": 25,
        }))
        .await;
    assert!(response.status_code().is_redirection());

    let user = rdrs::models::user::find_by_username(&app.db, "reader")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rdrs::models::user_settings::get_offline_keep(&app.db, user.id)
            .await
            .unwrap(),
        25
    );
}

#[tokio::test]
async fn the_preferences_form_rejects_a_budget_over_the_cap() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    set_keep(&app.db, "reader", 25).await;
    login(&mut app.server, "reader").await;

    let response = app
        .server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 30,
            "retention_read_days": 0,
            "offline_keep": 5000,
        }))
        .await;

    assert!(common::flash_text(&response).contains("offline_keep"));
    let user = rdrs::models::user::find_by_username(&app.db, "reader")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rdrs::models::user_settings::get_offline_keep(&app.db, user.id)
            .await
            .unwrap(),
        25,
        "a rejected value must leave the stored one alone"
    );
}

/// Same contract the other two number fields have: a value offered by the
/// form's own `<datalist>` that the handler then refuses is worse than no
/// suggestion, because the reader picked it out of the browser's dropdown.
#[tokio::test]
async fn offline_keep_suggestions_are_all_accepted() {
    let app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    let user = rdrs::models::user::find_by_username(&app.db, "reader")
        .await
        .unwrap()
        .unwrap();

    for &v in rdrs::models::user_settings::OFFLINE_KEEP_SUGGESTIONS {
        rdrs::models::user_settings::update_offline_keep(&app.db, user.id, v)
            .await
            .unwrap_or_else(|e| panic!("suggestion {v} rejected: {e}"));
    }
}

// --- The client's bootstrap ---

#[tokio::test]
async fn signed_in_pages_carry_the_cache_name_and_budget() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 1).await;
    set_keep(&app.db, "reader", 50).await;
    login(&mut app.server, "reader").await;

    let body = app.server.get("/").await.text();

    // offline.js reads both before its first network call, so that another
    // account's cached articles are gone before anything can be served from
    // them. Absent from the document, that wipe would wait on a round trip.
    assert!(body.contains("data-offline-key=\""));
    assert!(body.contains("data-offline-keep=\"50\""));
    assert!(body.contains("/static/js/offline.js"));
}

/// The scriptless half of the way in. `<rdrs-sidebar>` renders the link for
/// everyone else, but a reader with no JavaScript has only this nav — and with
/// Load More out of reach offline, a page they cannot navigate away from is a
/// page they cannot get past.
#[tokio::test]
async fn the_scriptless_navigation_leads_to_the_library() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 1).await;
    set_keep(&app.db, "reader", 50).await;
    login(&mut app.server, "reader").await;

    let body = app.server.get("/").await.text();

    assert!(body.contains("href=\"/entries/offline\""));
}

/// The other state, in its own app: the chrome a page is built from is read
/// once per session and cached, so one request cannot observe the setting on
/// both sides of a change.
#[tokio::test]
async fn the_scriptless_navigation_omits_the_library_while_nothing_is_saved() {
    let mut app = create_test_app(default_test_config()).await;
    seed_users(&app.db).await;
    seed_entries(&app.db, "reader", 1).await;
    login(&mut app.server, "reader").await;

    let body = app.server.get("/").await.text();

    // A link to a page nothing is being saved to is an invitation to an empty
    // list.
    assert!(!body.contains("/entries/offline"));
}

#[tokio::test]
async fn the_sign_in_page_bootstraps_no_offline_state() {
    let app = create_test_app(default_test_config()).await;
    let body = app.server.get("/login").await.text();

    // Same reasoning as pwa.js: a reader who never gets past /login has no
    // cache to name, and naming one would hand an anonymous page a key.
    assert!(!body.contains("data-offline-key"));
    assert!(!body.contains("/static/js/offline.js"));
}
