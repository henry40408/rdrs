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
        tracing::info!(
            event = "retention.worker_started",
            interval_hours,
            "retention worker started"
        );
        let mut interval = tokio::time::interval(Duration::from_secs(interval_hours * 3600));

        loop {
            tokio::select! {
                () = cancel_token.cancelled() => {
                    tracing::info!(event = "retention.worker_stopping", "retention worker stopping");
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
                            Err(e) => {
                                tracing::error!(event = "retention.prune_failed", error = %e, "retention prune failed");
                                break;
                            }
                        };
                        total += deleted;
                        if deleted < BATCH_SIZE as u64 {
                            break;
                        }
                    }

                    if total > 0 {
                        tracing::info!(event = "retention.pruned", count = total, "retention pruned read entries");
                        match run_maintenance(&db).await {
                            Ok(true) => tracing::info!(event = "retention.vacuumed", "retention maintenance ran VACUUM"),
                            Ok(false) => {}
                            Err(e) => tracing::error!(event = "retention.maintenance_failed", error = %e, "retention maintenance failed"),
                        }
                    } else if let Err(e) = db.optimize().await {
                        // Statistics go stale as the table grows, which has
                        // nothing to do with whether a prune found anything to
                        // delete — and on a deployment where nobody opted into
                        // retention, `total` is always 0, so gating the refresh
                        // on it means the planner never gets fresh numbers. Only
                        // the freelist-driven VACUUM genuinely needs the prune to
                        // have run, so that half stays inside `run_maintenance`.
                        tracing::error!(event = "retention.optimize_failed", error = %e, "planner statistics refresh failed");
                    }
                }
            }
        }

        tracing::info!(
            event = "retention.worker_stopped",
            "retention worker stopped"
        );
    })
}

/// Post-prune maintenance: refresh planner stats, gated full VACUUM, truncating
/// WAL checkpoint. Returns whether a VACUUM ran. Must run outside a transaction.
pub async fn run_maintenance(db: &Db) -> AppResult<bool> {
    db.optimize().await.map_err(AppError::Database)?;

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

    /// Regression: the statistics refresh used to sit inside the `total > 0`
    /// branch, so a deployment where nobody opted into retention — `total` is
    /// then always 0 — never got one, and its planner statistics aged
    /// indefinitely. Only the freelist-driven VACUUM depends on a prune having
    /// happened; the refresh does not.
    ///
    /// An index with no `sqlite_stat1` row is what `PRAGMA optimize` acts on, so
    /// that is what this leaves for the tick to find. File-backed because
    /// `Db::connect` already refreshes on open, so the index has to be created
    /// afterwards and survive on disk.
    #[tokio::test]
    async fn test_worker_refreshes_statistics_without_a_prune() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sqlite3");
        let db = Db::connect(path.to_str().unwrap(), crate::config::Backend::Sqlite)
            .await
            .unwrap();

        let crate::db::DbInner::Sqlite(pool) = db.inner() else {
            unreachable!("connected with Backend::Sqlite")
        };
        for stmt in [
            "CREATE TABLE probe (id INTEGER PRIMARY KEY, k INTEGER)",
            "CREATE INDEX probe_k ON probe(k)",
            "INSERT INTO probe (k) WITH RECURSIVE s(i) AS \
             (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < 200) SELECT i FROM s",
            "ANALYZE",
            "CREATE INDEX probe_k_id ON probe(k, id)",
        ] {
            sqlx::query(stmt).execute(pool).await.unwrap();
        }

        // Nobody opted into retention, so the tick prunes nothing. The interval
        // fires immediately on the first tick, so the refresh lands right away.
        let token = CancellationToken::new();
        let handle = start_retention_worker(db.clone(), 1000, token.clone());

        let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let stat: Option<String> =
                    sqlx::query_scalar("SELECT stat FROM sqlite_stat1 WHERE idx = 'probe_k_id'")
                        .fetch_optional(pool)
                        .await
                        .unwrap();
                if stat.is_some() {
                    return stat;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        token.cancel();
        handle.await.unwrap();

        assert_eq!(
            refreshed.ok().flatten().as_deref(),
            Some("200 1 1"),
            "a tick that pruned nothing must still refresh planner statistics"
        );
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
