use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::models::entry;
use crate::{db_execute, query_scalar};

/// Entries deleted per transaction during a drain.
const BATCH_SIZE: usize = 500;
/// Run a full VACUUM only when freed pages reach this fraction of the file. A
/// full VACUUM rewrites the whole database under a write lock (~`db_size/650`
/// seconds), so it is not worth doing for the handful of pages a routine prune
/// frees — only after a large drain.
const VACUUM_FREELIST_RATIO: f64 = 0.20;

/// Start the retention worker. Every `interval_hours` it prunes read, aged,
/// non-starred entries for users who opted in (those with
/// `user_settings.retention_read_days > 0`), then runs maintenance. A no-op
/// when nobody opted in.
pub fn start_retention_worker(
    db: Db,
    interval_hours: u64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Background priority: DB operations yield to interactive work on SQLite.
        let db = db.background();
        tracing::info!("Retention worker started: interval={}h", interval_hours);
        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    tracing::info!("Retention worker stopping...");
                    break;
                }
                _ = interval.tick() => {
                    let mut total = 0u64;
                    loop {
                        if cancel_token.is_cancelled() {
                            break;
                        }
                        let deleted = match entry::prune_read_retention_batch(&db, BATCH_SIZE).await {
                            Ok(n) => n,
                            Err(e) => { tracing::error!("Retention prune failed: {}", e); break; }
                        };
                        total += deleted;
                        if deleted < BATCH_SIZE as u64 {
                            break;
                        }
                    }

                    if total > 0 {
                        tracing::info!("Retention pruned {} read entries", total);
                        match run_maintenance(&db).await {
                            Ok(true) => tracing::info!("Retention maintenance: VACUUM ran"),
                            Ok(false) => {}
                            Err(e) => tracing::error!("Retention maintenance failed: {}", e),
                        }
                    }
                }
            }
        }

        tracing::info!("Retention worker stopped");
    })
}

/// Post-prune maintenance: refresh planner stats, gated full VACUUM, truncating
/// WAL checkpoint. Returns whether a VACUUM ran. Must run outside a transaction.
pub async fn run_maintenance(db: &Db) -> AppResult<bool> {
    db_execute!(db, "PRAGMA optimize;").map_err(AppError::Database)?;

    let page_count: i64 =
        query_scalar!(db, i64, "PRAGMA page_count;").map_err(AppError::Database)?;
    let freelist: i64 =
        query_scalar!(db, i64, "PRAGMA freelist_count;").map_err(AppError::Database)?;
    let vacuumed = page_count > 0 && (freelist as f64 / page_count as f64) >= VACUUM_FREELIST_RATIO;
    if vacuumed {
        db_execute!(db, "VACUUM;").map_err(AppError::Database)?;
    }

    db_execute!(db, "PRAGMA wal_checkpoint(TRUNCATE);").map_err(AppError::Database)?;
    Ok(vacuumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::user::Role;
    use crate::models::{category, feed, user, user_settings};

    async fn setup_pool() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    #[tokio::test]
    async fn test_run_maintenance_no_vacuum_below_ratio() {
        let db = setup_pool().await;
        // Fresh DB: ~0 freelist -> no VACUUM, but must not error.
        assert!(!run_maintenance(&db).await.unwrap());
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancellation() {
        let db = setup_pool().await;
        let token = CancellationToken::new();
        let handle = start_retention_worker(db, 1000, token.clone());
        token.cancel();
        let res = tokio::time::timeout(Duration::from_secs(5), handle).await;
        assert!(
            res.is_ok(),
            "retention worker should stop after cancellation"
        );
    }

    #[tokio::test]
    async fn test_drain_deletes_opted_in_aged_read_entries() {
        let db = setup_pool().await;
        let uid = user::create_user(&db, "u", "h", Role::User)
            .await
            .unwrap()
            .id;
        let cid = category::create_category(&db, uid, "C").await.unwrap().id;
        let fid = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cid,
                url: "https://e.com/f.xml",
                title: Some("F"),
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
        entry::upsert_entry_id(&db, fid, "old", Some("o"), None, None, None, None, None)
            .await
            .unwrap();
        db_execute!(
            &db,
            "UPDATE entry SET read_at = datetime('now','-40 days') WHERE guid='old' AND feed_id=$1",
            fid,
        )
        .unwrap();
        user_settings::update_retention_read_days(&db, uid, 30)
            .await
            .unwrap();

        // Simulate one worker tick's drain.
        let deleted = entry::prune_read_retention_batch(&db, BATCH_SIZE)
            .await
            .unwrap();
        assert_eq!(deleted, 1);
    }
}
