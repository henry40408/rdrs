use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

/// Parse Chinese month names to month number
fn parse_chinese_month(s: &str) -> Option<u32> {
    match s {
        "一月" => Some(1),
        "二月" => Some(2),
        "三月" => Some(3),
        "四月" => Some(4),
        "五月" => Some(5),
        "六月" => Some(6),
        "七月" => Some(7),
        "八月" => Some(8),
        "九月" => Some(9),
        "十月" => Some(10),
        "十一月" => Some(11),
        "十二月" => Some(12),
        _ => None,
    }
}

/// Parse Chinese date format like "週二, 6 一月 2026 14:28:00 +0000"
fn parse_chinese_datetime(s: &str) -> Option<DateTime<Utc>> {
    // Remove weekday prefix if present (e.g., "週二, " or "星期二, ")
    let s = s.trim();
    let s = if let Some(pos) = s.find(", ") {
        &s[pos + 2..]
    } else {
        s
    };

    // Expected format: "6 一月 2026 14:28:00 +0000"
    let parts: Vec<&str> = s.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = parse_chinese_month(parts[1])?;
    let year: i32 = parts[2].parse().ok()?;

    // Parse time and timezone: "14:28:00 +0000"
    let time_tz = parts[3];
    let time_parts: Vec<&str> = time_tz.splitn(2, ' ').collect();
    let time_str = time_parts.first()?;

    let time = NaiveTime::parse_from_str(time_str, "%H:%M:%S").ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive_dt = NaiveDateTime::new(date, time);

    // Parse timezone offset if present
    if let Some(tz_str) = time_parts.get(1) {
        if let Ok(offset) = parse_timezone_offset(tz_str) {
            let dt = DateTime::<FixedOffset>::from_naive_utc_and_offset(
                naive_dt - offset,
                FixedOffset::east_opt(0).unwrap(),
            );
            return Some(dt.with_timezone(&Utc));
        }
    }

    Some(naive_dt.and_utc())
}

/// Parse timezone offset like "+0000", "+0800", "-0500"
fn parse_timezone_offset(s: &str) -> Result<chrono::Duration, ()> {
    let s = s.trim();
    if s.len() < 5 {
        return Err(());
    }

    let sign = match s.chars().next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return Err(()),
    };

    let hours: i64 = s[1..3].parse().map_err(|_| ())?;
    let minutes: i64 = s[3..5].parse().map_err(|_| ())?;

    Ok(chrono::Duration::seconds(sign * (hours * 3600 + minutes * 60)))
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
    pub category_id: i64,
    pub category_name: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EntryFilter {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    pub unread_only: bool,
    pub starred_only: bool,
}

fn parse_datetime(s: &str) -> DateTime<Utc> {
    // Try RFC 3339 first (standard format)
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        // Then try SQL datetime format
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        // Then try dateparser for various formats (RFC 2822, localized dates, etc.)
        .or_else(|_| {
            dateparser::parse(s).map(|dt| dt.with_timezone(&Utc))
        })
        // Then try Chinese date format
        .or_else(|_| parse_chinese_datetime(s).ok_or(()))
        .unwrap_or_else(|_| Utc::now())
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
        category_id: row.get(15)?,
        category_name: row.get(16)?,
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
               f.title, f.url, c.id, c.name
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
        .filter_map(Result::ok)
        .collect();

    Ok(entries)
}

pub fn list_by_user(
    conn: &Connection,
    user_id: i64,
    filter: &EntryFilter,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<EntryWithFeed>> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

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

    let where_clause = conditions.join(" AND ");

    let sql = format!(
        r#"
        SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
               e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
               f.title, f.url, c.id, c.name
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE {}
        ORDER BY COALESCE(e.published_at, e.created_at) DESC
        LIMIT ?{} OFFSET ?{}
        "#,
        where_clause,
        params_vec.len() + 1,
        params_vec.len() + 2
    );

    params_vec.push(Box::new(limit));
    params_vec.push(Box::new(offset));

    let mut stmt = conn.prepare(&sql)?;

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    let entries = stmt
        .query_map(params_refs.as_slice(), row_to_entry_with_feed)?
        .filter_map(Result::ok)
        .collect();

    Ok(entries)
}

pub fn count_by_user(conn: &Connection, user_id: i64, filter: &EntryFilter) -> AppResult<i64> {
    let mut conditions = vec!["c.user_id = ?1".to_string()];
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(user_id)];

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
        // Update existing entry (preserve read_at and starred_at)
        conn.execute(
            r#"
            UPDATE entry
            SET title = ?1, link = ?2, content = ?3, summary = ?4, author = ?5,
                published_at = ?6, updated_at = datetime('now')
            WHERE id = ?7
            "#,
            params![
                title,
                link,
                content,
                summary,
                author,
                published_at_str,
                existing.id
            ],
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

pub fn mark_all_read_by_feed(conn: &Connection, feed_id: i64) -> AppResult<i64> {
    let rows = conn.execute(
        "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE feed_id = ?1 AND read_at IS NULL",
        params![feed_id],
    )?;

    Ok(rows as i64)
}

pub fn mark_all_read_by_user(conn: &Connection, user_id: i64) -> AppResult<i64> {
    let rows = conn.execute(
        r#"
        UPDATE entry
        SET read_at = datetime('now'), updated_at = datetime('now')
        WHERE read_at IS NULL AND feed_id IN (
            SELECT f.id FROM feed f
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = ?1
        )
        "#,
        params![user_id],
    )?;

    Ok(rows as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::models::category;
    use crate::models::feed;
    use chrono::{Datelike, Timelike};
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
        feed::create_feed(conn, category_id, url, Some("Test Feed"), None, None)
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
    fn test_parse_datetime_rfc3339() {
        let dt = parse_datetime("2026-01-06T14:28:00Z");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 28);
    }

    #[test]
    fn test_parse_datetime_sql_format() {
        let dt = parse_datetime("2026-01-06 14:28:00");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
    }

    #[test]
    fn test_parse_chinese_datetime_with_weekday() {
        let dt = parse_chinese_datetime("週二, 6 一月 2026 14:28:00 +0000");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 28);
    }

    #[test]
    fn test_parse_chinese_datetime_different_months() {
        assert!(parse_chinese_datetime("週一, 15 三月 2026 10:00:00 +0800").is_some());
        assert!(parse_chinese_datetime("週五, 25 十二月 2026 23:59:59 +0000").is_some());
        assert!(parse_chinese_datetime("週日, 1 七月 2026 00:00:00 -0500").is_some());
    }

    #[test]
    fn test_parse_chinese_month() {
        assert_eq!(parse_chinese_month("一月"), Some(1));
        assert_eq!(parse_chinese_month("六月"), Some(6));
        assert_eq!(parse_chinese_month("十二月"), Some(12));
        assert_eq!(parse_chinese_month("invalid"), None);
    }

    #[test]
    fn test_parse_timezone_offset() {
        assert_eq!(parse_timezone_offset("+0000").unwrap().num_seconds(), 0);
        assert_eq!(parse_timezone_offset("+0800").unwrap().num_seconds(), 8 * 3600);
        assert_eq!(parse_timezone_offset("-0500").unwrap().num_seconds(), -5 * 3600);
    }
}
