//! Entries a reader's client rendered, and the per-feed open rate.
//!
//! Compare `entry.created_at` to `pixel_tracking_enabled_at` column to column,
//! never to a bound timestamp: `SQLite` stores `datetime('now')` TEXT while sqlx
//! binds RFC 3339, and `'T' > ' '` breaks the comparison.

use chrono::{DateTime, Utc};

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::{db_execute, query_all, query_opt};

/// Tracked entries a feed needs before its open rate is shown instead of `—`;
/// smaller samples swing too much to mean anything.
pub const MIN_TRACKED_FOR_RATE: i64 = 5;

/// One feed's open counts over the tracked window.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedOpenRate {
    pub feed_id: i64,
    pub title: Option<String>,
    /// Entries created since opt-in and not yet pruned by retention.
    pub tracked: i64,
    /// Of those, the ones a client rendered.
    pub opened: i64,
}

impl FeedOpenRate {
    /// Whole-percent open rate, or `None` below [`MIN_TRACKED_FOR_RATE`].
    pub fn percent(&self) -> Option<i64> {
        if self.tracked < MIN_TRACKED_FOR_RATE {
            return None;
        }
        // Rounded integer division, matching `bar_percent`.
        Some((self.opened.saturating_mul(100) + self.tracked / 2) / self.tracked)
    }
}

/// The window the open rate actually speaks about.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrackingWindow {
    pub enabled_at: Option<DateTime<Utc>>,
    /// Oldest surviving entry; retention can make this far newer than `enabled_at`.
    pub oldest_tracked: Option<DateTime<Utc>>,
}

impl TrackingWindow {
    /// "Tracked since": the later of the opt-in and the oldest surviving entry,
    /// since pruned entries no longer count in the denominator.
    pub fn tracked_since(&self) -> Option<DateTime<Utc>> {
        match (self.enabled_at, self.oldest_tracked) {
            (Some(enabled), Some(oldest)) => Some(enabled.max(oldest)),
            (Some(enabled), None) => Some(enabled),
            _ => None,
        }
    }
}

/// Record that `entry_id` was rendered for `user_id`; returns whether it was new.
///
/// Ownership and opt-in are enforced in the statement itself; `ON CONFLICT DO
/// NOTHING` makes re-fetches idempotent.
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

/// Per-feed open counts (untracked feeds as `0/0`) in one aggregate to avoid
/// N+1. Empty when opted out, which hides the column.
pub async fn open_rates_by_feed(db: &Db, user_id: i64) -> AppResult<Vec<FeedOpenRate>> {
    query_all!(db, FeedOpenRate, OPEN_RATES_SQL, user_id).map_err(AppError::Database)
}

/// Hoisted for the query-plan test; must stay covered by `idx_entry_feed_created_at`.
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

/// Hoisted like [`OPEN_RATES_SQL`]; depends on the same index.
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

    /// `EXPLAIN QUERY PLAN` detail column, joined.
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

    /// Without `idx_entry_feed_created_at` both queries read `created_at` off the
    /// table per entry; the plan is the only observable that pins this.
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
        // 1/6 (16.7%) rounds up to 17, 4/6 (66.7%) to 67.
        assert_eq!(rate(1, 6), Some(17));
        assert_eq!(rate(4, 6), Some(67));
    }

    #[test]
    fn tracked_since_takes_the_later_of_opt_in_and_surviving_data() {
        let enabled = Utc::now() - chrono::Duration::days(30);
        let oldest = Utc::now() - chrono::Duration::days(7);
        // Retention pruned the early entries.
        let w = TrackingWindow {
            enabled_at: Some(enabled),
            oldest_tracked: Some(oldest),
        };
        assert_eq!(w.tracked_since(), Some(oldest));

        // Nothing pruned yet.
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
