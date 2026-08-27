use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::models::{api_token, entry_summary, session};

/// Start the cleanup worker that periodically removes expired summaries,
/// expired session rows, *and* expired `api_token` rows.
///
/// The session sweep is the backstop for the lazy per-request deletes in
/// `middleware/auth.rs` / `handlers/greader/auth.rs`: those only fire when a
/// row is touched, so a session abandoned on an old device would otherwise
/// live forever (see `session::delete_expired`). The `api_token` sweep is the
/// same backstop for `handlers/greader/auth.rs`'s `validate_api_token` (see
/// `api_token::delete_expired`). All three sweeps run on the same tick but
/// fail independently — one sweep's error must not skip another's.
///
/// # Arguments
/// * `db` - Database connection
/// * `interval_hours` - How often to run cleanup (in hours)
/// * `ttl_hours` - Delete summaries older than this many hours
/// * `cancel_token` - Token to signal graceful shutdown
pub fn start_cleanup_worker(
    db: Db,
    interval_hours: u64,
    ttl_hours: i64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Background priority: DB operations yield to interactive work on SQLite.
        let db = db.background();
        tracing::info!(
            event = "cleanup.worker_started",
            interval_hours,
            ttl_hours,
            "summary cleanup worker started"
        );

        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                () = cancel_token.cancelled() => {
                    tracing::info!(event = "cleanup.worker_stopping", "summary cleanup worker stopping");
                    break;
                }
                _ = interval.tick() => {
                    run_sweeps(&db, ttl_hours).await;
                }
            }
        }

        tracing::info!(
            event = "cleanup.worker_stopped",
            "summary cleanup worker stopped"
        );
    })
}

/// One cleanup pass: expired summaries, expired sessions, expired API tokens.
///
/// Extracted from the worker's `select!` arm so it can be driven directly by a
/// test — the independence property below is otherwise unobservable, since a
/// test that calls each `delete_expired` itself would pass just as happily
/// against a `continue`-on-error chain.
///
/// Each sweep gets its own `match` rather than a `?` or a `continue`: a
/// failure in one must not skip the others. A broken `entry_summary` table
/// silently halting session and token expiry would turn one bug into an
/// unbounded credential lifetime.
async fn run_sweeps(db: &Db, ttl_hours: i64) {
    tracing::debug!(event = "cleanup.sweep_started", "running cleanup sweep");

    match entry_summary::delete_expired(db, ttl_hours).await {
        Ok(n) if n > 0 => {
            tracing::info!(
                event = "cleanup.swept",
                kind = "summary",
                count = n,
                "cleaned up expired summaries"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(event = "cleanup.sweep_failed", kind = "summary", error = %e, "failed to clean up expired summaries");
        }
    }

    match session::delete_expired(db).await {
        Ok(n) if n > 0 => {
            tracing::info!(
                event = "cleanup.swept",
                kind = "session",
                count = n,
                "swept expired sessions"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(event = "cleanup.sweep_failed", kind = "session", error = %e, "failed to sweep expired sessions");
        }
    }

    match api_token::delete_expired(db).await {
        Ok(n) if n > 0 => {
            tracing::info!(
                event = "cleanup.swept",
                kind = "api_token",
                count = n,
                "swept expired API tokens"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(event = "cleanup.sweep_failed", kind = "api_token", error = %e, "failed to sweep expired API tokens");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_execute;
    use crate::models::user::Role;
    use crate::models::{category, entry, feed, user};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_delete_expired() {
        let db = setup_db().await;
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let category_id = category::create_category(&db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;

        let (entry, _) = entry::upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Entry"),
            Some("https://example.com"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        entry_summary::upsert_pending(&db, user_id, entry.id)
            .await
            .unwrap();
        entry_summary::set_completed(&db, user_id, entry.id, "Summary text")
            .await
            .unwrap();

        assert!(entry_summary::exists(&db, user_id, entry.id).await.unwrap());

        // Manually set created_at to 25 hours ago
        db_execute!(
            &db,
            "UPDATE entry_summary SET created_at = datetime('now', '-25 hours') WHERE user_id = $1 AND entry_id = $2",
            user_id,
            entry.id,
        )
        .unwrap();

        let deleted = entry_summary::delete_expired(&db, 24).await.unwrap();
        assert_eq!(deleted, 1);

        assert!(!entry_summary::exists(&db, user_id, entry.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_cleanup_worker_stops_on_cancellation() {
        let db = setup_db().await;
        let cancel_token = CancellationToken::new();

        let handle = start_cleanup_worker(db, 1000, 24, cancel_token.clone());

        // Cancel immediately
        cancel_token.cancel();

        // Worker should stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(
            result.is_ok(),
            "Cleanup worker should stop after cancellation"
        );
    }

    #[tokio::test]
    async fn test_cleanup_worker_runs_cleanup_on_interval() {
        let db = setup_db().await;

        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let category_id = category::create_category(&db, user_id, "Tech")
            .await
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/feed.xml",
                title: Some("Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;

        let (entry_obj, _) = entry::upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Entry"),
            Some("https://example.com"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        entry_summary::upsert_pending(&db, user_id, entry_obj.id)
            .await
            .unwrap();
        entry_summary::set_completed(&db, user_id, entry_obj.id, "Summary text")
            .await
            .unwrap();

        db_execute!(
            &db,
            "UPDATE entry_summary SET created_at = datetime('now', '-25 hours') WHERE user_id = $1 AND entry_id = $2",
            user_id,
            entry_obj.id,
        )
        .unwrap();

        let exists_before = entry_summary::exists(&db, 1, 1).await.unwrap();
        assert!(exists_before);

        let deleted = entry_summary::delete_expired(&db, 24).await.unwrap();
        assert_eq!(deleted, 1);

        let exists_after = entry_summary::exists(&db, 1, 1).await.unwrap();
        assert!(!exists_after);
    }

    #[tokio::test]
    async fn test_cleanup_worker_sweeps_expired_sessions() {
        let db = setup_db().await;
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;

        let expired = session::create_session(&db, user_id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        db_execute!(
            &db,
            "UPDATE session SET expires_at = datetime('now', '-1 hours') WHERE id = $1",
            expired.id,
        )
        .unwrap();

        let fresh = session::create_session(&db, user_id, "test-agent", "127.0.0.1")
            .await
            .unwrap();

        // Run the session sweep directly (simulating what the worker does on
        // each tick, same as `test_cleanup_worker_runs_cleanup_on_interval`
        // does for the summary sweep above).
        let swept = session::delete_expired(&db).await.unwrap();
        assert_eq!(swept, 1);

        assert!(
            session::find_by_token(&db, &expired.session_token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            session::find_by_token(&db, &fresh.session_token)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_cleanup_worker_sweeps_expired_api_tokens() {
        let db = setup_db().await;
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;

        let expired =
            api_token::create_api_token(&db, user_id, "greader", "", "test-agent", "127.0.0.1")
                .await
                .unwrap();
        db_execute!(
            &db,
            "UPDATE api_token SET expires_at = datetime('now', '-1 hours') WHERE id = $1",
            expired.id,
        )
        .unwrap();

        let fresh =
            api_token::create_api_token(&db, user_id, "greader", "", "test-agent", "127.0.0.1")
                .await
                .unwrap();

        // Run the api_token sweep directly (simulating what the worker does on
        // each tick), same shape as the session sweep test above.
        let swept = api_token::delete_expired(&db).await.unwrap();
        assert_eq!(swept, 1);

        assert!(
            api_token::find_by_token(&db, &expired.token)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            api_token::find_by_token(&db, &fresh.token)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_a_failing_sweep_does_not_skip_the_others() {
        // The property the three separate `match`es exist for. Every other
        // test in this file calls `delete_expired` directly, so all of them
        // would pass unchanged against a `continue`-on-error chain; this one
        // drives the real per-tick body with one sweep guaranteed to fail.
        let db = setup_db().await;
        let user_id = user::create_user(&db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;

        let expired_session = session::create_session(&db, user_id, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        db_execute!(
            &db,
            "UPDATE session SET expires_at = datetime('now', '-1 hours') WHERE id = $1",
            expired_session.id,
        )
        .unwrap();

        let expired_token =
            api_token::create_api_token(&db, user_id, "greader", "", "test-agent", "127.0.0.1")
                .await
                .unwrap();
        db_execute!(
            &db,
            "UPDATE api_token SET expires_at = datetime('now', '-1 hours') WHERE id = $1",
            expired_token.id,
        )
        .unwrap();

        // Break the *first* sweep specifically: it runs before the other two,
        // so if an error propagated instead of being logged and swallowed,
        // neither of the assertions below could hold.
        db_execute!(&db, "DROP TABLE entry_summary").unwrap();

        run_sweeps(&db, 24).await;

        assert!(
            session::find_by_token(&db, &expired_session.session_token)
                .await
                .unwrap()
                .is_none(),
            "the session sweep must still run after the summary sweep failed"
        );
        assert!(
            api_token::find_by_token(&db, &expired_token.token)
                .await
                .unwrap()
                .is_none(),
            "the api_token sweep must still run after the summary sweep failed"
        );
    }
}
