package server

import (
	"testing"
	"time"
)

var timefmtNow = time.Date(2026, 6, 1, 12, 0, 0, 0, time.UTC)

// ageCases sit on both sides of every band edge, so a shifted comparison or a
// wrong divisor changes the text.
var ageCases = []struct {
	age      time.Duration
	relative string
	compact  string
}{
	{0, "Just now", "now"},
	{59 * time.Second, "Just now", "now"},
	{60 * time.Second, "1 minute ago", "1m"},
	{5*time.Minute + 30*time.Second, "5 minutes ago", "5m"},
	{3599 * time.Second, "59 minutes ago", "59m"},
	{3600 * time.Second, "1 hour ago", "1h"},
	{3*time.Hour + 59*time.Minute, "3 hours ago", "3h"},
	{86399 * time.Second, "23 hours ago", "23h"},
	{86400 * time.Second, "1 day ago", "1d"},
	{2*24*time.Hour + 23*time.Hour, "2 days ago", "2d"},
	{2_591_999 * time.Second, "29 days ago", "29d"},
	{2_592_000 * time.Second, "1 month ago", "1mo"},
	{75 * 24 * time.Hour, "2 months ago", "2mo"},
	{31_535_999 * time.Second, "12 months ago", "12mo"},
	{31_536_000 * time.Second, "1 year ago", "1y"},
	{800 * 24 * time.Hour, "2 years ago", "2y"},
}

func TestRelativeTimeBandsAndUnits(t *testing.T) {
	t.Parallel()
	for _, tc := range ageCases {
		published := timefmtNow.Add(-tc.age)
		got, datetime := relativeTime(&published, timefmtNow)
		if got != tc.relative {
			t.Errorf("age %v = %q, want %q", tc.age, got, tc.relative)
		}
		if datetime != rfc3339Offset(published) {
			t.Errorf("age %v datetime = %q", tc.age, datetime)
		}
	}
}

func TestRelativeTimeCompactBandsAndUnits(t *testing.T) {
	t.Parallel()
	for _, tc := range ageCases {
		published := timefmtNow.Add(-tc.age)
		if got := relativeTimeCompact(&published, timefmtNow); got != tc.compact {
			t.Errorf("age %v = %q, want %q", tc.age, got, tc.compact)
		}
	}
}

func TestRelativeTimeMissingTime(t *testing.T) {
	t.Parallel()
	if got, datetime := relativeTime(nil, timefmtNow); got != "Never" || datetime != "" {
		t.Errorf("relativeTime(nil) = %q, %q", got, datetime)
	}
	if got := relativeTimeCompact(nil, timefmtNow); got != "—" {
		t.Errorf("relativeTimeCompact(nil) = %q", got)
	}
}

func TestFreshnessBandEdgesFollowTheFeedOwnDate(t *testing.T) {
	t.Parallel()
	day := 24 * time.Hour
	cases := []struct {
		age       time.Duration
		class     string
		key       string
		fetchedAs string
	}{
		{30 * day, "", "fresh", "muted"},
		{30*day + 23*time.Hour, "", "fresh", "muted"},
		{31 * day, "feed-freshness-warning", "warning", "feed-freshness-warning"},
		{90*day + 23*time.Hour, "feed-freshness-warning", "warning", "feed-freshness-warning"},
		{91 * day, "feed-freshness-stale", "stale", "feed-freshness-stale"},
	}
	for _, tc := range cases {
		stamp := timefmtNow.Add(-tc.age)
		class, key := freshness(&stamp, nil, timefmtNow)
		if class != tc.class || key != tc.key {
			t.Errorf("own date, age %v = %q/%q, want %q/%q", tc.age, class, key, tc.class, tc.key)
		}
		class, key = freshness(nil, &stamp, timefmtNow)
		if class != tc.fetchedAs || key != tc.key {
			t.Errorf("fetched, age %v = %q/%q, want %q/%q", tc.age, class, key, tc.fetchedAs, tc.key)
		}
	}
	if class, key := freshness(nil, nil, timefmtNow); class != "feed-freshness-stale" || key != "stale" {
		t.Errorf("no dates = %q/%q", class, key)
	}
}
