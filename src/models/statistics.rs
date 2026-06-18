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
    /// Unread = entries published in the period that are not yet read.
    ///
    /// `read_entries` counts the read subset of the *same* publish cohort as
    /// `total_entries`, so this difference is always non-negative — no clamp
    /// needed (and none wanted: a clamp would mask a future cohort regression).
    pub fn unread_entries(&self) -> i64 {
        self.total_entries - self.read_entries
    }

    /// Fraction of period-published entries that have been read.
    ///
    /// Naturally bounded to 0–100% because read is a subset of total.
    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            (self.read_entries as f64 / self.total_entries as f64) * 100.0
        }
    }
}

/// A single day's read count.
pub struct DailyReadCount {
    pub date: NaiveDate,
    pub count: i64,
}

/// A contiguous span of days collapsed into one chart bar.
///
/// `start == end` when the bucket covers a single day; otherwise it spans
/// `[start, end]` inclusive and `count` is the sum over those days.
pub struct DailyBucket {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub count: i64,
}

/// Collapse per-day read counts into at most `max_bars` contiguous buckets so
/// a dense date range stays readable (and tappable) as a fixed number of bars.
///
/// Each bucket covers `ceil(len / max_bars)` consecutive days; when the input
/// already fits within `max_bars`, every bucket is a single day (no change).
/// Input is assumed chronologically ordered, as produced by
/// [`get_daily_read_counts`].
pub fn bucket_daily_counts(daily: &[DailyReadCount], max_bars: usize) -> Vec<DailyBucket> {
    let max_bars = max_bars.max(1);
    if daily.is_empty() {
        return Vec::new();
    }
    let bucket_size = daily.len().div_ceil(max_bars);
    daily
        .chunks(bucket_size)
        .map(|chunk| DailyBucket {
            start: chunk.first().expect("chunk is non-empty").date,
            end: chunk.last().expect("chunk is non-empty").date,
            count: chunk.iter().map(|d| d.count).sum(),
        })
        .collect()
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
    /// Fraction of period-published entries (site-wide) that have been read.
    ///
    /// Naturally bounded to 0–100% because read is a subset of total.
    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            (self.read_entries as f64 / self.total_entries as f64) * 100.0
        }
    }
}

/// Admin database storage + record stats (period-independent).
pub struct AdminDatabaseStats {
    pub db_size_bytes: i64,
    pub reclaimable_bytes: i64,
    pub fragmentation_ratio: f64,
    pub total_entries: i64,
    pub avg_new_entries_per_day: f64,
    pub coverage_days: f64,
    pub tombstone_count: i64,
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

    // Read/starred counts are the read/starred *subset of the same publish
    // cohort* as total_entries — i.e. entries published in the period that
    // have since been read/starred (whenever) — not "reading activity in the
    // period". This keeps Read ⊆ Total so Unread and Read Rate stay coherent.
    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
          AND e.read_at IS NOT NULL
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
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
          AND e.starred_at IS NOT NULL
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
        result.push(DailyReadCount {
            date: current,
            count,
        });
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
    let total_users: i64 = conn.query_row("SELECT COUNT(*) FROM user", [], |row| row.get(0))?;

    let total_feeds: i64 = conn.query_row("SELECT COUNT(*) FROM feed", [], |row| row.get(0))?;

    Ok(AdminCounts {
        total_users,
        total_feeds,
    })
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

    // Read subset of the same publish cohort as total_entries (see
    // get_personal_overview), so Site Read Rate stays bounded to 0–100%.
    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= ?1
          AND COALESCE(published_at, created_at) < ?2
          AND read_at IS NOT NULL
        "#,
        params![from, to],
        |row| row.get(0),
    )?;

    Ok(AdminEntryStats {
        total_entries,
        read_entries,
    })
}

/// Get site-wide database storage + record stats (period-independent).
pub fn get_admin_database_stats(conn: &Connection) -> AppResult<AdminDatabaseStats> {
    let page_count: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let freelist: i64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;

    let db_size_bytes = page_count * page_size;
    let reclaimable_bytes = freelist * page_size;
    let fragmentation_ratio = if db_size_bytes > 0 {
        reclaimable_bytes as f64 / db_size_bytes as f64
    } else {
        0.0
    };

    let total_entries: i64 = conn.query_row("SELECT COUNT(*) FROM entry", [], |row| row.get(0))?;
    // Bare MIN/MAX so SQLite uses the idx_entry_created_at endpoint optimization.
    let min_created: Option<String> =
        conn.query_row("SELECT MIN(created_at) FROM entry", [], |row| row.get(0))?;
    let max_created: Option<String> =
        conn.query_row("SELECT MAX(created_at) FROM entry", [], |row| row.get(0))?;
    let tombstone_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM entry_tombstone", [], |row| row.get(0))?;

    // Use the fallible parser (not parse_datetime, whose Utc::now() fallback
    // would silently corrupt these aggregates on an unparseable timestamp).
    let (coverage_days, avg_new_entries_per_day) = match (
        min_created
            .as_deref()
            .and_then(crate::utils::datetime::try_parse_datetime),
        max_created
            .as_deref()
            .and_then(crate::utils::datetime::try_parse_datetime),
    ) {
        (Some(min), Some(max)) => {
            let coverage = (max - min).num_seconds() as f64 / 86_400.0;
            // Average over the span we actually retain entries for, not the
            // age since the oldest entry: retention prunes read entries, so
            // `total_entries / age` systematically understated the rate (old
            // unread entries stretch the denominator while their pruned
            // neighbours are gone from the numerator). Numerator and
            // denominator now cover the same retained set. Guard a sub-day
            // span (single entry, or all created the same day) at 1 day.
            let avg = total_entries as f64 / coverage.max(1.0);
            (coverage, avg)
        }
        _ => (0.0, 0.0),
    };

    Ok(AdminDatabaseStats {
        db_size_bytes,
        reclaimable_bytes,
        fragmentation_ratio,
        total_entries,
        avg_new_entries_per_day,
        coverage_days,
        tombstone_count,
    })
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

    /// Helper: insert an entry with an explicit created_at (YYYY-MM-DD HH:MM:SS).
    fn insert_entry_created_at(
        conn: &Connection,
        feed_id: i64,
        guid: &str,
        created_at: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO entry (feed_id, guid, created_at) VALUES (?1, ?2, ?3)",
            params![feed_id, guid, created_at],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// Helper: insert a tombstone row.
    fn insert_tombstone(conn: &Connection, feed_id: i64, guid: &str) {
        conn.execute(
            "INSERT INTO entry_tombstone (feed_id, guid) VALUES (?1, ?2)",
            params![feed_id, guid],
        )
        .unwrap();
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

        let overview = get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

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

        let overview = get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 3);
        assert_eq!(overview.read_entries, 2);
        assert_eq!(overview.starred_entries, 1);
        assert_eq!(overview.unread_entries(), 1);
    }

    #[test]
    fn test_personal_overview_uses_publish_cohort() {
        // Read/starred counts must be the read/starred subset of the entries
        // *published in the period* — not "activity in the period". This pins
        // the fix for Unread always 0 / Read Rate always 100%.
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // Published BEFORE the period but read+starred DURING it. The old
        // activity-based query counted these; the publish cohort must not.
        let old = insert_entry(&conn, feed_id, "old", "2023-12-01");
        mark_read(&conn, old, "2024-01-15");
        mark_starred(&conn, old, "2024-01-16");

        // In cohort, read inside the period, also starred.
        let e1 = insert_entry(&conn, feed_id, "e1", "2024-01-05");
        mark_read(&conn, e1, "2024-01-06");
        mark_starred(&conn, e1, "2024-01-07");

        // In cohort, never read → unread.
        insert_entry(&conn, feed_id, "e2", "2024-01-10");

        // In cohort, read AFTER the period ends → still "read" (read_at set).
        let e3 = insert_entry(&conn, feed_id, "e3", "2024-01-20");
        mark_read(&conn, e3, "2024-03-01");

        let overview = get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 3, "old (Dec) entry excluded");
        assert_eq!(overview.read_entries, 2, "e1 + e3, not the Dec entry");
        assert_eq!(overview.starred_entries, 1, "only e1, not the Dec entry");
        assert_eq!(overview.unread_entries(), 1, "e2");
        assert!(
            (overview.read_rate() - (2.0 / 3.0 * 100.0)).abs() < 1e-6,
            "read_rate was {}",
            overview.read_rate()
        );
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

        let overview = get_personal_overview(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(overview.total_entries, 1);
    }

    #[test]
    fn test_daily_read_counts_empty() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        let counts = get_daily_read_counts(&conn, user_id, "2024-01-01", "2024-01-04").unwrap();

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

        let counts = get_daily_read_counts(&conn, user_id, "2024-01-01", "2024-01-05").unwrap();

        assert_eq!(counts.len(), 4); // Jan 1–4
        assert_eq!(counts[0].count, 0); // Jan 1: no reads on that day
        assert_eq!(counts[1].count, 2); // Jan 2: e1+e2 read
        assert_eq!(counts[2].count, 1); // Jan 3: e3 read
        assert_eq!(counts[3].count, 0); // Jan 4: nothing
    }

    /// Build a chronological run of `DailyReadCount`s starting at `2024-01-01`,
    /// one per element of `counts`.
    fn daily_run(counts: &[i64]) -> Vec<DailyReadCount> {
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        counts
            .iter()
            .enumerate()
            .map(|(i, &count)| DailyReadCount {
                date: start + chrono::Duration::days(i as i64),
                count,
            })
            .collect()
    }

    #[test]
    fn test_bucket_daily_counts_empty() {
        assert!(bucket_daily_counts(&[], 14).is_empty());
    }

    #[test]
    fn test_bucket_daily_counts_no_aggregation_within_max() {
        let daily = daily_run(&[0, 2, 1, 0]);
        let buckets = bucket_daily_counts(&daily, 14);

        assert_eq!(buckets.len(), 4);
        for (i, b) in buckets.iter().enumerate() {
            assert_eq!(b.start, daily[i].date);
            assert_eq!(b.end, daily[i].date, "single-day bucket spans one day");
            assert_eq!(b.count, daily[i].count);
        }
    }

    #[test]
    fn test_bucket_daily_counts_aggregates_over_max() {
        // 15 days > max 14 → bucket_size = ceil(15/14) = 2 → ceil(15/2) = 8 buckets.
        let daily = daily_run(&[1; 15]);
        let buckets = bucket_daily_counts(&daily, 14);

        assert_eq!(buckets.len(), 8);
        // First bucket spans the first two days, summed.
        assert_eq!(
            buckets[0].start,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()
        );
        assert_eq!(buckets[0].end, NaiveDate::from_ymd_opt(2024, 1, 2).unwrap());
        assert_eq!(buckets[0].count, 2);
        // Every bucket holds at most 14 bars and no bucket exceeds bucket_size days.
        assert!(buckets.len() <= 14);
    }

    #[test]
    fn test_bucket_daily_counts_last_bucket_partial() {
        // 15 days, size 2 → last (8th) bucket has a single leftover day.
        let daily = daily_run(&[1; 15]);
        let buckets = bucket_daily_counts(&daily, 14);

        let last = buckets.last().unwrap();
        assert_eq!(last.start, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(last.end, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(last.count, 1);
    }

    #[test]
    fn test_bucket_daily_counts_sums_within_bucket() {
        // 28 days → size = ceil(28/14) = 2; counts 1..=28 → bucket 0 = 1+2 = 3.
        let counts: Vec<i64> = (1..=28).collect();
        let daily = daily_run(&counts);
        let buckets = bucket_daily_counts(&daily, 14);

        assert_eq!(buckets.len(), 14);
        assert_eq!(buckets[0].count, 3); // 1 + 2
        assert_eq!(buckets[13].count, 55); // 27 + 28
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

        let counts = get_entries_by_category(&conn, user_id, "2024-01-01", "2024-02-01").unwrap();

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

        // Published before the period but read inside it: the old query
        // counted this toward read_entries and could push the rate past 100%.
        let old = insert_entry(&conn, feed_id, "g-old", "2023-12-01");
        mark_read(&conn, old, "2024-01-07");

        let stats = get_admin_entry_stats(&conn, "2024-01-01", "2024-02-01").unwrap();

        assert_eq!(stats.total_entries, 2, "Dec entry is outside the period");
        assert_eq!(stats.read_entries, 1, "only g1; the Dec entry is excluded");
        assert!((stats.read_rate() - 50.0).abs() < 1e-6);
    }

    #[test]
    fn test_admin_database_stats_empty() {
        let conn = setup_db();
        create_user_with_data(&conn);

        let s = get_admin_database_stats(&conn).unwrap();

        // A freshly-initialized DB still has pages, so size is positive.
        assert!(s.db_size_bytes > 0);
        assert!(s.reclaimable_bytes >= 0);
        assert!((0.0..=1.0).contains(&s.fragmentation_ratio));
        // No entries / tombstones yet → record metrics are zero.
        assert_eq!(s.total_entries, 0);
        assert_eq!(s.coverage_days, 0.0);
        assert_eq!(s.avg_new_entries_per_day, 0.0);
        assert_eq!(s.tombstone_count, 0);
    }

    #[test]
    fn test_admin_database_stats_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // 4 entries spanning exactly 3 days (2024-01-01 .. 2024-01-04).
        insert_entry_created_at(&conn, feed_id, "a", "2024-01-01 00:00:00");
        insert_entry_created_at(&conn, feed_id, "b", "2024-01-02 00:00:00");
        insert_entry_created_at(&conn, feed_id, "c", "2024-01-03 00:00:00");
        insert_entry_created_at(&conn, feed_id, "d", "2024-01-04 00:00:00");

        insert_tombstone(&conn, feed_id, "dead-1");
        insert_tombstone(&conn, feed_id, "dead-2");

        let s = get_admin_database_stats(&conn).unwrap();

        assert_eq!(s.total_entries, 4);
        assert_eq!(s.tombstone_count, 2);
        // span = 2024-01-04 - 2024-01-01 = 3 days exactly.
        assert!(
            (s.coverage_days - 3.0).abs() < 1e-6,
            "coverage was {}",
            s.coverage_days
        );
        // avg = retained entries / coverage span = 4 / 3. Now deterministic
        // (no Utc::now() in the denominator) and unaffected by prune drift.
        assert!(
            (s.avg_new_entries_per_day - 4.0 / 3.0).abs() < 1e-6,
            "avg was {}",
            s.avg_new_entries_per_day
        );
    }

    #[test]
    fn test_admin_database_stats_avg_guards_subday_span() {
        // A single entry → coverage span 0 → denominator guarded at 1 day so
        // the average is finite (and equals the entry count) rather than inf.
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        insert_entry_created_at(&conn, feed_id, "only", "2024-01-01 12:00:00");

        let s = get_admin_database_stats(&conn).unwrap();

        assert_eq!(s.total_entries, 1);
        assert_eq!(s.coverage_days, 0.0);
        assert!((s.avg_new_entries_per_day - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_admin_database_stats_parses_rfc3339_created_at() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);
        let feed_id = get_feed_id(&conn, user_id);

        // RFC 3339 timestamps spanning exactly 2 days. The previous SQL-only
        // parser would have failed these and collapsed coverage to 0.0.
        insert_entry_created_at(&conn, feed_id, "a", "2024-01-01T00:00:00Z");
        insert_entry_created_at(&conn, feed_id, "b", "2024-01-03T00:00:00Z");

        let s = get_admin_database_stats(&conn).unwrap();

        assert_eq!(s.total_entries, 2);
        assert!(
            (s.coverage_days - 2.0).abs() < 1e-6,
            "coverage was {}",
            s.coverage_days
        );
        // avg = 2 entries / 2-day span = 1.0 exactly.
        assert!(
            (s.avg_new_entries_per_day - 1.0).abs() < 1e-6,
            "avg was {}",
            s.avg_new_entries_per_day
        );
    }
}
