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

use rusqlite::Connection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::db::DbPool;
use crate::error::AppResult;
use crate::utils::text::strip_to_search_text;

/// Rows backfilled per transaction.
const BATCH_SIZE: usize = 500;

/// Backfill `content_text` for up to `batch_size` legacy entries whose
/// `content_text` is still NULL (rows predating migration v10). Rows with NULL
/// `content` are left NULL — there is nothing to search. Runs in one
/// transaction. Returns the number of rows updated; a value < `batch_size`
/// means the table is drained.
pub fn backfill_content_text_batch(conn: &Connection, batch_size: usize) -> AppResult<usize> {
    // strip_to_search_text joins across tags so terms split by inline markup
    // stay matchable; mirrors the per-entry stripping done on upsert.
    let batch: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, content FROM entry \
             WHERE content_text IS NULL AND content IS NOT NULL LIMIT ?1",
        )?;
        stmt.query_map([batch_size as i64], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(Result::ok)
            .collect()
    };
    if batch.is_empty() {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut upd = tx.prepare_cached("UPDATE entry SET content_text = ?1 WHERE id = ?2")?;
        for (id, content) in &batch {
            upd.execute(rusqlite::params![strip_to_search_text(content), id])?;
        }
    }
    tx.commit()?;
    Ok(batch.len())
}

/// Spawn the one-shot `content_text` backfill worker (see module docs). Drains
/// legacy NULL-`content_text` rows at Background priority so interactive writes
/// preempt between batches, logging progress, then exits. Cancellation stops it
/// mid-drain; remaining rows resume on the next start.
pub fn start_content_text_backfill(db: DbPool, cancel_token: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        let total: i64 = match db
            .background(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM entry WHERE content_text IS NULL AND content IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
            })
            .await
        {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => {
                tracing::error!("content_text backfill: count failed: {}", e);
                return;
            }
            Err(e) => {
                tracing::error!("content_text backfill: DB access failed: {}", e);
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
            let n = match db
                .background(move |conn| backfill_content_text_batch(conn, BATCH_SIZE))
                .await
            {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    tracing::error!("content_text backfill batch failed: {}", e);
                    return;
                }
                Err(e) => {
                    tracing::error!("content_text backfill DB access failed: {}", e);
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
    use crate::db::init_db;
    use std::time::Duration;

    /// Fresh schema with a single feed under user 1 / category 1, ready for
    /// raw `entry` inserts that simulate legacy (NULL `content_text`) rows.
    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO user (id, username, password_hash) VALUES (1, 'u', 'x');
             INSERT INTO category (id, user_id, name) VALUES (1, 1, 'c');
             INSERT INTO feed (id, category_id, url) VALUES (1, 1, 'http://x');",
        )
        .unwrap();
        conn
    }

    /// Insert a legacy entry (content set, `content_text` left NULL).
    fn insert_legacy(conn: &Connection, id: i64, content: Option<&str>) {
        conn.execute(
            "INSERT INTO entry (id, feed_id, guid, content) VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![id, format!("g{id}"), content],
        )
        .unwrap();
    }

    fn content_text(conn: &Connection, id: i64) -> Option<String> {
        conn.query_row("SELECT content_text FROM entry WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn setup_pool_with(conn: Connection) -> DbPool {
        let read_conn = Connection::open_in_memory().unwrap();
        let (pool, _h) = DbPool::new(conn, read_conn);
        pool
    }

    #[test]
    fn test_backfill_batch_fills_null_content_text() {
        let conn = setup_conn();
        insert_legacy(&conn, 1, Some("超<b>少女</b>"));
        let n = backfill_content_text_batch(&conn, 500).unwrap();
        assert_eq!(n, 1);
        // strip_to_search_text joins across inline tags: 超少女, not 超 少女.
        assert_eq!(content_text(&conn, 1).as_deref(), Some("超少女"));
    }

    #[test]
    fn test_backfill_batch_skips_null_content() {
        let conn = setup_conn();
        insert_legacy(&conn, 1, None);
        let n = backfill_content_text_batch(&conn, 500).unwrap();
        assert_eq!(n, 0, "NULL content has nothing to search; must be skipped");
        assert_eq!(content_text(&conn, 1), None);
    }

    #[test]
    fn test_backfill_batch_respects_limit() {
        let conn = setup_conn();
        for id in 1..=3 {
            insert_legacy(&conn, id, Some("<b>x</b>"));
        }
        assert_eq!(backfill_content_text_batch(&conn, 2).unwrap(), 2);
        assert_eq!(backfill_content_text_batch(&conn, 2).unwrap(), 1);
        assert_eq!(
            backfill_content_text_batch(&conn, 2).unwrap(),
            0,
            "table drained: further batches are no-ops"
        );
    }

    #[tokio::test]
    async fn test_worker_backfills_all_rows() {
        let conn = setup_conn();
        insert_legacy(&conn, 1, Some("<b>hello</b>"));
        insert_legacy(&conn, 2, Some("超<i>少女</i>"));
        let db = setup_pool_with(conn);

        let handle = start_content_text_backfill(db.clone(), CancellationToken::new());
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("backfill worker should finish")
            .unwrap();

        let (a, b) = db
            .user(|c| (content_text(c, 1), content_text(c, 2)))
            .await
            .unwrap();
        assert_eq!(a.as_deref(), Some("hello"));
        assert_eq!(b.as_deref(), Some("超少女"));
    }

    #[tokio::test]
    async fn test_worker_cancelled_before_drain_leaves_rows() {
        let conn = setup_conn();
        insert_legacy(&conn, 1, Some("<b>hello</b>"));
        let db = setup_pool_with(conn);

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

        let text = db.user(|c| content_text(c, 1)).await.unwrap();
        assert_eq!(text, None, "cancelled-before-drain must not backfill");
    }
}
