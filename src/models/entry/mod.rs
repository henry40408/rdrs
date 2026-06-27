use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::utils::datetime::parse_datetime;

mod filters;
use filters::{
    apply_continuation_condition, apply_filter_conditions, apply_time_conditions,
    published_sort_entry_hint,
};
// Only the unit tests exercise this predicate directly; production code reaches
// it through `published_sort_entry_hint`.
#[cfg(test)]
use filters::is_no_entry_side_predicate;

/// Sort order for entries
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntrySortOrder {
    #[default]
    PublishedAt, // COALESCE(published_at, created_at) DESC
    ReadAt,    // read_at DESC
    StarredAt, // starred_at DESC
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: Option<String>,
    pub link: Option<String>,
    pub content: Option<String>,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub starred_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryWithFeed {
    #[serde(flatten)]
    pub entry: Entry,
    pub feed_title: Option<String>,
    pub feed_url: String,
    pub site_url: Option<String>,
    pub category_id: i64,
    pub category_name: String,
    pub feed_has_icon: bool,
    pub custom_referrer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EntryFilter {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    pub unread_only: bool,
    pub starred_only: bool,
    pub read_only: bool,
    pub search: Option<String>,
    pub has_summary: Option<bool>,
    /// Snapshot boundary for the unread filter — a UTC `YYYY-MM-DD HH:MM:SS`
    /// string, the same format `datetime('now')` writes into `entry.read_at`.
    /// When set together with `unread_only`, entries read at-or-after this
    /// instant still count as unread, so reading-pane navigation can return
    /// to entries the reader just finished during this page view. Ignored
    /// when `unread_only` is false.
    pub read_after: Option<String>,
}

/// Pagination cursor. The wire format on the API is opaque to clients; we
/// emit the new composite form `<iso_8601_ts>|<id>` and accept the legacy
/// bare-`i64` form as a one-time grace path for in-flight cursors that may
/// still live in browser URLs/JS state at deploy time.
#[derive(Debug, Clone)]
pub enum ContinuationCursor {
    /// New `(sort_ts, id)` composite. `sort_ts` is the entry's sort-field
    /// value as TEXT (the same byte-string SQLite stores), so the predicate
    /// compares against an indexed column without conversion.
    Composite { sort_ts: String, id: i64 },
    /// Legacy `e.id < ?` cursor — accepted on input only; emitted only by
    /// pre-#164 clients.
    LegacyId(i64),
}

impl ContinuationCursor {
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        if let Some((ts, id)) = s.split_once('|') {
            if ts.is_empty() {
                return None;
            }
            id.parse::<i64>().ok().map(|id| Self::Composite {
                sort_ts: ts.to_string(),
                id,
            })
        } else {
            s.parse::<i64>().ok().map(Self::LegacyId)
        }
    }

    pub fn encode_composite(sort_ts: &str, id: i64) -> String {
        format!("{}|{}", sort_ts, id)
    }
}

/// Parameters for continuation-based pagination (Google Reader style).
#[derive(Debug, Clone, Default)]
pub struct ContinuationParams {
    pub oldest_first: bool,
    pub limit: i64,
    pub continuation: Option<ContinuationCursor>,
    /// Oldest timestamp (seconds since epoch)
    pub ot: Option<i64>,
    /// Newest timestamp (seconds since epoch)
    pub nt: Option<i64>,
    /// Sort order (default: PublishedAt)
    pub sort_order: EntrySortOrder,
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<Entry> {
    let published_at: Option<String> = row.get(8)?;
    let read_at: Option<String> = row.get(9)?;
    let starred_at: Option<String> = row.get(10)?;
    let created_at: String = row.get(11)?;
    let updated_at: String = row.get(12)?;

    Ok(Entry {
        id: row.get(0)?,
        feed_id: row.get(1)?,
        guid: row.get(2)?,
        title: row.get(3)?,
        link: row.get(4)?,
        content: row.get(5)?,
        summary: row.get(6)?,
        author: row.get(7)?,
        published_at: published_at.map(|s| parse_datetime(&s)),
        read_at: read_at.map(|s| parse_datetime(&s)),
        starred_at: starred_at.map(|s| parse_datetime(&s)),
        created_at: parse_datetime(&created_at),
        updated_at: parse_datetime(&updated_at),
    })
}

fn row_to_entry_with_feed(row: &rusqlite::Row) -> rusqlite::Result<EntryWithFeed> {
    let published_at: Option<String> = row.get(8)?;
    let read_at: Option<String> = row.get(9)?;
    let starred_at: Option<String> = row.get(10)?;
    let created_at: String = row.get(11)?;
    let updated_at: String = row.get(12)?;
    let has_icon: i64 = row.get(18)?;

    Ok(EntryWithFeed {
        entry: Entry {
            id: row.get(0)?,
            feed_id: row.get(1)?,
            guid: row.get(2)?,
            title: row.get(3)?,
            link: row.get(4)?,
            content: row.get(5)?,
            summary: row.get(6)?,
            author: row.get(7)?,
            published_at: published_at.map(|s| parse_datetime(&s)),
            read_at: read_at.map(|s| parse_datetime(&s)),
            starred_at: starred_at.map(|s| parse_datetime(&s)),
            created_at: parse_datetime(&created_at),
            updated_at: parse_datetime(&updated_at),
        },
        feed_title: row.get(13)?,
        feed_url: row.get(14)?,
        site_url: row.get(15)?,
        category_id: row.get(16)?,
        category_name: row.get(17)?,
        feed_has_icon: has_icon > 0,
        custom_referrer: row.get(19)?,
    })
}

const SELECT_COLUMNS: &str = "id, feed_id, guid, title, link, content, summary, author, published_at, read_at, starred_at, created_at, updated_at";

/// Full SELECT column list for `EntryWithFeed` rows, in the exact order
/// `row_to_entry_with_feed` reads (columns 0–19). The `has_icon` column (18) is
/// computed two ways depending on the query shape; this variant uses a
/// correlated COUNT subquery, for queries that do NOT `LEFT JOIN image`. Keep
/// both variants and the mapper in sync.
const ENTRY_WITH_FEED_COLUMNS_COUNT: &str = "e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author, e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at, f.title, f.url, f.site_url, c.id, c.name, (SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) as has_icon, f.custom_referrer";

/// Same columns as [`ENTRY_WITH_FEED_COLUMNS_COUNT`] but computes `has_icon`
/// from a `LEFT JOIN image i` already present in the query.
const ENTRY_WITH_FEED_COLUMNS_JOIN: &str = "e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author, e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at, f.title, f.url, f.site_url, c.id, c.name, CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon, f.custom_referrer";

pub fn find_by_id(conn: &Connection, id: i64) -> AppResult<Option<Entry>> {
    conn.query_row(
        &format!("SELECT {} FROM entry WHERE id = ?1", SELECT_COLUMNS),
        params![id],
        row_to_entry,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn find_by_id_with_feed(conn: &Connection, id: i64) -> AppResult<Option<EntryWithFeed>> {
    conn.query_row(
        &format!(
            r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_COUNT}
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE e.id = ?1
        "#
        ),
        params![id],
        row_to_entry_with_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

/// Fetch a single entry by id, scoped to a specific user via the feed→category
/// ownership join. Returns `None` if the entry does not exist or belongs to a
/// different user (callers should treat both as 404).
pub fn find_by_id_for_user(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntryWithFeed>> {
    conn.query_row(
        &format!(
            r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN}
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE e.id = ?1 AND c.user_id = ?2
        "#
        ),
        params![entry_id, user_id],
        row_to_entry_with_feed,
    )
    .optional()
    .map_err(AppError::Database)
}

/// Fetch the sort-field value (as the exact TEXT string SQLite stores) for
/// emitting a composite cursor. Returns `None` if the entry doesn't exist.
pub fn fetch_sort_ts(
    conn: &Connection,
    entry_id: i64,
    sort_order: EntrySortOrder,
) -> AppResult<Option<String>> {
    let column_expr = match sort_order {
        EntrySortOrder::ReadAt => "read_at",
        EntrySortOrder::StarredAt => "starred_at",
        EntrySortOrder::PublishedAt => "COALESCE(published_at, created_at)",
    };
    let sql = format!("SELECT {} FROM entry WHERE id = ?1", column_expr);
    conn.query_row(&sql, params![entry_id], |row| {
        row.get::<_, Option<String>>(0)
    })
    .optional()
    .map(|opt| opt.flatten())
    .map_err(AppError::Database)
}

pub fn find_by_guid_and_feed(
    conn: &Connection,
    guid: &str,
    feed_id: i64,
) -> AppResult<Option<Entry>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM entry WHERE guid = ?1 AND feed_id = ?2",
            SELECT_COLUMNS
        ),
        params![guid, feed_id],
        row_to_entry,
    )
    .optional()
    .map_err(AppError::Database)
}

pub fn list_by_feed(
    conn: &Connection,
    feed_id: i64,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<Entry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM entry WHERE feed_id = ?1 ORDER BY COALESCE(published_at, created_at) DESC LIMIT ?2 OFFSET ?3",
        SELECT_COLUMNS
    ))?;

    let entries = stmt
        .query_map(params![feed_id, limit, offset], row_to_entry)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

pub fn list_by_user(
    conn: &Connection,
    user_id: i64,
    filter: &EntryFilter,
    sort_order: EntrySortOrder,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<EntryWithFeed>> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    apply_filter_conditions(&mut conditions, &mut params_vec, filter);

    let where_clause = conditions.join(" AND ");

    let order_by = match sort_order {
        EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at) DESC",
        EntrySortOrder::ReadAt => "e.read_at DESC",
        EntrySortOrder::StarredAt => "e.starred_at DESC",
    };

    // Force the right entry index for the high-traffic list pages. The
    // SQLite planner otherwise picks `category -> feed -> entry` and walks
    // every row before sorting (worst case for a single-user instance that
    // owns 100% of entries). The hint only applies to the published-order
    // sort; the read_at / starred_at sorts have their own dedicated indexes.
    let entry_hint = if sort_order == EntrySortOrder::PublishedAt {
        published_sort_entry_hint(filter)
    } else {
        ""
    };

    let sql = format!(
        r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN}
        FROM entry e{}
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE {}
        ORDER BY {}
        LIMIT ?{} OFFSET ?{}
        "#,
        entry_hint,
        where_clause,
        order_by,
        params_vec.len() + 1,
        params_vec.len() + 2
    );

    params_vec.push(Box::new(limit));
    params_vec.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let entries = stmt
        .query_map(params_refs.as_slice(), row_to_entry_with_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

pub fn count_by_user(conn: &Connection, user_id: i64, filter: &EntryFilter) -> AppResult<i64> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    apply_filter_conditions(&mut conditions, &mut params_vec, filter);

    let where_clause = conditions.join(" AND ");

    let sql = format!(
        r#"
        SELECT COUNT(*)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE {}
        "#,
        where_clause
    );

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let count: i64 = conn.query_row(&sql, params_refs.as_slice(), |row| row.get(0))?;

    Ok(count)
}

pub fn count_unread_by_user(conn: &Connection, user_id: i64) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1 AND e.read_at IS NULL
        "#,
        params![user_id],
        |row| row.get(0),
    )?;

    Ok(count)
}

/// Returns a map of feed_id -> unread count for a user
pub fn count_unread_by_feed(
    conn: &Connection,
    user_id: i64,
) -> AppResult<std::collections::HashMap<i64, i64>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT f.id, COUNT(e.id)
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN entry e INDEXED BY idx_entry_unread_feed
            ON e.feed_id = f.id AND e.read_at IS NULL
        WHERE c.user_id = ?1
        GROUP BY f.id
        "#,
    )?;

    let rows = stmt.query_map(params![user_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (feed_id, count) = row?;
        map.insert(feed_id, count);
    }

    Ok(map)
}

/// Returns a map of category_id -> unread count for a user
pub fn count_unread_by_category(
    conn: &Connection,
    user_id: i64,
) -> AppResult<std::collections::HashMap<i64, i64>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.id, COUNT(e.id)
        FROM category c
        LEFT JOIN feed f ON f.category_id = c.id
        LEFT JOIN entry e INDEXED BY idx_entry_unread_feed
            ON e.feed_id = f.id AND e.read_at IS NULL
        WHERE c.user_id = ?1
        GROUP BY c.id
        "#,
    )?;

    let rows = stmt.query_map(params![user_id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (category_id, count) = row?;
        map.insert(category_id, count);
    }

    Ok(map)
}

/// Result of an entry upsert. The insert path is guarded against tombstones,
/// so a third "skipped" state exists alongside insert/update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted(i64),
    Updated(i64),
    SkippedTombstoned,
}

/// Upsert an entry, returning [`UpsertOutcome`] without re-reading the full row.
///
/// This is the lean variant used by the feed-sync hot loop, which only needs
/// the outcome flag. It avoids the full-row `find_by_id` re-read that
/// [`upsert_entry`] performs, looks the existing row up by `id` only, and uses
/// `prepare_cached` so the three hot statements are compiled once per
/// connection rather than once per entry. Wrap a sync loop in a single
/// transaction (see `feed_sync`) to collapse the per-entry commits.
#[allow(clippy::too_many_arguments)]
pub fn upsert_entry_id(
    conn: &Connection,
    feed_id: i64,
    guid: &str,
    title: Option<&str>,
    link: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    author: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<UpsertOutcome> {
    let published_at_str = published_at.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());

    // Look up only the id of any existing row (not the full record).
    let existing: Option<i64> = conn
        .prepare_cached("SELECT id FROM entry WHERE guid = ?1 AND feed_id = ?2")?
        .query_row(params![guid, feed_id], |row| row.get(0))
        .optional()?;

    if let Some(id) = existing {
        // Update existing entry (preserve read_at, starred_at, and published_at)
        // We don't update published_at because:
        // 1. The published date shouldn't change for existing entries
        // 2. Some feeds don't provide dates, causing fallback to current time on each refresh
        conn.prepare_cached(
            r#"
            UPDATE entry
            SET title = ?1, link = ?2, content = ?3, summary = ?4, author = ?5,
                updated_at = datetime('now')
            WHERE id = ?6
            "#,
        )?
        .execute(params![title, link, content, summary, author, id])?;

        return Ok(UpsertOutcome::Updated(id));
    }

    // Insert, but never resurrect a tombstoned guid. The WHERE NOT EXISTS makes
    // the tombstone check atomic with the insert, so a retention delete that
    // commits between a separate check and this statement cannot bring the
    // entry back as unread.
    let inserted = conn
        .prepare_cached(
            r#"
            INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at)
            SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
            WHERE NOT EXISTS (
                SELECT 1 FROM entry_tombstone WHERE feed_id = ?1 AND guid = ?2
            )
            "#,
        )?
        .execute(params![
            feed_id,
            guid,
            title,
            link,
            content,
            summary,
            author,
            published_at_str
        ])?;

    if inserted == 0 {
        return Ok(UpsertOutcome::SkippedTombstoned);
    }
    Ok(UpsertOutcome::Inserted(conn.last_insert_rowid()))
}

/// Idempotent `(feed_id, guid)` tombstone insert. Shared by the single-shot
/// [`insert_tombstone`] helper and the batched `prune_read_retention_batch`
/// loop so the statement text lives in exactly one place.
const INSERT_TOMBSTONE_SQL: &str = "INSERT INTO entry_tombstone (feed_id, guid) VALUES (?1, ?2)
     ON CONFLICT(feed_id, guid) DO NOTHING";

/// Record a tombstone for `(feed_id, guid)`. Idempotent.
pub fn insert_tombstone(conn: &Connection, feed_id: i64, guid: &str) -> AppResult<()> {
    conn.execute(INSERT_TOMBSTONE_SQL, params![feed_id, guid])?;
    Ok(())
}

/// Delete up to `batch_size` read, aged, non-starred entries belonging to users
/// who have opted into retention (`user_settings.retention_read_days > 0`),
/// recording a tombstone for each. Returns the number of entries deleted.
///
/// One batch runs in a single transaction so the tombstone+delete pair is
/// atomic against a concurrent feed refresh. Victims are gathered first (Rust
/// side) so the delete targets exact ids rather than re-running a `LIMIT`
/// without `ORDER BY`. Each user's own threshold is applied via the join.
pub fn prune_read_retention_batch(conn: &Connection, batch_size: usize) -> AppResult<u64> {
    let tx = conn.unchecked_transaction()?;

    let victims: Vec<(i64, i64, String)> = {
        let mut stmt = tx.prepare_cached(
            r#"
            SELECT e.id, e.feed_id, e.guid
            FROM entry e
            JOIN feed f           ON f.id = e.feed_id
            JOIN category c       ON c.id = f.category_id
            JOIN user_settings us ON us.user_id = c.user_id
            WHERE us.retention_read_days > 0
              AND e.read_at    IS NOT NULL
              AND e.starred_at IS NULL
              AND e.read_at < datetime('now', '-' || us.retention_read_days || ' days')
            LIMIT ?1
            "#,
        )?;

        stmt.query_map(params![batch_size as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    if victims.is_empty() {
        return Ok(0);
    }

    {
        let mut ins = tx.prepare_cached(INSERT_TOMBSTONE_SQL)?;
        let mut del = tx.prepare_cached("DELETE FROM entry WHERE id = ?1")?;
        for (id, feed_id, guid) in &victims {
            ins.execute(params![feed_id, guid])?;
            del.execute(params![id])?;
        }
    }

    tx.commit()?;
    Ok(victims.len() as u64)
}

/// Upsert an entry and return the resulting [`Entry`] plus whether it was new.
///
/// Thin wrapper over [`upsert_entry_id`] for callers that need the full record
/// (tests, summary worker/cleanup seeding). The feed-sync hot path should call
/// [`upsert_entry_id`] directly to skip the extra full-row read this performs.
#[allow(clippy::too_many_arguments)]
pub fn upsert_entry(
    conn: &Connection,
    feed_id: i64,
    guid: &str,
    title: Option<&str>,
    link: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    author: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<(Entry, bool)> {
    let (id, is_new) = match upsert_entry_id(
        conn,
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at,
    )? {
        UpsertOutcome::Inserted(id) => (id, true),
        UpsertOutcome::Updated(id) => (id, false),
        UpsertOutcome::SkippedTombstoned => {
            return Err(AppError::Internal(
                "upsert_entry called for a tombstoned guid".to_string(),
            ));
        }
    };
    let entry = find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)?;
    Ok((entry, is_new))
}

pub fn mark_as_read(conn: &Connection, id: i64) -> AppResult<Entry> {
    let rows = conn.execute(
        "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1 AND read_at IS NULL",
        params![id],
    )?;

    if rows == 0 {
        // Entry might already be read or not exist
        if find_by_id(conn, id)?.is_none() {
            return Err(AppError::EntryNotFound);
        }
    }

    find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)
}

pub fn mark_as_unread(conn: &Connection, id: i64) -> AppResult<Entry> {
    let rows = conn.execute(
        "UPDATE entry SET read_at = NULL, updated_at = datetime('now') WHERE id = ?1",
        params![id],
    )?;

    if rows == 0 {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)
}

/// Explicitly star an entry (set starred_at if not already set).
pub fn star_entry(conn: &Connection, id: i64) -> AppResult<Entry> {
    let rows = conn.execute(
        "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1 AND starred_at IS NULL",
        params![id],
    )?;

    if rows == 0 && find_by_id(conn, id)?.is_none() {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)
}

/// Explicitly unstar an entry (clear starred_at).
pub fn unstar_entry(conn: &Connection, id: i64) -> AppResult<Entry> {
    let rows = conn.execute(
        "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = ?1",
        params![id],
    )?;

    if rows == 0 {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)
}

pub fn toggle_star(conn: &Connection, id: i64) -> AppResult<Entry> {
    let entry = find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)?;

    if entry.starred_at.is_some() {
        conn.execute(
            "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
    } else {
        conn.execute(
            "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![id],
        )?;
    }

    find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)
}

/// Set the starred state for an entry, scoped to the owning user. Idempotent
/// — a no-op if the entry is already in the desired state. Returns the
/// resulting `EntryWithFeed` plus a `changed` bool (parallels
/// `set_read_for_user`). `None` when the entry does not exist or belongs to
/// a different user (callers treat both as 404).
pub fn set_starred_for_user(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    desired_starred: bool,
) -> AppResult<Option<(EntryWithFeed, bool)>> {
    let cur = find_by_id_for_user(conn, user_id, entry_id)?;
    let Some(e) = cur else {
        return Ok(None);
    };
    let was_starred = e.entry.starred_at.is_some();
    let changed = was_starred != desired_starred;
    if changed {
        let sql = if desired_starred {
            "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1"
        } else {
            "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = ?1"
        };
        conn.execute(sql, params![entry_id])?;
    }
    Ok(find_by_id_for_user(conn, user_id, entry_id)?.map(|ewf| (ewf, changed)))
}

/// Set the read state for an entry, scoped to the owning user. Idempotent —
/// a no-op if the entry is already in the desired state. Returns the
/// resulting `EntryWithFeed` (or `None` if the entry does not exist or
/// belongs to a different user), plus a bool indicating whether the call
/// actually changed state (used by handlers to decide whether to emit a
/// flash toast).
pub fn set_read_for_user(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    desired_read: bool,
) -> AppResult<Option<(EntryWithFeed, bool)>> {
    let cur = find_by_id_for_user(conn, user_id, entry_id)?;
    let Some(e) = cur else {
        return Ok(None);
    };
    let was_read = e.entry.read_at.is_some();
    let changed = was_read != desired_read;
    if changed {
        let sql = if desired_read {
            "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1"
        } else {
            "UPDATE entry SET read_at = NULL, updated_at = datetime('now') WHERE id = ?1"
        };
        conn.execute(sql, params![entry_id])?;
    }
    Ok(find_by_id_for_user(conn, user_id, entry_id)?.map(|ewf| (ewf, changed)))
}

/// Unread count per feed for a user.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnreadCount {
    pub feed_id: i64,
    pub unread: i64,
}

/// Return the unread entry count grouped by feed for the given user.
pub fn unread_counts_per_feed(conn: &Connection, user_id: i64) -> AppResult<Vec<UnreadCount>> {
    let mut stmt = conn.prepare(
        "SELECT e.feed_id, COUNT(*) AS unread \
         FROM entry e INDEXED BY idx_entry_unread_feed \
         INNER JOIN feed f ON f.id = e.feed_id \
         INNER JOIN category c ON c.id = f.category_id \
         WHERE c.user_id = ?1 AND e.read_at IS NULL \
         GROUP BY e.feed_id",
    )?;
    let rows = stmt
        .query_map([user_id], |row| {
            Ok(UnreadCount {
                feed_id: row.get(0)?,
                unread: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Batch query entries by IDs with feed info, verifying user ownership.
pub fn find_by_ids_with_feed(
    conn: &Connection,
    user_id: i64,
    ids: &[i64],
) -> AppResult<Vec<EntryWithFeed>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let placeholders: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_COUNT}
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1 AND e.id IN ({})
        "#,
        in_clause
    );

    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
    for id in ids {
        params_vec.push(Box::new(*id));
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let entries = stmt
        .query_map(params_refs.as_slice(), row_to_entry_with_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// List entry IDs with timestamps for a user, using continuation-based pagination.
/// Returns Vec<(entry_id, timestamp_usec)>.
pub fn list_ids_by_user(
    conn: &Connection,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<(i64, i64)>> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    apply_filter_conditions(&mut conditions, &mut params_vec, filter);
    apply_time_conditions(
        &mut conditions,
        &mut params_vec,
        pagination.ot,
        pagination.nt,
    );
    apply_continuation_condition(
        &mut conditions,
        &mut params_vec,
        pagination.continuation.as_ref(),
        pagination.sort_order,
        pagination.oldest_first,
    );

    let where_clause = conditions.join(" AND ");
    let order = match (pagination.sort_order, pagination.oldest_first) {
        (EntrySortOrder::ReadAt, true) => "e.read_at ASC, e.id ASC",
        (EntrySortOrder::ReadAt, false) => "e.read_at DESC, e.id DESC",
        (EntrySortOrder::StarredAt, true) => "e.starred_at ASC, e.id ASC",
        (EntrySortOrder::StarredAt, false) => "e.starred_at DESC, e.id DESC",
        (_, true) => "COALESCE(e.published_at, e.created_at) ASC, e.id ASC",
        (_, false) => "COALESCE(e.published_at, e.created_at) DESC, e.id DESC",
    };

    let sql = format!(
        r#"
        SELECT e.id, CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) * 1000000
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE {}
        ORDER BY {}
        LIMIT ?{}
        "#,
        where_clause,
        order,
        params_vec.len() + 1
    );

    params_vec.push(Box::new(pagination.limit));
    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// List entries with continuation-based pagination (for Google Reader stream/contents).
pub fn list_by_user_with_continuation(
    conn: &Connection,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<EntryWithFeed>> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

    apply_filter_conditions(&mut conditions, &mut params_vec, filter);
    apply_time_conditions(
        &mut conditions,
        &mut params_vec,
        pagination.ot,
        pagination.nt,
    );
    apply_continuation_condition(
        &mut conditions,
        &mut params_vec,
        pagination.continuation.as_ref(),
        pagination.sort_order,
        pagination.oldest_first,
    );

    let where_clause = conditions.join(" AND ");
    let order = match (pagination.sort_order, pagination.oldest_first) {
        (EntrySortOrder::ReadAt, true) => "e.read_at ASC, e.id ASC",
        (EntrySortOrder::ReadAt, false) => "e.read_at DESC, e.id DESC",
        (EntrySortOrder::StarredAt, true) => "e.starred_at ASC, e.id ASC",
        (EntrySortOrder::StarredAt, false) => "e.starred_at DESC, e.id DESC",
        (_, true) => "COALESCE(e.published_at, e.created_at) ASC, e.id ASC",
        (_, false) => "COALESCE(e.published_at, e.created_at) DESC, e.id DESC",
    };

    // Page-0 (cursorless) index hint. Without it the planner walks
    // category->feed->entry and temp-B-tree-sorts the whole corpus before
    // LIMIT. Mirrors `list_by_user`'s hint, but only when there is no
    // continuation predicate — at depth the predicate already drives the sort
    // index, so we leave that proven-fast plan untouched. Only the
    // published-order sorts have dedicated indexes.
    let entry_hint = if pagination.sort_order == EntrySortOrder::PublishedAt
        && pagination.continuation.is_none()
    {
        published_sort_entry_hint(filter)
    } else {
        ""
    };

    let sql = format!(
        r#"
        SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN}
        FROM entry e{}
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE {}
        ORDER BY {}
        LIMIT ?{}
        "#,
        entry_hint,
        where_clause,
        order,
        params_vec.len() + 1
    );

    params_vec.push(Box::new(pagination.limit));
    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let entries = stmt
        .query_map(params_refs.as_slice(), row_to_entry_with_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// Apply common filter conditions to query builder.
/// True when no `EntryFilter` field would add a predicate against the `entry`
/// table itself. Used to gate the `INDEXED BY idx_entry_sort_ts` hint: without
/// any entry-side filter, scanning the sort index DESC with LIMIT is far
/// cheaper than the planner's default `category -> feed -> entry` walk.
pub fn mark_all_read_by_feed(
    conn: &Connection,
    feed_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    let age_condition = older_than_days
        .map(|days| {
            format!(
                " AND COALESCE(published_at, created_at) < datetime('now', '-{} days')",
                days
            )
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE feed_id = ?1 AND read_at IS NULL{}",
        age_condition
    );

    let rows = conn.execute(&sql, params![feed_id])?;
    Ok(rows as i64)
}

pub fn mark_all_read_by_user(
    conn: &Connection,
    user_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    let age_condition = older_than_days
        .map(|days| {
            format!(
                " AND COALESCE(published_at, created_at) < datetime('now', '-{} days')",
                days
            )
        })
        .unwrap_or_default();

    let sql = format!(
        r#"
        UPDATE entry
        SET read_at = datetime('now'), updated_at = datetime('now')
        WHERE read_at IS NULL{} AND feed_id IN (
            SELECT f.id FROM feed f
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = ?1
        )
        "#,
        age_condition
    );

    let rows = conn.execute(&sql, params![user_id])?;
    Ok(rows as i64)
}

/// Result of finding neighboring entries
#[derive(Debug, Clone, Serialize)]
pub struct EntryNeighbors {
    pub prev_id: Option<i64>,
    pub next_id: Option<i64>,
}

/// Find neighboring entries (prev/next) for a given entry within a user's entries.
/// Entries are ordered by COALESCE(published_at, created_at) DESC.
/// - prev_id: the entry that comes before (newer/higher in list)
/// - next_id: the entry that comes after (older/lower in list)
///
/// Uses EntryFilter to support all filtering conditions (unread, starred, read, feed, category, has_summary).
pub fn find_neighbors(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    filter: &EntryFilter,
) -> AppResult<EntryNeighbors> {
    // Get the current entry's sort timestamp
    let sort_time: Option<String> = conn
        .query_row(
            r#"
            SELECT COALESCE(e.published_at, e.created_at)
            FROM entry e
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE e.id = ?1 AND c.user_id = ?2
            "#,
            params![entry_id, user_id],
            |row| row.get(0),
        )
        .optional()?;

    let sort_time = match sort_time {
        Some(t) => t,
        None => {
            return Ok(EntryNeighbors {
                prev_id: None,
                next_id: None,
            });
        }
    };

    // Build filter conditions using apply_filter_conditions.
    // Prev query base params: ?1=user_id, ?2=sort_time
    let mut prev_conditions = Vec::new();
    let mut prev_params: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(user_id), Box::new(sort_time.clone())];
    apply_filter_conditions(&mut prev_conditions, &mut prev_params, filter);

    // Next query base params: ?1=user_id, ?2=sort_time, ?3=entry_id
    let mut next_conditions = Vec::new();
    let mut next_params: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(user_id), Box::new(sort_time), Box::new(entry_id)];
    apply_filter_conditions(&mut next_conditions, &mut next_params, filter);

    let prev_extra = if prev_conditions.is_empty() {
        String::new()
    } else {
        format!(" AND {}", prev_conditions.join(" AND "))
    };
    let next_extra = if next_conditions.is_empty() {
        String::new()
    } else {
        format!(" AND {}", next_conditions.join(" AND "))
    };

    // Pin the published-order index and force the entry table to drive the
    // join so the planner walks it in sort order and stops at LIMIT 1.
    //
    // The snapshot-widened unread predicate `(read_at IS NULL OR
    // read_at >= ?)` needs special handling. `published_sort_entry_hint`
    // returns no hint for unread, and the OR otherwise makes the planner pick
    // a MULTI-INDEX OR that pulls the whole read-majority of the table into a
    // temp B-tree just to take one row — an O(table) scan that grows with
    // inbox size (~2ms/call at 50k entries, ~8ms at 200k, exec only). Pinning
    // `idx_entry_sort_ts` and using CROSS JOIN to force entry-first ordering
    // turns that into an indexed range scan that short-circuits at the first
    // matching neighbour (~21µs, flat with inbox size), identical results.
    //
    // Gated on `read_after`: only the snapshot OR triggers the bad plan. The
    // strict `read_at IS NULL` path (no `read_after`) keeps its prior plan,
    // which can still use the partial `idx_entry_unread_feed` and so must not
    // be force-pinned to the full sort index.
    let (entry_hint, join_kw) = if filter.unread_only && filter.read_after.is_some() {
        (" INDEXED BY idx_entry_sort_ts", "CROSS JOIN")
    } else {
        (published_sort_entry_hint(filter), "INNER JOIN")
    };

    // Find previous entry (newer, comes before in DESC order)
    let prev_sql = format!(
        r#"
        SELECT e.id
        FROM entry e{hint}
        {join} feed f ON e.feed_id = f.id
        {join} category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND COALESCE(e.published_at, e.created_at) > ?2
          {extra}
        ORDER BY COALESCE(e.published_at, e.created_at) ASC
        LIMIT 1
        "#,
        hint = entry_hint,
        join = join_kw,
        extra = prev_extra
    );
    let prev_refs: Vec<&dyn rusqlite::ToSql> = prev_params.iter().map(|p| p.as_ref()).collect();
    let prev_id: Option<i64> = conn
        .query_row(&prev_sql, prev_refs.as_slice(), |row| row.get(0))
        .optional()?;

    // Find next entry (older, comes after in DESC order)
    let next_sql = format!(
        r#"
        SELECT e.id
        FROM entry e{hint}
        {join} feed f ON e.feed_id = f.id
        {join} category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND (COALESCE(e.published_at, e.created_at) < ?2
               OR (COALESCE(e.published_at, e.created_at) = ?2 AND e.id < ?3))
          {extra}
        ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
        LIMIT 1
        "#,
        hint = entry_hint,
        join = join_kw,
        extra = next_extra
    );
    let next_refs: Vec<&dyn rusqlite::ToSql> = next_params.iter().map(|p| p.as_ref()).collect();
    let next_id: Option<i64> = conn
        .query_row(&next_sql, next_refs.as_slice(), |row| row.get(0))
        .optional()?;

    Ok(EntryNeighbors { prev_id, next_id })
}

pub fn mark_all_read_by_category(
    conn: &Connection,
    category_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    let age_condition = older_than_days
        .map(|days| {
            format!(
                " AND COALESCE(published_at, created_at) < datetime('now', '-{} days')",
                days
            )
        })
        .unwrap_or_default();

    let sql = format!(
        r#"
        UPDATE entry
        SET read_at = datetime('now'), updated_at = datetime('now')
        WHERE read_at IS NULL{} AND feed_id IN (
            SELECT id FROM feed WHERE category_id = ?1
        )
        "#,
        age_condition
    );

    let rows = conn.execute(&sql, params![category_id])?;
    Ok(rows as i64)
}

/// Mark multiple entries as read by their IDs.
/// Only marks entries that belong to the user (via feed -> category -> user).
/// Returns the count of entries that were actually marked as read.
/// Apply a bulk `SET` to the given entry ids in a single statement, scoped to
/// the feeds the user owns. `set_clause` is the `SET ...` body; `extra_where`
/// is an optional predicate (e.g. `" AND read_at IS NULL"`) appended after the
/// `id IN (...)` clause. Returns the number of rows updated. Empty `entry_ids`
/// is a no-op returning 0.
fn update_entries_by_ids(
    conn: &Connection,
    user_id: i64,
    entry_ids: &[i64],
    set_clause: &str,
    extra_where: &str,
) -> AppResult<i64> {
    if entry_ids.is_empty() {
        return Ok(0);
    }

    // Build placeholders for IN clause (?2, ?3, ...; ?1 is user_id)
    let placeholders: Vec<String> = entry_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        r#"
        UPDATE entry
        SET {set_clause}
        WHERE id IN ({in_clause}){extra_where}
          AND feed_id IN (
              SELECT f.id FROM feed f
              INNER JOIN category c ON f.category_id = c.id
              WHERE c.user_id = ?1
          )
        "#,
    );

    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
    for id in entry_ids {
        params_vec.push(Box::new(*id));
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let rows = conn.execute(&sql, params_refs.as_slice())?;
    Ok(rows as i64)
}

/// Bulk mark the given entries as read (only those currently unread), scoped to
/// the user's feeds. Returns the number of rows updated.
pub fn mark_read_by_ids(conn: &Connection, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        conn,
        user_id,
        entry_ids,
        "read_at = datetime('now'), updated_at = datetime('now')",
        " AND read_at IS NULL",
    )
}

/// Bulk mark the given entries as unread, scoped to the user's feeds.
pub fn mark_unread_by_ids(conn: &Connection, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        conn,
        user_id,
        entry_ids,
        "read_at = NULL, updated_at = datetime('now')",
        "",
    )
}

/// Bulk star the given entries (only those not already starred), scoped to the
/// user's feeds.
pub fn star_by_ids(conn: &Connection, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        conn,
        user_id,
        entry_ids,
        "starred_at = datetime('now'), updated_at = datetime('now')",
        " AND starred_at IS NULL",
    )
}

/// Bulk unstar the given entries, scoped to the user's feeds.
pub fn unstar_by_ids(conn: &Connection, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        conn,
        user_id,
        entry_ids,
        "starred_at = NULL, updated_at = datetime('now')",
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::category;
    use crate::models::feed;
    use crate::models::user::{self, Role};
    use chrono::TimeZone;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    fn create_test_user(conn: &Connection, username: &str) -> i64 {
        user::create_user(conn, username, "hash123", Role::User)
            .unwrap()
            .id
    }

    fn create_test_category(conn: &Connection, user_id: i64, name: &str) -> i64 {
        category::create_category(conn, user_id, name).unwrap().id
    }

    fn create_test_feed(conn: &Connection, category_id: i64, url: &str) -> i64 {
        feed::create_feed(
            conn,
            &feed::CreateFeedParams {
                category_id,
                url,
                title: Some("Test Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn test_upsert_entry() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // Insert new entry
        let (entry, is_new) = upsert_entry(
            &conn,
            feed_id,
            "guid-123",
            Some("Test Entry"),
            Some("https://example.com/entry"),
            Some("Content"),
            Some("Summary"),
            Some("Author"),
            Some(Utc::now()),
        )
        .unwrap();

        assert!(is_new);
        assert_eq!(entry.title, Some("Test Entry".to_string()));
        assert!(entry.read_at.is_none());
        assert!(entry.starred_at.is_none());

        // Update existing entry
        let (updated, is_new) = upsert_entry(
            &conn,
            feed_id,
            "guid-123",
            Some("Updated Title"),
            Some("https://example.com/entry"),
            Some("Updated Content"),
            None,
            None,
            None,
        )
        .unwrap();

        assert!(!is_new);
        assert_eq!(updated.title, Some("Updated Title".to_string()));
        assert_eq!(updated.id, entry.id);
    }

    #[test]
    fn test_mark_as_read() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        let (entry, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-123",
            Some("Test"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert!(entry.read_at.is_none());

        let read = mark_as_read(&conn, entry.id).unwrap();
        assert!(read.read_at.is_some());

        let unread = mark_as_unread(&conn, entry.id).unwrap();
        assert!(unread.read_at.is_none());
    }

    #[test]
    fn test_toggle_star() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        let (entry, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-123",
            Some("Test"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert!(entry.starred_at.is_none());

        let starred = toggle_star(&conn, entry.id).unwrap();
        assert!(starred.starred_at.is_some());

        let unstarred = toggle_star(&conn, entry.id).unwrap();
        assert!(unstarred.starred_at.is_none());
    }

    #[test]
    fn test_count_unread() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        for i in 0..5 {
            upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some("Test"),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        }

        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 5);

        // Mark 2 as read
        let entries = list_by_feed(&conn, feed_id, 10, 0).unwrap();
        mark_as_read(&conn, entries[0].id).unwrap();
        mark_as_read(&conn, entries[1].id).unwrap();

        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 3);
    }

    #[test]
    fn test_search_entries_by_title() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // Create entries with different titles
        upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("Rust Programming Guide"),
            None,
            Some("Content about Rust"),
            None,
            None,
            None,
        )
        .unwrap();
        upsert_entry(
            &conn,
            feed_id,
            "guid-2",
            Some("Python Tutorial"),
            None,
            Some("Content about Python"),
            None,
            None,
            None,
        )
        .unwrap();

        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].entry.title,
            Some("Rust Programming Guide".to_string())
        );
    }

    #[test]
    fn test_search_entries_by_content() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("Entry 1"),
            None,
            Some("This article discusses WebAssembly"),
            None,
            None,
            None,
        )
        .unwrap();
        upsert_entry(
            &conn,
            feed_id,
            "guid-2",
            Some("Entry 2"),
            None,
            Some("This article discusses JavaScript"),
            None,
            None,
            None,
        )
        .unwrap();

        let filter = EntryFilter {
            search: Some("WebAssembly".to_string()),
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.title, Some("Entry 1".to_string()));
    }

    #[test]
    fn test_search_case_insensitive() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("UPPERCASE Title"),
            None,
            Some("lowercase content"),
            None,
            None,
            None,
        )
        .unwrap();

        // Search with lowercase should match uppercase title
        let filter = EntryFilter {
            search: Some("uppercase".to_string()),
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);

        // Search with uppercase should match lowercase content
        let filter = EntryFilter {
            search: Some("LOWERCASE".to_string()),
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_combined_with_filters() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        let (entry1, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("Rust Article"),
            None,
            Some("Content"),
            None,
            None,
            None,
        )
        .unwrap();
        let (entry2, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-2",
            Some("Rust Tutorial"),
            None,
            Some("Content"),
            None,
            None,
            None,
        )
        .unwrap();

        // Mark entry1 as read
        mark_as_read(&conn, entry1.id).unwrap();
        // Star entry2
        toggle_star(&conn, entry2.id).unwrap();

        // Search for "Rust" with unread_only - should only return entry2
        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            unread_only: true,
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, entry2.id);

        // Search for "Rust" with starred_only - should only return entry2
        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            starred_only: true,
            ..Default::default()
        };
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, entry2.id);
    }

    #[test]
    fn test_search_pagination() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // Create 5 entries that match the search
        for i in 0..5 {
            upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some(&format!("Rust Article {}", i)),
                None,
                Some("Content"),
                None,
                None,
                None,
            )
            .unwrap();
        }

        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            ..Default::default()
        };

        // First page (limit 2)
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 2, 0).unwrap();
        assert_eq!(results.len(), 2);

        // Second page
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 2, 2).unwrap();
        assert_eq!(results.len(), 2);

        // Third page (only 1 remaining)
        let results =
            list_by_user(&conn, user_id, &filter, EntrySortOrder::default(), 2, 4).unwrap();
        assert_eq!(results.len(), 1);

        // Count should be 5
        let count = count_by_user(&conn, user_id, &filter).unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn test_mark_read_by_ids() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let user2_id = create_test_user(&conn, "testuser2");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let category2_id = create_test_category(&conn, user2_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");
        let feed2_id = create_test_feed(&conn, category2_id, "https://example2.com/feed.xml");

        // Create entries for user 1
        let (entry1, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-1",
            Some("Entry 1"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (entry2, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-2",
            Some("Entry 2"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (entry3, _) = upsert_entry(
            &conn,
            feed_id,
            "guid-3",
            Some("Entry 3"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Create entry for user 2
        let (other_entry, _) = upsert_entry(
            &conn,
            feed2_id,
            "guid-4",
            Some("Other Entry"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // All entries should be unread
        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 3);
        assert_eq!(count_unread_by_user(&conn, user2_id).unwrap(), 1);

        // Mark entries 1 and 2 as read (user 1)
        let marked = mark_read_by_ids(&conn, user_id, &[entry1.id, entry2.id]).unwrap();
        assert_eq!(marked, 2);

        // Entry 3 should still be unread
        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 1);

        // Try to mark user 2's entry as read with user 1's credentials - should not work
        let marked = mark_read_by_ids(&conn, user_id, &[other_entry.id]).unwrap();
        assert_eq!(marked, 0);

        // User 2's entry should still be unread
        assert_eq!(count_unread_by_user(&conn, user2_id).unwrap(), 1);

        // Mark already-read entries again - should return 0
        let marked = mark_read_by_ids(&conn, user_id, &[entry1.id, entry2.id]).unwrap();
        assert_eq!(marked, 0);

        // Empty array should return 0
        let marked = mark_read_by_ids(&conn, user_id, &[]).unwrap();
        assert_eq!(marked, 0);

        // Mark remaining entry
        let marked = mark_read_by_ids(&conn, user_id, &[entry3.id]).unwrap();
        assert_eq!(marked, 1);

        // All user 1 entries should now be read
        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 0);
    }

    #[test]
    fn test_upsert_entry_id_returns_id_and_is_new() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // First upsert: new row
        let first = upsert_entry_id(
            &conn,
            feed_id,
            "guid-1",
            Some("Title"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let id1 = match first {
            UpsertOutcome::Inserted(id) => id,
            o => panic!("expected Inserted, got {o:?}"),
        };
        let row = find_by_id(&conn, id1).unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some("Title"));

        // Second upsert with same guid+feed: update, same id
        let second = upsert_entry_id(
            &conn,
            feed_id,
            "guid-1",
            Some("Title 2"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(id) if id == id1));
        let row = find_by_id(&conn, id1).unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some("Title 2"));
    }

    #[test]
    fn test_upsert_skips_tombstoned_guid() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        insert_tombstone(&conn, feed_id, "ghost").unwrap();
        let outcome = upsert_entry_id(
            &conn,
            feed_id,
            "ghost",
            Some("Ghost"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));
        assert!(
            find_by_guid_and_feed(&conn, "ghost", feed_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_upsert_inserts_then_updates() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        let first = upsert_entry_id(
            &conn,
            feed_id,
            "g1",
            Some("First"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let id = match first {
            UpsertOutcome::Inserted(id) => id,
            other => panic!("expected Inserted, got {other:?}"),
        };

        let second = upsert_entry_id(
            &conn,
            feed_id,
            "g1",
            Some("Updated"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(uid) if uid == id));
    }

    #[test]
    fn test_star_unstar_and_mark_unread_by_ids() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let user2_id = create_test_user(&conn, "testuser2");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let category2_id = create_test_category(&conn, user2_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");
        let feed2_id = create_test_feed(&conn, category2_id, "https://example2.com/feed.xml");

        let (e1, _) = upsert_entry(
            &conn,
            feed_id,
            "g1",
            Some("E1"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (e2, _) = upsert_entry(
            &conn,
            feed_id,
            "g2",
            Some("E2"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let (other, _) = upsert_entry(
            &conn,
            feed2_id,
            "g3",
            Some("E3"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Star e1, e2 — only currently-unstarred rows count
        let starred = star_by_ids(&conn, user_id, &[e1.id, e2.id]).unwrap();
        assert_eq!(starred, 2);
        assert!(
            find_by_id(&conn, e1.id)
                .unwrap()
                .unwrap()
                .starred_at
                .is_some()
        );

        // Starring again is a no-op (already starred)
        assert_eq!(star_by_ids(&conn, user_id, &[e1.id, e2.id]).unwrap(), 0);

        // Ownership scope: cannot star another user's entry
        assert_eq!(star_by_ids(&conn, user_id, &[other.id]).unwrap(), 0);
        assert!(
            find_by_id(&conn, other.id)
                .unwrap()
                .unwrap()
                .starred_at
                .is_none()
        );

        // Unstar e1
        assert_eq!(unstar_by_ids(&conn, user_id, &[e1.id]).unwrap(), 1);
        assert!(
            find_by_id(&conn, e1.id)
                .unwrap()
                .unwrap()
                .starred_at
                .is_none()
        );

        // Mark read then mark unread by ids
        assert_eq!(
            mark_read_by_ids(&conn, user_id, &[e1.id, e2.id]).unwrap(),
            2
        );
        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 0);
        let unread = mark_unread_by_ids(&conn, user_id, &[e1.id, e2.id]).unwrap();
        assert_eq!(unread, 2);
        assert_eq!(count_unread_by_user(&conn, user_id).unwrap(), 2);

        // Empty input is a no-op
        assert_eq!(star_by_ids(&conn, user_id, &[]).unwrap(), 0);
        assert_eq!(mark_unread_by_ids(&conn, user_id, &[]).unwrap(), 0);
    }

    #[test]
    fn test_find_neighbors_starred_only() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // Create 5 entries with distinct timestamps
        let mut entries = Vec::new();
        for i in 0..5 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some(&format!("Entry {}", i)),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .unwrap();
            entries.push(entry);
        }

        // Star entries 1 and 3 (0-indexed)
        star_entry(&conn, entries[1].id).unwrap();
        star_entry(&conn, entries[3].id).unwrap();

        // From entry 3 (starred), with starred_only filter:
        // prev should be entry 1 (the only other starred entry that is newer... wait, entry 3 is newer)
        // Actually entries are ordered by published_at DESC, so entry 4 is newest.
        // entry 3 published_at = now+30s, entry 1 published_at = now+10s
        // prev (newer than entry 3) = none starred that is newer
        // next (older than entry 3) = entry 1 (starred, older)
        let filter = EntryFilter {
            starred_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&conn, user_id, entries[3].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, None); // no starred entry newer than entry 3
        assert_eq!(neighbors.next_id, Some(entries[1].id)); // entry 1 is older and starred

        // From entry 1 (starred):
        // prev (newer) = entry 3 (starred, newer)
        // next (older) = none
        let neighbors = find_neighbors(&conn, user_id, entries[1].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[3].id));
        assert_eq!(neighbors.next_id, None);

        // Without filter, entry 3 should see entry 4 as prev and entry 2 as next
        let no_filter = EntryFilter::default();
        let neighbors = find_neighbors(&conn, user_id, entries[3].id, &no_filter).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[2].id));
    }

    #[test]
    fn test_find_neighbors_unread_only_read_after() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // 5 entries published ascending — entries[4] is newest in the
        // published-DESC list order.
        let mut entries = Vec::new();
        for i in 0..5 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some(&format!("Entry {}", i)),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .unwrap();
            entries.push(entry);
        }

        // entries[1]: read an hour ago — before the snapshot boundary.
        conn.execute(
            "UPDATE entry SET read_at = datetime('now', '-1 hour') WHERE id = ?1",
            params![entries[1].id],
        )
        .unwrap();
        // entries[2]: read just now — inside the snapshot.
        conn.execute(
            "UPDATE entry SET read_at = datetime('now') WHERE id = ?1",
            params![entries[2].id],
        )
        .unwrap();

        // Snapshot boundary 10 minutes ago: the just-read entries[2] is
        // inside it, the hour-old entries[1] is not.
        let snapshot = (Utc::now() - chrono::Duration::minutes(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let filter = EntryFilter {
            unread_only: true,
            read_after: Some(snapshot),
            ..Default::default()
        };

        // From entries[3]: next (older) lands on the just-read entries[2]
        // — still reachable inside the snapshot.
        let neighbors = find_neighbors(&conn, user_id, entries[3].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[2].id));

        // From entries[2]: prev is the unread entries[3]; next skips the
        // pre-snapshot entries[1] and lands on the unread entries[0].
        let neighbors = find_neighbors(&conn, user_id, entries[2].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[3].id));
        assert_eq!(neighbors.next_id, Some(entries[0].id));

        // Plain unread_only without read_after keeps the strict live
        // filter: both read entries are skipped.
        let strict = EntryFilter {
            unread_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&conn, user_id, entries[3].id, &strict).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[0].id));
    }

    #[test]
    fn test_find_neighbors_read_only() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "testuser");
        let category_id = create_test_category(&conn, user_id, "Tech");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/feed.xml");

        // Create 4 entries with distinct timestamps
        let mut entries = Vec::new();
        for i in 0..4 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &conn,
                feed_id,
                &format!("guid-{}", i),
                Some(&format!("Entry {}", i)),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .unwrap();
            entries.push(entry);
        }

        // Mark entries 0 and 2 as read
        mark_as_read(&conn, entries[0].id).unwrap();
        mark_as_read(&conn, entries[2].id).unwrap();

        // From entry 2 (read, published_at = now+20s), with read_only filter:
        // prev (newer) = none (entry 3 is newer but unread)
        // next (older) = entry 0 (read, older)
        let filter = EntryFilter {
            read_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&conn, user_id, entries[2].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, None);
        assert_eq!(neighbors.next_id, Some(entries[0].id));

        // From entry 0 (read, published_at = now+0s):
        // prev (newer) = entry 2 (read, newer)
        // next (older) = none
        let neighbors = find_neighbors(&conn, user_id, entries[0].id, &filter).unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[2].id));
        assert_eq!(neighbors.next_id, None);
    }

    #[test]
    fn cursor_parses_composite_format() {
        let c = ContinuationCursor::parse("2026-04-26 12:34:56|142").expect("composite parses");
        match c {
            ContinuationCursor::Composite { sort_ts, id } => {
                assert_eq!(sort_ts, "2026-04-26 12:34:56");
                assert_eq!(id, 142);
            }
            _ => panic!("expected Composite"),
        }
    }

    #[test]
    fn cursor_parses_bare_i64_as_legacy() {
        let c = ContinuationCursor::parse("142").expect("legacy parses");
        match c {
            ContinuationCursor::LegacyId(id) => assert_eq!(id, 142),
            _ => panic!("expected LegacyId"),
        }
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(ContinuationCursor::parse("not-a-cursor").is_none());
        assert!(ContinuationCursor::parse("|").is_none());
        assert!(ContinuationCursor::parse("ts|notnum").is_none());
        assert!(ContinuationCursor::parse("").is_none());
    }

    #[test]
    fn cursor_encodes_composite() {
        assert_eq!(
            ContinuationCursor::encode_composite("2026-04-26 12:34:56", 142),
            "2026-04-26 12:34:56|142"
        );
    }

    #[test]
    fn fetch_sort_ts_returns_published_or_created_for_publishedat() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        // Entry with published_at set
        conn.execute(
            "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g1", "2026-04-01 10:00:00"],
        )
        .unwrap();
        let id1: i64 = conn.last_insert_rowid();

        // Entry with published_at NULL → COALESCE falls back to created_at
        conn.execute(
            "INSERT INTO entry (feed_id, guid, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g2", "2026-04-02 11:00:00"],
        )
        .unwrap();
        let id2: i64 = conn.last_insert_rowid();

        let ts1 = fetch_sort_ts(&conn, id1, EntrySortOrder::PublishedAt).unwrap();
        let ts2 = fetch_sort_ts(&conn, id2, EntrySortOrder::PublishedAt).unwrap();
        assert_eq!(ts1.as_deref(), Some("2026-04-01 10:00:00"));
        assert_eq!(ts2.as_deref(), Some("2026-04-02 11:00:00"));
    }

    #[test]
    fn fetch_sort_ts_returns_read_at_for_readat_sort() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        conn.execute(
            "INSERT INTO entry (feed_id, guid, read_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed_id, "g1", "2026-04-03 12:00:00"],
        )
        .unwrap();
        let id: i64 = conn.last_insert_rowid();

        let ts = fetch_sort_ts(&conn, id, EntrySortOrder::ReadAt).unwrap();
        assert_eq!(ts.as_deref(), Some("2026-04-03 12:00:00"));
    }

    #[test]
    fn fetch_sort_ts_returns_none_for_missing_id() {
        let conn = setup_db();
        let ts = fetch_sort_ts(&conn, 99999, EntrySortOrder::PublishedAt).unwrap();
        assert_eq!(ts, None);
    }

    #[test]
    fn composite_cursor_walks_non_monotonic_data_without_skip() {
        // Repro for #164: when id↔published_at order diverges (OPML re-import,
        // back-dated feed items), the legacy `e.id < ?` cursor silently skips.
        // The composite cursor must visit every entry.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        // 6 monotonic entries (newer ts ⇒ later id), then 4 "back-dated" entries
        // with NEW ids but OLD timestamps (mimics OPML re-import).
        let monotonic = [
            ("g1", "2026-04-01 10:00:00"),
            ("g2", "2026-04-02 10:00:00"),
            ("g3", "2026-04-03 10:00:00"),
            ("g4", "2026-04-04 10:00:00"),
            ("g5", "2026-04-05 10:00:00"),
            ("g6", "2026-04-06 10:00:00"),
        ];
        let backdated = [
            ("g7-bd", "2026-03-01 10:00:00"),
            ("g8-bd", "2026-03-02 10:00:00"),
            ("g9-bd", "2026-03-03 10:00:00"),
            ("g10-bd", "2026-03-04 10:00:00"),
        ];
        for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![feed_id, guid, ts],
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut seen: Vec<i64> = Vec::new();
        let mut cursor: Option<ContinuationCursor> = None;
        let page_limit: i64 = 3;

        // Walk pages (DESC sort by COALESCE(published_at, created_at)) until empty
        loop {
            let pagination = ContinuationParams {
                oldest_first: false,
                limit: page_limit,
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let page =
                list_by_user_with_continuation(&conn, user_id, &filter, &pagination).unwrap();
            if page.is_empty() {
                break;
            }
            for ewf in &page {
                assert!(
                    !seen.contains(&ewf.entry.id),
                    "duplicate id {}",
                    ewf.entry.id
                );
                seen.push(ewf.entry.id);
            }
            let last = page.last().unwrap();
            let sort_ts = fetch_sort_ts(&conn, last.entry.id, EntrySortOrder::PublishedAt)
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            });
            // safety: don't loop forever
            if seen.len() > 100 {
                panic!("runaway loop");
            }
        }

        assert_eq!(
            seen.len(),
            10,
            "must visit all 10 entries; saw {}",
            seen.len()
        );
    }

    #[test]
    fn composite_cursor_walks_non_monotonic_data_oldest_first_without_skip() {
        // Same shape as composite_cursor_walks_non_monotonic_data_without_skip
        // but exercises the oldest_first=true (ASC) path of the bounded-OR
        // predicate. Triggered in production by the GReader `r=o` query param.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        let monotonic = [
            ("g1", "2026-04-01 10:00:00"),
            ("g2", "2026-04-02 10:00:00"),
            ("g3", "2026-04-03 10:00:00"),
            ("g4", "2026-04-04 10:00:00"),
            ("g5", "2026-04-05 10:00:00"),
            ("g6", "2026-04-06 10:00:00"),
        ];
        let backdated = [
            ("g7-bd", "2026-03-01 10:00:00"),
            ("g8-bd", "2026-03-02 10:00:00"),
            ("g9-bd", "2026-03-03 10:00:00"),
            ("g10-bd", "2026-03-04 10:00:00"),
        ];
        for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![feed_id, guid, ts],
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut seen: Vec<i64> = Vec::new();
        let mut cursor: Option<ContinuationCursor> = None;
        let page_limit: i64 = 3;

        loop {
            let pagination = ContinuationParams {
                oldest_first: true,
                limit: page_limit,
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let page =
                list_by_user_with_continuation(&conn, user_id, &filter, &pagination).unwrap();
            if page.is_empty() {
                break;
            }
            for ewf in &page {
                assert!(
                    !seen.contains(&ewf.entry.id),
                    "duplicate id {}",
                    ewf.entry.id
                );
                seen.push(ewf.entry.id);
            }
            let last = page.last().unwrap();
            let sort_ts = fetch_sort_ts(&conn, last.entry.id, EntrySortOrder::PublishedAt)
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            });
            if seen.len() > 100 {
                panic!("runaway loop");
            }
        }

        assert_eq!(
            seen.len(),
            10,
            "must visit all 10 entries on ASC walk; saw {}",
            seen.len()
        );
    }

    #[test]
    fn legacy_bare_i64_cursor_still_paginates() {
        // In-flight cursors from pre-#164 deployments must still work for one
        // grace period. Under monotonic data (the common case), the legacy
        // `e.id < ?` predicate is correct.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");

        for i in 1..=5 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    feed_id,
                    format!("g{}", i),
                    format!("2026-04-0{} 10:00:00", i)
                ],
            )
            .unwrap();
        }

        // Get id of "newest" entry (highest id, latest ts)
        let max_id: i64 = conn
            .query_row("SELECT MAX(id) FROM entry", [], |r| r.get(0))
            .unwrap();

        let pagination = ContinuationParams {
            oldest_first: false,
            limit: 10,
            continuation: Some(ContinuationCursor::LegacyId(max_id)),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let page =
            list_by_user_with_continuation(&conn, user_id, &EntryFilter::default(), &pagination)
                .unwrap();

        // 4 entries below the boundary id
        assert_eq!(page.len(), 4);
        for ewf in &page {
            assert!(ewf.entry.id < max_id);
        }
    }

    #[test]
    fn is_no_entry_side_predicate_matches_only_user_only_filter() {
        // Default filter (used by /entries) has no entry-side predicate.
        assert!(is_no_entry_side_predicate(&EntryFilter::default()));

        // Any of these flags pulls in an entry-side predicate.
        let cases = [
            EntryFilter {
                feed_id: Some(1),
                ..Default::default()
            },
            EntryFilter {
                category_id: Some(1),
                ..Default::default()
            },
            EntryFilter {
                unread_only: true,
                ..Default::default()
            },
            EntryFilter {
                starred_only: true,
                ..Default::default()
            },
            EntryFilter {
                read_only: true,
                ..Default::default()
            },
            EntryFilter {
                search: Some("foo".into()),
                ..Default::default()
            },
            EntryFilter {
                has_summary: Some(true),
                ..Default::default()
            },
            EntryFilter {
                has_summary: Some(false),
                ..Default::default()
            },
        ];
        for f in &cases {
            assert!(
                !is_no_entry_side_predicate(f),
                "expected entry-side predicate for filter: {:?}",
                f
            );
        }
    }

    /// Captures the EXPLAIN QUERY PLAN output for a SELECT. Concatenates all
    /// `detail` columns so callers can `assert!(plan.contains("idx_entry_…"))`
    /// to lock in the planner choice. The bound values are placeholders — the
    /// planner only needs parameter count to match.
    fn explain_plan_for(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> String {
        let explain_sql = format!("EXPLAIN QUERY PLAN {}", sql);
        let mut stmt = conn.prepare(&explain_sql).unwrap();
        let rows: Vec<String> = stmt
            .query_map(params, |row| row.get::<_, String>(3))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        rows.join(" | ")
    }

    #[test]
    fn list_by_user_uses_partial_index_for_starred() {
        let conn = setup_db();
        let _ = create_test_user(&conn, "u");
        // Tiny in-memory dataset is enough — INDEXED BY is mandatory and the
        // planner has no choice to override the hint.
        let sql = r#"
            SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
                   e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
                   f.title, f.url, f.site_url, c.id, c.name,
                   CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
                   f.custom_referrer
            FROM entry e INDEXED BY idx_entry_starred_sort
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
            WHERE c.user_id = ?1 AND e.starred_at IS NOT NULL
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        "#;
        let plan = explain_plan_for(&conn, sql, &[&1i64]);
        assert!(
            plan.contains("idx_entry_starred_sort"),
            "plan missing partial index: {}",
            plan
        );
    }

    #[test]
    fn list_by_user_uses_partial_index_for_read() {
        let conn = setup_db();
        let _ = create_test_user(&conn, "u");
        let sql = r#"
            SELECT e.id FROM entry e INDEXED BY idx_entry_read_sort
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = ?1 AND e.read_at IS NOT NULL
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        "#;
        let plan = explain_plan_for(&conn, sql, &[&1i64]);
        assert!(
            plan.contains("idx_entry_read_sort"),
            "plan missing partial index: {}",
            plan
        );
    }

    #[test]
    fn list_by_user_no_predicate_uses_sort_ts_index() {
        // End-to-end: prepared SQL must include the INDEXED BY hint for the
        // "All Entries" case, otherwise the planner falls back to walking
        // every row via category->feed->entry.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    feed_id,
                    format!("g{}", i),
                    format!("2026-05-0{} 10:00:00", i)
                ],
            )
            .unwrap();
        }

        // Sanity: the public API returns the right rows under the hint.
        let rows = list_by_user(
            &conn,
            user_id,
            &EntryFilter::default(),
            EntrySortOrder::PublishedAt,
            10,
            0,
        )
        .unwrap();
        assert_eq!(rows.len(), 3);

        // Plan check: a hand-built copy of the same query (same shape as the
        // builder produces with the no-predicate hint) must scan via
        // `idx_entry_sort_ts`. We test the shape, not the runtime statement
        // (rusqlite caches prepared SQL outside of test reach).
        let sql = r#"
            SELECT e.id FROM entry e INDEXED BY idx_entry_sort_ts
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = ?1
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        "#;
        let plan = explain_plan_for(&conn, sql, &[&user_id]);
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan missing sort_ts index: {}",
            plan
        );
    }

    #[test]
    fn continuation_page0_unfiltered_uses_sort_ts_index() {
        // Regression guard for the page-0 index hint added to
        // `list_by_user_with_continuation`. Without `INDEXED BY idx_entry_sort_ts`
        // the planner walks category->feed->entry and temp-B-tree-sorts the whole
        // corpus before LIMIT — a ~350× slowdown on a large instance. The
        // behavioral walk test (`test_continuation_walk_is_gapless_unfiltered`)
        // would NOT catch a dropped hint because results are identical. This test
        // pins the query plan directly.
        //
        // Mirrors the same pattern as `list_by_user_no_predicate_uses_sort_ts_index`:
        // replicate the SQL shape the builder emits for the cursorless case and
        // assert EXPLAIN QUERY PLAN mentions the index.
        let conn = setup_db();
        let user_id = create_test_user(&conn, "u");
        let cat_id = create_test_category(&conn, user_id, "c");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/f.xml");
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    feed_id,
                    format!("g{}", i),
                    format!("2026-05-0{} 10:00:00", i)
                ],
            )
            .unwrap();
        }

        // Hand-built copy of the SQL `list_by_user_with_continuation` emits for:
        //   filter = EntryFilter::default(), sort_order = PublishedAt,
        //   continuation = None, oldest_first = false, ot = None, nt = None.
        // The entry hint resolves to " INDEXED BY idx_entry_sort_ts" because
        // `is_no_entry_side_predicate` is true and sort_order is PublishedAt.
        // The ORDER BY includes the tie-breaker `e.id DESC` that the continuation
        // builder always appends (unlike list_by_user which omits it).
        let sql = r#"
            SELECT e.id
            FROM entry e INDEXED BY idx_entry_sort_ts
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
            WHERE c.user_id = ?1
            ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
            LIMIT ?2
        "#;
        let plan = explain_plan_for(&conn, sql, &[&user_id, &51i64]);
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan missing sort_ts index: {}",
            plan
        );
        assert!(
            !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
            "plan must not temp-B-tree-sort: {}",
            plan
        );
    }

    /// Locks the query plan for the snapshot-widened unread neighbours query.
    /// Without the `idx_entry_sort_ts` hint + entry-first CROSS JOIN that
    /// `find_neighbors` emits for unread filters, the planner answers the
    /// `(read_at IS NULL OR read_at >= ?)` predicate with a MULTI-INDEX OR
    /// that scans the read-majority of the table into a temp B-tree — an
    /// O(table) scan per call that grows unbounded with inbox size. The hint
    /// turns it into an indexed range scan that short-circuits at LIMIT 1.
    #[test]
    fn find_neighbors_unread_read_after_uses_sort_ts_not_multi_index_or() {
        let conn = setup_db();
        let _ = create_test_user(&conn, "u");
        // Mirrors the next-side SQL `find_neighbors` builds for an unread
        // filter with `read_after` set (see the join_kw / entry_hint branch).
        let sql = r#"
            SELECT e.id
            FROM entry e INDEXED BY idx_entry_sort_ts
            CROSS JOIN feed f ON e.feed_id = f.id
            CROSS JOIN category c ON f.category_id = c.id
            WHERE c.user_id = ?1
              AND (COALESCE(e.published_at, e.created_at) < ?2
                   OR (COALESCE(e.published_at, e.created_at) = ?2 AND e.id < ?3))
              AND (e.read_at IS NULL OR e.read_at >= ?4)
            ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
            LIMIT 1
        "#;
        let plan = explain_plan_for(
            &conn,
            sql,
            &[&1i64, &"2020-01-01 00:00:00", &1i64, &"2020-01-01 00:00:00"],
        );
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan must pin idx_entry_sort_ts: {}",
            plan
        );
        assert!(
            !plan.contains("MULTI-INDEX OR"),
            "plan must not fan out into a MULTI-INDEX OR: {}",
            plan
        );
        assert!(
            !plan.contains("idx_entry_read_at"),
            "plan must not scan via the read_at index: {}",
            plan
        );
    }

    #[test]
    fn test_continuation_walk_is_gapless_unfiltered() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "walker");
        let category_id = create_test_category(&conn, user_id, "C");
        let feed_id = create_test_feed(&conn, category_id, "https://example.com/walk.xml");
        // 5 entries, distinct published_at so order is deterministic.
        for i in 0..5 {
            upsert_entry(
                &conn,
                feed_id,
                &format!("g{i}"),
                Some(&format!("T{i}")),
                None,
                None,
                None,
                None,
                Some(
                    chrono::Utc
                        .with_ymd_and_hms(2024, 1, 1 + i as u32, 0, 0, 0)
                        .unwrap(),
                ),
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut cursor: Option<ContinuationCursor> = None;
        let mut seen: Vec<i64> = Vec::new();
        loop {
            let params = ContinuationParams {
                oldest_first: false,
                limit: 3, // page size 2 + 1 sentinel
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let rows = list_by_user_with_continuation(&conn, user_id, &filter, &params).unwrap();
            let has_more = rows.len() > 2;
            let page = &rows[..rows.len().min(2)];
            if page.is_empty() {
                break;
            }
            for e in page {
                seen.push(e.entry.id);
            }
            if !has_more {
                break;
            }
            let last = page.last().unwrap();
            let ts = fetch_sort_ts(&conn, last.entry.id, EntrySortOrder::PublishedAt)
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts: ts,
                id: last.entry.id,
            });
        }

        // All 5 seen exactly once, newest-first.
        assert_eq!(seen.len(), 5, "walk must visit every entry once: {seen:?}");
        let mut uniq = seen.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "no duplicates across pages: {seen:?}");
    }

    #[test]
    fn test_prune_respects_threshold_star_and_optin() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "pruneuser");
        let cat_id = create_test_category(&conn, user_id, "PruneCat");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/prune.xml");

        // Helper: insert a read entry aged `days_old` (and optionally starred).
        let mk = |guid: &str, days: i64, starred: bool| {
            upsert_entry_id(
                &conn,
                feed_id,
                guid,
                Some(guid),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now', ?2) WHERE guid = ?1 AND feed_id = ?3",
                rusqlite::params![guid, format!("-{days} days"), feed_id],
            )
            .unwrap();
            if starred {
                conn.execute(
                    "UPDATE entry SET starred_at = datetime('now') WHERE guid = ?1 AND feed_id = ?2",
                    rusqlite::params![guid, feed_id],
                ).unwrap();
            }
        };
        mk("old", 40, false); // read, 40d, not starred -> victim once enabled
        mk("oldstar", 40, true); // starred -> never deleted
        mk("fresh", 1, false); // too recent -> kept
        upsert_entry_id(
            &conn,
            feed_id,
            "unread",
            Some("u"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(); // unread

        // Opt-in disabled (default 0): nothing pruned.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 0);

        // Enable retention at 30 days for the feed's owner.
        let user_id_check: i64 = conn.query_row(
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = ?1",
            rusqlite::params![feed_id], |r| r.get(0),
        ).unwrap();
        crate::models::user_settings::update_retention_read_days(&conn, user_id_check, 30).unwrap();

        // Only "old" is pruned; a tombstone is written for it.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 1);
        assert!(
            find_by_guid_and_feed(&conn, "old", feed_id)
                .unwrap()
                .is_none()
        );
        assert!(
            find_by_guid_and_feed(&conn, "oldstar", feed_id)
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_guid_and_feed(&conn, "fresh", feed_id)
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_guid_and_feed(&conn, "unread", feed_id)
                .unwrap()
                .is_some()
        );

        // Tombstone present -> a refresh serving "old" again is skipped.
        let outcome = upsert_entry_id(
            &conn,
            feed_id,
            "old",
            Some("Old"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));

        // Idempotent: nothing left to prune.
        assert_eq!(prune_read_retention_batch(&conn, 500).unwrap(), 0);
    }

    #[test]
    fn test_prune_batch_size_limits_rows() {
        let conn = setup_db();
        let user_id = create_test_user(&conn, "batchuser");
        let cat_id = create_test_category(&conn, user_id, "BatchCat");
        let feed_id = create_test_feed(&conn, cat_id, "https://example.com/batch.xml");
        let user_id_check: i64 = conn.query_row(
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = ?1",
            rusqlite::params![feed_id], |r| r.get(0),
        ).unwrap();
        crate::models::user_settings::update_retention_read_days(&conn, user_id_check, 1).unwrap();
        for i in 0..5 {
            let g = format!("g{i}");
            upsert_entry_id(&conn, feed_id, &g, Some(&g), None, None, None, None, None).unwrap();
            conn.execute(
                "UPDATE entry SET read_at = datetime('now', '-10 days') WHERE guid = ?1 AND feed_id = ?2",
                rusqlite::params![g, feed_id],
            ).unwrap();
        }
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 1);
        assert_eq!(prune_read_retention_batch(&conn, 2).unwrap(), 0);
    }
}
