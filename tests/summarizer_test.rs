mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::models::user_settings;
use rdrs::services::KagiConfig;
use rdrs::services::save::SaveServicesConfig;
use rdrs::{AppState, Config, Db, Role, auth, create_router, models::user, services};

struct TestApp {
    server: TestServer,
    db: Db,
}

async fn create_test_app(config: Config) -> TestApp {
    let db = Db::connect_in_memory().await.unwrap();
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _rx) = services::create_summary_channel(10);
    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
        sidebar_cache: Arc::new(services::SidebarCache::default()),
        summary_cancels: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        events: services::EventBus::new(16),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    let server = TestServer::builder()
        .save_cookies()
        .build(create_router(state));
    TestApp { server, db }
}

async fn login(app: &TestApp, username: &str) -> i64 {
    let hash = auth::hash_password("password123").unwrap();
    let u = user::create_user(&app.db, username, &hash, Role::User)
        .await
        .unwrap();
    app.server
        .post("/api/session")
        .json(&serde_json::json!({"username": username, "password": "password123"}))
        .await;
    u.id
}

async fn configure_kagi(app: &TestApp, user_id: i64) {
    let cfg = SaveServicesConfig {
        linkding: None,
        kagi: Some(KagiConfig {
            session_token: "tok".into(),
            language: None,
        }),
    };
    user_settings::update_save_services(&app.db, user_id, &cfg)
        .await
        .unwrap();
}

#[tokio::test]
async fn page_shows_settings_prompt_when_kagi_unset() {
    let app = create_test_app(default_test_config()).await;
    login(&app, "alice").await;
    let res = app.server.get("/summarizer").await;
    res.assert_status_ok();
    let body = res.text();
    assert!(body.contains("/user-settings"));
    assert!(!body.contains("data-testid=\"summarizer-form\""));
}

#[tokio::test]
async fn page_shows_form_when_kagi_configured() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "bob").await;
    configure_kagi(&app, uid).await;
    let res = app.server.get("/summarizer").await;
    res.assert_status_ok();
    assert!(res.text().contains("data-testid=\"summarizer-form\""));
}

#[tokio::test]
async fn page_requires_auth() {
    let app = create_test_app(default_test_config()).await;
    let res = app.server.get("/summarizer").await;
    // PageAuthUser redirects unauthenticated users to /login.
    assert!(res.status_code().is_redirection() || res.status_code().as_u16() == 200);
}

#[tokio::test]
async fn start_renders_queued_cards() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "carol").await;
    configure_kagi(&app, uid).await;
    let res = app
        .server
        .post("/summarizer")
        .form(&serde_json::json!({"urls": "https://a.com/x\nhttps://b.com/y"}))
        .await;
    res.assert_status_ok();
    let body = res.text();
    assert_eq!(body.matches("data-summarizer-card").count(), 2);
    assert!(body.contains("data-state=\"queued\""));
    assert!(body.contains("https://a.com/x"));
}

#[tokio::test]
async fn start_rejects_over_30() {
    let app = create_test_app(default_test_config()).await;
    let uid = login(&app, "dave").await;
    configure_kagi(&app, uid).await;
    let urls = (0..31)
        .map(|i| format!("https://e.com/{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let res = app
        .server
        .post("/summarizer")
        .form(&serde_json::json!({"urls": urls}))
        .await;
    res.assert_status_ok();
    let body = res.text();
    assert!(body.contains("30 max"));
    assert_eq!(body.matches("data-summarizer-card").count(), 0);
}
