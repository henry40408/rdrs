use std::sync::Arc;

use chrono::Utc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::feed_sync;
use super::sidebar_cache::SidebarCache;
use crate::db::DbPool;
use crate::models::feed;
use crate::services::EventBus;

pub fn start_background_sync(
    db: DbPool,
    user_agent: String,
    cancel_token: CancellationToken,
    sidebar_cache: Arc<SidebarCache>,
    events: EventBus,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("Background sync task started");

        let mut ticker = interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!("Background sync stopping...");
                    break;
                }
                _ = ticker.tick() => {
                    let now = Utc::now();
                    let bucket = (now.timestamp() / 60 % 60) as u8;

                    debug!("Running background sync for bucket {}", bucket);

                    let results = feed_sync::refresh_bucket(db.clone(), bucket, &user_agent).await;

                    let success_count = results.iter().filter(|(_, r)| r.is_ok()).count();
                    let fail_count = results.iter().filter(|(_, r)| r.is_err()).count();

                    if !results.is_empty() {
                        info!(
                            "Background sync bucket {}: {} succeeded, {} failed",
                            bucket, success_count, fail_count
                        );
                    }

                    for (feed_id, result) in &results {
                        if let Err(e) = result {
                            error!("Background sync feed {} failed: {}", feed_id, e);
                        }
                    }

                    // Feeds that gained unread entries this cycle: bust each
                    // owner's sidebar cache and nudge their open tabs to refetch.
                    let changed_feed_ids: Vec<i64> = results
                        .iter()
                        .filter(|(_, r)| matches!(r, Ok(s) if s.new_entries > 0))
                        .map(|(id, _)| *id)
                        .collect();
                    if !changed_feed_ids.is_empty() {
                        match db
                            .read_background(move |conn| {
                                feed::owner_user_ids_for_feeds(conn, &changed_feed_ids)
                            })
                            .await
                        {
                            Ok(Ok(user_ids)) => {
                                for uid in user_ids {
                                    sidebar_cache.bust(uid);
                                    events.emit_sidebar(uid);
                                }
                            }
                            Ok(Err(e)) => error!("sidebar owner lookup failed: {e}"),
                            Err(e) => error!("sidebar owner lookup DB error: {e}"),
                        }
                    }
                }
            }
        }

        info!("Background sync stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::services::EventBus;
    use rusqlite::Connection;

    fn setup_db_pool() -> DbPool {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        let read_conn = Connection::open_in_memory().unwrap();
        let (pool, _handle) = DbPool::new(conn, read_conn);
        pool
    }

    #[tokio::test]
    async fn test_background_sync_stops_on_cancellation() {
        let db = setup_db_pool();
        let cancel_token = CancellationToken::new();

        let handle = start_background_sync(
            db,
            "Test-Agent/1.0".to_string(),
            cancel_token.clone(),
            Arc::new(SidebarCache::default()),
            EventBus::new(8),
        );

        // Cancel immediately
        cancel_token.cancel();

        // Worker should stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(
            result.is_ok(),
            "Background sync should stop after cancellation"
        );
    }

    #[tokio::test]
    async fn test_background_sync_with_empty_bucket() {
        let db = setup_db_pool();
        let cancel_token = CancellationToken::new();

        let handle = start_background_sync(
            db,
            "Test-Agent/1.0".to_string(),
            cancel_token.clone(),
            Arc::new(SidebarCache::default()),
            EventBus::new(8),
        );

        // Give worker time to run one tick (interval starts immediately with first tick)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Cancel worker
        cancel_token.cancel();

        // Worker should stop
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        assert!(result.is_ok(), "Background sync should stop gracefully");
    }

    #[test]
    fn test_bucket_calculation() {
        // Verify bucket calculation logic: (timestamp / 60 % 60)
        // Bucket should be 0-59 based on current minute
        let now = Utc::now();
        let bucket = (now.timestamp() / 60 % 60) as u8;
        assert!(bucket < 60, "Bucket should be between 0 and 59");
    }
}
