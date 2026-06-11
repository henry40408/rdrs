use std::time::Duration;

use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::DbPool;
use crate::error::AppResult;
use crate::models::entry;

/// Entries deleted per transaction during a drain.
const BATCH_SIZE: usize = 500;
/// Run a full VACUUM only when freed pages reach this fraction of the file. A
/// full VACUUM rewrites the whole database under a write lock (~db_size/650
/// seconds), so it is not worth doing for the handful of pages a routine prune
/// frees — only after a large drain.
const VACUUM_FREELIST_RATIO: f64 = 0.20;

/// Start the retention worker. Every `interval_hours` it prunes read+aged
/// +non-starred entries for users who opted in (`user_settings.retention_read_days
/// > 0`), then runs maintenance. A no-op when nobody opted in.
pub fn start_retention_worker(
    db: DbPool,
    interval_hours: u64,
    cancel_token: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
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
                        let deleted = match db
                            .background(move |conn| entry::prune_read_retention_batch(conn, BATCH_SIZE))
                            .await
                        {
                            Ok(Ok(n)) => n,
                            Ok(Err(e)) => { tracing::error!("Retention prune failed: {}", e); break; }
                            Err(e) => { tracing::error!("Retention DB access failed: {}", e); break; }
                        };
                        total += deleted;
                        if deleted < BATCH_SIZE as u64 {
                            break;
                        }
                    }

                    if total > 0 {
                        tracing::info!("Retention pruned {} read entries", total);
                        match db.background(run_maintenance).await {
                            Ok(Ok(true)) => tracing::info!("Retention maintenance: VACUUM ran"),
                            Ok(Ok(false)) => {}
                            Ok(Err(e)) => tracing::error!("Retention maintenance failed: {}", e),
                            Err(e) => tracing::error!("Retention maintenance DB access failed: {}", e),
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
pub fn run_maintenance(conn: &Connection) -> AppResult<bool> {
    conn.execute_batch("PRAGMA optimize;")?;

    let page_count: i64 = conn.pragma_query_value(None, "page_count", |r| r.get(0))?;
    let freelist: i64 = conn.pragma_query_value(None, "freelist_count", |r| r.get(0))?;
    let vacuumed = page_count > 0 && (freelist as f64 / page_count as f64) >= VACUUM_FREELIST_RATIO;
    if vacuumed {
        conn.execute_batch("VACUUM;")?;
    }

    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(vacuumed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::user::Role;
    use crate::models::{category, feed, user, user_settings};
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn setup_pool() -> DbPool {
        let conn = setup_db();
        let read_conn = Connection::open_in_memory().unwrap();
        let (pool, _h) = DbPool::new(conn, read_conn);
        pool
    }

    #[test]
    fn test_run_maintenance_no_vacuum_below_ratio() {
        let conn = setup_db();
        // Fresh DB: ~0 freelist -> no VACUUM, but must not error.
        assert!(!run_maintenance(&conn).unwrap());
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancellation() {
        let db = setup_pool();
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
        let db = setup_pool();
        db.user(|conn| {
            let uid = user::create_user(conn, "u", "h", Role::User).unwrap().id;
            let cid = category::create_category(conn, uid, "C").unwrap().id;
            let fid = feed::create_feed(
                conn,
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
            .unwrap()
            .id;
            entry::upsert_entry_id(conn, fid, "old", Some("o"), None, None, None, None, None)
                .unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now','-40 days') WHERE guid='old' AND feed_id=?1",
                rusqlite::params![fid],
            )
            .unwrap();
            user_settings::update_retention_read_days(conn, uid, 30).unwrap();
        })
        .await
        .unwrap();

        // Simulate one worker tick's drain.
        let deleted: u64 = db
            .background(|conn| entry::prune_read_retention_batch(conn, BATCH_SIZE).unwrap())
            .await
            .unwrap();
        assert_eq!(deleted, 1);
    }
}
