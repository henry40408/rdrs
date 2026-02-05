use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::DbPool;
use crate::models::entry_summary;

/// Start the summary cleanup worker that periodically removes expired summaries
///
/// # Arguments
/// * `db` - Database connection
/// * `interval_hours` - How often to run cleanup (in hours)
/// * `ttl_hours` - Delete summaries older than this many hours
/// * `cancel_token` - Token to signal graceful shutdown
pub fn start_cleanup_worker(
    db: DbPool,
    interval_hours: u64,
    ttl_hours: i64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(
            "Summary cleanup worker started: interval={}h, ttl={}h",
            interval_hours,
            ttl_hours
        );

        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Summary cleanup worker stopping...");
                    break;
                }
                _ = interval.tick() => {
                    tracing::debug!("Running summary cleanup...");

                    let deleted = match db
                        .background(move |conn| entry_summary::delete_expired(conn, ttl_hours))
                        .await
                    {
                        Ok(Ok(count)) => count,
                        Ok(Err(e)) => {
                            tracing::error!("Failed to cleanup expired summaries: {}", e);
                            continue;
                        }
                        Err(e) => {
                            tracing::error!("Failed to access DB for cleanup: {}", e);
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
    use crate::db::init_db;
    use crate::models::user::Role;
    use crate::models::{category, entry, feed, user};
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn test_delete_expired() {
        let conn = setup_db();
        let user_id = user::create_user(&conn, "testuser", "hash", Role::User)
            .unwrap()
            .id;
        let category_id = category::create_category(&conn, user_id, "Tech")
            .unwrap()
            .id;
        let feed_id = feed::create_feed(
            &conn,
            category_id,
            "https://example.com/feed.xml",
            Some("Feed"),
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .id;

        let (entry, _) = entry::upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("Entry"),
            Some("https://example.com"),
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Create a summary
        entry_summary::upsert_pending(&conn, user_id, entry.id).unwrap();
        entry_summary::set_completed(&conn, user_id, entry.id, "Summary text").unwrap();

        // Verify it exists
        assert!(entry_summary::exists(&conn, user_id, entry.id).unwrap());

        // Manually set created_at to 25 hours ago
        conn.execute(
            "UPDATE entry_summary SET created_at = datetime('now', '-25 hours') WHERE user_id = ?1 AND entry_id = ?2",
            rusqlite::params![user_id, entry.id],
        )
        .unwrap();

        // Delete entries older than 24 hours
        let deleted = entry_summary::delete_expired(&conn, 24).unwrap();
        assert_eq!(deleted, 1);

        // Verify it's gone
        assert!(!entry_summary::exists(&conn, user_id, entry.id).unwrap());
    }
}
