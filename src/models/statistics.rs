use chrono::NaiveDate;
use rusqlite::{params, Connection};

use crate::error::AppResult;

/// Overview metrics for a user within a date range.
#[derive(Default)]
pub struct PersonalOverview {
    pub total_entries: i64,
    pub read_entries: i64,
    pub starred_entries: i64,
    pub summaries: i64,
}

impl PersonalOverview {
    /// Unread = total published in period minus those read in period.
    /// Clamped to 0 since total and read use different date columns.
    pub fn unread_entries(&self) -> i64 {
        (self.total_entries - self.read_entries).max(0)
    }

    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            ((self.read_entries as f64 / self.total_entries as f64) * 100.0).min(100.0)
        }
    }
}

/// A single day's read count.
pub struct DailyReadCount {
    pub date: NaiveDate,
    pub count: i64,
}

/// A category with its entry count.
pub struct CategoryCount {
    pub name: String,
    pub count: i64,
}

/// A feed with its entry count.
pub struct FeedCount {
    pub title: String,
    pub count: i64,
}

/// Admin site-wide counts (period-independent).
pub struct AdminCounts {
    pub total_users: i64,
    pub total_feeds: i64,
}

/// Admin site-wide entry stats (period-dependent).
pub struct AdminEntryStats {
    pub total_entries: i64,
    pub read_entries: i64,
}

impl AdminEntryStats {
    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            ((self.read_entries as f64 / self.total_entries as f64) * 100.0).min(100.0)
        }
    }
}

/// Get personal overview metrics for a user within a date range.
///
/// `from` and `to` are date strings in `YYYY-MM-DD` format. The range is
/// `[from, to)` — i.e. `from` is inclusive and `to` is exclusive.
pub fn get_personal_overview(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<PersonalOverview> {
    let total_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.read_at >= ?2
          AND e.read_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let starred_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.starred_at >= ?2
          AND e.starred_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let summaries: i64 = conn.query_row(
        r#"
        SELECT COUNT(es.id)
        FROM entry_summary es
        WHERE es.user_id = ?1
          AND es.status = 'completed'
          AND es.created_at >= ?2
          AND es.created_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    Ok(PersonalOverview {
        total_entries,
        read_entries,
        starred_entries,
        summaries,
    })
}

/// Get daily read counts for a user within a date range, including zero-count days.
///
/// `from` and `to` are date strings in `YYYY-MM-DD` format. The range is
/// `[from, to)`.
pub fn get_daily_read_counts(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<DailyReadCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT DATE(e.read_at) AS read_date, COUNT(e.id) AS cnt
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.read_at >= ?2
          AND e.read_at < ?3
        GROUP BY read_date
        ORDER BY read_date
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to], |row| {
        let date_str: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((date_str, count))
    })?;

    let mut counts_map = std::collections::HashMap::new();
    for row in rows {
        let (date_str, count) = row?;
        if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            counts_map.insert(date, count);
        }
    }

    let from_date = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Utc::now().date_naive());
    let to_date = NaiveDate::parse_from_str(to, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Utc::now().date_naive());

    let mut result = Vec::new();
    let mut current = from_date;
    while current < to_date {
        let count = counts_map.get(&current).copied().unwrap_or(0);
        result.push(DailyReadCount { date: current, count });
        current += chrono::Duration::days(1);
    }

    Ok(result)
}

/// Get entry counts grouped by category for a user within a date range.
///
/// Only categories with at least one entry are returned, ordered by count DESC.
pub fn get_entries_by_category(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<CategoryCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.name, COUNT(e.id) AS cnt
        FROM category c
        LEFT JOIN feed f ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
            AND COALESCE(e.published_at, e.created_at) >= ?2
            AND COALESCE(e.published_at, e.created_at) < ?3
        WHERE c.user_id = ?1
        GROUP BY c.id
        HAVING cnt > 0
        ORDER BY cnt DESC
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to], |row| {
        Ok(CategoryCount {
            name: row.get(0)?,
            count: row.get(1)?,
        })
    })?;

    let result = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

/// Get top feeds by entry count for a user within a date range.
///
/// `limit` caps the number of results. Only feeds with at least one entry are
/// returned, ordered by count DESC.
pub fn get_top_feeds(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
    limit: i64,
) -> AppResult<Vec<FeedCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT f.title, COUNT(e.id) AS cnt
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
            AND COALESCE(e.published_at, e.created_at) >= ?2
            AND COALESCE(e.published_at, e.created_at) < ?3
        WHERE c.user_id = ?1
        GROUP BY f.id
        HAVING cnt > 0
        ORDER BY cnt DESC
        LIMIT ?4
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to, limit], |row| {
        Ok(FeedCount {
            title: row.get(0)?,
            count: row.get(1)?,
        })
    })?;

    let result = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(result)
}

/// Get site-wide admin counts (period-independent).
pub fn get_admin_counts(conn: &Connection) -> AppResult<AdminCounts> {
    let total_users: i64 =
        conn.query_row("SELECT COUNT(*) FROM user", [], |row| row.get(0))?;

    let total_feeds: i64 =
        conn.query_row("SELECT COUNT(*) FROM feed", [], |row| row.get(0))?;

    Ok(AdminCounts { total_users, total_feeds })
}

/// Get site-wide admin entry stats within a date range.
pub fn get_admin_entry_stats(
    conn: &Connection,
    from: &str,
    to: &str,
) -> AppResult<AdminEntryStats> {
    let total_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= ?1
          AND COALESCE(published_at, created_at) < ?2
        "#,
        params![from, to],
        |row| row.get(0),
    )?;

    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(id)
        FROM entry
        WHERE read_at >= ?1
          AND read_at < ?2
        "#,
        params![from, to],
        |row| row.get(0),
    )?;

    Ok(AdminEntryStats { total_entries, read_entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();
        conn
    }

    fn create_user_with_data(conn: &Connection) -> i64 {
        let password_hash = crate::auth::hash_password("test").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, 'user')",
            params!["testuser", password_hash],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'Tech')",
            params![user_id],
        )
        .unwrap();
        let cat_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, 'https://example.com/feed', 'Test Feed')",
            params![cat_id],
        )
        .unwrap();
        user_id
    }

    /// Helper: get the feed_id for the first feed belonging to user's category.
    fn get_feed_id(conn: &Connection, user_id: i64) -> i64 {
        conn.query_row(
            "SELECT f.id FROM feed f INNER JOIN category c ON f.category_id = c.id WHERE c.user_id = ?1 LIMIT 1",
            params![user_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Helper: insert an entry with a specific published_at date (YYYY-MM-DD).
    fn insert_entry(conn: &Connection, feed_id: i64, guid: &str, published_at: &str) -> i64 {
        conn.execute(
            "INSERT INTO entry (feed_id, guid, published_at) VALUES (?1, ?2, ?3)",
            params![feed_id, guid, published_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Helper: mark entry as read at a specific datetime.
    fn mark_read(conn: &Connection, entry_id: i64, read_at: &str) {
        conn.execute(
            "UPDATE entry SET read_at = ?1 WHERE id = ?2",
            params![read_at, entry_id],
        )
        .unwrap();
    }

    /// Helper: mark entry as starred at a specific datetime.
    fn mark_starred(conn: &Connection, entry_id: i64, starred_at: &str) {
        conn.execute(
            "UPDATE entry SET starred_at = ?1 WHERE id = ?2",
            params![starred_at, entry_id],
        )
        .unwrap();
    }

    #[test]
    fn test_personal_overview_empty() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        let overview =
            get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 0);
        assert_eq!(overview.read_entries, 0);
        assert_eq!(overview.starred_entries, 0);
        assert_eq!(overview.summaries, 0);
        assert_eq!(overview.unread_entries(), 0);
        assert_eq!(overview.read_rate(), 0.0);
    }

    #[test]
    fn test_personal_overview_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        let e1 = insert_entry(&conn, feed_id, "g1", "2024-01-05");
        let e2 = insert_entry(&conn, feed_id, "g2", "2024-01-10");
        let e3 = insert_entry(&conn, feed_id, "g3", "2024-01-15");

        mark_read(&conn, e1, "2024-01-06");
        mark_read(&conn, e2, "2024-01-11");
        mark_starred(&conn, e3, "2024-01-16");

        let overview =
            get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 3);
        assert_eq!(overview.read_entries, 2);
        assert_eq!(overview.starred_entries, 1);
        assert_eq!(overview.unread_entries(), 1);
    }

    #[test]
    fn test_personal_overview_respects_date_range() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // Inside range
        insert_entry(&conn, feed_id, "g-in", "2024-01-15");
        // Outside range (before)
        insert_entry(&conn, feed_id, "g-before", "2023-12-31");
        // Outside range (on to boundary — exclusive)
        insert_entry(&conn, feed_id, "g-on-to", "2024-02-01");

        let overview =
            get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 1);
    }

    #[test]
    fn test_daily_read_counts_empty() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        let counts =
            get_daily_read_counts(&conn, user_id, "2024-01-01", "2024-01-04").unwrap();

        assert_eq!(counts.len(), 3); // Jan 1, 2, 3
        for c in &counts {
            assert_eq!(c.count, 0);
        }
    }

    #[test]
    fn test_daily_read_counts_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        let e1 = insert_entry(&conn, feed_id, "g1", "2024-01-01");
        let e2 = insert_entry(&conn, feed_id, "g2", "2024-01-01");
        let e3 = insert_entry(&conn, feed_id, "g3", "2024-01-02");

        mark_read(&conn, e1, "2024-01-02");
        mark_read(&conn, e2, "2024-01-02");
        mark_read(&conn, e3, "2024-01-03");

        let counts =
            get_daily_read_counts(&conn, user_id, "2024-01-01", "2024-01-05").unwrap();

        assert_eq!(counts.len(), 4); // Jan 1–4
        assert_eq!(counts[0].count, 0); // Jan 1: no reads on that day
        assert_eq!(counts[1].count, 2); // Jan 2: e1+e2 read
        assert_eq!(counts[2].count, 1); // Jan 3: e3 read
        assert_eq!(counts[3].count, 0); // Jan 4: nothing
    }

    #[test]
    fn test_entries_by_category() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // Add a second category + feed
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'Science')",
            params![user_id],
        )
        .unwrap();
        let cat2_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, 'https://science.com/feed', 'Science Feed')",
            params![cat2_id],
        )
        .unwrap();
        let feed2_id = conn.last_insert_rowid();

        // 2 entries in Tech, 1 in Science
        insert_entry(&conn, feed_id, "t1", "2024-01-05");
        insert_entry(&conn, feed_id, "t2", "2024-01-10");
        insert_entry(&conn, feed2_id, "s1", "2024-01-07");

        let counts =
            get_entries_by_category(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0].name, "Tech");
        assert_eq!(counts[0].count, 2);
        assert_eq!(counts[1].name, "Science");
        assert_eq!(counts[1].count, 1);
    }

    #[test]
    fn test_top_feeds() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        insert_entry(&conn, feed_id, "g1", "2024-01-05");

        let feeds = get_top_feeds(&conn, user_id, "2024-01-01", "2024-02-01", 10).unwrap();

        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].title, "Test Feed");
        assert_eq!(feeds[0].count, 1);
    }

    #[test]
    fn test_admin_counts() {
        let conn = setup_db();
        create_user_with_data(&conn);

        let counts = get_admin_counts(&conn).unwrap();

        assert_eq!(counts.total_users, 1);
        assert_eq!(counts.total_feeds, 1);
    }

    #[test]
    fn test_admin_entry_stats() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        let e1 = insert_entry(&conn, feed_id, "g1", "2024-01-05");
        let _e2 = insert_entry(&conn, feed_id, "g2", "2024-01-10");

        mark_read(&conn, e1, "2024-01-06");

        let stats = get_admin_entry_stats(&conn, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.read_entries, 1);
        assert!((stats.read_rate() - 50.0).abs() < 1e-6);
    }
}
