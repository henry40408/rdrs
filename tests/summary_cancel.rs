//! Integration tests for `POST /entries/{id}/summarize/cancel`.
//!
//! Tests cover:
//! - Failed-summary clear deletes the record
//! - Non-owner returns 404
//! - In-flight token is cancelled and removed from the registry

mod common;
use common::default_test_config;

use std::sync::Arc;

use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool, Role};
use rusqlite::Connection;
use serde_json::json;
use tokio_util::sync::CancellationToken;

struct TestApp {
    server: TestServer,
    db: DbPool,
    state: AppState,
}

fn open_shared_memory(name: &str) -> Connection {
    let uri = format!("file:{}?mode=memory&cache=shared", name);
    Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

fn create_test_app(config: Config, db_name: &str) -> TestApp {
    let write_conn = open_shared_memory(db_name);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(db_name);

    let (db, _handle) = DbPool::new(write_conn, read_conn);
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
    };

    let app = create_router(state.clone());
    let server = TestServer::builder().save_cookies().build(app);

    TestApp { server, db, state }
}

/// Seed a user, category, feed, and one entry. Returns (user_id, entry_id).
async fn setup_user_with_entry(db: &DbPool, username: &str, password: &str) -> (i64, i64) {
    let username = username.to_owned();
    let password = password.to_owned();
    db.user(move |conn| {
        let password_hash = auth::hash_password(&password).unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params![username, password_hash, Role::Admin.as_str()],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
            rusqlite::params![user_id, "Test Category"],
        )
        .unwrap();
        let category_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                category_id,
                format!("https://example.com/{}/feed.xml", username),
                "Test Feed"
            ],
        )
        .unwrap();
        let feed_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, link, content, published_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            rusqlite::params![
                feed_id,
                format!("{}-guid-1", username),
                "Test Entry",
                "https://example.com/entry/1",
                "<p>Content</p>"
            ],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();

        (user_id, entry_id)
    })
    .await
    .unwrap()
}

async fn login(server: &TestServer, username: &str, password: &str) {
    server
        .post("/api/session")
        .json(&json!({
            "username": username,
            "password": password
        }))
        .await
        .assert_status_ok();
}

// ============================================================================
// Case 1: Failed-summary clear deletes the record
// ============================================================================

#[tokio::test]
async fn test_cancel_clears_failed_summary() {
    let app = create_test_app(default_test_config(), "test_cancel_clears_failed");
    let (uid, eid) = setup_user_with_entry(&app.db, "user1", "password123").await;
    login(&app.server, "user1", "password123").await;

    // Seed a failed summary record
    app.db
        .user(move |conn| {
            rdrs::models::entry_summary::upsert_pending(conn, uid, eid).unwrap();
            rdrs::models::entry_summary::set_failed(conn, uid, eid, "API error").unwrap();
            Ok::<_, rdrs::error::AppError>(())
        })
        .await
        .unwrap()
        .unwrap();

    // POST cancel
    let response = app
        .server
        .post(&format!("/entries/{}/summarize/cancel", eid))
        .await;
    response.assert_status_ok();

    // Assert the summary record is gone
    let gone = app
        .db
        .read_user(move |c| rdrs::models::entry_summary::find_by_user_and_entry(c, uid, eid))
        .await
        .unwrap()
        .unwrap();
    assert!(gone.is_none(), "expected summary record to be deleted");
}

// ============================================================================
// Case 2: Non-owner returns 404
// ============================================================================

#[tokio::test]
async fn test_cancel_non_owner_returns_404() {
    let app = create_test_app(default_test_config(), "test_cancel_non_owner");

    // Create owner user with an entry
    let (_uid1, eid) = setup_user_with_entry(&app.db, "owner", "password123").await;

    // Create a second user who does NOT own that entry
    app.db
        .user(move |conn| {
            let password_hash = auth::hash_password("password456").unwrap();
            conn.execute(
                "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
                rusqlite::params!["attacker", password_hash, "user"],
            )
            .unwrap();
            Ok::<_, rdrs::error::AppError>(())
        })
        .await
        .unwrap()
        .unwrap();

    // Login as attacker
    login(&app.server, "attacker", "password456").await;

    // Try to cancel the owner's entry
    let response = app
        .server
        .post(&format!("/entries/{}/summarize/cancel", eid))
        .await;
    response.assert_status_not_found();
}

// ============================================================================
// Case 3: In-flight token is cancelled and removed from the registry
// ============================================================================

#[tokio::test]
async fn test_cancel_removes_inflight_token() {
    let app = create_test_app(default_test_config(), "test_cancel_inflight_token");
    let (uid, eid) = setup_user_with_entry(&app.db, "tokenuser", "password123").await;
    login(&app.server, "tokenuser", "password123").await;

    // Seed a pending summary record so delete has a row
    app.db
        .user(move |conn| {
            rdrs::models::entry_summary::upsert_pending(conn, uid, eid).unwrap();
            Ok::<_, rdrs::error::AppError>(())
        })
        .await
        .unwrap()
        .unwrap();

    // Insert a CancellationToken into the registry
    let cancel_token = CancellationToken::new();
    let cloned_token = cancel_token.clone();
    {
        let mut map = app.state.summary_cancels.lock().unwrap();
        map.insert((uid, eid), cancel_token);
    }

    // POST cancel
    let response = app
        .server
        .post(&format!("/entries/{}/summarize/cancel", eid))
        .await;
    response.assert_status_ok();

    // Assert the cloned token is now cancelled
    assert!(
        cloned_token.is_cancelled(),
        "expected token to be cancelled"
    );

    // Assert the token has been removed from the registry
    assert!(
        app.state
            .summary_cancels
            .lock()
            .unwrap()
            .get(&(uid, eid))
            .is_none(),
        "expected token to be removed from registry"
    );
}
