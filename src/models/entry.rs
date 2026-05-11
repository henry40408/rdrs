use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::utils::datetime::parse_datetime;

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
        r#"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               (SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) as has_icon,
               f.custom_referrer
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE e.id = ?1
        "#,
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
        r#"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
               f.custom_referrer
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE e.id = ?1 AND c.user_id = ?2
        "#,
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

    let sql = format!(
        r#"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
               f.custom_referrer
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
        WHERE {}
        ORDER BY {}
        LIMIT ?{} OFFSET ?{}
        "#,
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
        LEFT JOIN entry e ON e.feed_id = f.id AND e.read_at IS NULL
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
        LEFT JOIN entry e ON e.feed_id = f.id AND e.read_at IS NULL
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

pub fn count_by_feed(conn: &Connection, feed_id: i64) -> AppResult<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM entry WHERE feed_id = ?1",
        params![feed_id],
        |row| row.get(0),
    )?;

    Ok(count)
}

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
    let published_at_str = published_at.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string());

    // Try to find existing entry
    if let Some(existing) = find_by_guid_and_feed(conn, guid, feed_id)? {
        // Update existing entry (preserve read_at, starred_at, and published_at)
        // We don't update published_at because:
        // 1. The published date shouldn't change for existing entries
        // 2. Some feeds don't provide dates, causing fallback to current time on each refresh
        conn.execute(
            r#"
            UPDATE entry
            SET title = ?1, link = ?2, content = ?3, summary = ?4, author = ?5,
                updated_at = datetime('now')
            WHERE id = ?6
            "#,
            params![title, link, content, summary, author, existing.id],
        )?;

        let updated = find_by_id(conn, existing.id)?.ok_or(AppError::EntryNotFound)?;
        return Ok((updated, false));
    }

    // Insert new entry
    conn.execute(
        r#"
        INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            feed_id,
            guid,
            title,
            link,
            content,
            summary,
            author,
            published_at_str
        ],
    )?;

    let id = conn.last_insert_rowid();
    let entry = find_by_id(conn, id)?.ok_or(AppError::EntryNotFound)?;

    Ok((entry, true))
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

/// Toggle the starred state for an entry, scoped to the owning user.
///
/// Returns `None` if the entry does not exist or belongs to a different user
/// (callers treat both as 404). On success returns the updated `EntryWithFeed`.
pub fn toggle_starred(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntryWithFeed>> {
    let cur = find_by_id_for_user(conn, user_id, entry_id)?;
    let Some(e) = cur else {
        return Ok(None);
    };
    if e.entry.starred_at.is_some() {
        conn.execute(
            "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = ?1",
            params![entry_id],
        )?;
    } else {
        conn.execute(
            "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![entry_id],
        )?;
    }
    find_by_id_for_user(conn, user_id, entry_id)
}

/// Toggle the read state for an entry, scoped to the owning user.
///
/// Returns `None` if the entry does not exist or belongs to a different user.
pub fn toggle_read(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntryWithFeed>> {
    let cur = find_by_id_for_user(conn, user_id, entry_id)?;
    let Some(e) = cur else {
        return Ok(None);
    };
    if e.entry.read_at.is_some() {
        conn.execute(
            "UPDATE entry SET read_at = NULL, updated_at = datetime('now') WHERE id = ?1",
            params![entry_id],
        )?;
    } else {
        conn.execute(
            "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE id = ?1",
            params![entry_id],
        )?;
    }
    find_by_id_for_user(conn, user_id, entry_id)
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
         FROM entry e \
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
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               (SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) as has_icon,
               f.custom_referrer
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

    let sql = format!(
        r#"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, f.site_url, c.id, c.name,
               CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
               f.custom_referrer
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
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
    let entries = stmt
        .query_map(params_refs.as_slice(), row_to_entry_with_feed)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

/// Apply common filter conditions to query builder.
fn apply_filter_conditions(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    filter: &EntryFilter,
) {
    if let Some(feed_id) = filter.feed_id {
        conditions.push(format!("e.feed_id = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(feed_id));
    }

    if let Some(category_id) = filter.category_id {
        conditions.push(format!("c.id = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(category_id));
    }

    if filter.unread_only {
        conditions.push("e.read_at IS NULL".to_string());
    }

    if filter.starred_only {
        conditions.push("e.starred_at IS NOT NULL".to_string());
    }

    if filter.read_only {
        conditions.push("e.read_at IS NOT NULL".to_string());
    }

    if let Some(ref search) = filter.search {
        let search_pattern = format!("%{}%", search);
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "(e.title LIKE ?{} COLLATE NOCASE OR e.content LIKE ?{} COLLATE NOCASE)",
            param_idx, param_idx
        ));
        params_vec.push(Box::new(search_pattern));
    }

    if let Some(has_summary) = filter.has_summary {
        if has_summary {
            conditions.push(
                "EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = ?1 AND es.entry_id = e.id)".to_string()
            );
        } else {
            conditions.push(
                "NOT EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = ?1 AND es.entry_id = e.id)".to_string()
            );
        }
    }
}

/// Apply time range conditions (ot = oldest timestamp, nt = newest timestamp, in seconds).
fn apply_time_conditions(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ot: Option<i64>,
    nt: Option<i64>,
) {
    if let Some(oldest_ts) = ot {
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) >= ?{}",
            param_idx
        ));
        params_vec.push(Box::new(oldest_ts));
    }

    if let Some(newest_ts) = nt {
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) <= ?{}",
            param_idx
        ));
        params_vec.push(Box::new(newest_ts));
    }
}

/// Apply continuation-based pagination condition.
///
/// Composite cursor uses the V2 bounded-OR form, which the SQLite planner
/// can convert to an indexed range scan even when sort_ts is an expression
/// (`COALESCE(...)`). See PoC at `docs/superpowers/specs/2026-04-26-composite-cursor-pagination-design.md`.
fn apply_continuation_condition(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    continuation: Option<&ContinuationCursor>,
    sort_order: EntrySortOrder,
    oldest_first: bool,
) {
    let Some(cursor) = continuation else {
        return;
    };

    match cursor {
        ContinuationCursor::Composite { sort_ts, id } => {
            let sort_ts_expr = match sort_order {
                EntrySortOrder::ReadAt => "e.read_at",
                EntrySortOrder::StarredAt => "e.starred_at",
                EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at)",
            };
            let (cmp_outer, cmp_inner) = if oldest_first {
                (">=", ">")
            } else {
                ("<=", "<")
            };
            let ts1 = params_vec.len() + 1;
            let ts2 = params_vec.len() + 2;
            let id_idx = params_vec.len() + 3;
            conditions.push(format!(
                "{expr} {cmp_outer} ?{ts1} AND ({expr} {cmp_inner} ?{ts2} OR e.id {cmp_inner} ?{id_idx})",
                expr = sort_ts_expr,
                cmp_outer = cmp_outer,
                cmp_inner = cmp_inner,
                ts1 = ts1,
                ts2 = ts2,
                id_idx = id_idx,
            ));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(*id));
        }
        ContinuationCursor::LegacyId(id) => {
            let cmp = if oldest_first { ">" } else { "<" };
            let id_idx = params_vec.len() + 1;
            conditions.push(format!("e.id {} ?{}", cmp, id_idx));
            params_vec.push(Box::new(*id));
        }
    }
}

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
            })
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

    // Find previous entry (newer, comes before in DESC order)
    let prev_sql = format!(
        r#"
        SELECT e.id
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND COALESCE(e.published_at, e.created_at) > ?2
          {}
        ORDER BY COALESCE(e.published_at, e.created_at) ASC
        LIMIT 1
        "#,
        prev_extra
    );
    let prev_refs: Vec<&dyn rusqlite::ToSql> = prev_params.iter().map(|p| p.as_ref()).collect();
    let prev_id: Option<i64> = conn
        .query_row(&prev_sql, prev_refs.as_slice(), |row| row.get(0))
        .optional()?;

    // Find next entry (older, comes after in DESC order)
    let next_sql = format!(
        r#"
        SELECT e.id
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND (COALESCE(e.published_at, e.created_at) < ?2
               OR (COALESCE(e.published_at, e.created_at) = ?2 AND e.id < ?3))
          {}
        ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
        LIMIT 1
        "#,
        next_extra
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
pub fn mark_read_by_ids(conn: &Connection, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    if entry_ids.is_empty() {
        return Ok(0);
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> = entry_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        r#"
        UPDATE entry
        SET read_at = datetime('now'), updated_at = datetime('now')
        WHERE read_at IS NULL
          AND id IN ({})
          AND feed_id IN (
              SELECT f.id FROM feed f
              INNER JOIN category c ON f.category_id = c.id
              WHERE c.user_id = ?1
          )
        "#,
        in_clause
    );

    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];
    for id in entry_ids {
        params_vec.push(Box::new(*id));
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let rows = conn.execute(&sql, params_refs.as_slice())?;
    Ok(rows as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::category;
    use crate::models::feed;
    use crate::models::user::{self, Role};

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
}
