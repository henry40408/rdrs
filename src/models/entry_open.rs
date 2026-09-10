//! Which entries a reader's client actually rendered, and the per-feed rate
//! derived from it.
//!
//! Every predicate here compares `entry.created_at` against
//! `user_settings.pixel_tracking_enabled_at` **column to column**, never against
//! a bound timestamp. On `SQLite` those columns hold `datetime('now')` TEXT
//! (`%Y-%m-%d %H:%M:%S`) while sqlx encodes a bound `DateTime<Utc>` as RFC 3339
//! (`...T...+00:00`), and `'T' > ' '`, so a bound comparison silently reports
//! every entry as newer than the baseline. Writes use the `datetime('now')`
//! literal for the same reason — `pg_rewrite` turns it into `now()` on
//! `PostgreSQL`, so one statement stays correct on both backends.

use chrono::{DateTime, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_opt};

/// Tracked entries a feed needs before its open rate is shown rather than
/// suppressed as `—`.
///
/// Below this, one open either way swings the percentage far enough to invert
/// the "should I unsubscribe?" answer the number exists to support — a feed that
/// has published twice cannot tell you anything about itself yet.
pub const MIN_TRACKED_FOR_RATE: i64 = 5;

/// One feed's open counts over the tracked window.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedOpenRate {
    pub feed_id: i64,
    pub title: Option<String>,
    /// Entries that carried a pixel — those created at or after the opt-in and
    /// not yet pruned by retention.
    pub tracked: i64,
    /// Of those, the ones a client actually rendered.
    pub opened: i64,
}

impl FeedOpenRate {
    /// Whole-percent open rate, or `None` while the sample is too small to say
    /// anything (see [`MIN_TRACKED_FOR_RATE`]).
    pub fn percent(&self) -> Option<i64> {
        if self.tracked < MIN_TRACKED_FOR_RATE {
            return None;
        }
        // Rounded integer division, matching `bar_percent` on the statistics
        // page — no float, so no lossy cast to justify.
        Some((self.opened.saturating_mul(100) + self.tracked / 2) / self.tracked)
    }
}

/// The window the open rate actually speaks about.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrackingWindow {
    pub enabled_at: Option<DateTime<Utc>>,
    /// Oldest entry still in the window. Retention prunes read entries out from
    /// under the metric, so this can be far newer than `enabled_at`.
    pub oldest_tracked: Option<DateTime<Utc>>,
}

impl TrackingWindow {
    /// The date to show as "tracked since": the later of the opt-in and the
    /// oldest entry that survived retention.
    ///
    /// Reporting the raw opt-in date would overstate the window — the entries
    /// backing the earlier part of it have been deleted, so the denominator no
    /// longer covers them.
    pub fn tracked_since(&self) -> Option<DateTime<Utc>> {
        match (self.enabled_at, self.oldest_tracked) {
            (Some(enabled), Some(oldest)) => Some(enabled.max(oldest)),
            (Some(enabled), None) => Some(enabled),
            _ => None,
        }
    }
}

/// Record that `entry_id` was rendered by one of `user_id`'s clients. Returns
/// whether this was the first time.
///
/// Ownership, opt-in and the opt-in baseline are all enforced inside the one
/// statement, so a hit on a valid token for an entry the reader does not own —
/// or for one that predates their opt-in — writes nothing rather than being
/// caught by a separate check the caller could forget. `ON CONFLICT DO NOTHING`
/// makes a re-fetch (a re-render, a second client, a proxy retry) idempotent:
/// the metric counts entries opened, not requests served.
pub async fn record_open(db: &Db, user_id: i64, entry_id: i64) -> AppResult<bool> {
    let affected = db_execute!(
        db,
        "INSERT INTO entry_open (user_id, entry_id) \
         SELECT us.user_id, e.id \
         FROM entry e \
         JOIN feed f ON f.id = e.feed_id \
         JOIN category c ON c.id = f.category_id \
         JOIN user_settings us ON us.user_id = c.user_id \
         WHERE us.user_id = $1 \
           AND e.id = $2 \
           AND us.pixel_tracking_enabled_at IS NOT NULL \
           AND e.created_at >= us.pixel_tracking_enabled_at \
         ON CONFLICT (user_id, entry_id) DO NOTHING",
        user_id,
        entry_id
    )
    .map_err(AppError::Database)?;
    Ok(affected > 0)
}

/// Per-feed open counts for one reader, feeds with no tracked entries included
/// (as `0/0`) so `/feeds` can render a row for every feed.
///
/// One aggregate for the whole page rather than a count per feed: `/feeds`
/// already renders every subscription, and a per-row query would be an N+1 over
/// the largest table in the schema. Returns nothing at all when the reader is
/// opted out, which is what suppresses the column.
pub async fn open_rates_by_feed(db: &Db, user_id: i64) -> AppResult<Vec<FeedOpenRate>> {
    query_all!(db, FeedOpenRate, OPEN_RATES_SQL, user_id).map_err(AppError::Database)
}

/// Hoisted so the query-plan regression test asserts against the SQL that
/// actually runs. It needs `idx_entry_feed_created_at` to stay off the `entry`
/// table entirely — see the 0014 migration for what it cost without it.
const OPEN_RATES_SQL: &str = "SELECT f.id AS feed_id, f.title AS title, \
                COUNT(e.id) AS tracked, \
                COUNT(o.entry_id) AS opened \
         FROM feed f \
         JOIN category c ON c.id = f.category_id \
         JOIN user_settings us ON us.user_id = c.user_id \
         LEFT JOIN entry e ON e.feed_id = f.id \
              AND e.created_at >= us.pixel_tracking_enabled_at \
         LEFT JOIN entry_open o ON o.entry_id = e.id AND o.user_id = us.user_id \
         WHERE c.user_id = $1 AND us.pixel_tracking_enabled_at IS NOT NULL \
         GROUP BY f.id, f.title";

/// The opt-in date and the oldest entry still inside the tracked window.
pub async fn tracking_window(db: &Db, user_id: i64) -> AppResult<TrackingWindow> {
    let found =
        query_opt!(db, TrackingWindow, TRACKING_WINDOW_SQL, user_id).map_err(AppError::Database)?;
    Ok(found.unwrap_or(TrackingWindow {
        enabled_at: None,
        oldest_tracked: None,
    }))
}

/// Hoisted for the same reason as [`OPEN_RATES_SQL`], and dependent on the same
/// index: the `MIN(created_at)` is taken over every entry in every feed the
/// reader subscribes to.
const TRACKING_WINDOW_SQL: &str = "SELECT us.pixel_tracking_enabled_at AS enabled_at, \
                MIN(e.created_at) AS oldest_tracked \
         FROM user_settings us \
         LEFT JOIN category c ON c.user_id = us.user_id \
         LEFT JOIN feed f ON f.category_id = c.id \
         LEFT JOIN entry e ON e.feed_id = f.id \
              AND e.created_at >= us.pixel_tracking_enabled_at \
         WHERE us.user_id = $1 AND us.pixel_tracking_enabled_at IS NOT NULL \
         GROUP BY us.pixel_tracking_enabled_at";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbInner;

    /// `EXPLAIN QUERY PLAN` yields (id, parent, notused, detail); only the last
    /// column carries the index name.
    async fn plan_for(db: &Db, sql: &str) -> String {
        let DbInner::Sqlite(pool) = db.inner() else {
            unreachable!("connect_in_memory is always SQLite")
        };
        let rows: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {sql}")))
                .bind(1_i64)
                .fetch_all(pool)
                .await
                .unwrap();
        rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
    }

    /// The open-rate section is the most expensive thing on `/statistics`, and
    /// only for readers who turned pixel tracking on. Both of its queries bound
    /// the raw `entry.created_at` per feed, which no index keyed before 0014:
    /// `idx_entry_feed_sort` keys the *coalesced* timestamp, so it serves the
    /// join and then reads `created_at` off the table, once per entry in the
    /// feed, unnarrowed. On a 567 MB / 70k-entry database that was 147,640 and
    /// 147,933 page misses — roughly 1.2 GB of reads for one render — against
    /// 141 and 130 once `idx_entry_feed_created_at` covers them.
    ///
    /// The plan is the only observable that fails before the index and passes
    /// after, so it is what this pins.
    #[tokio::test]
    async fn test_open_rates_query_is_index_covered() {
        let db = Db::connect_in_memory().await.unwrap();

        let plan = plan_for(&db, OPEN_RATES_SQL).await;
        assert!(
            plan.contains("COVERING INDEX idx_entry_feed_created_at"),
            "open rates must be served by the index alone, plan was:\n{plan}"
        );

        let plan = plan_for(&db, TRACKING_WINDOW_SQL).await;
        assert!(
            plan.contains("COVERING INDEX idx_entry_feed_created_at"),
            "the tracking window must be served by the index alone, plan was:\n{plan}"
        );
    }

    #[test]
    fn percent_is_suppressed_below_the_sample_floor() {
        let r = FeedOpenRate {
            feed_id: 1,
            title: None,
            tracked: MIN_TRACKED_FOR_RATE - 1,
            opened: 1,
        };
        assert_eq!(r.percent(), None);
    }

    #[test]
    fn percent_rounds_to_whole_numbers() {
        let rate = |opened, tracked| {
            FeedOpenRate {
                feed_id: 1,
                title: None,
                tracked,
                opened,
            }
            .percent()
        };
        assert_eq!(rate(0, 10), Some(0));
        assert_eq!(rate(5, 10), Some(50));
        assert_eq!(rate(10, 10), Some(100));
        // 1/3 rounds up to 33, 2/3 to 67.
        assert_eq!(rate(1, 6), Some(17));
        assert_eq!(rate(4, 6), Some(67));
    }

    #[test]
    fn tracked_since_takes_the_later_of_opt_in_and_surviving_data() {
        let enabled = Utc::now() - chrono::Duration::days(30);
        let oldest = Utc::now() - chrono::Duration::days(7);
        // Retention has eaten the first three weeks, so the honest baseline is
        // the oldest entry that is still there, not the opt-in date.
        let w = TrackingWindow {
            enabled_at: Some(enabled),
            oldest_tracked: Some(oldest),
        };
        assert_eq!(w.tracked_since(), Some(oldest));

        // Nothing has been pruned yet: the opt-in date is the honest baseline.
        let w = TrackingWindow {
            enabled_at: Some(enabled),
            oldest_tracked: Some(enabled + chrono::Duration::seconds(1)),
        };
        assert_eq!(
            w.tracked_since(),
            Some(enabled + chrono::Duration::seconds(1))
        );

        // Opted in but nothing has arrived since.
        let w = TrackingWindow {
            enabled_at: Some(enabled),
            oldest_tracked: None,
        };
        assert_eq!(w.tracked_since(), Some(enabled));

        // Opted out.
        let w = TrackingWindow {
            enabled_at: None,
            oldest_tracked: None,
        };
        assert_eq!(w.tracked_since(), None);
    }
}
