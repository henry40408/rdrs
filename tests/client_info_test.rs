//! `PUT /api/user/client-info` and the settings-page prefill its hint links to.

mod common;
use common::{create_test_app, default_test_config};

use axum::http::StatusCode;
use axum_test::TestServer;
use serde_json::{Value, json};

async fn authed_server() -> TestServer {
    let mut server = create_test_app(default_test_config()).await.server;
    server
        .post("/api/setup")
        .json(&json!({"username": "testuser", "password": "vulture-mango-77-quilt"}))
        .await
        .assert_status(StatusCode::CREATED);
    common::login(&mut server, "testuser").await;
    server
}

async fn report(server: &TestServer, body: Value) -> axum_test::TestResponse {
    server.put("/api/user/client-info").json(&body).await
}

/// The hints a report earned; also checks that no flash cookie was set, since
/// one would be consumed by whichever request reads flashes next.
async fn hints_for(server: &TestServer, body: Value) -> Vec<Value> {
    let response = report(server, body).await;
    response.assert_status_ok();
    assert!(response.maybe_cookie("flash").is_none());
    response.json::<Value>()["hints"]
        .as_array()
        .unwrap()
        .clone()
}

async fn assert_no_hint(server: &TestServer, body: Value) {
    assert_eq!(hints_for(server, body).await, Vec::<Value>::new());
}

async fn store_timezone(server: &TestServer, name: &str) {
    server
        .post("/user-settings/preferences")
        .form(&json!({
            "theme": "system",
            "entries_per_page": 30,
            "retention_read_days": 0,
            "timezone": name,
        }))
        .await
        .assert_status(StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn a_matching_timezone_earns_no_hint() {
    let server = authed_server().await;

    assert_no_hint(&server, json!({"timezone": "UTC"})).await;
}

#[tokio::test]
async fn a_different_timezone_earns_a_hint_linking_to_preferences() {
    let server = authed_server().await;

    let hints = hints_for(&server, json!({"timezone": "Asia/Taipei"})).await;

    assert_eq!(hints.len(), 1);
    let message = hints[0]["message"].as_str().unwrap();
    assert!(message.contains("Asia/Taipei"), "{message}");
    assert!(message.contains("UTC"), "names the stored zone: {message}");
    assert_eq!(
        hints[0]["action_href"],
        "/user-settings?timezone=Asia%2FTaipei"
    );
    assert!(hints[0]["action_label"].as_str().is_some());
}

#[tokio::test]
async fn a_hint_changes_no_setting() {
    let server = authed_server().await;
    hints_for(&server, json!({"timezone": "Asia/Taipei"})).await;

    let page = common::get_ok(&server, "/user-settings").await;

    assert!(page.contains("name=\"timezone\" value=\"UTC\""));
}

#[tokio::test]
async fn an_alias_of_the_stored_timezone_earns_no_hint() {
    let server = authed_server().await;

    // Browsers report either spelling of these.
    assert_no_hint(&server, json!({"timezone": "Etc/UTC"})).await;

    store_timezone(&server, "Asia/Kolkata").await;
    assert_no_hint(&server, json!({"timezone": "Asia/Calcutta"})).await;
}

#[tokio::test]
async fn an_unknown_timezone_name_is_refused() {
    let server = authed_server().await;

    let response = report(&server, json!({"timezone": "Mars/Olympus_Mons"})).await;

    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_report_without_fields_earns_no_hint() {
    let server = authed_server().await;

    assert_no_hint(&server, json!({})).await;
}

#[tokio::test]
async fn an_anonymous_report_is_refused() {
    let server = create_test_app(default_test_config()).await.server;

    let response = report(&server, json!({"timezone": "Asia/Taipei"})).await;

    assert!(response.status_code().is_client_error());
}

#[tokio::test]
async fn the_settings_page_prefills_a_suggested_timezone_without_saving_it() {
    let server = authed_server().await;

    let page = common::get_ok(&server, "/user-settings?timezone=Asia%2FTaipei").await;
    assert!(page.contains("name=\"timezone\" value=\"Asia/Taipei\""));
    assert!(page.contains("data-testid=\"timezone-suggestion\""));

    let page = common::get_ok(&server, "/user-settings").await;
    assert!(page.contains("name=\"timezone\" value=\"UTC\""));
    assert!(!page.contains("data-testid=\"timezone-suggestion\""));
}

#[tokio::test]
async fn the_settings_page_ignores_a_suggestion_it_cannot_use() {
    let server = authed_server().await;
    store_timezone(&server, "Asia/Tokyo").await;

    for query in [
        "timezone=Mars%2FOlympus_Mons",
        "timezone=",
        "timezone=Asia%2FTokyo",
        "timezone=Asia%2FTokyo&timezone=Asia%2FTaipei",
        "other=1",
    ] {
        let page = common::get_ok(&server, &format!("/user-settings?{query}")).await;
        assert!(
            page.contains("name=\"timezone\" value=\"Asia/Tokyo\""),
            "{query}"
        );
        assert!(
            !page.contains("data-testid=\"timezone-suggestion\""),
            "{query}"
        );
    }
}
