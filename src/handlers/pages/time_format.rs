//! Relative-time and freshness formatting helpers shared by the SSR page
//! handlers. Pure functions over `chrono` datetimes — no request state.

pub fn format_relative_time_compact(dt: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(dt) = dt else {
        return "—".to_string();
    };
    let duration = chrono::Utc::now().signed_duration_since(dt);
    let seconds = duration.num_seconds();
    if seconds < 60 {
        "now".to_string()
    } else if seconds < 3600 {
        format!("{}m", duration.num_minutes())
    } else if seconds < 86400 {
        format!("{}h", duration.num_hours())
    } else if seconds < 2_592_000 {
        format!("{}d", duration.num_days())
    } else if seconds < 31_536_000 {
        format!("{}mo", duration.num_days() / 30)
    } else {
        format!("{}y", duration.num_days() / 365)
    }
}

/// Format a datetime as a human-readable relative time string.
/// Returns (`relative_text`, `iso_datetime_for_tooltip`).
pub fn format_relative_time(dt: Option<chrono::DateTime<chrono::Utc>>) -> (String, String) {
    match dt {
        None => ("Never".to_string(), String::new()),
        Some(dt) => {
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(dt);
            let seconds = duration.num_seconds();
            let relative = if seconds < 60 {
                "Just now".to_string()
            } else if seconds < 3600 {
                let mins = duration.num_minutes();
                format!("{} minute{} ago", mins, if mins == 1 { "" } else { "s" })
            } else if seconds < 86400 {
                let hours = duration.num_hours();
                format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
            } else if seconds < 2_592_000 {
                let days = duration.num_days();
                format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
            } else if seconds < 31_536_000 {
                let months = duration.num_days() / 30;
                format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
            } else {
                let years = duration.num_days() / 365;
                format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
            };
            (relative, dt.to_rfc3339())
        }
    }
}

/// Age (in days) up to which a feed still counts as fresh. Named because the
/// `/feeds` help disclosure quotes these thresholds back to the user — the
/// template reads them from here so the prose can never drift from the rule.
pub const FRESH_MAX_DAYS: i64 = 30;

/// Age (in days) up to which a feed is merely a warning rather than stale.
pub const WARNING_MAX_DAYS: i64 = 90;

/// Compute freshness CSS class and key from `feed_updated_at` and `fetched_at`.
pub fn compute_freshness(
    feed_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    fetched_at: Option<chrono::DateTime<chrono::Utc>>,
) -> (String, String) {
    let now = chrono::Utc::now();
    match feed_updated_at {
        Some(updated) => {
            let days = (now - updated).num_days();
            if days <= FRESH_MAX_DAYS {
                (String::new(), "fresh".to_string())
            } else if days <= WARNING_MAX_DAYS {
                ("feed-freshness-warning".to_string(), "warning".to_string())
            } else {
                ("feed-freshness-stale".to_string(), "stale".to_string())
            }
        }
        None => match fetched_at {
            Some(fetched) if (now - fetched).num_days() <= FRESH_MAX_DAYS => {
                ("muted".to_string(), "fresh".to_string())
            }
            Some(fetched) if (now - fetched).num_days() <= WARNING_MAX_DAYS => {
                ("feed-freshness-warning".to_string(), "warning".to_string())
            }
            _ => ("feed-freshness-stale".to_string(), "stale".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    /// A timestamp `d` in the past, for driving the "ago" branches. Values are
    /// chosen comfortably inside each band so sub-second test latency can't tip
    /// them across a boundary.
    #[allow(
        clippy::unnecessary_wraps,
        reason = "returns Option to feed the Option-taking function under test directly"
    )]
    fn ago(d: Duration) -> Option<chrono::DateTime<Utc>> {
        Some(Utc::now() - d)
    }

    #[test]
    fn compact_none_renders_dash() {
        assert_eq!(format_relative_time_compact(None), "—");
    }

    #[test]
    fn compact_covers_every_band() {
        assert_eq!(
            format_relative_time_compact(ago(Duration::seconds(30))),
            "now"
        );
        assert_eq!(
            format_relative_time_compact(ago(Duration::minutes(5))),
            "5m"
        );
        assert_eq!(format_relative_time_compact(ago(Duration::hours(3))), "3h");
        assert_eq!(format_relative_time_compact(ago(Duration::days(4))), "4d");
        // 45 days / 30 = 1 month band
        assert_eq!(format_relative_time_compact(ago(Duration::days(45))), "1mo");
        // 400 days / 365 = 1 year band
        assert_eq!(format_relative_time_compact(ago(Duration::days(400))), "1y");
    }

    #[test]
    fn relative_none_is_never_with_empty_tooltip() {
        let (text, iso) = format_relative_time(None);
        assert_eq!(text, "Never");
        assert!(iso.is_empty());
    }

    #[test]
    fn relative_just_now_and_tooltip_is_rfc3339() {
        let dt = Utc::now() - Duration::seconds(30);
        let (text, iso) = format_relative_time(Some(dt));
        assert_eq!(text, "Just now");
        assert_eq!(iso, dt.to_rfc3339());
    }

    #[test]
    fn relative_singular_units() {
        assert_eq!(
            format_relative_time(ago(Duration::seconds(61))).0,
            "1 minute ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::seconds(3700))).0,
            "1 hour ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::hours(25))).0,
            "1 day ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::days(35))).0,
            "1 month ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::days(400))).0,
            "1 year ago"
        );
    }

    #[test]
    fn relative_plural_units() {
        assert_eq!(
            format_relative_time(ago(Duration::minutes(5))).0,
            "5 minutes ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::hours(5))).0,
            "5 hours ago"
        );
        assert_eq!(format_relative_time(ago(Duration::days(5))).0, "5 days ago");
        assert_eq!(
            format_relative_time(ago(Duration::days(95))).0,
            "3 months ago"
        );
        assert_eq!(
            format_relative_time(ago(Duration::days(800))).0,
            "2 years ago"
        );
    }

    #[test]
    fn freshness_from_feed_updated_at() {
        assert_eq!(
            compute_freshness(ago(Duration::days(10)), None),
            (String::new(), "fresh".to_string())
        );
        assert_eq!(
            compute_freshness(ago(Duration::days(60)), None),
            ("feed-freshness-warning".to_string(), "warning".to_string())
        );
        assert_eq!(
            compute_freshness(ago(Duration::days(120)), None),
            ("feed-freshness-stale".to_string(), "stale".to_string())
        );
    }

    #[test]
    fn freshness_falls_back_to_fetched_at() {
        assert_eq!(
            compute_freshness(None, ago(Duration::days(10))),
            ("muted".to_string(), "fresh".to_string())
        );
        assert_eq!(
            compute_freshness(None, ago(Duration::days(60))),
            ("feed-freshness-warning".to_string(), "warning".to_string())
        );
        assert_eq!(
            compute_freshness(None, ago(Duration::days(120))),
            ("feed-freshness-stale".to_string(), "stale".to_string())
        );
    }

    #[test]
    fn freshness_unknown_is_stale() {
        assert_eq!(
            compute_freshness(None, None),
            ("feed-freshness-stale".to_string(), "stale".to_string())
        );
    }
}
