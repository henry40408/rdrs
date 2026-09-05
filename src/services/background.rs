use std::sync::Arc;

use chrono::Utc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::feed_sync;
use super::sidebar_cache::SidebarCache;
use crate::db::Db;
use crate::models::feed;
use crate::services::EventBus;
use crate::utils::url_validation::FetchPolicy;

pub fn start_background_sync(
    db: Db,
    user_agent: String,
    fetch_allow_private: FetchPolicy,
    cancel_token: CancellationToken,
    sidebar_cache: Arc<SidebarCache>,
    events: EventBus,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Background priority: DB operations yield to interactive work on SQLite.
        let db = db.background();
        info!(
            event = "sync.worker_started",
            "background sync task started"
        );

        let mut ticker = interval(Duration::from_secs(60));

        loop {
            tokio::select! {
                () = cancel_token.cancelled() => {
                    info!(event = "sync.worker_stopping", "background sync stopping");
                    break;
                }
                _ = ticker.tick() => {
                    let now = Utc::now();
                    #[allow(
                        clippy::cast_sign_loss,
                        reason = "current unix time is positive, so `timestamp()/60 % 60` is in 0..=59"
                    )]
                    let bucket = (now.timestamp() / 60 % 60) as u8;

                    debug!(event = "sync.tick", bucket, "running background sync for bucket");

                    let results = feed_sync::refresh_bucket(db.clone(), bucket, &user_agent, &fetch_allow_private)
                        .await;

                    let success_count = results.iter().filter(|(_, r)| r.is_ok()).count();
                    let fail_count = results.iter().filter(|(_, r)| r.is_err()).count();

                    if !results.is_empty() {
                        info!(
                            event = "sync.bucket_finished",
                            bucket,
                            succeeded = success_count,
                            failed = fail_count,
                            "background sync bucket finished"
                        );
                    }

                    for (feed_id, result) in &results {
                        if let Err(e) = result {
                            error!(event = "feed.sync_failed", feed_id, error = %e, "background sync failed for feed");
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
                        match feed::owner_user_ids_for_feeds(&db, &changed_feed_ids).await {
                            Ok(user_ids) => {
                                for uid in user_ids {
                                    sidebar_cache.bust(uid);
                                    events.emit_sidebar(uid);
                                }
                            }
                            Err(e) => error!(event = "sync.owner_lookup_failed", error = %e, "sidebar owner lookup failed"),
                        }
                    }
                }
            }
        }

        info!(event = "sync.worker_stopped", "background sync stopped");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::EventBus;

    async fn setup_db_pool() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_background_sync_stops_on_cancellation() {
        let db = setup_db_pool().await;
        let cancel_token = CancellationToken::new();

        let handle = start_background_sync(
            db,
            "Test-Agent/1.0".to_string(),
            FetchPolicy::default(),
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
        let db = setup_db_pool().await;
        let cancel_token = CancellationToken::new();

        let handle = start_background_sync(
            db,
            "Test-Agent/1.0".to_string(),
            FetchPolicy::default(),
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
        #[allow(
            clippy::cast_sign_loss,
            reason = "current unix time is positive, so `timestamp()/60 % 60` is in 0..=59"
        )]
        let bucket = (now.timestamp() / 60 % 60) as u8;
        assert!(bucket < 60, "Bucket should be between 0 and 59");
    }
}
