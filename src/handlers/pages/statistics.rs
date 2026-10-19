use askama::Template;
use axum::extract::{Query, State};

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::entry_open::FeedOpenRate;
use crate::models::statistics::{
    AdminCounts, AdminDatabaseStats, AdminEntryStats, CategoryCount, DailyReadCount, FeedCount,
};

#[derive(serde::Deserialize)]
pub struct StatisticsQuery {
    pub period: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Returns (`from`, `to_exclusive`, `active_period`) as ISO dates.
pub fn resolve_statistics_period(query: &StatisticsQuery) -> (String, String, String) {
    let today = chrono::Utc::now().date_naive();
    let days_ago = |n| today - chrono::Duration::days(n);
    let parse = |s: &Option<String>| {
        s.as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    };

    // Custom ranges cap at a year; invalid ones fall back to 7d.
    let (from, to, period) = match query.period.as_deref().unwrap_or("7d") {
        "30d" => (days_ago(30), today, "30d"),
        "90d" => (days_ago(90), today, "90d"),
        "all" => (chrono::NaiveDate::default(), today, "all"),
        "custom" => match (parse(&query.from), parse(&query.to)) {
            (Some(f), Some(t)) if f <= t => (f, t.min(f + chrono::Duration::days(365)), "custom"),
            _ => (days_ago(7), today, "7d"),
        },
        _ => (days_ago(7), today, "7d"),
    };
    (
        from.to_string(),
        (to + chrono::Duration::days(1)).to_string(),
        period.to_string(),
    )
}

/// One daily-read chart bar; may span several days once bucketed (see
/// [`crate::models::statistics::bucket_daily_counts`]).
pub struct DailyReadView {
    pub date_label: String,
    pub count: i64,
    /// See `bar_percent`.
    pub height_percent: u8,
    pub short_label: String,
    /// Only the first busiest bucket, so just one count label is drawn.
    pub is_max: bool,
}

/// One row in the "Entries by Category" list, with pre-computed bar width.
pub struct CategoryStatsView {
    pub name: String,
    pub count: i64,
    pub width_percent: u8,
}

/// One row in the "Top Feeds" list, with pre-computed bar width.
pub struct FeedStatsView {
    pub title: String,
    pub count: i64,
    pub width_percent: u8,
}

/// "Feeds by Open Rate" row. The bar is the absolute rate, not scaled to the max.
pub struct FeedOpenRateView {
    pub title: String,
    pub percent: i64,
    pub opened: i64,
    pub tracked: i64,
    pub width_percent: u8,
}

/// `count` as a whole percentage of `max`: whole because it selects a `pct-N`
/// class (CSP forbids inline `style`). Non-zero counts floor at 1%.
fn bar_percent(count: i64, max: i64) -> u8 {
    if max <= 0 || count <= 0 {
        return 0;
    }
    let pct = count.saturating_mul(100).saturating_add(max / 2) / max;
    u8::try_from(pct.clamp(1, 100)).unwrap_or(100)
}

/// Format a byte count for display (binary units, one decimal above 1 KB).
fn format_db_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Site-wide stats block shown to non-masquerading admins.
pub struct AdminStatsView {
    pub total_users: i64,
    pub total_feeds: i64,
    pub total_entries: i64,
    pub read_rate_fmt: String,
}

/// Reclaimable-space card, rendered only when the backend can measure it.
pub struct ReclaimableView {
    pub size_fmt: String,
    pub frag_pct: i64,
}

/// Database storage + record stats block (admin, non-masquerading).
pub struct AdminDatabaseStatsView {
    pub size_fmt: String,
    /// `None` on `PostgreSQL`, which can't report free space without an extension.
    pub reclaimable: Option<ReclaimableView>,
    pub total_entries: i64,
    pub avg_per_day_fmt: String,
    pub coverage_fmt: String,
    pub tombstone_count: i64,
}

/// Per-route template for `/statistics`.
#[derive(Template)]
#[template(path = "statistics.html")]
pub struct StatisticsTemplate {
    pub title: &'static str,
    pub git_version: &'static str,
    pub layout: AppLayoutContext,
    pub active_period: String,
    pub custom_from: String,
    pub custom_to: String,
    pub total_entries: i64,
    pub read_entries: i64,
    pub unread_entries: i64,
    pub starred_entries: i64,
    pub summaries: i64,
    pub read_rate_fmt: String,
    pub daily_max: i64,
    pub daily_read_counts: Vec<DailyReadView>,
    pub categories: Vec<CategoryStatsView>,
    pub top_feeds: Vec<FeedStatsView>,
    /// Lowest open rate first; empty when tracking is off or no feed has enough data.
    pub open_rate_feeds: Vec<FeedOpenRateView>,
    /// Start of the tracked window (retention may push it past the opt-in date).
    pub tracked_since: Option<String>,
    pub min_tracked_for_rate: i64,
    pub admin: Option<AdminStatsView>,
    pub admin_db: Option<AdminDatabaseStatsView>,
}

/// Chart bars, plus the busiest bucket's count that scales them.
fn daily_read_views(daily: &[DailyReadCount]) -> (i64, Vec<DailyReadView>) {
    // Cap bar count so long ranges stay tappable on mobile.
    const MAX_DAILY_BARS: usize = 14;
    let buckets = crate::models::statistics::bucket_daily_counts(daily, MAX_DAILY_BARS);

    let daily_max = buckets.iter().map(|b| b.count).max().unwrap_or(0);

    let max_idx = if daily_max > 0 {
        buckets.iter().position(|b| b.count == daily_max)
    } else {
        None
    };
    let views = buckets
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let short_label = b.start.format("%m/%d").to_string();
            let date_label = if b.start == b.end {
                b.start.format("%Y-%m-%d").to_string()
            } else {
                format!("{} – {}", b.start.format("%Y-%m-%d"), b.end.format("%m/%d"))
            };
            DailyReadView {
                date_label,
                count: b.count,
                height_percent: bar_percent(b.count, daily_max),
                short_label,
                is_max: Some(i) == max_idx,
            }
        })
        .collect();
    (daily_max, views)
}

fn category_views(cats: Vec<CategoryCount>) -> Vec<CategoryStatsView> {
    let cat_max = cats.iter().map(|c| c.count).max().unwrap_or(0);
    cats.into_iter()
        .map(|c| CategoryStatsView {
            count: c.count,
            width_percent: bar_percent(c.count, cat_max),
            name: c.name,
        })
        .collect()
}

fn top_feed_views(feeds: Vec<FeedCount>) -> Vec<FeedStatsView> {
    let feed_max = feeds.iter().map(|f| f.count).max().unwrap_or(0);
    feeds
        .into_iter()
        .map(|f| FeedStatsView {
            count: f.count,
            width_percent: bar_percent(f.count, feed_max),
            title: f.title,
        })
        .collect()
}

/// Feeds below the tracking minimum have no rate (`percent()` is `None`), so they are left out.
fn open_rate_views(rates: Vec<FeedOpenRate>) -> Vec<FeedOpenRateView> {
    let mut open_rate_feeds: Vec<FeedOpenRateView> = rates
        .into_iter()
        .filter_map(|r| {
            let percent = r.percent()?;
            Some(FeedOpenRateView {
                title: r.title.unwrap_or_else(|| "(untitled feed)".to_string()),
                percent,
                opened: r.opened,
                tracked: r.tracked,
                width_percent: u8::try_from(percent.clamp(0, 100)).unwrap_or(100),
            })
        })
        .collect();
    open_rate_feeds.sort_by_key(|f| (f.percent, f.title.to_lowercase()));
    open_rate_feeds.truncate(10);
    open_rate_feeds
}

fn admin_stats_view(
    admin_counts: Option<AdminCounts>,
    admin_entry_stats: Option<AdminEntryStats>,
) -> Option<AdminStatsView> {
    match (admin_counts, admin_entry_stats) {
        (Some(c), Some(e)) => Some(AdminStatsView {
            total_users: c.total_users,
            total_feeds: c.total_feeds,
            total_entries: e.total_entries,
            read_rate_fmt: format!("{:.1}", e.read_rate()),
        }),
        _ => None,
    }
}

fn admin_database_view(stats: Option<AdminDatabaseStats>) -> Option<AdminDatabaseStatsView> {
    stats.map(|s| AdminDatabaseStatsView {
        size_fmt: format_db_bytes(s.db_size_bytes),
        reclaimable: s.reclaimable.map(|r| ReclaimableView {
            size_fmt: format_db_bytes(r.bytes),
            frag_pct: (r.fragmentation_ratio * 100.0).round() as i64,
        }),
        total_entries: s.total_entries,
        avg_per_day_fmt: format!("{}", s.avg_new_entries_per_day.round() as i64),
        coverage_fmt: format!("{}d", s.coverage_days.round() as i64),
        tombstone_count: s.tombstone_count,
    })
}

/// `GET /statistics`.
pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<StatisticsQuery>,
) -> (Flash, StatisticsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;

    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };
    let show_admin_stats = is_admin && !is_masquerading;

    let (from, to, active_period) = resolve_statistics_period(&query);
    let chart_from = if active_period == "all" {
        let today = chrono::Utc::now().date_naive();
        (today - chrono::Duration::days(90)).to_string()
    } else {
        from.clone()
    };

    let user_id = auth_user.user.id;
    let from_c = from.clone();
    let to_c = to.clone();
    let chart_from_c = chart_from.clone();

    let (overview, daily, cats, feeds, admin_counts, admin_entry_stats, admin_db_stats) = {
        let overview =
            crate::models::statistics::get_personal_overview(&state.db, user_id, &from_c, &to_c)
                .await
                .unwrap_or_default();
        let daily = crate::models::statistics::get_daily_read_counts(
            &state.db,
            user_id,
            &chart_from_c,
            &to_c,
        )
        .await
        .unwrap_or_default();
        let cats =
            crate::models::statistics::get_entries_by_category(&state.db, user_id, &from_c, &to_c)
                .await
                .unwrap_or_default();
        let feeds =
            crate::models::statistics::get_top_feeds(&state.db, user_id, &from_c, &to_c, 10)
                .await
                .unwrap_or_default();
        let admin_counts = if show_admin_stats {
            crate::models::statistics::get_admin_counts(&state.db)
                .await
                .ok()
        } else {
            None
        };
        let admin_entry_stats = if show_admin_stats {
            crate::models::statistics::get_admin_entry_stats(&state.db, &from_c, &to_c)
                .await
                .ok()
        } else {
            None
        };
        // Plain read-through: a concurrent miss just recomputes harmlessly.
        let admin_db_stats = if show_admin_stats {
            if let Some(cached) = state.admin_db_stats_cache.get(&()) {
                Some(cached)
            } else {
                let fresh = crate::models::statistics::get_admin_database_stats(&state.db)
                    .await
                    .ok();
                if let Some(stats) = fresh.clone() {
                    state.admin_db_stats_cache.insert((), stats);
                }
                fresh
            }
        } else {
            None
        };
        (
            overview,
            daily,
            cats,
            feeds,
            admin_counts,
            admin_entry_stats,
            admin_db_stats,
        )
    };

    let (custom_from, custom_to) = if active_period == "custom" {
        (
            query.from.clone().unwrap_or_default(),
            query.to.clone().unwrap_or_default(),
        )
    } else {
        (String::new(), String::new())
    };

    let (daily_max, daily_read_counts) = daily_read_views(&daily);
    let categories = category_views(cats);
    let top_feeds = top_feed_views(feeds);

    // Ignores the period filter: open rate is only meaningful since opt-in.
    let open_rate_feeds = open_rate_views(
        crate::models::entry_open::open_rates_by_feed(&state.db, user_id)
            .await
            .unwrap_or_default(),
    );

    let tracked_since = crate::models::entry_open::tracking_window(&state.db, user_id)
        .await
        .unwrap_or(crate::models::entry_open::TrackingWindow {
            enabled_at: None,
            oldest_tracked: None,
        })
        .tracked_since()
        .map(|t| t.format("%Y-%m-%d").to_string());

    let admin = admin_stats_view(admin_counts, admin_entry_stats);
    let admin_db = admin_database_view(admin_db_stats);

    (
        flash,
        StatisticsTemplate {
            title: "Statistics",
            git_version: crate::GIT_VERSION,
            layout,
            active_period,
            custom_from,
            custom_to,
            total_entries: overview.total_entries,
            read_entries: overview.read_entries,
            unread_entries: overview.unread_entries(),
            starred_entries: overview.starred_entries,
            summaries: overview.summaries,
            read_rate_fmt: format!("{:.1}", overview.read_rate()),
            daily_max,
            daily_read_counts,
            categories,
            top_feeds,
            open_rate_feeds,
            tracked_since,
            min_tracked_for_rate: crate::models::entry_open::MIN_TRACKED_FOR_RATE,
            admin,
            admin_db,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::format_db_bytes;

    #[test]
    fn test_format_db_bytes() {
        assert_eq!(format_db_bytes(0), "0 B");
        assert_eq!(format_db_bytes(512), "512 B");
        assert_eq!(format_db_bytes(1536), "1.5 KB");
        assert_eq!(format_db_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_db_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
