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

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{AppState, Config, Db, auth, create_router, services};

async fn create_test_server(config: Config) -> TestServer {
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db,
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
    TestServer::builder()
        .save_cookies()
        .build(create_router(state))
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
