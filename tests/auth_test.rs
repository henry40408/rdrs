use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool};
use rusqlite::Connection;
use serde_json::json;

fn create_test_server(config: Config) -> TestServer {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let (db, _handle) = DbPool::new(conn);
    let state = AppState {
        db,
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    let app = create_router(state);
    TestServer::builder().save_cookies().build(app).unwrap()
}

use std::sync::Arc;

fn default_test_config() -> Config {
    Config {
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
    }
}

#[tokio::test]
async fn test_register_first_user_becomes_admin() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
    assert_eq!(body["role"], "admin");
}

#[tokio::test]
async fn test_register_second_user_becomes_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::CREATED);
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "user1");
    assert_eq!(body["role"], "user");
}

#[tokio::test]
async fn test_register_duplicate_username() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "different123"
        }))
        .await;

    response.assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_register_disabled() {
    let config = Config {
        signup_enabled: false,
        ..default_test_config()
    };
    let server = create_test_server(config);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_register_multi_user_disabled() {
    let config = Config {
        signup_enabled: true,
        multi_user_enabled: false,
        ..default_test_config()
    };
    let server = create_test_server(config);

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;

    response.assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_login_success() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_login_wrong_password() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "wrongpassword"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_login_nonexistent_user() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "nonexistent",
            "password": "password123"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_get_current_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    login_response.assert_status_ok();

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_get_current_user_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/api/user").await;
    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_logout() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server.delete("/api/session").await.assert_status_ok();

    server.get("/api/user").await.assert_status_unauthorized();
}

#[tokio::test]
async fn test_change_password() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .put("/api/user/password")
        .json(&json!({
            "current_password": "password123",
            "new_password": "newpassword456"
        }))
        .await
        .assert_status_ok();

    // After password change, all sessions are invalidated, so user needs to login again
    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "newpassword456"
        }))
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn test_change_password_wrong_current() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server
        .put("/api/user/password")
        .json(&json!({
            "current_password": "wrongpassword",
            "new_password": "newpassword456"
        }))
        .await;

    response.assert_status_unauthorized();
}

#[tokio::test]
async fn test_admin_list_users() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/api/admin/users").await;
    response.assert_status_ok();
    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 2);
}

#[tokio::test]
async fn test_admin_list_users_forbidden() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/api/admin/users").await;
    response.assert_status_forbidden();
}

#[tokio::test]
async fn test_admin_disable_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .put("/api/admin/users/2")
        .json(&json!({
            "disabled": true
        }))
        .await
        .assert_status_ok();

    server.delete("/api/session").await.assert_status_ok();

    let response = server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await;

    response.assert_status_forbidden();
}

#[tokio::test]
async fn test_admin_cannot_disable_self() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server
        .put("/api/admin/users/1")
        .json(&json!({
            "disabled": true
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_admin_delete_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .delete("/api/admin/users/2")
        .await
        .assert_status(StatusCode::NO_CONTENT);

    let response = server.get("/api/admin/users").await;
    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body.len(), 1);
}

#[tokio::test]
async fn test_admin_cannot_delete_self() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.delete("/api/admin/users/1").await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_admin_update_role() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .put("/api/admin/users/2")
        .json(&json!({
            "role": "admin"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/api/admin/users").await;
    let body: Vec<serde_json::Value> = response.json();
    assert_eq!(body[1]["role"], "admin");
}

#[tokio::test]
async fn test_masquerade() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .post("/api/admin/masquerade/2")
        .await
        .assert_status_ok();

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "user1");

    server
        .post("/api/admin/unmasquerade")
        .await
        .assert_status_ok();

    let response = server.get("/api/user").await;
    response.assert_status_ok();
    let body: serde_json::Value = response.json();
    assert_eq!(body["username"], "admin");
}

#[tokio::test]
async fn test_masquerade_already_masquerading() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    server
        .post("/api/admin/masquerade/2")
        .await
        .assert_status_ok();

    let response = server.post("/api/admin/masquerade/2").await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_unmasquerade_not_masquerading() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.post("/api/admin/unmasquerade").await;
    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_login_page() {
    let server = create_test_server(default_test_config());

    let response = server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Login"));
}

#[tokio::test]
async fn test_register_page() {
    let server = create_test_server(default_test_config());

    let response = server.get("/register").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Register"));
}

#[tokio::test]
async fn test_validation_short_password() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "short"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_validation_empty_username() {
    let server = create_test_server(default_test_config());

    let response = server
        .post("/api/register")
        .json(&json!({
            "username": "",
            "password": "password123"
        }))
        .await;

    response.assert_status_bad_request();
}

#[tokio::test]
async fn test_unread_page() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    let login_response = server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await;

    login_response.assert_status_ok();

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    // Username is displayed in the navigation bar
    assert!(body.contains("admin"));
    assert!(body.contains("Sign Out"));
}

#[tokio::test]
async fn test_unread_page_unauthorized() {
    let server = create_test_server(default_test_config());

    let response = server.get("/").await;
    // Page routes redirect to login instead of returning 401
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_admin_page_accessible_by_admin() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/admin").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Admin Panel"));
}

#[tokio::test]
async fn test_admin_page_forbidden_for_regular_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/admin").await;
    // Page routes redirect to login instead of returning 403
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_admin_page_unauthorized_without_login() {
    let server = create_test_server(default_test_config());

    let response = server.get("/admin").await;
    // Page routes redirect to login instead of returning 401
    response.assert_status_see_other();
}

#[tokio::test]
async fn test_unread_page_shows_admin_link_for_admin() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("[Admin]"));
    assert!(body.contains(r#"href="/admin""#));
}

#[tokio::test]
async fn test_unread_page_hides_admin_link_for_regular_user() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/register")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "user1",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    let response = server.get("/").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("[Admin]"));
    assert!(!body.contains(r#"href="/admin""#));
}

#[tokio::test]
async fn test_flash_message_displayed_on_login_page() {
    let server = create_test_server(default_test_config());

    // Set a flash message cookie using add_cookie with cookie::Cookie
    let response = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"success","message":"Test flash message"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Test flash message"));
    assert!(body.contains("flash-success"));
}

#[tokio::test]
async fn test_flash_message_cleared_after_display() {
    let server = create_test_server(default_test_config());

    // First request with flash cookie
    let response = server
        .get("/login")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"info","message":"First message"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("First message"));

    // Second request should not have the flash message (cookie was cleared)
    let response = server.get("/login").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("First message"));
}

#[tokio::test]
async fn test_flash_message_on_unread_page() {
    let server = create_test_server(default_test_config());

    server
        .post("/api/register")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status(StatusCode::CREATED);

    server
        .post("/api/session")
        .json(&json!({
            "username": "admin",
            "password": "password123"
        }))
        .await
        .assert_status_ok();

    // Request unread page with flash message
    let response = server
        .get("/")
        .add_cookie(cookie::Cookie::new(
            "flash",
            r#"[{"level":"warning","message":"Warning test"}]"#,
        ))
        .await;

    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Warning test"));
    assert!(body.contains("flash-warning"));
}
