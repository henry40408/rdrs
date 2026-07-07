use chrono::NaiveDate;

use crate::db::{Db, DbInner};
use crate::error::{AppError, AppResult};
use crate::{query_all, query_scalar};

/// Render a timestamp column/expression as the `%Y-%m-%d %H:%M:%S` TEXT this
/// module reads into `String`. `SQLite` stores exactly that TEXT (pass-through);
/// PG columns are `TIMESTAMPTZ`, so wrap in `to_char(..., 'YYYY-MM-DD
/// HH24:MI:SS')` (UTC session) — a bare read would fail to decode a timestamptz
/// into `String`. Mirrors `entry::filters::Dialect::cursor_ts`.
fn ts_text(db: &Db, expr: &str) -> String {
    if db.is_postgres() {
        format!("to_char({expr}, 'YYYY-MM-DD HH24:MI:SS')")
    } else {
        expr.to_string()
    }
}

/// Parse a `YYYY-MM-DD` date-range bound for binding. As a `NaiveDate` it
/// compares correctly against the timestamp columns on both backends: `SQLite`
/// compares the `YYYY-MM-DD` TEXT lexicographically against the stored
/// `%Y-%m-%d %H:%M:%S`, and `PostgreSQL` implicitly casts `date` to
/// `timestamptz` — a raw `%Y-%m-%d` *string* bind would not coerce against a
/// timestamptz column. Falls back to today on an unparseable input (matching the
/// range-fill fallback used when charting).
fn parse_ymd(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_else(|_| chrono::Utc::now().date_naive())
}

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
#[derive(sqlx::FromRow)]
pub struct CategoryCount {
    pub name: String,
    pub count: i64,
}

/// A feed with its entry count.
#[derive(sqlx::FromRow)]
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
pub async fn get_personal_overview(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<PersonalOverview> {
    // Bind the range bounds as dates (see `parse_ymd`) so the `>= $2 / < $3`
    // comparisons against the timestamp columns work on both backends.
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    let total_entries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
        "#,
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    // Read/starred counts are the read/starred *subset of the same publish
    // cohort* as total_entries — i.e. entries published in the period that
    // have since been read/starred (whenever) — not "reading activity in the
    // period". This keeps Read ⊆ Total so Unread and Read Rate stay coherent.
    let read_entries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
          AND e.read_at IS NOT NULL
        "#,
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    let starred_entries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
          AND e.starred_at IS NOT NULL
        "#,
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    let summaries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(es.id)
        FROM entry_summary es
        WHERE es.user_id = $1
          AND es.status = 'completed'
          AND es.created_at >= $2
          AND es.created_at < $3
        "#,
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)?;

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
pub async fn get_daily_read_counts(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<DailyReadCount>> {
    // Ad-hoc `(date_string, count)` rows → fetch as a tuple and post-process in
    // Rust (fill zero-count days below). Only the day bucket dialect-forks:
    // SQLite's `DATE()` vs PG's `to_char(...)`. The range bounds are bound as
    // dates (see `parse_ymd`) so the raw `read_at >= $2 / < $3` comparison works
    // on both backends without wrapping the column.
    let day_bucket = if db.is_postgres() {
        "to_char(e.read_at, 'YYYY-MM-DD')".to_string()
    } else {
        "DATE(e.read_at)".to_string()
    };
    let (from_d, to_d) = (parse_ymd(from), parse_ymd(to));
    let sql = format!(
        "SELECT {day_bucket} AS read_date, COUNT(e.id) AS cnt \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1 \
           AND e.read_at >= $2 \
           AND e.read_at < $3 \
         GROUP BY read_date \
         ORDER BY read_date"
    );
    let rows: Vec<(String, i64)> = match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_as::<sqlx::Sqlite, (String, i64)>(sqlx::AssertSqlSafe(sql))
                .bind(user_id)
                .bind(from_d)
                .bind(to_d)
                .fetch_all(pool)
                .await
        }
        DbInner::Postgres(pool) => {
            sqlx::query_as::<sqlx::Postgres, (String, i64)>(sqlx::AssertSqlSafe(sql))
                .bind(user_id)
                .bind(from_d)
                .bind(to_d)
                .fetch_all(pool)
                .await
        }
    }
    .map_err(AppError::Database)?;

    let mut counts_map = std::collections::HashMap::new();
    for (date_str, count) in rows {
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
pub async fn get_entries_by_category(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<CategoryCount>> {
    // `AS count` so `FromRow` (column-name match) populates `CategoryCount.count`;
    // HAVING/ORDER BY reference the aggregate directly (portable, alias-free).
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    query_all!(
        db,
        CategoryCount,
        r#"
        SELECT c.name, COUNT(e.id) AS count
        FROM category c
        LEFT JOIN feed f ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
            AND COALESCE(e.published_at, e.created_at) >= $2
            AND COALESCE(e.published_at, e.created_at) < $3
        WHERE c.user_id = $1
        GROUP BY c.id
        HAVING COUNT(e.id) > 0
        ORDER BY COUNT(e.id) DESC
        "#,
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)
}

/// Get top feeds by entry count for a user within a date range.
///
/// `limit` caps the number of results. Only feeds with at least one entry are
/// returned, ordered by count DESC.
pub async fn get_top_feeds(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
    limit: i64,
) -> AppResult<Vec<FeedCount>> {
    // `AS count` so `FromRow` (column-name match) populates `FeedCount.count`;
    // HAVING/ORDER BY reference the aggregate directly (portable, alias-free).
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    query_all!(
        db,
        FeedCount,
        r#"
        SELECT f.title, COUNT(e.id) AS count
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
            AND COALESCE(e.published_at, e.created_at) >= $2
            AND COALESCE(e.published_at, e.created_at) < $3
        WHERE c.user_id = $1
        GROUP BY f.id
        HAVING COUNT(e.id) > 0
        ORDER BY COUNT(e.id) DESC
        LIMIT $4
        "#,
        user_id,
        from,
        to,
        limit,
    )
    .map_err(AppError::Database)
}

/// Get site-wide admin counts (period-independent).
pub async fn get_admin_counts(db: &Db) -> AppResult<AdminCounts> {
    let total_users: i64 =
        query_scalar!(db, i64, "SELECT COUNT(*) FROM \"user\"").map_err(AppError::Database)?;

    let total_feeds: i64 =
        query_scalar!(db, i64, "SELECT COUNT(*) FROM feed").map_err(AppError::Database)?;

    Ok(AdminCounts {
        total_users,
        total_feeds,
    })
}

/// Get site-wide admin entry stats within a date range.
pub async fn get_admin_entry_stats(db: &Db, from: &str, to: &str) -> AppResult<AdminEntryStats> {
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    let total_entries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= $1
          AND COALESCE(published_at, created_at) < $2
        "#,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    // Read subset of the same publish cohort as total_entries (see
    // get_personal_overview), so Site Read Rate stays bounded to 0–100%.
    let read_entries: i64 = query_scalar!(
        db,
        i64,
        r#"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= $1
          AND COALESCE(published_at, created_at) < $2
          AND read_at IS NOT NULL
        "#,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    Ok(AdminEntryStats {
        total_entries,
        read_entries,
    })
}

/// Get site-wide database storage + record stats (period-independent).
pub async fn get_admin_database_stats(db: &Db) -> AppResult<AdminDatabaseStats> {
    // Storage stats dialect-fork. SQLite exposes page-level accounting via
    // PRAGMAs (total size = page_count * page_size; reclaimable = freelist *
    // page_size). PostgreSQL reports the on-disk database size via
    // `pg_database_size()`; it has no directly comparable freelist/reclaimable
    // figure (bloat is a VACUUM concern), so reclaimable is reported as 0.
    let (db_size_bytes, reclaimable_bytes) = match db.inner() {
        DbInner::Sqlite(pool) => {
            let page_count = sqlx::query_scalar::<_, i64>("PRAGMA page_count")
                .fetch_one(pool)
                .await
                .map_err(AppError::Database)?;
            let page_size = sqlx::query_scalar::<_, i64>("PRAGMA page_size")
                .fetch_one(pool)
                .await
                .map_err(AppError::Database)?;
            let freelist = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
                .fetch_one(pool)
                .await
                .map_err(AppError::Database)?;
            (page_count * page_size, freelist * page_size)
        }
        DbInner::Postgres(pool) => {
            let size = sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())")
                .fetch_one(pool)
                .await
                .map_err(AppError::Database)?;
            (size, 0)
        }
    };

    let fragmentation_ratio = if db_size_bytes > 0 {
        reclaimable_bytes as f64 / db_size_bytes as f64
    } else {
        0.0
    };

    let total_entries: i64 =
        query_scalar!(db, i64, "SELECT COUNT(*) FROM entry").map_err(AppError::Database)?;
    // Bare MIN/MAX so SQLite uses the idx_entry_created_at endpoint
    // optimization; the timestamp is read back as the `%Y-%m-%d %H:%M:%S` TEXT
    // `try_parse_datetime` expects (to_char on PG — see `ts_text`).
    let min_sql = format!("SELECT {} FROM entry", ts_text(db, "MIN(created_at)"));
    let max_sql = format!("SELECT {} FROM entry", ts_text(db, "MAX(created_at)"));
    let min_created: Option<String> = match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(min_sql))
                .fetch_one(pool)
                .await
        }
        DbInner::Postgres(pool) => {
            sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(min_sql))
                .fetch_one(pool)
                .await
        }
    }
    .map_err(AppError::Database)?;
    let max_created: Option<String> = match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(max_sql))
                .fetch_one(pool)
                .await
        }
        DbInner::Postgres(pool) => {
            sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(max_sql))
                .fetch_one(pool)
                .await
        }
    }
    .map_err(AppError::Database)?;
    let tombstone_count: i64 = query_scalar!(db, i64, "SELECT COUNT(*) FROM entry_tombstone")
        .map_err(AppError::Database)?;

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
    use crate::models::user::Role;
    use crate::models::{category, feed, user};

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    async fn create_user_with_data(db: &Db) -> i64 {
        let user_id = user::create_user(db, "testuser", "hash", Role::User)
            .await
            .unwrap()
            .id;
        let cat = category::create_category(db, user_id, "Tech")
            .await
            .unwrap();
        feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url: "https://example.com/feed",
                title: Some("Test Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap();
        user_id
    }

    /// Helper: get the `feed_id` for the first feed belonging to user's category.
    async fn get_feed_id(db: &Db, user_id: i64) -> i64 {
        query_scalar!(
            db,
            i64,
            "SELECT f.id FROM feed f INNER JOIN category c ON f.category_id = c.id WHERE c.user_id = $1 LIMIT 1",
            user_id,
        )
        .unwrap()
    }

    /// Helper: insert an entry with a specific `published_at` date (YYYY-MM-DD).
    async fn insert_entry(db: &Db, feed_id: i64, guid: &str, published_at: &str) -> i64 {
        query_scalar!(
            db,
            i64,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3) RETURNING id",
            feed_id,
            guid,
            published_at,
        )
        .unwrap()
    }

    /// Helper: insert an entry with an explicit `created_at` (YYYY-MM-DD HH:MM:SS).
    async fn insert_entry_created_at(db: &Db, feed_id: i64, guid: &str, created_at: &str) -> i64 {
        query_scalar!(
            db,
            i64,
            "INSERT INTO entry (feed_id, guid, created_at) VALUES ($1, $2, $3) RETURNING id",
            feed_id,
            guid,
            created_at,
        )
        .unwrap()
    }

    /// Helper: insert a tombstone row.
    async fn insert_tombstone(db: &Db, feed_id: i64, guid: &str) {
        crate::db_execute!(
            db,
            "INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, $2)",
            feed_id,
            guid,
        )
        .unwrap();
    }

    /// Helper: mark entry as read at a specific datetime.
    async fn mark_read(db: &Db, entry_id: i64, read_at: &str) {
        crate::db_execute!(
            db,
            "UPDATE entry SET read_at = $1 WHERE id = $2",
            read_at,
            entry_id,
        )
        .unwrap();
    }

    /// Helper: mark entry as starred at a specific datetime.
    async fn mark_starred(db: &Db, entry_id: i64, starred_at: &str) {
        crate::db_execute!(
            db,
            "UPDATE entry SET starred_at = $1 WHERE id = $2",
            starred_at,
            entry_id,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_personal_overview_empty() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;

        let overview = get_personal_overview(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

        assert_eq!(overview.total_entries, 0);
        assert_eq!(overview.read_entries, 0);
        assert_eq!(overview.starred_entries, 0);
        assert_eq!(overview.summaries, 0);
        assert_eq!(overview.unread_entries(), 0);
        assert_eq!(overview.read_rate(), 0.0);
    }

    #[tokio::test]
    async fn test_personal_overview_with_data() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        let e1 = insert_entry(&db, feed_id, "g1", "2024-01-05").await;
        let e2 = insert_entry(&db, feed_id, "g2", "2024-01-10").await;
        let e3 = insert_entry(&db, feed_id, "g3", "2024-01-15").await;

        mark_read(&db, e1, "2024-01-06").await;
        mark_read(&db, e2, "2024-01-11").await;
        mark_starred(&db, e3, "2024-01-16").await;

        let overview = get_personal_overview(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

        assert_eq!(overview.total_entries, 3);
        assert_eq!(overview.read_entries, 2);
        assert_eq!(overview.starred_entries, 1);
        assert_eq!(overview.unread_entries(), 1);
    }

    #[tokio::test]
    async fn test_personal_overview_uses_publish_cohort() {
        // Read/starred counts must be the read/starred subset of the entries
        // *published in the period* — not "activity in the period". This pins
        // the fix for Unread always 0 / Read Rate always 100%.
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // Published BEFORE the period but read+starred DURING it. The old
        // activity-based query counted these; the publish cohort must not.
        let old = insert_entry(&db, feed_id, "old", "2023-12-01").await;
        mark_read(&db, old, "2024-01-15").await;
        mark_starred(&db, old, "2024-01-16").await;

        // In cohort, read inside the period, also starred.
        let e1 = insert_entry(&db, feed_id, "e1", "2024-01-05").await;
        mark_read(&db, e1, "2024-01-06").await;
        mark_starred(&db, e1, "2024-01-07").await;

        // In cohort, never read → unread.
        insert_entry(&db, feed_id, "e2", "2024-01-10").await;

        // In cohort, read AFTER the period ends → still "read" (read_at set).
        let e3 = insert_entry(&db, feed_id, "e3", "2024-01-20").await;
        mark_read(&db, e3, "2024-03-01").await;

        let overview = get_personal_overview(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

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

    #[tokio::test]
    async fn test_personal_overview_respects_date_range() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // Inside range
        insert_entry(&db, feed_id, "g-in", "2024-01-15").await;
        // Outside range (before)
        insert_entry(&db, feed_id, "g-before", "2023-12-31").await;
        // Outside range (on to boundary — exclusive)
        insert_entry(&db, feed_id, "g-on-to", "2024-02-01").await;

        let overview = get_personal_overview(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

        assert_eq!(overview.total_entries, 1);
    }

    #[tokio::test]
    async fn test_daily_read_counts_empty() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;

        let counts = get_daily_read_counts(&db, user_id, "2024-01-01", "2024-01-04")
            .await
            .unwrap();

        assert_eq!(counts.len(), 3); // Jan 1, 2, 3
        for c in &counts {
            assert_eq!(c.count, 0);
        }
    }

    #[tokio::test]
    async fn test_daily_read_counts_with_data() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        let e1 = insert_entry(&db, feed_id, "g1", "2024-01-01").await;
        let e2 = insert_entry(&db, feed_id, "g2", "2024-01-01").await;
        let e3 = insert_entry(&db, feed_id, "g3", "2024-01-02").await;

        mark_read(&db, e1, "2024-01-02").await;
        mark_read(&db, e2, "2024-01-02").await;
        mark_read(&db, e3, "2024-01-03").await;

        let counts = get_daily_read_counts(&db, user_id, "2024-01-01", "2024-01-05")
            .await
            .unwrap();

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

    #[tokio::test]
    async fn test_bucket_daily_counts_empty() {
        assert!(bucket_daily_counts(&[], 14).is_empty());
    }

    #[tokio::test]
    async fn test_bucket_daily_counts_no_aggregation_within_max() {
        let daily = daily_run(&[0, 2, 1, 0]);
        let buckets = bucket_daily_counts(&daily, 14);

        assert_eq!(buckets.len(), 4);
        for (i, b) in buckets.iter().enumerate() {
            assert_eq!(b.start, daily[i].date);
            assert_eq!(b.end, daily[i].date, "single-day bucket spans one day");
            assert_eq!(b.count, daily[i].count);
        }
    }

    #[tokio::test]
    async fn test_bucket_daily_counts_aggregates_over_max() {
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

    #[tokio::test]
    async fn test_bucket_daily_counts_last_bucket_partial() {
        // 15 days, size 2 → last (8th) bucket has a single leftover day.
        let daily = daily_run(&[1; 15]);
        let buckets = bucket_daily_counts(&daily, 14);

        let last = buckets.last().unwrap();
        assert_eq!(last.start, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(last.end, NaiveDate::from_ymd_opt(2024, 1, 15).unwrap());
        assert_eq!(last.count, 1);
    }

    #[tokio::test]
    async fn test_bucket_daily_counts_sums_within_bucket() {
        // 28 days → size = ceil(28/14) = 2; counts 1..=28 → bucket 0 = 1+2 = 3.
        let counts: Vec<i64> = (1..=28).collect();
        let daily = daily_run(&counts);
        let buckets = bucket_daily_counts(&daily, 14);

        assert_eq!(buckets.len(), 14);
        assert_eq!(buckets[0].count, 3); // 1 + 2
        assert_eq!(buckets[13].count, 55); // 27 + 28
    }

    #[tokio::test]
    async fn test_entries_by_category() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // Add a second category + feed
        let cat2 = category::create_category(&db, user_id, "Science")
            .await
            .unwrap();
        let feed2_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cat2.id,
                url: "https://science.com/feed",
                title: Some("Science Feed"),
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

        // 2 entries in Tech, 1 in Science
        insert_entry(&db, feed_id, "t1", "2024-01-05").await;
        insert_entry(&db, feed_id, "t2", "2024-01-10").await;
        insert_entry(&db, feed2_id, "s1", "2024-01-07").await;

        let counts = get_entries_by_category(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0].name, "Tech");
        assert_eq!(counts[0].count, 2);
        assert_eq!(counts[1].name, "Science");
        assert_eq!(counts[1].count, 1);
    }

    #[tokio::test]
    async fn test_top_feeds() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        insert_entry(&db, feed_id, "g1", "2024-01-05").await;

        let feeds = get_top_feeds(&db, user_id, "2024-01-01", "2024-02-01", 10)
            .await
            .unwrap();

        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].title, "Test Feed");
        assert_eq!(feeds[0].count, 1);
    }

    #[tokio::test]
    async fn test_admin_counts() {
        let db = setup_db().await;
        create_user_with_data(&db).await;

        let counts = get_admin_counts(&db).await.unwrap();

        assert_eq!(counts.total_users, 1);
        assert_eq!(counts.total_feeds, 1);
    }

    #[tokio::test]
    async fn test_admin_entry_stats() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        let e1 = insert_entry(&db, feed_id, "g1", "2024-01-05").await;
        let _e2 = insert_entry(&db, feed_id, "g2", "2024-01-10").await;

        mark_read(&db, e1, "2024-01-06").await;

        // Published before the period but read inside it: the old query
        // counted this toward read_entries and could push the rate past 100%.
        let old = insert_entry(&db, feed_id, "g-old", "2023-12-01").await;
        mark_read(&db, old, "2024-01-07").await;

        let stats = get_admin_entry_stats(&db, "2024-01-01", "2024-02-01")
            .await
            .unwrap();

        assert_eq!(stats.total_entries, 2, "Dec entry is outside the period");
        assert_eq!(stats.read_entries, 1, "only g1; the Dec entry is excluded");
        assert!((stats.read_rate() - 50.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_admin_database_stats_empty() {
        let db = setup_db().await;
        create_user_with_data(&db).await;

        let s = get_admin_database_stats(&db).await.unwrap();

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

    #[tokio::test]
    async fn test_admin_database_stats_with_data() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // 4 entries spanning exactly 3 days (2024-01-01 .. 2024-01-04).
        insert_entry_created_at(&db, feed_id, "a", "2024-01-01 00:00:00").await;
        insert_entry_created_at(&db, feed_id, "b", "2024-01-02 00:00:00").await;
        insert_entry_created_at(&db, feed_id, "c", "2024-01-03 00:00:00").await;
        insert_entry_created_at(&db, feed_id, "d", "2024-01-04 00:00:00").await;

        insert_tombstone(&db, feed_id, "dead-1").await;
        insert_tombstone(&db, feed_id, "dead-2").await;

        let s = get_admin_database_stats(&db).await.unwrap();

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

    #[tokio::test]
    async fn test_admin_database_stats_avg_guards_subday_span() {
        // A single entry → coverage span 0 → denominator guarded at 1 day so
        // the average is finite (and equals the entry count) rather than inf.
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        insert_entry_created_at(&db, feed_id, "only", "2024-01-01 12:00:00").await;

        let s = get_admin_database_stats(&db).await.unwrap();

        assert_eq!(s.total_entries, 1);
        assert_eq!(s.coverage_days, 0.0);
        assert!((s.avg_new_entries_per_day - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_admin_database_stats_parses_rfc3339_created_at() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // RFC 3339 timestamps spanning exactly 2 days. The previous SQL-only
        // parser would have failed these and collapsed coverage to 0.0.
        insert_entry_created_at(&db, feed_id, "a", "2024-01-01T00:00:00Z").await;
        insert_entry_created_at(&db, feed_id, "b", "2024-01-03T00:00:00Z").await;

        let s = get_admin_database_stats(&db).await.unwrap();

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
