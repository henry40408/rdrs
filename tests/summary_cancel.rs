//! Integration tests for `POST /entries/{id}/summarize/cancel`.
//!
//! Tests cover:
//! - Failed-summary clear deletes the record
//! - Non-owner returns 404
//! - In-flight token is cancelled and removed from the registry

mod common;
use common::{create_test_app, default_test_config, login_with as login};

use rdrs::models::{category, entry, feed, user};
use rdrs::{Db, Role, auth};
use tokio_util::sync::CancellationToken;

/// Seed a user, category, feed, and one entry. Returns (`user_id`, `entry_id`).
async fn setup_user_with_entry(db: &Db, username: &str, password: &str) -> (i64, i64) {
    let password_hash = auth::hash_password(password).unwrap();
    let user = user::create_user(db, username, &password_hash, Role::Admin)
        .await
        .unwrap();

    let cat = category::create_category(db, user.id, "Test Category")
        .await
        .unwrap();

    let feed = feed::create_feed(
        db,
        &feed::CreateFeedParams {
            category_id: cat.id,
            url: &format!("https://example.com/{username}/feed.xml"),
            title: Some("Test Feed"),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (e, _) = entry::upsert_entry(
        db,
        feed.id,
        &format!("{username}-guid-1"),
        Some("Test Entry"),
        Some("https://example.com/entry/1"),
        Some("<p>Content</p>"),
        None,
        None,
        None,
    )
    .await
    .unwrap();

    (user.id, e.id)
}

// --- Case 1: Failed-summary clear deletes the record ---

#[tokio::test]
async fn test_cancel_clears_failed_summary() {
    let mut app = create_test_app(default_test_config()).await;
    let (uid, eid) = setup_user_with_entry(&app.db, "user1", "vulture-mango-77-quilt").await;
    login(&mut app.server, "user1", "vulture-mango-77-quilt").await;

    rdrs::models::entry_summary::upsert_pending(&app.db, uid, eid)
        .await
        .unwrap();
    rdrs::models::entry_summary::set_failed(&app.db, uid, eid, "API error")
        .await
        .unwrap();

    // POST cancel
    let response = app
        .server
        .post(&format!("/entries/{eid}/summarize/cancel"))
        .await;
    response.assert_status_ok();

    let gone = rdrs::models::entry_summary::find_by_user_and_entry(&app.db, uid, eid)
        .await
        .unwrap();
    assert!(gone.is_none(), "expected summary record to be deleted");
}

// --- Case 2: Non-owner returns 404 ---

#[tokio::test]
async fn test_cancel_non_owner_returns_404() {
    let mut app = create_test_app(default_test_config()).await;

    let (_uid1, eid) = setup_user_with_entry(&app.db, "owner", "vulture-mango-77-quilt").await;

    let password_hash = auth::hash_password("password456").unwrap();
    user::create_user(&app.db, "attacker", &password_hash, Role::User)
        .await
        .unwrap();

    login(&mut app.server, "attacker", "password456").await;

    let response = app
        .server
        .post(&format!("/entries/{eid}/summarize/cancel"))
        .await;
    response.assert_status_not_found();
}

// --- Case: summarize POST emits a Pending event on the EventBus ---

#[tokio::test]
async fn summarize_emits_pending_event() {
    let mut app = create_test_app(default_test_config()).await;
    let mut sub = app.state.events.subscribe();
    let (uid, eid) = setup_user_with_entry(&app.db, "pendinguser", "vulture-mango-77-quilt").await;
    login(&mut app.server, "pendinguser", "vulture-mango-77-quilt").await;

    app.server
        .post(&format!("/entries/{eid}/summarize"))
        .await
        .assert_status_ok();

    let ev = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .expect("event emitted")
        .unwrap();
    assert_eq!(ev.user_id, uid);
    assert!(matches!(
        ev.kind,
        rdrs::services::EventKind::Summary {
            status: Some(rdrs::services::SummaryStatus::Pending),
            ..
        }
    ));
}

// --- Case 3: In-flight token is cancelled and removed from the registry ---

#[tokio::test]
async fn test_cancel_removes_inflight_token() {
    let mut app = create_test_app(default_test_config()).await;
    let (uid, eid) = setup_user_with_entry(&app.db, "tokenuser", "vulture-mango-77-quilt").await;
    login(&mut app.server, "tokenuser", "vulture-mango-77-quilt").await;

    // Seed a pending summary record so delete has a row
    rdrs::models::entry_summary::upsert_pending(&app.db, uid, eid)
        .await
        .unwrap();

    let cancel_token = CancellationToken::new();
    let cloned_token = cancel_token.clone();
    {
        let mut map = app.state.summary_cancels.lock().unwrap();
        map.insert((uid, eid), cancel_token);
    }

    // POST cancel
    let response = app
        .server
        .post(&format!("/entries/{eid}/summarize/cancel"))
        .await;
    response.assert_status_ok();

    assert!(
        cloned_token.is_cancelled(),
        "expected token to be cancelled"
    );

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
