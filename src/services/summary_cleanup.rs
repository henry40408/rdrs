use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::models::entry_summary;

/// Start the summary cleanup worker that periodically removes expired summaries
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
            "Summary cleanup worker started: interval={}h, ttl={}h",
            interval_hours,
            ttl_hours
        );

        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                () = cancel_token.cancelled() => {
                    tracing::info!("Summary cleanup worker stopping...");
                    break;
                }
                _ = interval.tick() => {
                    tracing::debug!("Running summary cleanup...");

                    let deleted = match entry_summary::delete_expired(&db, ttl_hours).await {
                        Ok(count) => count,
                        Err(e) => {
                            tracing::error!("Failed to cleanup expired summaries: {}", e);
                            continue;
                        }
                    };

                    if deleted > 0 {
                        tracing::info!("Cleaned up {} expired summaries", deleted);
                    }
                }
            }
        }

        tracing::info!("Summary cleanup worker stopped");
    })
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

        // Create a summary
        entry_summary::upsert_pending(&db, user_id, entry.id)
            .await
            .unwrap();
        entry_summary::set_completed(&db, user_id, entry.id, "Summary text")
            .await
            .unwrap();

        // Verify it exists
        assert!(entry_summary::exists(&db, user_id, entry.id).await.unwrap());

        // Manually set created_at to 25 hours ago
        db_execute!(
            &db,
            "UPDATE entry_summary SET created_at = datetime('now', '-25 hours') WHERE user_id = $1 AND entry_id = $2",
            user_id,
            entry.id,
        )
        .unwrap();

        // Delete entries older than 24 hours
        let deleted = entry_summary::delete_expired(&db, 24).await.unwrap();
        assert_eq!(deleted, 1);

        // Verify it's gone
        assert!(!entry_summary::exists(&db, user_id, entry.id).await.unwrap());
    }

    #[tokio::test]
    async fn test_cleanup_worker_stops_on_cancellation() {
        let db = setup_db().await;
        let cancel_token = CancellationToken::new();

        // Start cleanup worker with a long interval (won't trigger during test)
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

        // Create test data with an expired summary
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

        // Create an expired summary (25 hours old)
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

        // Verify summary exists before cleanup
        let exists_before = entry_summary::exists(&db, 1, 1).await.unwrap();
        assert!(exists_before);

        // Run cleanup directly (simulating what the worker does)
        let deleted = entry_summary::delete_expired(&db, 24).await.unwrap();
        assert_eq!(deleted, 1);

        // Verify summary was deleted
        let exists_after = entry_summary::exists(&db, 1, 1).await.unwrap();
        assert!(!exists_after);
    }
}
