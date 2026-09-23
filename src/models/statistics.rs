use chrono::NaiveDate;

use crate::db::{Db, DbInner};
use crate::error::{AppError, AppResult};
use crate::{query_all, query_scalar};

/// Render a timestamp column as `%Y-%m-%d %H:%M:%S` TEXT; PG `TIMESTAMPTZ`
/// needs `to_char`. Mirrors `entry::filters::Dialect::cursor_ts`.
fn ts_text(db: &Db, expr: &str) -> String {
    if db.is_postgres() {
        format!("to_char({expr}, 'YYYY-MM-DD HH24:MI:SS')")
    } else {
        expr.to_string()
    }
}

/// Parse a `YYYY-MM-DD` range bound as a `NaiveDate`, which compares correctly
/// on both backends (a string bind would not coerce on PG). Falls back to today.
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
    /// Entries published in the period and not yet read; never negative since
    /// read is a subset of the same cohort (no clamp, so regressions surface).
    pub fn unread_entries(&self) -> i64 {
        self.total_entries - self.read_entries
    }

    /// Fraction of period-published entries that have been read (0–100%).
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

/// A contiguous span of days `[start, end]` (inclusive) collapsed into one bar.
pub struct DailyBucket {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub count: i64,
}

/// Collapse chronologically ordered per-day counts into at most `max_bars`
/// buckets of `ceil(len / max_bars)` days each.
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
    /// Fraction of period-published entries (site-wide) that have been read (0–100%).
    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            (self.read_entries as f64 / self.total_entries as f64) * 100.0
        }
    }
}

/// Free space a `VACUUM` would reclaim; `SQLite` only (PG reports `None`).
#[derive(Clone)]
pub struct ReclaimableSpace {
    pub bytes: i64,
    /// `bytes / db_size_bytes`, in `0.0..=1.0`.
    pub fragmentation_ratio: f64,
}

/// Admin database storage + record stats (period-independent). The page omits
/// the card when `reclaimable` is `None` rather than showing a misleading zero.
#[derive(Clone)]
pub struct AdminDatabaseStats {
    pub db_size_bytes: i64,
    pub reclaimable: Option<ReclaimableSpace>,
    pub total_entries: i64,
    pub avg_new_entries_per_day: f64,
    pub coverage_days: f64,
    pub tombstone_count: i64,
}

/// Read-count half of [`get_personal_overview`], hoisted for the query-plan test.
/// The `INNER JOIN` to `category` needs `idx_entry_feed_read_sort` (0011).
const READ_ENTRIES_SQL: &str = r"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
          AND e.read_at IS NOT NULL
        ";

/// Starred-count half of [`get_personal_overview`], hoisted for the query-plan test.
///
/// `feed_id IN (SELECT ...)` instead of a join on purpose: a join makes `SQLite`
/// pick `idx_entry_starred_sort` and pay a table lookup per row, instead of
/// `idx_entry_feed_starred_sort` (0013).
const STARRED_ENTRIES_SQL: &str = r"
        SELECT COUNT(e.id)
        FROM entry e
        WHERE e.feed_id IN (
                SELECT f.id
                FROM feed f
                INNER JOIN category c ON f.category_id = c.id
                WHERE c.user_id = $1
              )
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
          AND e.starred_at IS NOT NULL
        ";

/// `from`/`to` are `YYYY-MM-DD`; the range is `[from, to)`.
pub async fn get_personal_overview(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<PersonalOverview> {
    // Bind bounds as dates (see `parse_ymd`) for cross-backend comparisons.
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    let total_entries: i64 = query_scalar!(
        db,
        i64,
        r"
        SELECT COUNT(e.id)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = $1
          AND COALESCE(e.published_at, e.created_at) >= $2
          AND COALESCE(e.published_at, e.created_at) < $3
        ",
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)?;

    // Read/starred are subsets of the same publish cohort as total_entries,
    // not period activity, so Read ⊆ Total.
    let read_entries: i64 =
        query_scalar!(db, i64, READ_ENTRIES_SQL, user_id, from, to).map_err(AppError::Database)?;

    let starred_entries: i64 = query_scalar!(db, i64, STARRED_ENTRIES_SQL, user_id, from, to)
        .map_err(AppError::Database)?;

    let summaries: i64 = query_scalar!(
        db,
        i64,
        r"
        SELECT COUNT(es.id)
        FROM entry_summary es
        WHERE es.user_id = $1
          AND es.status = 'completed'
          AND es.created_at >= $2
          AND es.created_at < $3
        ",
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

/// Daily-read chart query, hoisted for the query-plan test. Zero days are filled
/// in Rust. Needs `idx_entry_feed_read_at` to stay covering (see 0012).
fn daily_read_counts_sql(db: &Db) -> String {
    let day_bucket = if db.is_postgres() {
        "to_char(e.read_at, 'YYYY-MM-DD')"
    } else {
        "DATE(e.read_at)"
    };
    format!(
        "SELECT {day_bucket} AS read_date, COUNT(e.id) AS cnt \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1 \
           AND e.read_at >= $2 \
           AND e.read_at < $3 \
         GROUP BY read_date \
         ORDER BY read_date"
    )
}

/// Days with no reads are zero-filled. `from`/`to` are `YYYY-MM-DD`, range `[from, to)`.
pub async fn get_daily_read_counts(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<DailyReadCount>> {
    let (from_d, to_d) = (parse_ymd(from), parse_ymd(to));
    let sql = daily_read_counts_sql(db);
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

/// Only categories with at least one entry are returned, ordered by count DESC.
pub async fn get_entries_by_category(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<CategoryCount>> {
    // `AS count` so `FromRow` fills `count`; HAVING/ORDER BY use the aggregate.
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    query_all!(
        db,
        CategoryCount,
        r"
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
        ",
        user_id,
        from,
        to,
    )
    .map_err(AppError::Database)
}

/// Top `limit` feeds with at least one entry, ordered by count DESC.
pub async fn get_top_feeds(
    db: &Db,
    user_id: i64,
    from: &str,
    to: &str,
    limit: i64,
) -> AppResult<Vec<FeedCount>> {
    // `AS count` so `FromRow` fills `count`; HAVING/ORDER BY use the aggregate.
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    query_all!(
        db,
        FeedCount,
        r"
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
        ",
        user_id,
        from,
        to,
        limit,
    )
    .map_err(AppError::Database)
}

/// Period-independent, unlike the other admin stats.
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

pub async fn get_admin_entry_stats(db: &Db, from: &str, to: &str) -> AppResult<AdminEntryStats> {
    let (from, to) = (parse_ymd(from), parse_ymd(to));
    let total_entries: i64 = query_scalar!(
        db,
        i64,
        r"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= $1
          AND COALESCE(published_at, created_at) < $2
        ",
        from,
        to,
    )
    .map_err(AppError::Database)?;

    // Same publish cohort as total_entries, so the rate stays within 0–100%.
    let read_entries: i64 = query_scalar!(
        db,
        i64,
        r"
        SELECT COUNT(id)
        FROM entry
        WHERE COALESCE(published_at, created_at) >= $1
          AND COALESCE(published_at, created_at) < $2
          AND read_at IS NOT NULL
        ",
        from,
        to,
    )
    .map_err(AppError::Database)?;

    Ok(AdminEntryStats {
        total_entries,
        read_entries,
    })
}

/// Period-independent. Storage figures are dialect-specific.
pub async fn get_admin_database_stats(db: &Db) -> AppResult<AdminDatabaseStats> {
    // PG has no reliable free-space figure without `pgstattuple`
    // (`n_dead_tup` excludes TOAST, counts tuples not bytes, and resets on
    // autovacuum), so it reports `None` and the UI drops the card.
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
            (page_count * page_size, Some(freelist * page_size))
        }
        DbInner::Postgres(pool) => {
            let size = sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())")
                .fetch_one(pool)
                .await
                .map_err(AppError::Database)?;
            (size, None)
        }
    };

    let reclaimable = reclaimable_bytes.map(|bytes| ReclaimableSpace {
        bytes,
        fragmentation_ratio: if db_size_bytes > 0 {
            bytes as f64 / db_size_bytes as f64
        } else {
            0.0
        },
    });

    let total_entries: i64 =
        query_scalar!(db, i64, "SELECT COUNT(*) FROM entry").map_err(AppError::Database)?;
    // Bare MIN/MAX so SQLite uses the idx_entry_created_at endpoint optimization.
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

    // Fallible parser: `parse_datetime`'s now() fallback would corrupt aggregates.
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
            // Average over the retained span, not age since the oldest entry,
            // since retention prunes read entries. Sub-day span guarded at 1 day.
            let avg = total_entries as f64 / coverage.max(1.0);
            (coverage, avg)
        }
        _ => (0.0, 0.0),
    };

    Ok(AdminDatabaseStats {
        db_size_bytes,
        reclaimable,
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
    use crate::models::{category, feed};
    use crate::test_support::{seed_user, setup_db};

    async fn create_user_with_data(db: &Db) -> i64 {
        let user_id = seed_user(db, "testuser", Role::User).await.id;
        let cat = category::create_category(db, user_id, "Tech")
            .await
            .unwrap();
        feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url: "https://example.com/feed",
                title: Some("Test Feed"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        user_id
    }

    /// A second user for scoping assertions (username and feed URL are UNIQUE).
    async fn create_second_user_with_feed(db: &Db) -> (i64, i64) {
        let user_id = seed_user(db, "otheruser", Role::User).await.id;
        let cat = category::create_category(db, user_id, "Theirs")
            .await
            .unwrap();
        let feed = feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id: cat.id,
                url: "https://other.example.com/feed",
                title: Some("Other Feed"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        (user_id, feed.id)
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
    #[allow(clippy::float_cmp, reason = "exact-value test assertion")]
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
        // Read/starred counts are subsets of entries published in the period.
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        // Published before the period but read+starred during it: excluded.
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

    /// One `DailyReadCount` per element of `counts`, from `2024-01-01`.
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

        let cat2 = category::create_category(&db, user_id, "Science")
            .await
            .unwrap();
        let feed2_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id: cat2.id,
                url: "https://science.com/feed",
                title: Some("Science Feed"),
                ..Default::default()
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

        // Published before the period but read inside it: must not count.
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
    #[allow(clippy::float_cmp, reason = "exact-value test assertion")]
    async fn test_admin_database_stats_empty() {
        let db = setup_db().await;
        create_user_with_data(&db).await;

        let s = get_admin_database_stats(&db).await.unwrap();

        // A freshly-initialized DB still has pages, so size is positive.
        assert!(s.db_size_bytes > 0);
        // PG's `None` arm is asserted in `tests/postgres_test.rs`.
        let r = s.reclaimable.expect("SQLite reports reclaimable space");
        assert!(r.bytes >= 0);
        assert!((0.0..=1.0).contains(&r.fragmentation_ratio));
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
        // avg = retained entries / coverage span = 4 / 3.
        assert!(
            (s.avg_new_entries_per_day - 4.0 / 3.0).abs() < 1e-6,
            "avg was {}",
            s.avg_new_entries_per_day
        );
    }

    #[tokio::test]
    #[allow(clippy::float_cmp, reason = "exact-value test assertion")]
    async fn test_admin_database_stats_avg_guards_subday_span() {
        // Single entry → zero span → denominator guarded at 1 day.
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

        // RFC 3339 timestamps spanning exactly 2 days.
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

    /// Without `idx_entry_feed_read_sort` the read count reads `read_at` off the
    /// table per entry; the plan is the only observable that pins this.
    #[tokio::test]
    async fn test_read_entries_query_is_index_covered() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;
        let e = insert_entry(&db, feed_id, "g1", "2024-01-05").await;
        mark_read(&db, e, "2024-01-06").await;

        let DbInner::Sqlite(pool) = db.inner() else {
            unreachable!("connect_in_memory is always SQLite")
        };
        // Only the detail column carries the index name.
        let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {READ_ENTRIES_SQL}"
        )))
        .bind(user_id)
        .bind(parse_ymd("2024-01-01"))
        .bind(parse_ymd("2024-02-01"))
        .fetch_all(pool)
        .await
        .unwrap();
        let plan = rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n");

        assert!(
            plan.contains("idx_entry_feed_read_sort"),
            "read count must be served by the partial index, plan was:\n{plan}"
        );
        assert!(
            !plan.contains("idx_entry_feed_sort"),
            "falling back to the non-covering index means a table lookup per row, plan was:\n{plan}"
        );
    }

    /// The daily-read chart filters `read_at` as a range; only a COVERING plan on
    /// `idx_entry_feed_read_at` avoids a table lookup per row.
    #[tokio::test]
    async fn test_daily_read_counts_query_is_index_covered() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;
        let e = insert_entry(&db, feed_id, "g1", "2024-01-05").await;
        mark_read(&db, e, "2024-01-06").await;

        let DbInner::Sqlite(pool) = db.inner() else {
            unreachable!("connect_in_memory is always SQLite")
        };
        let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {}",
            daily_read_counts_sql(&db)
        )))
        .bind(user_id)
        .bind(parse_ymd("2024-01-01"))
        .bind(parse_ymd("2024-02-01"))
        .fetch_all(pool)
        .await
        .unwrap();
        let plan = rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n");

        assert!(
            plan.contains("COVERING INDEX idx_entry_feed_read_at"),
            "the daily chart must be served by the index alone, plan was:\n{plan}"
        );
    }

    /// The starred count needs both the `feed_id IN (SELECT ...)` rewrite and the
    /// 0013 index; either alone is far worse, so the plan is pinned.
    #[tokio::test]
    async fn test_starred_entries_query_is_index_covered() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;
        let e = insert_entry(&db, feed_id, "g1", "2024-01-05").await;
        mark_starred(&db, e, "2024-01-06").await;

        let DbInner::Sqlite(pool) = db.inner() else {
            unreachable!("connect_in_memory is always SQLite")
        };
        let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {STARRED_ENTRIES_SQL}"
        )))
        .bind(user_id)
        .bind(parse_ymd("2024-01-01"))
        .bind(parse_ymd("2024-02-01"))
        .fetch_all(pool)
        .await
        .unwrap();
        let plan = rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n");

        assert!(
            plan.contains("idx_entry_feed_starred_sort"),
            "starred count must be served by the partial index, plan was:\n{plan}"
        );
        assert!(
            !plan.contains("idx_entry_starred_sort ("),
            "leading with the sort-keyed index means a table lookup per starred row, plan was:\n{plan}"
        );
    }

    /// The starred rewrite must still count only this user's starred entries
    /// published inside the window.
    #[tokio::test]
    async fn test_starred_count_is_scoped_to_the_user_and_window() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;

        let inside = insert_entry(&db, feed_id, "inside", "2024-01-05").await;
        mark_starred(&db, inside, "2024-01-06").await;
        // Starred, but published outside the window.
        let outside = insert_entry(&db, feed_id, "outside", "2024-03-05").await;
        mark_starred(&db, outside, "2024-03-06").await;
        // Inside the window, but never starred.
        insert_entry(&db, feed_id, "unstarred", "2024-01-07").await;

        // Another user's starred entry in the window: excluded by the subquery scope.
        let (other_id, other_feed) = create_second_user_with_feed(&db).await;
        let theirs = insert_entry(&db, other_feed, "theirs", "2024-01-05").await;
        mark_starred(&db, theirs, "2024-01-06").await;

        let overview = get_personal_overview(&db, user_id, "2024-01-01", "2024-02-01")
            .await
            .unwrap();
        assert_eq!(overview.starred_entries, 1);
        assert_eq!(
            get_personal_overview(&db, other_id, "2024-01-01", "2024-02-01")
                .await
                .unwrap()
                .starred_entries,
            1
        );
    }

    /// The chart must still report the same numbers after the SQL change.
    #[tokio::test]
    async fn test_daily_read_counts_fills_gaps_and_counts_reads() {
        let db = setup_db().await;
        let user_id = create_user_with_data(&db).await;
        let feed_id = get_feed_id(&db, user_id).await;
        for (i, day) in ["2024-01-01", "2024-01-01", "2024-01-03"]
            .iter()
            .enumerate()
        {
            let e = insert_entry(&db, feed_id, &format!("g{i}"), "2023-12-01").await;
            mark_read(&db, e, &format!("{day} 09:00:00")).await;
        }
        // Read outside the window: excluded even though the entry qualifies.
        let outside = insert_entry(&db, feed_id, "outside", "2023-12-01").await;
        mark_read(&db, outside, "2024-02-10 09:00:00").await;

        let daily = get_daily_read_counts(&db, user_id, "2024-01-01", "2024-01-04")
            .await
            .unwrap();

        let counts: Vec<i64> = daily.iter().map(|d| d.count).collect();
        assert_eq!(counts, vec![2, 0, 1], "2024-01-02 must be a filled zero");
        assert_eq!(
            daily.first().unwrap().date,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()
        );
    }
}
