use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::models::{api_token, entry_summary, session};

/// Start the worker that periodically deletes expired summaries, sessions and
/// `api_token` rows. The latter two back up the lazy per-request deletes, which
/// never fire for an abandoned device. Sweeps fail independently.
pub fn start_cleanup_worker(
    db: Db,
    interval_hours: u64,
    ttl_hours: i64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
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

/// One cleanup pass, extracted so a test can drive it. Each sweep has its own
/// `match` so one failure cannot skip the others: a broken summary table must
/// not halt session and token expiry.
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
    use crate::models::{category, entry, feed};
    use crate::test_support::{seed_user, setup_db};

    #[tokio::test]
    async fn test_delete_expired() {
        let db = setup_db().await;
        let user_id = seed_user(&db, "testuser", Role::User).await.id;
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
                ..Default::default()
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

        cancel_token.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(
            result.is_ok(),
            "Cleanup worker should stop after cancellation"
        );
    }

    #[tokio::test]
    async fn test_cleanup_worker_runs_cleanup_on_interval() {
        let db = setup_db().await;

        let user_id = seed_user(&db, "testuser", Role::User).await.id;
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
                ..Default::default()
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
        let user_id = seed_user(&db, "testuser", Role::User).await.id;

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

        // Run the session sweep directly, as the worker does each tick.
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
        let user_id = seed_user(&db, "testuser", Role::User).await.id;

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

        // Run the api_token sweep directly, as the worker does each tick.
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
        // Drives the real per-tick body with one sweep failing; the other tests
        // would pass against a `continue`-on-error chain.
        let db = setup_db().await;
        let user_id = seed_user(&db, "testuser", Role::User).await.id;

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

        // Break the first sweep, so an error that propagated would fail both
        // assertions below.
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
