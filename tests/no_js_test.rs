//! The paths a browser takes with JavaScript disabled.
//!
//! Every test here deliberately refuses the two things `static/js/` does for a
//! normal browser: it never sets an `X-CSRF-Token` header (that is `csrf.js`
//! patching `fetch`), and it never posts JSON to `/api/*` (that is `login.js` /
//! `setup.js` intercepting the submit). What is left is what a browser with
//! scripting off actually sends — a urlencoded form POST carrying only the
//! fields in the markup — and it has to work.
//!
//! Before this suite existed, none of it did: no template rendered the `_csrf`
//! field, so `csrf_guard` rejected every mutation, and the sign-in form had no
//! `action`/`method` at all.

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum_test::TestServer;
use rdrs::{AppState, Config, Db, auth, create_router, services};

async fn create_test_server(config: Config) -> TestServer {
    create_test_server_with_db(config).await.0
}

async fn create_test_server_with_db(config: Config) -> (TestServer, Db) {
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

    // A cookie jar and nothing else — no default `X-CSRF-Token` header, which
    // is the whole point of this suite.
    let server = TestServer::builder()
        .save_cookies()
        .build(create_router(state));
    (server, db)
}

/// Pull the first server-rendered `_csrf` value out of a page, the way a
/// browser submitting the form would.
fn csrf_field(html: &str) -> String {
    let marker = r#"<input type="hidden" name="_csrf" value=""#;
    let start = html
        .find(marker)
        .unwrap_or_else(|| panic!("no _csrf field rendered in:\n{html}"))
        + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').expect("unterminated _csrf value");
    rest[..end].to_string()
}

/// Create the first account through `POST /setup`, as a native form submit.
/// Leaves the caller signed *out* — setup redirects to `/login` rather than
/// opening a session.
async fn setup_account(server: &TestServer) {
    let setup_page = server.get("/setup").await;
    setup_page.assert_status_ok();
    let created = server
        .post("/setup")
        .form(&[
            ("_csrf", csrf_field(&setup_page.text())),
            ("username", "testuser".to_string()),
            ("password", "vulture-mango-77-quilt".to_string()),
            ("confirm-password", "vulture-mango-77-quilt".to_string()),
        ])
        .await;
    created.assert_status(StatusCode::SEE_OTHER);
}

/// Create the first account and sign in, using only native form POSTs.
async fn setup_and_login(server: &TestServer) {
    setup_account(server).await;

    let login_page = server.get("/login").await;
    login_page.assert_status_ok();
    let signed_in = server
        .post("/login")
        .form(&[
            ("_csrf", csrf_field(&login_page.text())),
            ("username", "testuser".to_string()),
            ("password", "vulture-mango-77-quilt".to_string()),
        ])
        .await;
    signed_in.assert_status(StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn setup_and_login_work_as_native_form_posts() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    // Signed in: the landing page renders rather than bouncing to /login.
    let home = server.get("/").await;
    home.assert_status_ok();
}

#[tokio::test]
async fn login_form_carries_action_and_method() {
    let server = create_test_server(default_test_config()).await;
    let page = server.get("/login").await;
    let html = page.text();

    // Without both of these a submit becomes `GET /login?password=…`, putting
    // the password in the address bar and the browser history.
    assert!(
        html.contains(r#"method="post""#) && html.contains(r#"action="/login""#),
        "login form must post to /login:\n{html}"
    );
}

#[tokio::test]
async fn failed_login_re_renders_the_form_with_an_error() {
    let server = create_test_server(default_test_config()).await;
    // Account exists but we stay signed out — a signed-in GET /login redirects.
    setup_account(&server).await;

    let login_page = server.get("/login").await;
    let response = server
        .post("/login")
        .form(&[
            ("_csrf", csrf_field(&login_page.text())),
            ("username", "testuser".to_string()),
            ("password", "wrong-password-entirely".to_string()),
        ])
        .await;

    // 200 with the form back, not a bare error status: the visitor has to be
    // able to try again without JavaScript to re-render anything.
    response.assert_status_ok();
    let html = response.text();
    assert!(
        html.contains("Invalid credentials"),
        "expected the generic failure message:\n{html}"
    );
    assert!(
        html.contains(r#"action="/login""#),
        "the form must come back"
    );
}

#[tokio::test]
async fn logged_in_mutation_succeeds_with_only_the_form_field() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    let page = server.get("/categories").await;
    page.assert_status_ok();

    let response = server
        .post("/categories")
        .form(&[
            ("_csrf", csrf_field(&page.text())),
            ("name", "From a scriptless browser".to_string()),
        ])
        .await;

    // Redirect, not 403: `csrf_guard` accepted the rendered field.
    response.assert_status(StatusCode::SEE_OTHER);
    let after = server.get("/categories").await;
    assert!(
        after.text().contains("From a scriptless browser"),
        "the category should have been created"
    );
}

/// A mutation's only confirmation is its flash message, and `<rdrs-flash>` used
/// to be filled in by JavaScript reading a JSON bootstrap — so with scripting
/// off every action succeeded in total silence. The banner is server-rendered
/// now, so following the redirect shows it.
#[tokio::test]
async fn a_mutation_confirms_itself_with_a_server_rendered_banner() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    let page = server.get("/categories").await;
    let created = server
        .post("/categories")
        .form(&[
            ("_csrf", csrf_field(&page.text())),
            ("name", "Announced".to_string()),
        ])
        .await;
    created.assert_status(StatusCode::SEE_OTHER);

    // Follow the redirect the way a browser would, carrying the flash cookie
    // the response just set.
    let landing = created.header(header::LOCATION);
    let after = server.get(landing.to_str().unwrap()).await;
    after.assert_status_ok();
    let html = after.text();

    assert!(
        html.contains(r#"data-testid="flash-message""#),
        "the flash must be rendered as a banner, not left in a JSON blob:\n{html}"
    );
    assert!(
        html.contains("Category created."),
        "the banner must carry the message text"
    );
    // The element doubles as the client's mount point, so it has to be the
    // thing the server rendered into — not a sibling the JS would ignore.
    assert!(
        html.contains("<rdrs-flash"),
        "the banners belong inside `<rdrs-flash>`"
    );
    // Nothing is left that only JavaScript could read.
    assert!(
        !html.contains("rdrs-flash-bootstrap"),
        "the JSON bootstrap is gone; SSR is the only path now"
    );
}

/// With nothing to say, `<rdrs-flash>` must render *childless* — not even
/// whitespace. `.banner-stack:empty { display: none }` is what keeps the empty
/// mount out of the layout, and `:empty` counts text nodes: a stray newline
/// from the template turns it into a permanent invisible band on every page,
/// which on mobile pushes the header out of alignment.
#[tokio::test]
async fn an_empty_flash_mount_has_no_children_at_all() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    let html = server.get("/categories").await.text();
    assert!(
        html.contains("<rdrs-flash"),
        "the mount point must still be rendered for the client to append into"
    );
    assert!(
        html.contains("tabindex=\"-1\"></rdrs-flash>"),
        "an empty flash mount must close immediately, with no whitespace inside:\n{html}"
    );
}

/// A scriptless reader keeps the UTC timestamp (`rdrs-flash.js` rewrites it to
/// local time when it runs), so it has to be both rendered and machine-readable.
#[tokio::test]
async fn a_server_rendered_banner_timestamps_itself_unambiguously() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    let page = server.get("/categories").await;
    let created = server
        .post("/categories")
        .form(&[
            ("_csrf", csrf_field(&page.text())),
            ("name", "Timed".to_string()),
        ])
        .await;
    let landing = created.header(header::LOCATION);
    let after = server.get(landing.to_str().unwrap()).await;
    let html = after.text();

    assert!(
        html.contains(r#"data-testid="flash-time""#),
        "the banner must show a time"
    );
    assert!(
        html.contains("<time class=\"banner-time\" datetime=\""),
        "and carry it as an RFC 3339 `datetime` the client can localize"
    );
}

#[tokio::test]
async fn mutation_without_the_form_field_is_still_rejected() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    // The guard must not have been loosened by any of the above: a POST with
    // neither the header nor the field is still forbidden.
    let response = server
        .post("/categories")
        .form(&[("name", "No token at all".to_string())])
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

/// Seed a category, a feed and one unread entry directly, returning
/// `(feed_id, entry_id)`. Subscribing through the UI would need a live feed to
/// fetch, which has nothing to do with what these tests assert.
async fn seed_entry(db: &Db) -> (i64, i64) {
    let user_id: i64 = rdrs::query_scalar!(db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/no-js-feed",
            title: Some("No-JS Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (entry, _) = rdrs::models::entry::upsert_entry(
        db,
        feed.id,
        "guid-no-js",
        Some("E"),
        Some("https://x/n"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    (feed.id, entry.id)
}

/// A scriptless entry action is a top-level navigation, and every entry-action
/// handler answers with a bare `<template>` fragment that renders as a blank
/// page when loaded as a document. The response has to be a redirect back to
/// the list instead — with the action still applied.
#[tokio::test]
async fn entry_actions_redirect_a_scriptless_form_post() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (feed_id, entry_id) = seed_entry(&db).await;

    let list_url = format!("/feeds/{feed_id}/entries?status=unread");
    let page = server.get(&list_url).await;
    page.assert_status_ok();
    let token = csrf_field(&page.text());

    // What a browser with scripting off actually sends for a row's mark-read
    // button: a urlencoded POST, tagged `Sec-Fetch-Dest: document` because it is
    // a navigation, referred by the list page it was fired from.
    let response = server
        .post(&format!("/entries/{entry_id}/read"))
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .add_header(
            header::REFERER,
            HeaderValue::from_str(&format!("http://localhost{list_url}")).unwrap(),
        )
        .form(&[("_csrf", token.clone())])
        .await;

    // Back to the exact list, filters intact and *without* `?entry=`: an action
    // fired from a row must not drag the reader into that entry's pane.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), list_url);

    // The redirect must not have cost the action. The write is dispatched off
    // the critical path via a detached task, so poll for it.
    let mut read_at: Option<String> = None;
    for _ in 0..100 {
        read_at = rdrs::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            entry_id
        )
        .unwrap();
        if read_at.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        read_at.is_some(),
        "a scriptless /read must still mark the entry read"
    );

    // Fired from the reading pane, whose URL already carries `?entry=`: the
    // pane stays open across the redirect.
    let pane_url = format!("/entries?entry={entry_id}");
    let pane = server.get(&pane_url).await;
    pane.assert_status_ok();
    let response = server
        .post(&format!("/entries/{entry_id}/star"))
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .add_header(
            header::REFERER,
            HeaderValue::from_str(&format!("http://localhost{pane_url}")).unwrap(),
        )
        .form(&[("_csrf", csrf_field(&pane.text()))])
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), pane_url);

    // No usable referrer (a fresh tab, a stripped header): fall back to All
    // Entries with the entry open, the only feedback available until the flash
    // banner is server-rendered.
    let response = server
        .post(&format!("/entries/{entry_id}/unstar"))
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .form(&[("_csrf", token)])
        .await;
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("/entries?entry={entry_id}")
    );
}

/// The entry title is a plain `<a href="/entries/{id}/fragment">`, so with
/// scripting off clicking it is a top-level navigation. It has to do what the
/// swap helper's `fetch()` would have done — mark the entry read — and then
/// land somewhere that is not the blank partial.
#[tokio::test]
async fn opening_an_entry_marks_it_read_without_javascript() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (feed_id, entry_id) = seed_entry(&db).await;

    let list_url = format!("/feeds/{feed_id}/entries?status=unread");
    let response = server
        .get(&format!("/entries/{entry_id}/fragment"))
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .add_header(
            header::REFERER,
            HeaderValue::from_str(&format!("http://localhost{list_url}")).unwrap(),
        )
        .await;

    // Lands on the list it was opened from, pane pre-opened on the entry.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(
        response.header(header::LOCATION),
        format!("{list_url}&entry={entry_id}")
    );

    let mut read_at: Option<String> = None;
    for _ in 0..100 {
        read_at = rdrs::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            entry_id
        )
        .unwrap();
        if read_at.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        read_at.is_some(),
        "opening an entry with scripting off must mark it read"
    );

    // The landing page reflects it: the row's toggle now offers the inverse
    // action, which is the only read-state feedback a scriptless reader gets.
    let landed = server.get(&format!("{list_url}&entry={entry_id}")).await;
    landed.assert_status_ok();
    assert!(
        landed
            .text()
            .contains(&format!(r#"action="/entries/{entry_id}/unread""#)),
        "the row must come back offering Mark Unread"
    );
}

/// Follow a scriptless entry action's redirect the way a browser would, and
/// return the landing page's HTML.
async fn follow(server: &TestServer, response: &axum_test::TestResponse) -> String {
    response.assert_status(StatusCode::SEE_OTHER);
    let landing = response.header(header::LOCATION);
    let page = server.get(landing.to_str().unwrap()).await;
    page.assert_status_ok();
    page.text()
}

/// Post an entry action the way a scriptless browser does: a form navigation,
/// carrying the rendered `_csrf` field and nothing else.
async fn post_action(server: &TestServer, path: &str, csrf: &str) -> axum_test::TestResponse {
    server
        .post(path)
        .add_header(
            HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        )
        .form(&[("_csrf", csrf.to_string())])
        .await
}

/// The redirect #481 introduced answers with no body, so an action whose result
/// is not visible in the page it lands on said nothing at all. The message the
/// swap helper would have toasted now rides along as a flash cookie.
#[tokio::test]
async fn a_scriptless_entry_action_reports_itself() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (_feed_id, entry_id) = seed_entry(&db).await;

    let page = server.get(&format!("/entries?entry={entry_id}")).await;
    let csrf = csrf_field(&page.text());

    // Mark read first so the unread toggle has something to undo — "Marked as
    // unread." is only raised when the call actually changes state.
    post_action(&server, &format!("/entries/{entry_id}/read"), &csrf)
        .await
        .assert_status(StatusCode::SEE_OTHER);

    let unread = post_action(&server, &format!("/entries/{entry_id}/unread"), &csrf).await;
    let html = follow(&server, &unread).await;
    assert!(
        html.contains("Marked as unread."),
        "the action's message must survive the redirect as a banner:\n{html}"
    );
    assert!(
        html.contains(r#"data-testid="flash-message""#),
        "and be rendered as one"
    );
}

/// Star and mark-read change the list they return to, so they carry no message
/// — matching the swap helper, which raises no toast for them either. A banner
/// here would be noise on every click.
#[tokio::test]
async fn a_self_evident_action_stays_quiet() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (_feed_id, entry_id) = seed_entry(&db).await;

    let page = server.get(&format!("/entries?entry={entry_id}")).await;
    let csrf = csrf_field(&page.text());

    let starred = post_action(&server, &format!("/entries/{entry_id}/star"), &csrf).await;
    let html = follow(&server, &starred).await;
    assert!(
        !html.contains(r#"data-testid="flash-message""#),
        "starring speaks for itself — the row comes back starred:\n{html}"
    );
}

/// Save's whole effect is on someone else's server, so the flash is the only
/// evidence it happened. Pointed at an unreachable Linkding here, which still
/// proves the plumbing: the failure has to reach the reader rather than vanish.
#[tokio::test]
async fn a_failed_save_reaches_a_scriptless_reader() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (_feed_id, entry_id) = seed_entry(&db).await;

    let user_id: i64 = rdrs::query_scalar!(&db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    rdrs::models::user_settings::update_save_services(
        &db,
        user_id,
        &rdrs::services::save::SaveServicesConfig {
            linkding: Some(rdrs::services::save::linkding::LinkdingConfig {
                api_url: "http://127.0.0.1:1/linkding".to_string(),
                api_token: "t".to_string(),
            }),
            kagi: None,
        },
    )
    .await
    .unwrap();

    let page = server.get(&format!("/entries?entry={entry_id}")).await;
    let csrf = csrf_field(&page.text());

    let saved = post_action(&server, &format!("/entries/{entry_id}/save"), &csrf).await;
    let html = follow(&server, &saved).await;
    assert!(
        html.contains("Linkding"),
        "the save failure must be reported, not swallowed:\n{html}"
    );
    assert!(
        html.contains("banner--error"),
        "and it is an error, not a success"
    );
}

/// Fetch-full-content loads the article into the *open pane* and persists
/// nothing, so a scriptless caller — who gets a redirect and a page rebuilt
/// from the feed-supplied body — could never see it. Say so instead of doing
/// the work: the entry's link here is unreachable, so a fetch that did happen
/// would surface as a failure message rather than this one.
#[tokio::test]
async fn fetch_full_content_declines_rather_than_lying() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (_feed_id, entry_id) = seed_entry(&db).await;

    let page = server.get(&format!("/entries?entry={entry_id}")).await;
    let csrf = csrf_field(&page.text());

    let fetched = post_action(
        &server,
        &format!("/entries/{entry_id}/fetch-full-content"),
        &csrf,
    )
    .await;
    let html = follow(&server, &fetched).await;

    assert!(
        html.contains("needs JavaScript"),
        "the reader must be told why nothing changed:\n{html}"
    );
    assert!(
        !html.contains("Failed to fetch full content"),
        "no request should have been made to the source at all"
    );
    assert!(
        !html.contains("Fetched full content."),
        "and success must not be claimed for content that is not shown"
    );
}

/// `<rdrs-sidebar>` renders nothing until its script mounts, so with scripting
/// off there was no navigation at all — whatever page you landed on was where
/// you stayed. Every logged-in page now ships a `<noscript>` nav.
#[tokio::test]
async fn every_logged_in_page_offers_navigation_without_javascript() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (feed_id, entry_id) = seed_entry(&db).await;

    for path in [
        "/".to_string(),
        "/entries".to_string(),
        "/entries/starred".to_string(),
        "/categories".to_string(),
        "/feeds".to_string(),
        "/statistics".to_string(),
        "/settings".to_string(),
        format!("/feeds/{feed_id}/entries"),
        format!("/entries?entry={entry_id}"),
    ] {
        let html = server.get(&path).await.text();
        assert!(
            html.contains(r#"<nav class="nav-fallback""#),
            "{path}: no scriptless navigation"
        );
        // The destinations that make the app navigable at all: the lists, and
        // the two index pages standing in for the category / feed tree.
        for target in [
            r#"href="/entries""#,
            r#"href="/entries/starred""#,
            r#"href="/categories""#,
            r#"href="/feeds""#,
            r#"href="/settings""#,
        ] {
            assert!(html.contains(target), "{path}: missing {target}");
        }
    }
}

/// The fallback lives inside `<noscript>`, which a browser with scripting on
/// does not parse into elements — that is what keeps it off the CSR path
/// entirely, including the `:has(.nav-fallback)` rule that reclaims the
/// sidebar's reserved column.
#[tokio::test]
async fn the_navigation_fallback_stays_inside_noscript() {
    let (server, _db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;

    let html = server.get("/").await.text();
    let opened = html
        .find("<noscript>")
        .expect("the fallback is in a noscript");
    let closed = html[opened..]
        .find("</noscript>")
        .expect("that noscript closes")
        + opened;
    let nav = html
        .find(r#"<nav class="nav-fallback""#)
        .expect("nav present");
    assert!(
        nav > opened && nav < closed,
        "the nav must sit inside the noscript, or a scripted browser renders it too"
    );

    // The real sidebar is still the CSR element plus its JSON bootstrap; this
    // must not have turned into server-rendered sidebar markup, which was
    // measured and rejected (see rdrs-sidebar.js).
    assert!(html.contains("<rdrs-sidebar"), "the CSR sidebar stays");
    assert!(
        html.contains(r#"id="rdrs-sidebar-bootstrap""#),
        "and so does its bootstrap payload"
    );
    assert!(
        !html.contains(r#"<div class="sidebar""#),
        "the sidebar's own markup must not be server-rendered"
    );
}

/// The unread total rides along, because it is already loaded for the bootstrap
/// and is the one count worth a scriptless reader's bytes. Per-category counts
/// are deliberately absent.
#[tokio::test]
async fn the_navigation_fallback_carries_the_unread_total() {
    let (server, db) = create_test_server_with_db(default_test_config()).await;
    setup_and_login(&server).await;
    let (_feed_id, entry_id) = seed_entry(&db).await;

    let html = server.get("/").await.text();
    assert!(
        html.contains(r#"<span class="nav-fallback-count">1</span>"#),
        "one unread entry must show as a count:\n{html}"
    );

    // Read it through the app, not by writing to the DB: a direct write leaves
    // the sidebar cache holding the old total, which is precisely the staleness
    // this count must never show.
    let csrf = csrf_field(&html);
    post_action(&server, &format!("/entries/{entry_id}/read"), &csrf)
        .await
        .assert_status(StatusCode::SEE_OTHER);

    // The count goes away rather than showing a zero.
    let html = server.get("/").await.text();
    assert!(
        !html.contains("nav-fallback-count"),
        "an empty inbox shows no badge at all:\n{html}"
    );
}

#[tokio::test]
async fn entry_list_rows_render_the_token() {
    let server = create_test_server(default_test_config()).await;
    setup_and_login(&server).await;

    // The row star / mark-read forms come from `macros.html`, shared with the
    // swap fragments — a regression there silently 403s every row action.
    let page = server.get("/feeds").await;
    page.assert_status_ok();
    let html = page.text();
    assert!(
        html.matches(r#"name="_csrf""#).count() >= 1,
        "the feeds page renders forms and must carry the token:\n{html}"
    );
}
