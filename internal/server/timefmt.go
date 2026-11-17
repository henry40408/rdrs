package server

import (
	"fmt"
	"strings"
	"time"
)

// Feed freshness bands in days; also quoted by the /feeds help text.
const (
	freshMaxDays   = 30
	warningMaxDays = 90
)

// wholeDays truncates toward zero like chrono's num_days.
func wholeDays(d time.Duration) int64 { return int64(d / (24 * time.Hour)) }

func plural(n int64, unit string) string {
	if n == 1 {
		return fmt.Sprintf("%d %s ago", n, unit)
	}
	return fmt.Sprintf("%d %ss ago", n, unit)
}

// relativeTime is the "3 hours ago" text and the ISO datetime for the
// tooltip. A missing time is "Never" with no datetime.
func relativeTime(t *time.Time, now time.Time) (relative, datetime string) {
	if t == nil {
		return "Never", ""
	}
	age := now.Sub(*t)
	seconds := int64(age / time.Second)
	switch {
	case seconds < 60:
		relative = "Just now"
	case seconds < 3600:
		relative = plural(int64(age/time.Minute), "minute")
	case seconds < 86400:
		relative = plural(int64(age/time.Hour), "hour")
	case seconds < 2_592_000:
		relative = plural(wholeDays(age), "day")
	case seconds < 31_536_000:
		relative = plural(wholeDays(age)/30, "month")
	default:
		relative = plural(wholeDays(age)/365, "year")
	}
	return relative, rfc3339Offset(*t)
}

// rfc3339Offset is chrono's to_rfc3339 for a UTC time: "+00:00" and a
// fraction of 3, 6 or 9 digits only when needed.
func rfc3339Offset(t time.Time) string {
	return strings.TrimSuffix(chronoUTC(t), "Z") + "+00:00"
}

// freshness is the CSS class and key of a feed from its own date, else from
// when it was last fetched.
func freshness(feedUpdatedAt, fetchedAt *time.Time, now time.Time) (class, key string) {
	if feedUpdatedAt != nil {
		days := wholeDays(now.Sub(*feedUpdatedAt))
		switch {
		case days <= freshMaxDays:
			return "", "fresh"
		case days <= warningMaxDays:
			return "feed-freshness-warning", "warning"
		}
		return "feed-freshness-stale", "stale"
	}
	if fetchedAt == nil {
		return "feed-freshness-stale", "stale"
	}
	days := wholeDays(now.Sub(*fetchedAt))
	switch {
	case days <= freshMaxDays:
		return "muted", "fresh"
	case days <= warningMaxDays:
		return "feed-freshness-warning", "warning"
	}
	return "feed-freshness-stale", "stale"
}
