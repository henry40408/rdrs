//! Additional tests for page handlers and edge cases
//!
//! This test file covers additional scenarios for:
//! - Page templates rendering
//! - Masquerading behavior in pages
//! - Flash message handling

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum::http::{StatusCode, header};
use axum_test::TestServer;
use chrono::TimeZone;
use rdrs::{AppState, Config, Db, Role, auth, create_router, services};
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: Db,
}

async fn create_test_app_named(config: Config, _name: &str) -> TestApp {
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

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db }
}

async fn create_test_app(config: Config) -> TestApp {
    create_test_app_named(config, "test_pages").await
}

/// Setup admin and regular user
async fn setup_users(db: &Db) -> (i64, i64) {
    let password_hash = rdrs::auth::hash_password("vulture-mango-77-quilt").unwrap();
    let admin = rdrs::models::user::create_user(db, "admin", &password_hash, Role::Admin)
        .await
        .unwrap();

    let password_hash = rdrs::auth::hash_password("vulture-mango-77-quilt").unwrap();
    let user = rdrs::models::user::create_user(db, "user", &password_hash, Role::User)
        .await
        .unwrap();

    (admin.id, user.id)
}

async fn login(server: &mut TestServer, username: &str) {
    let login = server
        .post("/api/session")
        .json(&json!({
            "username": username,
            "password": "vulture-mango-77-quilt"
        }))
        .await;
    login.assert_status_ok();
    common::apply_csrf(server, &login);
}

// ============================================================================
// Page Rendering Tests
// ============================================================================

#[tokio::test]
async fn test_unread_page_renders_ssr_layout() {
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, _) = setup_users(&app.db).await;

    // Give admin a feed (no entries) so the empty unread list is the genuine
    // "all caught up" case rather than the no-feeds onboarding case.
    let cat = rdrs::models::category::create_category(&app.db, admin_id, "Tech")
        .await
        .unwrap();
    rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://example.com/feed.xml",
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

    login(&mut app.server, "admin").await;

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
    // Empty list views render the quiet in-context empty-state (heading +
    // detail), not the old plain muted line nor the loud Tier-1 banner.
    assert!(body.contains("class=\"empty-state-quiet\""));
    assert!(body.contains("class=\"empty-state-quiet-title\""));
    assert!(body.contains("All caught up"));
}

#[tokio::test]
async fn test_unread_page_shows_onboarding_when_no_feeds() {
    // A brand-new account with no feeds gets the getting-started guide, not the
    // misleading "All caught up" empty state.
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    login(&mut app.server, "admin").await;

    let response = app.server.get("/").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("data-testid=\"onboarding-guide\""));
    assert!(body.contains("Add your first feed"));
    assert!(body.contains("Import OPML"));
    assert!(!body.contains("All caught up"));
}

#[tokio::test]
async fn test_unread_page_while_masquerading() {
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, user_id) = setup_users(&app.db).await;

    login(&mut app.server, "admin").await;

    // Start masquerading as user via the SSR form endpoint.
    app.server
        .post(&format!("/admin/users/{user_id}/masquerade"))
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
async fn seed_one_entry(db: &Db, username: &str, slug: &str) -> i64 {
    let user = rdrs::models::user::find_by_username(db, username)
        .await
        .unwrap()
        .unwrap();
    let cat = rdrs::models::category::create_category(db, user.id, &format!("cat-{slug}"))
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: &format!("https://example.com/{slug}.xml"),
            title: Some(&format!("Feed {slug}")),
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
        &format!("guid-{slug}"),
        Some(&format!("Title for {slug}")),
        Some(&format!("https://example.com/{slug}")),
        Some(&format!("<p>Body for {slug}.</p>")),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    entry.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_page_entry_query_populates_reading_pane() {
    let mut app = create_test_app_named(default_test_config(), "test_unread_entry_query_ok").await;
    setup_users(&app.db).await;
    let entry_id = seed_one_entry(&app.db, "admin", "deep-link-ok").await;
    login(&mut app.server, "admin").await;

    let response = app.server.get(&format!("/?entry={entry_id}")).await;
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
    // The seeded entry has no summary, so its list-row status cluster must
    // render as a truly empty span (no whitespace text nodes) — otherwise the
    // `.entry-status:empty { display: none }` rule never matches. Regression
    // guard for the Askama whitespace-trim fix in _entry_row.html.
    assert!(
        body.contains(r#"<span class="entry-status"></span>"#),
        "unsummarized entry row must render an empty .entry-status span; body was: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_unread_page_entry_query_invalid_id_falls_back_to_empty_pane() {
    let mut app =
        create_test_app_named(default_test_config(), "test_unread_entry_query_invalid").await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app =
        create_test_app_named(default_test_config(), "test_unread_entry_query_other_user").await;
    setup_users(&app.db).await; // creates `admin` and `user`
    // Entry belongs to `user`; we log in as `admin`.
    let entry_id = seed_one_entry(&app.db, "user", "cross-user").await;
    login(&mut app.server, "admin").await;

    let response = app.server.get(&format!("/?entry={entry_id}")).await;
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
    let mut app = create_test_app_named(default_test_config(), "test_starred_entry_query_ok").await;
    setup_users(&app.db).await;
    let entry_id = seed_one_entry(&app.db, "admin", "starred-deep-link").await;
    login(&mut app.server, "admin").await;

    let response = app
        .server
        .get(&format!("/entries/starred?entry={entry_id}"))
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
    let mut app = create_test_app(default_test_config()).await;
    let (_admin_id, user_id) = setup_users(&app.db).await;

    login(&mut app.server, "admin").await;

    // Start masquerading via the SSR form endpoint.
    app.server
        .post(&format!("/admin/users/{user_id}/masquerade"))
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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Old CSR markers gone.
    assert!(!body.contains("<rdrs-user-settings-page>"));
    assert!(!body.contains("/static/js/pages/user-settings.js"));

    // SSR content present.
    assert!(body.contains("<h1>Settings</h1>"));
    assert!(body.contains("Account Information"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/password\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/preferences\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/linkding\">"));
    assert!(body.contains("<form method=\"post\" action=\"/user-settings/kagi\">"));
    assert!(body.contains("<rdrs-passkeys>"));
    assert!(body.contains("/static/js/passkey.js"));

    // Sidebar display preferences, defaulting to the pre-existing behaviour.
    assert!(body.contains(r#"name="sidebar_sort""#));
    assert!(body.contains(r#"<option value="name" selected>"#));
    assert!(body.contains(r#"name="sidebar_hide_read""#));
    assert!(
        !body.contains(r#"name="sidebar_hide_read" value="1" checked"#),
        "hide-read must be off by default"
    );
}

#[tokio::test]
async fn test_user_settings_lists_api_tokens() {
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    rdrs::models::api_token::create_api_token(
        &app.db,
        admin_id,
        "greader",
        "FeedMe/1.0",
        "FeedMe/1.0",
        "203.0.113.5",
    )
    .await
    .unwrap();

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("GReader API Tokens"));
    assert!(body.contains("FeedMe/1.0"));
    assert!(body.contains("203.0.113.5"));
    assert!(body.contains("/user-settings/api-tokens/revoke-all"));
}

#[tokio::test]
async fn test_api_token_revoke_is_user_scoped() {
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, _user_id) = setup_users(&app.db).await;

    let token = rdrs::models::api_token::create_api_token(
        &app.db,
        admin_id,
        "greader",
        "admin-client",
        "test-agent",
        "127.0.0.1",
    )
    .await
    .unwrap();

    // User B (the regular user) tries to revoke admin's token by guessing its id.
    login(&mut app.server, "user").await;
    let response = app
        .server
        .post(&format!("/user-settings/api-tokens/{}/revoke", token.id))
        .await;
    response.assert_status(StatusCode::SEE_OTHER);

    let found = rdrs::models::api_token::find_by_token(&app.db, &token.token)
        .await
        .unwrap();
    assert!(
        found.is_some(),
        "user B must not be able to revoke user A's (admin's) API token"
    );
}

#[tokio::test]
async fn test_favicon_links_carry_the_build_stamp() {
    // Without ?v=, the long-lived header on the icons would have no URL left to
    // change across an upgrade — the same trap the ES-module imports hit. This
    // asserts the template side of the pair; the handler side (version-gated
    // Cache-Control) is covered in handlers_test.
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let body = app.server.get("/user-settings").await.text();

    for icon in ["/favicon.ico", "/favicon.svg", "/apple-touch-icon.png"] {
        assert!(
            body.contains(&format!("{icon}?v={}", rdrs::GIT_VERSION)),
            "{icon} must be version-stamped"
        );
    }
}

#[tokio::test]
async fn test_session_revoke_is_user_scoped() {
    // Sibling of `test_api_token_revoke_is_user_scoped`: the session id now
    // travels to the browser so the revoke-one form can name it, which is only
    // safe because `delete_user_session_by_id` re-checks ownership.
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, _user_id) = setup_users(&app.db).await;

    let victim = rdrs::models::session::create_session(&app.db, admin_id, "test-agent", "10.0.0.1")
        .await
        .unwrap();

    // User B (the regular user) tries to revoke admin's session by its id.
    login(&mut app.server, "user").await;
    let response = app
        .server
        .post(&format!("/user-settings/sessions/{}/revoke", victim.id))
        .await;
    response.assert_status(StatusCode::SEE_OTHER);

    let found = rdrs::models::session::find_by_token(&app.db, &victim.session_token)
        .await
        .unwrap();
    assert!(
        found.is_some(),
        "user B must not be able to revoke user A's (admin's) session"
    );
}

#[tokio::test]
async fn test_user_settings_renders_session_cards() {
    let mut app = create_test_app(default_test_config()).await;
    let (admin_id, _user_id) = setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let other = rdrs::models::session::create_session(
        &app.db,
        admin_id,
        "Mozilla/5.0 (X11; Linux x86_64) OtherDevice/1.0",
        "198.51.100.7",
    )
    .await
    .unwrap();

    let response = app.server.get("/user-settings").await;
    response.assert_status_ok();
    let body = response.text();

    // The full User-Agent is rendered, not a truncated or tooltip-only copy.
    assert!(body.contains("Mozilla/5.0 (X11; Linux x86_64) OtherDevice/1.0"));
    assert!(body.contains("198.51.100.7"));
    // Every non-current session gets its own revoke form...
    assert!(body.contains(&format!(
        "action=\"/user-settings/sessions/{}/revoke\"",
        other.id
    )));
    // ...and the session doing the asking gets the note instead of a button.
    assert!(body.contains("This device"));
}

#[tokio::test]
async fn test_settings_page_renders_ssr_content() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // SSR content — no more <rdrs-settings-page> element / page-script.
    assert!(!body.contains("<rdrs-settings-page>"));
    assert!(!body.contains("/static/js/pages/settings.js"));

    // Server-rendered content from default config: a single Configuration
    // table listing each env var with its description, default, and current
    // value (Configuration + Environment Variables sections are merged).
    assert!(body.contains("<h1>App</h1>"));
    assert!(body.contains("Configuration"));
    assert!(body.contains("DATABASE_URL"));
    assert!(body.contains("RDRS_USER_AGENT"));
    assert!(body.contains("RDRS_MULTI_USER_ENABLED"));
    assert!(body.contains("RDRS_SECRET"));
    // The "Current" column header surfaces the running instance's values.
    assert!(body.contains("Current"));
    // The old standalone "Environment Variables" sub-heading is gone.
    assert!(!body.contains("Environment Variables"));
}

#[tokio::test]
async fn test_settings_page_reflects_custom_config() {
    let config = Config {
        database_url: "/data/custom.sqlite3".to_string(),
        server_bind: "0.0.0.0:9090".parse().unwrap(),
        user_agent: "Custom-Agent/2.0".to_string(),
        multi_user_enabled: true,
        secret_generated: false,
        ..default_test_config()
    };
    let mut app = create_test_app(config).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("Custom-Agent/2.0"));
    assert!(body.contains("(custom)"));
    // Server section reflects the actual runtime database path and port.
    assert!(body.contains("/data/custom.sqlite3"));
    assert!(body.contains("9090"));
    // Image proxy secret is configured (not auto-generated).
    assert!(body.contains("Configured"));
    assert!(!body.contains(">Auto-generated<"));
    // The multi-user flag renders as a Yes badge. (There used to be two:
    // RDRS_SIGNUP_ENABLED was retired along with self-service registration.)
    let yes_count = body
        .matches("<span class=\"success-text\">Yes</span>")
        .count();
    assert!(yes_count >= 1, "expected a Yes badge, got {yes_count}");
}

#[tokio::test]
async fn test_settings_page_reflects_auto_generated_image_proxy_secret() {
    let config = Config {
        secret_generated: true,
        ..default_test_config()
    };
    let mut app = create_test_app(config).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    // Image proxy secret status reflects the auto-generated runtime state.
    assert!(body.contains("Auto-generated"));
    assert!(!body.contains(">Configured<"));
}

#[tokio::test]
async fn test_settings_page_redacts_database_password() {
    let config = Config {
        database_url: "postgres://rdrs:sup3rs3cret@db.internal:5432/rdrs".to_string(),
        ..default_test_config()
    };
    let mut app = create_test_app(config).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/settings").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(
        !body.contains("sup3rs3cret"),
        "database password leaked into /settings"
    );
    assert!(body.contains("postgres://rdrs:***@db.internal:5432/rdrs"));
}

#[tokio::test]
async fn test_settings_page_forbidden_for_non_admin() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "user").await;

    // Non-admins are bounced to the login page rather than shown deployment
    // internals (database target, bind address, forward-auth headers).
    let response = app.server.get("/settings").await;
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_login_page_hides_signup_when_disabled() {
    let config = Config {
        ..default_test_config()
    };
    let app = create_test_app(config).await;

    let response = app.server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();

    // Register link should not be present or should be hidden
    // This depends on template logic
    assert!(body.contains("Login"));
}

#[tokio::test]
async fn setup_page_redirects_once_the_instance_has_an_account() {
    let config = Config {
        ..default_test_config()
    };
    let app = create_test_app(config).await;

    // With an empty database /setup renders the bootstrap form, so accounts
    // have to exist before this asserts anything.
    setup_users(&app.db).await;

    let response = app.server.get("/setup").await;

    // A redirect, not a disabled form: the page has exactly one purpose and it
    // is spent. Nothing about the accounts that exist is disclosed.
    response.assert_status(StatusCode::SEE_OTHER);
    assert_eq!(response.header(header::LOCATION), "/login");
}

#[tokio::test]
async fn test_categories_page_with_flash() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    // Pending flash messages are rendered as banners inside `<rdrs-flash>`,
    // so they are visible on first paint with or without JavaScript.
    assert!(body.contains(r#"data-testid="flash-message""#));
    assert!(body.contains("Category created successfully"));
}

#[tokio::test]
async fn test_feeds_page_with_flash() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    // SSR page still renders pending flash messages inline.
    assert!(body.contains(r#"data-testid="flash-message""#));
    assert!(body.contains("Failed to add feed"));
}

#[tokio::test]
async fn test_entries_page_with_flash() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    // Post-SSR migration: flash is rendered inline, no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains(r#"data-testid="flash-message""#));
    assert!(body.contains("Entries refreshed"));
}

#[tokio::test]
async fn test_entries_page_renders_ssr_layout() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    assert!(body.contains(r#"data-testid="flash-message""#));
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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    login(&mut app.server, "user").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    login(&mut app.server, "user").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    let config = serde_json::json!({
        "linkding": {
            "api_url": "https://linkding.example.com",
            "api_token": "secret"
        }
    });
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO user_settings (user_id, save_services) VALUES ($1, $2)",
        1_i64,
        config.to_string()
    )
    .unwrap();

    login(&mut app.server, "admin").await;

    let response = app.server.get("/api/user-settings").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["linkding_configured"], true);
    assert_eq!(body["linkding_api_url"], "https://linkding.example.com");
}

#[tokio::test]
async fn test_api_user_settings_returns_custom_entries_per_page() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO user_settings (user_id, entries_per_page) VALUES ($1, $2)",
        1_i64,
        100_i64
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/entries/read").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains("Read Entries") || body.contains("read"));
}

#[tokio::test]
async fn test_starred_entries_page() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/entries/starred").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains("Starred Entries") || body.contains("starred"));
}

#[tokio::test]
async fn test_summarized_entries_page() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/entries/summarized").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR layout: no CSR shell.
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(body.contains("Summarized Entries") || body.contains("summarized"));
}

#[tokio::test]
async fn test_search_page() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/search").await;
    response.assert_status_ok();
    let body = response.text();
    // SSR page: form + empty-state hint, no CSR shell.
    assert!(body.contains("<h1>Search</h1>"));
    assert!(body.contains("<form method=\"get\" action=\"/search\""));
    assert!(body.contains("data-testid=\"search-input\""));
    assert!(body.contains("Search your library"));
    assert!(body.contains("class=\"empty-state\""));
    // Old editorial empty-state class fully retired in favor of `.empty-state`.
    assert!(!body.contains("search-status"));
    assert!(!body.contains("<rdrs-entries-page>"));
    assert!(!body.contains("/static/js/pages/entries.js"));
}

#[tokio::test]
async fn test_search_page_with_results() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Search Cat"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        1_i64,
        "https://example.com/search-feed.xml",
        "Search Feed"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO entry (feed_id, guid, title, link, content) VALUES ($1, $2, $3, $4, $5)",
        1_i64,
        "search-guid-1",
        "Quokka Discovery in Western Australia",
        "https://example.com/quokka",
        "<p>The quokka is a small marsupial.</p>"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO entry (feed_id, guid, title, link, content) VALUES ($1, $2, $3, $4, $5)",
        1_i64,
        "search-guid-2",
        "Other Topic",
        "https://example.com/other",
        "<p>Unrelated article.</p>"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/search?q=zzznotfoundzzz").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Nothing matched"));
    assert!(body.contains("zzznotfoundzzz"));
    // Tier-1 empty-state with the no-results heading, behind the stable testid.
    assert!(body.contains("No matches"));
    assert!(body.contains("data-testid=\"search-empty\""));
}

#[tokio::test]
async fn test_search_page_invalid_query_shows_error_no_results() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    // "(rust OR" — unbalanced parenthesis, url-encoded.
    let response = app.server.get("/search?q=%28rust%20OR").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Search syntax error"));
    assert!(body.contains("data-testid=\"search-error\""));
    assert!(!body.contains("data-testid=\"search-results\""));
}

#[tokio::test]
async fn test_search_page_valid_structured_query_renders_without_error() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/search?q=is%3Aunread").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("Search syntax error"));
    assert!(!body.contains("data-testid=\"search-error\""));
}

#[tokio::test]
async fn test_search_page_has_syntax_help_panel() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/search").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("class=\"search-syntax-help\""));
    assert!(body.contains("Search syntax"));
    assert!(body.contains("is:unread"));
}

// ============================================================================
// Category Entries Page Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page() {
    let mut app = create_test_app_named(default_test_config(), "test_category_entries_page").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_ce", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_ce", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "Engineering")
        .await
        .unwrap();
    let feed1 = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let feed2 = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (a, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed1.id,
        "guid-ce-a",
        Some("Entry A"),
        Some("https://x/ce/a"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (b, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed2.id,
        "guid-ce-b",
        Some("Entry B"),
        Some("https://x/ce/b"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (cat_id, entry_a_id, entry_b_id) = (cat.id, a.id, b.id);

    let resp = app
        .server
        .get(&format!("/categories/{cat_id}/entries"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();

    assert!(
        html.contains("Engineering"),
        "page title must render the category name"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{entry_a_id}\"")),
        "row for entry from feed 1 must be present"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{entry_b_id}\"")),
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
        html.contains("Select an entry from the list to start reading."),
        "reading-pane placeholder must render"
    );
    if html.contains("id=\"load-more\"") {
        assert!(
            html.contains(&format!("action=\"/categories/{cat_id}/entries\"")),
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
        html.contains(&format!("active-category-id=\"{cat_id}\"")),
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
    let mut app = create_test_app_named(
        default_test_config(),
        "test_category_entries_page_not_found",
    )
    .await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_cnf", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_cnf", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

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
    let mut app = create_test_app_named(
        default_test_config(),
        "test_category_entries_page_other_user",
    )
    .await;

    // Register alice (owner of the category)
    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_cou", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Register bob (the cross-tenant user)
    common::seed_account(
        &app.db,
        "bob_cou",
        "vulture-mango-77-quilt",
        rdrs::Role::User,
    )
    .await;

    // Get alice's user_id and create her category
    let alice_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_cou'"
    )
    .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, alice_id, "Alice Cat")
        .await
        .unwrap();
    let cat_id: i64 = cat.id;

    // Log in as bob
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "bob_cou", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Bob tries to access alice's category entries — must be 404
    let resp = app
        .server
        .get(&format!("/categories/{cat_id}/entries"))
        .await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user category entries must return 404"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page_load_more_fragment() {
    let mut app =
        create_test_app_named(default_test_config(), "test_category_entries_page_lm").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_cl", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_cl", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "LMCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    for i in 0..3 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-clm-{i}"),
            Some(&format!("Entry {i}")),
            Some(&format!("https://x/clm/{i}")),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }
    let cat_id: i64 = cat.id;

    let resp = app
        .server
        .get(&format!("/categories/{cat_id}/entries?fragment=1"))
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

/// `GET /categories/{id}/entries?pane=1` — the category-switch fragment
/// `app.js` swaps in place of a document reload. It carries the whole left
/// column (header included, unlike the Load-More / search fragments) and an
/// emptied reading pane, and it stays empty even when `?entry=` is present:
/// switching category closes the entry that belonged to the previous one.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_page_pane_fragment() {
    let mut app =
        create_test_app_named(default_test_config(), "test_category_entries_page_pane").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_cp", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_cp", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "PaneCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/cpane-feed",
            title: Some("CPane Feed"),
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
        &app.db,
        feed.id,
        "guid-cpane-0",
        Some("Pane Entry"),
        Some("https://x/cpane/0"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let cat_id: i64 = cat.id;
    let entry_id: i64 = entry.id;

    let resp = app
        .server
        .get(&format!(
            "/categories/{cat_id}/entries?pane=1&entry={entry_id}"
        ))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains(r#"data-swap-target="[data-list-pane]""#)
            && html.contains(r##"data-swap-target="#reading-pane""##),
        "pane fragment must target both the list pane and the reading pane"
    );
    assert!(
        html.contains("<h1>PaneCat</h1>"),
        "pane fragment must carry the list-pane header"
    );
    assert!(
        html.contains("data-entry-row"),
        "pane fragment must include row markup"
    );
    assert!(
        !html.contains("<rdrs-sidebar"),
        "pane fragment must NOT include layout chrome"
    );
    assert!(
        html.contains("reading-pane-empty") && !html.contains("reading-pane-article"),
        "pane fragment must close the open entry even when ?entry= is present"
    );
}

/// `GET /feeds/{id}/entries?pane=1` — the same in-place navigation contract as
/// the category page, since the sidebar now lists feeds and clicking one must
/// not reload the document either.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_pane_fragment() {
    let mut app = create_test_app_named(default_test_config(), "test_feed_entries_page_pane").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fp", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fp", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "FPCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/fp-feed",
            title: Some("FP Feed"),
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
        &app.db,
        feed.id,
        "guid-fp-0",
        Some("FP Entry"),
        Some("https://x/fp/0"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (feed_id, entry_id) = (feed.id, entry.id);

    let resp = app
        .server
        .get(&format!("/feeds/{feed_id}/entries?pane=1&entry={entry_id}"))
        .await;
    resp.assert_status_ok();
    let html = resp.text();
    assert!(
        html.contains(r#"data-swap-target="[data-list-pane]""#)
            && html.contains(r##"data-swap-target="#reading-pane""##),
        "pane fragment must target both the list pane and the reading pane"
    );
    assert!(
        html.contains("<h1>") && html.contains("FP Feed"),
        "pane fragment must carry the list-pane header"
    );
    assert!(
        !html.contains("<rdrs-sidebar"),
        "pane fragment must NOT include layout chrome"
    );
    assert!(
        html.contains("reading-pane-empty") && !html.contains("reading-pane-article"),
        "pane fragment must close the open entry even when ?entry= is present"
    );
}

/// A feed page names its feed to the sidebar so the feed is highlighted inside
/// its category's expanded list — and names the parent category, which is what
/// makes that list expand at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_marks_active_feed() {
    let mut app = create_test_app_named(default_test_config(), "test_feed_entries_active").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_af", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_af", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "AFCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/af-feed",
            title: Some("AF Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (cat_id, feed_id) = (cat.id, feed.id);

    let html = app
        .server
        .get(&format!("/feeds/{feed_id}/entries"))
        .await
        .text();
    assert!(
        html.contains(&format!(r#"active-feed-id="{feed_id}""#)),
        "feed page must mark its feed active in the sidebar"
    );
    assert!(
        html.contains(&format!(r#"active-category-id="{cat_id}""#)),
        "feed page must keep its parent category active so the feed list expands"
    );

    let category_html = app
        .server
        .get(&format!("/categories/{cat_id}/entries"))
        .await
        .text();
    assert!(
        !category_html.contains("active-feed-id"),
        "a category page has no active feed"
    );
}

/// `POST /categories/{id}/entries/mark-read` marks only the entries matching
/// the scoped-search `q` as read, leaving non-matching entries untouched, and
/// redirects back to the category page preserving `?q=`.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_mark_read_scoped_search() {
    let mut app =
        create_test_app_named(default_test_config(), "test_category_mark_read_scoped").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_mr", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_mr", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "MarkReadCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/mr-feed",
            title: Some("MR Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (matching, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-mr-match",
        Some("Widget Roundup"),
        Some("https://x/mr/match"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (other, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-mr-other",
        Some("Something Else"),
        Some("https://x/mr/other"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (cat_id, matching_id, other_id) = (cat.id, matching.id, other.id);

    let response = app
        .server
        .post(&format!("/categories/{cat_id}/entries/mark-read"))
        .form(&[("q", "Widget")])
        .await;

    response.assert_status_see_other();
    let location = response.header(header::LOCATION);
    assert_eq!(
        location,
        format!("/categories/{cat_id}/entries?q=Widget"),
        "redirect must preserve the ?q= scoped-search keyword"
    );

    let matching_read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        matching_id
    )
    .unwrap();
    let other_read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        other_id
    )
    .unwrap();

    assert!(
        matching_read_at.is_some(),
        "entry matching the scoped search must be marked read"
    );
    assert!(
        other_read_at.is_none(),
        "entry not matching the scoped search must remain unread"
    );
}

/// `GET /api/sidebar/categories/{id}/feeds` — the lazily-loaded feed list the
/// sidebar shows under the open category, with per-feed unread counts. A
/// category belonging to another account must 404 rather than leak its feeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_sidebar_category_feeds_endpoint() {
    let mut app = create_test_app_named(default_test_config(), "test_sidebar_category_feeds").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_sf", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_sf", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "SFCat")
        .await
        .unwrap();
    let other_cat = rdrs::models::category::create_category(&app.db, user_id, "SFOther")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/sf-feed",
            title: Some("SF Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let other_feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: other_cat.id,
            url: "https://x/sf-other-feed",
            title: Some("SF Other Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    for i in 0..2 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-sf-{i}"),
            Some(&format!("SF Entry {i}")),
            Some(&format!("https://x/sf/{i}")),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }
    rdrs::models::entry::upsert_entry(
        &app.db,
        other_feed.id,
        "guid-sf-other",
        Some("Other Entry"),
        Some("https://x/sf/other"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (cat_id, feed_id) = (cat.id, feed.id);

    let resp = app
        .server
        .get(&format!("/api/sidebar/categories/{cat_id}/feeds"))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["category_id"], cat_id);
    let feeds = body["feeds"].as_array().unwrap();
    assert_eq!(
        feeds.len(),
        1,
        "only the requested category's feeds may be listed"
    );
    assert_eq!(feeds[0]["id"], feed_id);
    assert_eq!(feeds[0]["title"], "SF Feed");
    assert_eq!(
        feeds[0]["unread_count"], 2,
        "unread counts must be scoped to the feed"
    );
    assert_eq!(
        feeds[0]["has_icon"], false,
        "a feed with no stored icon must say so, or the sidebar renders a broken image"
    );

    rdrs::models::image::upsert(
        &app.db,
        rdrs::models::image::ENTITY_FEED,
        feed_id,
        &[1, 2, 3],
        "image/png",
        Some("https://x/icon.png"),
    )
    .await
    .unwrap();
    let with_icon: serde_json::Value = app
        .server
        .get(&format!("/api/sidebar/categories/{cat_id}/feeds"))
        .await
        .json();
    assert_eq!(
        with_icon["feeds"][0]["has_icon"], true,
        "a stored icon must be reported so the sidebar renders it"
    );

    // Another account's category is not readable.
    app.server
        .post("/api/session")
        .json(&json!({ "username": "alice_sf", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status_ok();
    let missing = app.server.get("/api/sidebar/categories/999999/feeds").await;
    assert_eq!(
        missing.status_code(),
        StatusCode::NOT_FOUND,
        "a category the caller doesn't own must 404"
    );
}

/// "Mark Above as Read" is dropped from a scoped-search render. It marks the
/// rows in the DOM, which under a search means the matches — one meaning too
/// close to the "Mark N matching as Read" button rendered right above it for
/// two controls to be safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_entries_hides_mark_above_while_searching() {
    let mut app =
        create_test_app_named(default_test_config(), "test_category_mark_above_search").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_ma", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_ma", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "MACat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/ma-feed",
            title: Some("MA Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-ma-0",
        Some("Widget News"),
        Some("https://x/ma/0"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let cat_id: i64 = cat.id;

    let plain = app
        .server
        .get(&format!("/categories/{cat_id}/entries"))
        .await;
    assert!(
        plain.text().contains("mark-above-read"),
        "unfiltered category page must still offer Mark Above as Read"
    );

    let searched = app
        .server
        .get(&format!("/categories/{cat_id}/entries?q=Widget"))
        .await;
    let html = searched.text();
    assert!(
        html.contains("data-entry-row"),
        "the scoped search must still return its match"
    );
    assert!(
        !html.contains("mark-above-read"),
        "Mark Above as Read must be hidden while a scoped search is active"
    );
}

/// `POST /feeds/{id}/entries/mark-read` — same as
/// `test_category_mark_read_scoped_search` but scoped to a feed, guarding
/// against a copy-paste argument-order bug in `feed_mark_read_form` /
/// `category_mark_read_form` (they must pass `Some(id)/None` vs
/// `None/Some(id)` correctly into the shared `mark_read_scoped` helper).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_mark_read_scoped_search() {
    let mut app = create_test_app_named(default_test_config(), "test_feed_mark_read_scoped").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fmr", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fmr", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "FeedMarkReadCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/fmr-feed",
            title: Some("FMR Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (matching, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fmr-match",
        Some("Widget Roundup"),
        Some("https://x/fmr/match"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (other, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fmr-other",
        Some("Something Else"),
        Some("https://x/fmr/other"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (feed_id, matching_id, other_id) = (feed.id, matching.id, other.id);

    let response = app
        .server
        .post(&format!("/feeds/{feed_id}/entries/mark-read"))
        .form(&[("q", "Widget")])
        .await;

    response.assert_status_see_other();
    let location = response.header(header::LOCATION);
    assert_eq!(
        location,
        format!("/feeds/{feed_id}/entries?q=Widget"),
        "redirect must preserve the ?q= scoped-search keyword"
    );

    let matching_read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        matching_id
    )
    .unwrap();
    let other_read_at: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        other_id
    )
    .unwrap();

    assert!(
        matching_read_at.is_some(),
        "entry matching the scoped search must be marked read"
    );
    assert!(
        other_read_at.is_none(),
        "entry not matching the scoped search must remain unread"
    );
}

/// On the `?status=all` tab, `matching_count` must equal the number of
/// entries `mark_read_by_filter` will actually mark (unread + matching
/// search), not the number of entries matching the active tab's filter.
/// With one matching-read and one matching-unread entry, the rendered
/// "Mark N matching" count must be 1, and posting the mark-read action must
/// mark exactly that one entry (leaving the already-read match untouched).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_category_matching_count_reflects_unread_only_on_all_tab() {
    let mut app = create_test_app_named(
        default_test_config(),
        "test_category_matching_count_all_tab",
    )
    .await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_mc", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_mc", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "MatchCountCat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/mc-feed",
            title: Some("MC Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    let (matching_unread, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-mc-unread",
        Some("Widget Roundup Unread"),
        Some("https://x/mc/unread"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (matching_read, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-mc-read",
        Some("Widget Roundup Read"),
        Some("https://x/mc/read"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::mark_as_read(&app.db, matching_read.id)
        .await
        .unwrap();
    let (cat_id, matching_unread_id, matching_read_id) =
        (cat.id, matching_unread.id, matching_read.id);

    let resp = app
        .server
        .get(&format!("/categories/{cat_id}/entries?status=all&q=Widget"))
        .await;
    resp.assert_status_ok();
    let html = resp.text();
    assert!(
        html.contains("Mark 1 matching as Read"),
        "count must reflect only the unread match (mark_read_by_filter only \
         touches read_at IS NULL rows), not both matches on the All tab: {html}"
    );

    let response = app
        .server
        .post(&format!("/categories/{cat_id}/entries/mark-read"))
        .form(&[("q", "Widget"), ("status", "all")])
        .await;
    response.assert_status_see_other();

    let unread_now: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        matching_unread_id
    )
    .unwrap();
    let read_now: Option<String> = rdrs::query_scalar!(
        &app.db,
        Option<String>,
        "SELECT read_at FROM entry WHERE id = $1",
        matching_read_id
    )
    .unwrap();

    assert!(
        unread_now.is_some(),
        "the previously-unread match must now be marked read"
    );
    assert!(
        read_now.is_some(),
        "the already-read match must remain marked read (untouched, not un-marked)"
    );
}

// ============================================================================
// Feed Entries Page Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page() {
    let mut app = create_test_app_named(default_test_config(), "test_feed_entries_page").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fe", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fe", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (a, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fe-a",
        Some("First Entry"),
        Some("https://x/a"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (b, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fe-b",
        Some("Second Entry"),
        Some("https://x/b"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (cat_id, feed_id, entry_a_id, entry_b_id) = (cat.id, feed.id, a.id, b.id);

    let resp = app.server.get(&format!("/feeds/{feed_id}/entries")).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();

    assert!(
        html.contains("FE Feed"),
        "page title must render feed title"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{entry_a_id}\"")),
        "row for first entry must be in the HTML"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{entry_b_id}\"")),
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
        html.contains("Select an entry from the list to start reading."),
        "reading-pane placeholder must render when no entry is selected"
    );
    if html.contains("id=\"load-more\"") {
        assert!(
            html.contains(&format!("action=\"/feeds/{feed_id}/entries\"")),
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
            r#"<a href="/categories/{cat_id}/entries">Tech</a>"#
        )),
        "breadcrumb must link to the parent category page"
    );

    // Sidebar must receive active-category-id so the parent category is highlighted.
    assert!(
        html.contains(&format!("active-category-id=\"{cat_id}\"")),
        "<rdrs-sidebar> must carry active-category-id for the feed's parent category"
    );

    // Feed has no icon seeded, so the header image must NOT render. (A
    // parallel test could seed an image row + assert the <img> appears; we
    // skip that variant to keep this test focused on layout-context wiring.)
    assert!(
        !html.contains(&format!(
            "src=\"/api/feeds/{feed_id}/icon\" alt=\"\" width=\"20\""
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
    let mut app =
        create_test_app_named(default_test_config(), "test_feed_entries_page_status").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fst", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fst", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "FST")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (u, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fst-u",
        Some("Unread Entry"),
        Some("https://x/fst/u"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (r, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fst-r",
        Some("Read Entry"),
        Some("https://x/fst/r"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (s, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-fst-s",
        Some("Starred Entry"),
        Some("https://x/fst/s"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::mark_as_read(&app.db, r.id)
        .await
        .unwrap();
    rdrs::models::entry::star_entry(&app.db, s.id)
        .await
        .unwrap();
    let (feed_id, unread_id, read_id, starred_id) = (feed.id, u.id, r.id, s.id);

    // Default URL (no ?status=) → unread is the default; read entry hidden,
    // unread + starred (which is still unread) both visible.
    let resp = app.server.get(&format!("/feeds/{feed_id}/entries")).await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let html = resp.text();
    assert!(
        html.contains(&format!("id=\"entry-row-{unread_id}\"")),
        "default view should include the unread entry"
    );
    assert!(
        !html.contains(&format!("id=\"entry-row-{read_id}\"")),
        "default view should hide the read entry (default = unread filter)"
    );
    assert!(
        html.contains(&format!("id=\"entry-row-{starred_id}\"")),
        "default view should include the starred-but-unread entry"
    );

    // ?status=all → every entry visible.
    let resp = app
        .server
        .get(&format!("/feeds/{feed_id}/entries?status=all"))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{unread_id}\"")));
    assert!(html.contains(&format!("id=\"entry-row-{read_id}\"")));
    assert!(html.contains(&format!("id=\"entry-row-{starred_id}\"")));

    // ?status=read → only read.
    let resp = app
        .server
        .get(&format!("/feeds/{feed_id}/entries?status=read"))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{read_id}\"")));
    assert!(!html.contains(&format!("id=\"entry-row-{unread_id}\"")));

    // ?status=starred → only starred.
    let resp = app
        .server
        .get(&format!("/feeds/{feed_id}/entries?status=starred"))
        .await;
    let html = resp.text();
    assert!(html.contains(&format!("id=\"entry-row-{starred_id}\"")));
    assert!(!html.contains(&format!("id=\"entry-row-{read_id}\"")));
    assert!(!html.contains(&format!("id=\"entry-row-{unread_id}\"")));

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
            r#"<option value="/feeds/{feed_id}/entries?status=starred" selected>Starred</option>"#
        )),
        "Starred <option> must be `selected` when ?status=starred"
    );
    // Option order: All, Unread, Read, Starred (Unread URL = base path).
    let all_pos = html
        .find(&format!(
            r#"<option value="/feeds/{feed_id}/entries?status=all""#
        ))
        .expect("All option present");
    let unread_pos = html
        .find(&format!(r#"<option value="/feeds/{feed_id}/entries""#))
        .expect("Unread option present (base URL)");
    let read_pos = html
        .find(&format!(
            r#"<option value="/feeds/{feed_id}/entries?status=read""#
        ))
        .expect("Read option present");
    let starred_pos = html
        .find(&format!(
            r#"<option value="/feeds/{feed_id}/entries?status=starred""#
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
    let mut app =
        create_test_app_named(default_test_config(), "test_feed_entries_page_not_found").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fnf", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fnf", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

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
    let mut app =
        create_test_app_named(default_test_config(), "test_feed_entries_page_other_user").await;

    // Register alice (owner of the feed)
    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fou", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);

    // Register bob (the cross-tenant user)
    common::seed_account(
        &app.db,
        "bob_fou",
        "vulture-mango-77-quilt",
        rdrs::Role::User,
    )
    .await;

    // Get alice's user_id and create her feed
    let alice_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_fou'"
    )
    .unwrap();
    let cat = rdrs::models::category::create_category(&app.db, alice_id, "Alice Cat")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let feed_id: i64 = feed.id;

    // Log in as bob
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "bob_fou", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Bob tries to access alice's feed entries — must be 404
    let resp = app.server.get(&format!("/feeds/{feed_id}/entries")).await;
    assert_eq!(
        resp.status_code(),
        StatusCode::NOT_FOUND,
        "cross-user feed entries must return 404"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_feed_entries_page_load_more_fragment() {
    let mut app = create_test_app_named(default_test_config(), "test_feed_entries_page_lm").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_fl", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_fl", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "T")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    for i in 0..3 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("guid-lm-{i}"),
            Some(&format!("Entry {i}")),
            Some(&format!("https://x/lm/{i}")),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }
    let feed_id: i64 = feed.id;

    let resp = app
        .server
        .get(&format!("/feeds/{feed_id}/entries?fragment=1"))
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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Feeds SSR Cat"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        1_i64,
        "https://example.com/feeds-ssr.xml",
        "Feeds SSR Title"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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

/// The two timestamps on every feed row are computed from rules a user cannot
/// guess (three date signals collapsed to a max, plus 304s that move only one
/// of them). The page has to state them, and state the same numbers
/// `compute_freshness` applies — hence the thresholds coming from the
/// constants rather than being retyped into the template.
#[tokio::test]
async fn test_feeds_page_explains_freshness_rules() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(
        body.contains("data-testid=\"feed-freshness-help\""),
        "the /feeds page must carry the freshness help disclosure"
    );
    // What "Updated" is derived from, and why a fetch can leave it alone.
    assert!(body.contains("Last-Modified"));
    assert!(body.contains("304 Not Modified"));
    assert!(body.contains("No date info"));
    // `effective_feed_updated_at` maxes over whichever signals exist — it does
    // not walk them in priority order — so the help must not read as a
    // fallback chain. A feed with only a Last-Modified header is judged by it,
    // rather than being called stale for lacking an in-feed date.
    assert!(
        body.contains("They are not ranked: a missing signal is skipped, not treated as old."),
        "help must state that the three Updated signals are maxed, not ranked"
    );
    // The live thresholds, not hand-copied ones.
    assert!(
        body.contains(&format!(
            "up to {} days is normal",
            rdrs::handlers::pages::FRESH_MAX_DAYS
        )),
        "help text must quote the fresh threshold the code uses"
    );
    assert!(
        body.contains(&format!(
            "over {} days",
            rdrs::handlers::pages::WARNING_MAX_DAYS
        )),
        "help text must quote the stale threshold the code uses"
    );
}

#[tokio::test]
async fn test_feed_edit_page_renders() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Edit Cat"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title, description) VALUES ($1, $2, $3, $4)",
        1_i64,
        "https://example.com/edit.xml",
        "Editable Feed",
        "Some desc"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
async fn test_unknown_route_logged_in_renders_chrome_404() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/this-page-does-not-exist").await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
    let body = response.text();
    assert!(
        body.contains("Page not found"),
        "fallback 404 should render the not-found heading, got: {body}"
    );
    assert!(
        body.contains("rdrs-sidebar"),
        "fallback 404 should render inside the app chrome (sidebar present), got: {body}"
    );
}

#[tokio::test]
async fn test_unknown_route_logged_out_redirects_to_login() {
    let app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    let response = app.server.get("/this-page-does-not-exist").await;
    response.assert_status_see_other();
    assert_eq!(response.header(axum::http::header::LOCATION), "/login");
}

#[tokio::test]
async fn test_feeds_import_page_renders() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Cats SSR"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        1_i64,
        "https://example.com/cats-ssr.xml",
        "A Feed"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/categories").await;
    response.assert_status_ok();
    let body = response.text();

    // No CSR shell on the SSR page.
    assert!(!body.contains("<rdrs-categories-page>"));
    // Empty state renders directly from the template (Tier-2 compact).
    assert!(body.contains("No categories yet"));
    assert!(body.contains("class=\"empty-state-compact\""));
}

#[tokio::test]
async fn test_admin_page_renders_ssr_content() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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

    // Multi-user (the test default), so the create form is the section's body.
    assert!(body.contains("data-testid=\"admin-create-user-form\""));
    assert!(!body.contains("data-testid=\"admin-create-user-unavailable\""));
}

#[tokio::test]
async fn test_admin_page_explains_why_account_creation_is_unavailable() {
    // Single-user instances used to hide the section outright, which left an
    // admin looking for a button that was never rendered. The heading stays and
    // the body says which variable turns it on.
    let mut app = create_test_app(Config {
        multi_user_enabled: false,
        ..default_test_config()
    })
    .await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

    let response = app.server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();

    assert!(body.contains("<h2>Add an account</h2>"));
    assert!(body.contains("data-testid=\"admin-create-user-unavailable\""));
    assert!(body.contains("RDRS_MULTI_USER_ENABLED=true"));
    assert!(!body.contains("data-testid=\"admin-create-user-form\""));
}

// ============================================================================
// SSR /feeds filter / sort tests
// ============================================================================

#[tokio::test]
async fn test_feeds_page_filter_errors_only_renders_error_rows() {
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Filter Test"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title, fetch_error) VALUES ($1, $2, $3, $4)",
        1_i64,
        "https://bad.com/feed.xml",
        "Bad Feed",
        "Connection refused"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        1_i64,
        "https://good.com/feed.xml",
        "Good Feed"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;

    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Cat A"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO category (user_id, name) VALUES ($1, $2)",
        1_i64,
        "Cat B"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        1_i64,
        "https://a.com/feed.xml",
        "Feed In A"
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)",
        2_i64,
        "https://b.com/feed.xml",
        "Feed In B"
    )
    .unwrap();

    login(&mut app.server, "admin").await;

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
    let app = create_test_app(default_test_config()).await;
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
async fn test_setup_page_does_not_load_logged_in_chrome() {
    let app = create_test_app(default_test_config()).await;
    let response = app.server.get("/setup").await;
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
    let mut app = create_test_app(default_test_config()).await;
    setup_users(&app.db).await;
    login(&mut app.server, "admin").await;

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
    let mut app = create_test_app_named(default_test_config(), "test_pages_unread_ssr").await;

    // Register and login as alice
    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_unread", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_unread", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    // Get user id
    let user_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_unread'"
    )
    .unwrap();

    // Seed: category + feed + 3 entries (2 unread, 1 read)
    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();

    let (_e1, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-entry-one",
        Some("Entry One"),
        Some("https://blog.example/one"),
        Some("<p>Content one</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-entry-two",
        Some("Entry Two"),
        Some("https://blog.example/two"),
        Some("<p>Content two</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (e3, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-entry-three",
        Some("Read Already"),
        Some("https://blog.example/three"),
        Some("<p>Content three</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let entry_three_id = e3.id;

    // Mark entry three as read
    rdrs::models::entry::mark_as_read(&app.db, entry_three_id)
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

    // Mark Above as Read button must render so the "o" shortcut can mark
    // the currently-loaded unread rows as read.
    assert!(
        html.contains(r#"id="mark-above-read""#),
        "unread page must render the Mark Above as Read button"
    );

    // `?fragment=1` without a cursor is the list-refresh response the
    // mark-above swap fetches: the whole list re-rendered in place, not the
    // Load-More append fragment (which would duplicate page 1 on top of it).
    let fragment = app.server.get("/?fragment=1").await;
    assert_eq!(fragment.status_code(), StatusCode::OK);
    let frag_html = fragment.text();
    assert!(
        frag_html.contains(r#"data-swap-target="[data-entries-list]""#),
        "refresh fragment must retarget the whole list container"
    );
    assert!(
        !frag_html.contains(r##"data-swap-target="#load-more""##),
        "a cursorless fragment must not be the Load-More append response"
    );
    assert!(frag_html.contains("Entry One"));
    assert!(
        !frag_html.contains("<rdrs-sidebar"),
        "the fragment is markup for a swap, not a document"
    );
}

// ============================================================================
// SSR entries family — PR-10 T2
// ============================================================================

#[tokio::test]
async fn test_entries_page_renders_ssr_rows() {
    let mut app = create_test_app_named(default_test_config(), "test_pages_entries_ssr").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_entries", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_entries", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_entries'"
    )
    .unwrap();

    // Seed: 3 entries — all visible on /entries (no filter exclusions).
    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-e1",
        Some("Entries Alpha"),
        Some("https://entries.example/alpha"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-e2",
        Some("Entries Beta"),
        Some("https://entries.example/beta"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-e3",
        Some("Entries Gamma"),
        Some("https://entries.example/gamma"),
        None,
        None,
        None,
        None,
    )
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
    let mut app = create_test_app_named(default_test_config(), "test_pages_read_ssr").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_read", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_read", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_read'"
    )
    .unwrap();

    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (e1, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-r1",
        Some("Read Alpha"),
        Some("https://read.example/alpha"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (e2, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-r2",
        Some("Read Beta"),
        Some("https://read.example/beta"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (e3, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-r3",
        Some("Unread One"),
        Some("https://read.example/unread"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (read_one_id, read_two_id, unread_id) = (e1.id, e2.id, e3.id);

    // Mark two entries as read; leave one unread.
    rdrs::models::entry::mark_as_read(&app.db, read_one_id)
        .await
        .unwrap();
    rdrs::models::entry::mark_as_read(&app.db, read_two_id)
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
    let mut app = create_test_app_named(default_test_config(), "test_pages_starred_ssr").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_starred", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_starred", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_starred'"
    )
    .unwrap();

    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (e1, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-s1",
        Some("Starred Alpha"),
        Some("https://starred.example/alpha"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (e2, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-s2",
        Some("Starred Beta"),
        Some("https://starred.example/beta"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-s3",
        Some("Unstarred One"),
        Some("https://starred.example/unstarred"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (starred_one_id, starred_two_id) = (e1.id, e2.id);

    // Star the two matching entries.
    rdrs::models::entry::star_entry(&app.db, starred_one_id)
        .await
        .unwrap();
    rdrs::models::entry::star_entry(&app.db, starred_two_id)
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
    let mut app = create_test_app_named(default_test_config(), "test_pages_summarized_ssr").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "alice_summarized", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "alice_summarized", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(
        &app.db,
        i64,
        "SELECT id FROM user WHERE username = 'alice_summarized'"
    )
    .unwrap();

    let cat = rdrs::models::category::create_category(&app.db, user_id, "Tech")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
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
    .await
    .unwrap();
    let (e1, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-sum1",
        Some("Summarized Alpha"),
        Some("https://sum.example/alpha"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (e2, _) = rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-sum2",
        Some("Summarized Beta"),
        Some("https://sum.example/beta"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    rdrs::models::entry::upsert_entry(
        &app.db,
        feed.id,
        "guid-sum3",
        Some("No Summary One"),
        Some("https://sum.example/nosummary"),
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (sum_one_id, sum_two_id) = (e1.id, e2.id);

    // Insert entry_summary rows for the two matching entries.
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO entry_summary (user_id, entry_id, status, summary_text) \
         VALUES ($1, $2, 'completed', 'Summary text alpha')",
        user_id,
        sum_one_id
    )
    .unwrap();
    rdrs::db_execute!(
        &app.db,
        "INSERT INTO entry_summary (user_id, entry_id, status, summary_text) \
         VALUES ($1, $2, 'completed', 'Summary text beta')",
        user_id,
        sum_two_id
    )
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

#[tokio::test]
async fn test_unread_load_more_uses_keyset_cursor() {
    let mut app = create_test_app_named(default_test_config(), "test_unread_keyset").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "kuser", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "kuser", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "K")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/keyset-feed",
            title: Some("K Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    for i in 0..60u32 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("kg-{i}"),
            Some(&format!("K {i}")),
            None,
            None,
            None,
            None,
            Some(
                chrono::Utc
                    .with_ymd_and_hms(2024, 1, 1, 0, i / 60, i % 60)
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    }

    // Page 1 (full render): first 50 rows + a Load-More form with a cursor.
    let html = app.server.get("/").await.text();
    let entry_ids_page1: std::collections::HashSet<String> = extract_entry_ids(&html);
    assert_eq!(entry_ids_page1.len(), 50, "page 1 shows the first 50");

    // Extract the cursor token from the Load-More form's hidden `after` input.
    let cursor = extract_after_value(&html).expect("Load-More form must carry an after cursor");
    assert!(
        cursor.contains('|'),
        "cursor is a composite token, got {cursor:?}"
    );

    // Page 2 (fragment) via the cursor.
    let encoded = cursor.replace(' ', "%20").replace('|', "%7C");
    let frag = app
        .server
        .get(&format!("/?fragment=1&after={encoded}"))
        .await
        .text();
    let entry_ids_page2 = extract_entry_ids(&frag);
    assert_eq!(entry_ids_page2.len(), 10, "page 2 shows the remaining 10");
    assert!(
        entry_ids_page1.is_disjoint(&entry_ids_page2),
        "keyset pages must not overlap"
    );
}

// Pull `data-entry-id="N"` values out of rendered HTML.
fn extract_entry_ids(html: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    let needle = "data-entry-id=\"";
    let mut rest = html;
    while let Some(i) = rest.find(needle) {
        rest = &rest[i + needle.len()..];
        if let Some(j) = rest.find('"') {
            out.insert(rest[..j].to_string());
            rest = &rest[j..];
        }
    }
    out
}

// Pull the value of the Load-More form's hidden `after` input.
fn extract_after_value(html: &str) -> Option<String> {
    let needle = "name=\"after\" value=\"";
    let i = html.find(needle)? + needle.len();
    let rest = &html[i..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}

/// Pull the Load-More form's hidden `snapshot` value.
fn extract_snapshot_value(html: &str) -> Option<String> {
    let needle = "name=\"snapshot\" value=\"";
    let i = html.find(needle)? + needle.len();
    let rest = &html[i..];
    let j = rest.find('"')?;
    Some(rest[..j].to_string())
}

/// Reading past the end of the loaded page used to lose the reader's place:
/// the entry they opened became read, Load More re-queried the *unread* list
/// without it, and the row `app.js` was waiting for in order to highlight the
/// selection never arrived (issue #482).
///
/// The page now paginates against its own render instant, so an entry read
/// while reading that page stays in its pages — the same snapshot rule the
/// neighbours navigation has always used.
#[tokio::test]
async fn test_unread_load_more_keeps_an_entry_read_during_this_page_view() {
    let mut app = create_test_app_named(default_test_config(), "test_unread_snapshot").await;

    app.server
        .post("/api/setup")
        .json(&json!({ "username": "snapuser", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&json!({ "username": "snapuser", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let user_id: i64 = rdrs::query_scalar!(&app.db, i64, "SELECT id FROM user LIMIT 1").unwrap();
    let cat = rdrs::models::category::create_category(&app.db, user_id, "S")
        .await
        .unwrap();
    let feed = rdrs::models::feed::create_feed(
        &app.db,
        &rdrs::models::feed::CreateFeedParams {
            category_id: cat.id,
            url: "https://x/snapshot-feed",
            title: Some("S Feed"),
            description: None,
            site_url: None,
            custom_user_agent: None,
            http2_disabled: None,
            custom_referrer: None,
        },
    )
    .await
    .unwrap();
    // Three pages' worth, so page 2 still renders a Load-More form of its own
    // and the snapshot has somewhere to be echoed to.
    for i in 0..160u32 {
        rdrs::models::entry::upsert_entry(
            &app.db,
            feed.id,
            &format!("sn-{i}"),
            Some(&format!("S {i}")),
            None,
            None,
            None,
            None,
            Some(
                chrono::Utc
                    .with_ymd_and_hms(2024, 1, 1, i / 3600, (i / 60) % 60, i % 60)
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    }

    // Page 1, and the snapshot it stamped.
    let html = app.server.get("/").await.text();
    let page1 = extract_entry_ids(&html);
    assert_eq!(page1.len(), 50);
    let cursor = extract_after_value(&html).expect("Load-More form must carry a cursor");
    let snapshot =
        extract_snapshot_value(&html).expect("Load-More form must carry the page's snapshot");

    // The reader opens the first entry of the *next* page — the one there is no
    // row for yet, which is the case that used to strand the selection. Reading
    // it marks it read.
    let frag_no_read = app
        .server
        .get(&format!(
            "/?fragment=1&after={}&snapshot={}",
            encode(&cursor),
            encode(&snapshot)
        ))
        .await
        .text();
    let next_id: i64 = extract_entry_ids(&frag_no_read)
        .iter()
        .filter_map(|s| s.parse().ok())
        .min()
        .expect("page 2 has rows");
    rdrs::models::entry::mark_as_read(&app.db, next_id)
        .await
        .unwrap();

    // Load More again, exactly as the form would: same cursor, same snapshot.
    let frag = app
        .server
        .get(&format!(
            "/?fragment=1&after={}&snapshot={}",
            encode(&cursor),
            encode(&snapshot)
        ))
        .await
        .text();
    assert!(
        extract_entry_ids(&frag).contains(&next_id.to_string()),
        "the entry read during this page view must still be listed — its row is \
         what the selection is waiting for (#482)"
    );

    // The next form carries the *same* snapshot forward, not a fresh one:
    // re-stamping would move the boundary and drop whatever was read since.
    assert_eq!(
        extract_snapshot_value(&frag).as_deref(),
        Some(snapshot.as_str()),
        "the snapshot must be echoed, never re-stamped"
    );

    // Without the snapshot — a stale form, or any other list view — the strict
    // unread filter still applies, so the read entry is gone. This is what the
    // old behaviour did on every page.
    let frag_strict = app
        .server
        .get(&format!("/?fragment=1&after={}", encode(&cursor)))
        .await
        .text();
    assert!(
        !extract_entry_ids(&frag_strict).contains(&next_id.to_string()),
        "no snapshot means strict unread — the widening must be opt-in"
    );
}

/// Percent-encode the two characters the cursor / snapshot tokens contain.
fn encode(value: &str) -> String {
    value
        .replace(' ', "%20")
        .replace('|', "%7C")
        .replace(':', "%3A")
}

#[tokio::test]
async fn test_settings_page_groups_and_forward_auth() {
    let mut config = default_test_config();
    config.auth_proxy_header = "Remote-User".to_string();
    config.trusted_proxy_networks = rdrs::config::parse_trusted_networks("10.0.0.0/8").unwrap();
    config.auth_proxy_admin_group = "admins".to_string();
    let mut app = create_test_app_named(config, "test_settings_groups_fa").await;

    app.server
        .post("/api/setup")
        .json(&serde_json::json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await
        .assert_status(StatusCode::CREATED);
    let __login = app
        .server
        .post("/api/session")
        .json(&serde_json::json!({ "username": "admin", "password": "vulture-mango-77-quilt" }))
        .await;
    __login.assert_status_ok();
    common::apply_csrf(&mut app.server, &__login);

    let res = app.server.get("/settings").await;
    res.assert_status_ok();
    let body = res.text();
    // group headers present
    assert!(
        body.contains("Authentication") && body.contains("Forward-Auth"),
        "expected 'Authentication' and 'Forward-Auth' in body"
    );
    assert!(body.contains("Accounts"), "expected 'Accounts' in body");
    // forward-auth rows present with current values reflected
    assert!(
        body.contains("RDRS_AUTH_PROXY_HEADER"),
        "expected RDRS_AUTH_PROXY_HEADER in body"
    );
    assert!(
        body.contains("Remote-User"),
        "expected 'Remote-User' in body"
    );
    assert!(
        body.contains("RDRS_AUTH_PROXY_LOGOUT_URL"),
        "expected RDRS_AUTH_PROXY_LOGOUT_URL in body"
    );
}
