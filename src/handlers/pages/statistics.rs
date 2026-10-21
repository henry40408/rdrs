use askama::Template;
use axum::extract::{Query, State};

use super::{AppLayoutContext, build_app_layout};
use crate::AppState;
use crate::db::Db;
use crate::error::AppResult;
use crate::middleware::auth::PageAuthUser;
use crate::middleware::flash::Flash;
use crate::models::entry_open::{self, FeedOpenRate};
use crate::models::statistics::{
    self, AdminCounts, AdminDatabaseStats, AdminEntryStats, CategoryCount, DailyReadCount,
    FeedCount, PersonalOverview,
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

/// Cap bar count so long ranges stay tappable on mobile.
const MAX_DAILY_BARS: usize = 14;

/// The returned max is taken over buckets, not days, so bars scale to what is drawn.
fn daily_read_views(daily: &[DailyReadCount]) -> (i64, Vec<DailyReadView>) {
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

/// The page degrades to empty sections rather than failing, so a broken query
/// would otherwise look like an empty database; log it to keep it visible.
fn log_failure<T>(what: &'static str, result: AppResult<T>) -> Option<T> {
    result
        .inspect_err(|e| {
            tracing::error!(event = "statistics.query_failed", query = what, error = %e, "statistics query failed");
        })
        .ok()
}

fn log_and_default<T: Default>(what: &'static str, result: AppResult<T>) -> T {
    log_failure(what, result).unwrap_or_default()
}

/// Admins see site-wide numbers, except while masquerading as another user.
fn shows_admin_stats(auth_user: &PageAuthUser) -> bool {
    if auth_user.session.is_masquerading() {
        return false;
    }
    auth_user.user.is_admin()
}

/// The "all" period charts only the last 90 days; any other period charts its
/// whole range.
fn chart_range_start(active_period: &str, from: &str) -> String {
    if active_period != "all" {
        return from.to_string();
    }
    let today = chrono::Utc::now().date_naive();
    (today - chrono::Duration::days(90)).to_string()
}

/// The `from` / `to` inputs to echo back, only for a custom range.
fn custom_range_inputs(query: &StatisticsQuery, active_period: &str) -> (String, String) {
    if active_period != "custom" {
        return (String::new(), String::new());
    }
    (
        query.from.clone().unwrap_or_default(),
        query.to.clone().unwrap_or_default(),
    )
}

/// Date bounds for the statistics queries; the daily chart may start later than
/// the rest (see [`chart_range_start`]).
struct StatsRange<'a> {
    from: &'a str,
    to: &'a str,
    chart_from: &'a str,
}

struct PersonalStats {
    overview: PersonalOverview,
    daily: Vec<DailyReadCount>,
    categories: Vec<CategoryCount>,
    top_feeds: Vec<FeedCount>,
}

async fn fetch_personal_stats(db: &Db, user_id: i64, range: &StatsRange<'_>) -> PersonalStats {
    let StatsRange {
        from,
        to,
        chart_from,
    } = *range;
    let overview = log_and_default(
        "overview",
        statistics::get_personal_overview(db, user_id, from, to).await,
    );
    let daily = log_and_default(
        "daily_read_counts",
        statistics::get_daily_read_counts(db, user_id, chart_from, to).await,
    );
    let categories = log_and_default(
        "entries_by_category",
        statistics::get_entries_by_category(db, user_id, from, to).await,
    );
    let top_feeds = log_and_default(
        "top_feeds",
        statistics::get_top_feeds(
            db,
            &statistics::TopFeedsQuery {
                user_id,
                from,
                to,
                limit: 10,
            },
        )
        .await,
    );
    PersonalStats {
        overview,
        daily,
        categories,
        top_feeds,
    }
}

#[derive(Default)]
struct AdminStats {
    counts: Option<AdminCounts>,
    entry_stats: Option<AdminEntryStats>,
    db_stats: Option<AdminDatabaseStats>,
}

async fn fetch_admin_stats(state: &AppState, range: &StatsRange<'_>) -> AdminStats {
    let counts = log_failure(
        "admin_counts",
        statistics::get_admin_counts(&state.db).await,
    );
    let entry_stats = log_failure(
        "admin_entry_stats",
        statistics::get_admin_entry_stats(&state.db, range.from, range.to).await,
    );
    let db_stats = cached_admin_db_stats(state).await;
    AdminStats {
        counts,
        entry_stats,
        db_stats,
    }
}

/// Plain read-through: a concurrent miss just recomputes harmlessly.
async fn cached_admin_db_stats(state: &AppState) -> Option<AdminDatabaseStats> {
    if let Some(cached) = state.admin_db_stats_cache.get(&()) {
        return Some(cached);
    }
    let fresh = log_failure(
        "admin_database_stats",
        statistics::get_admin_database_stats(&state.db).await,
    )?;
    state.admin_db_stats_cache.insert((), fresh.clone());
    Some(fresh)
}

async fn fetch_tracked_since(db: &Db, user_id: i64) -> Option<String> {
    log_failure(
        "tracking_window",
        entry_open::tracking_window(db, user_id).await,
    )
    .unwrap_or(entry_open::TrackingWindow {
        enabled_at: None,
        oldest_tracked: None,
    })
    .tracked_since()
    .map(|t| t.format("%Y-%m-%d").to_string())
}

/// `GET /statistics`.
pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    flash: Flash,
    Query(query): Query<StatisticsQuery>,
) -> (Flash, StatisticsTemplate) {
    let layout = build_app_layout(&state, &auth_user, &flash).await;
    let show_admin_stats = shows_admin_stats(&auth_user);

    let (from, to, active_period) = resolve_statistics_period(&query);
    let chart_from = chart_range_start(&active_period, &from);
    let range = StatsRange {
        from: &from,
        to: &to,
        chart_from: &chart_from,
    };

    let user_id = auth_user.user.id;
    let personal = fetch_personal_stats(&state.db, user_id, &range).await;
    let admin_stats = if show_admin_stats {
        fetch_admin_stats(&state, &range).await
    } else {
        AdminStats::default()
    };

    let (custom_from, custom_to) = custom_range_inputs(&query, &active_period);

    let PersonalStats {
        overview,
        daily,
        categories,
        top_feeds,
    } = personal;
    let (daily_max, daily_read_counts) = daily_read_views(&daily);
    let categories = category_views(categories);
    let top_feeds = top_feed_views(top_feeds);

    // Ignores the period filter: open rate is only meaningful since opt-in.
    let open_rate_feeds = open_rate_views(log_and_default(
        "open_rates",
        entry_open::open_rates_by_feed(&state.db, user_id).await,
    ));
    let tracked_since = fetch_tracked_since(&state.db, user_id).await;

    let admin = admin_stats_view(admin_stats.counts, admin_stats.entry_stats);
    let admin_db = admin_database_view(admin_stats.db_stats);

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
