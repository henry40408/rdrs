//! Background backfill of `entry.content_text` for rows predating migration
//! v10. The v10 schema step only adds the (nullable) column so startup is not
//! blocked; this worker fills the plain-text search column for legacy rows
//! asynchronously, at Background DB priority, so interactive requests preempt
//! it between batches. It is a one-shot: it drains and exits. Safe to spawn on
//! every boot — idempotent via the `content_text IS NULL` predicate, so a
//! fully-backfilled table costs a single COUNT. Interrupting it (SIGINT) leaves
//! the remaining rows for the next start.
//!
//! During the drain window, body search over not-yet-filled rows is degraded
//! (title still matches; `content_text` matching becomes available as rows are
//! filled). This is an accepted trade-off for a non-blocking startup.

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::utils::text::strip_to_search_text;
use crate::{db_execute_tx, query_all_tx, query_scalar};

/// Rows backfilled per transaction.
const BATCH_SIZE: usize = 500;

/// Backfill `content_text` for up to `batch_size` legacy entries whose
/// `content_text` is still NULL (rows predating migration v10). Rows with NULL
/// `content` are left NULL — there is nothing to search. Runs in one
/// transaction. Returns the number of rows updated; a value < `batch_size`
/// means the table is drained.
pub async fn backfill_content_text_batch(db: &Db, batch_size: usize) -> AppResult<usize> {
    // strip_to_search_text joins across tags so terms split by inline markup
    // stay matchable; mirrors the per-entry stripping done on upsert.
    let mut tx = db.begin().await?;
    let batch: Vec<(i64, String)> = query_all_tx!(
        &mut tx,
        (i64, String),
        "SELECT id, content FROM entry \
         WHERE content_text IS NULL AND content IS NOT NULL LIMIT $1",
        batch_size as i64
    )
    .map_err(AppError::Database)?;
    if batch.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }
    for (id, content) in &batch {
        db_execute_tx!(
            &mut tx,
            "UPDATE entry SET content_text = $1 WHERE id = $2",
            strip_to_search_text(content),
            *id
        )
        .map_err(AppError::Database)?;
    }
    tx.commit().await?;
    Ok(batch.len())
}

/// Spawn the one-shot `content_text` backfill worker (see module docs). Drains
/// legacy NULL-`content_text` rows at Background priority so interactive writes
/// preempt between batches, logging progress, then exits. Cancellation stops it
/// mid-drain; remaining rows resume on the next start.
pub fn start_content_text_backfill(db: Db, cancel_token: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        let total: i64 = match query_scalar!(
            &db,
            i64,
            "SELECT COUNT(*) FROM entry WHERE content_text IS NULL AND content IS NOT NULL"
        ) {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("content_text backfill: count failed: {}", e);
                return;
            }
        };
        if total == 0 {
            return;
        }
        tracing::info!(
            "content_text backfill started in background for {} legacy entries. \
             Body search over not-yet-filled rows is degraded until it completes.",
            total
        );

        let mut backfilled: i64 = 0;
        loop {
            if cancel_token.is_cancelled() {
                tracing::info!(
                    "content_text backfill interrupted at {}/{} entries; resumes on next start",
                    backfilled,
                    total
                );
                return;
            }
            let n = match backfill_content_text_batch(&db, BATCH_SIZE).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!("content_text backfill batch failed: {}", e);
                    return;
                }
            };
            if n == 0 {
                break;
            }
            backfilled += n as i64;
            tracing::info!(
                "content_text backfill progress {}/{} entries",
                backfilled,
                total
            );
        }
        tracing::info!("content_text backfill complete ({} entries)", backfilled);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_execute;
    use std::time::Duration;

    /// Fresh in-memory `Db` with a single feed under user 1 / category 1, ready
    /// for raw `entry` inserts that simulate legacy (NULL `content_text`) rows.
    async fn setup_db() -> Db {
        let db = Db::connect_in_memory().await.unwrap();
        for stmt in [
            "INSERT INTO \"user\" (id, username, password_hash) VALUES (1, 'u', 'x')",
            "INSERT INTO category (id, user_id, name) VALUES (1, 1, 'c')",
            "INSERT INTO feed (id, category_id, url) VALUES (1, 1, 'http://x')",
        ] {
            db_execute!(&db, stmt).unwrap();
        }
        db
    }

    /// Insert a legacy entry (content set, `content_text` left NULL).
    async fn insert_legacy(db: &Db, id: i64, content: Option<&str>) {
        db_execute!(
            db,
            "INSERT INTO entry (id, feed_id, guid, content) VALUES ($1, 1, $2, $3)",
            id,
            format!("g{id}"),
            content
        )
        .unwrap();
    }

    async fn content_text(db: &Db, id: i64) -> Option<String> {
        query_scalar!(
            db,
            Option<String>,
            "SELECT content_text FROM entry WHERE id = $1",
            id
        )
        .unwrap()
    }

    #[tokio::test]
    async fn test_backfill_batch_fills_null_content_text() {
        let db = setup_db().await;
        insert_legacy(&db, 1, Some("超<b>少女</b>")).await;
        let n = backfill_content_text_batch(&db, 500).await.unwrap();
        assert_eq!(n, 1);
        // strip_to_search_text joins across inline tags: 超少女, not 超 少女.
        assert_eq!(content_text(&db, 1).await.as_deref(), Some("超少女"));
    }

    #[tokio::test]
    async fn test_backfill_batch_skips_null_content() {
        let db = setup_db().await;
        insert_legacy(&db, 1, None).await;
        let n = backfill_content_text_batch(&db, 500).await.unwrap();
        assert_eq!(n, 0, "NULL content has nothing to search; must be skipped");
        assert_eq!(content_text(&db, 1).await, None);
    }

    #[tokio::test]
    async fn test_backfill_batch_respects_limit() {
        let db = setup_db().await;
        for id in 1..=3 {
            insert_legacy(&db, id, Some("<b>x</b>")).await;
        }
        assert_eq!(backfill_content_text_batch(&db, 2).await.unwrap(), 2);
        assert_eq!(backfill_content_text_batch(&db, 2).await.unwrap(), 1);
        assert_eq!(
            backfill_content_text_batch(&db, 2).await.unwrap(),
            0,
            "table drained: further batches are no-ops"
        );
    }

    #[tokio::test]
    async fn test_worker_backfills_all_rows() {
        let db = setup_db().await;
        insert_legacy(&db, 1, Some("<b>hello</b>")).await;
        insert_legacy(&db, 2, Some("超<i>少女</i>")).await;

        let handle = start_content_text_backfill(db.clone(), CancellationToken::new());
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("backfill worker should finish")
            .unwrap();

        let a = content_text(&db, 1).await;
        let b = content_text(&db, 2).await;
        assert_eq!(a.as_deref(), Some("hello"));
        assert_eq!(b.as_deref(), Some("超少女"));
    }

    #[tokio::test]
    async fn test_worker_cancelled_before_drain_leaves_rows() {
        let db = setup_db().await;
        insert_legacy(&db, 1, Some("<b>hello</b>")).await;

        // Pre-cancelled token: the worker takes its initial COUNT (total > 0),
        // then the first loop iteration sees cancellation and returns before
        // any batch runs. The legacy row stays NULL and resumes on next boot.
        let token = CancellationToken::new();
        token.cancel();
        let handle = start_content_text_backfill(db.clone(), token);
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("cancelled worker should return promptly")
            .unwrap();

        let text = content_text(&db, 1).await;
        assert_eq!(text, None, "cancelled-before-drain must not backfill");
    }
}
