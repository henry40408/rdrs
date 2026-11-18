package server

import (
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/timezone"
)

func day(year int, month time.Month, d int) timezone.Date {
	return timezone.Date{Year: year, Month: month, Day: d}
}

func TestStatisticsPeriodsEndOnTheReadersToday(t *testing.T) {
	today := day(2026, time.November, 17)
	text := func(s string) *string { return &s }
	cases := []struct {
		name         string
		query        statisticsQuery
		from, to, on string
	}{
		{"default", statisticsQuery{}, "2026-11-10", "2026-11-18", "7d"},
		{"30d", statisticsQuery{period: text("30d")}, "2026-10-18", "2026-11-18", "30d"},
		{"all", statisticsQuery{period: text("all")}, "1900-01-01", "2026-11-18", "all"},
		{"custom", statisticsQuery{period: text("custom"), from: text("2026-01-01"), to: text("2026-01-31")}, "2026-01-01", "2026-02-01", "custom"},
		{"custom without padding", statisticsQuery{period: text("custom"), from: text("2026-1-5"), to: text("2026-1-6")}, "2026-01-05", "2026-01-07", "custom"},
		{"custom capped at a year", statisticsQuery{period: text("custom"), from: text("2020-01-01"), to: text("2026-01-01")}, "2020-01-01", "2021-01-01", "custom"},
		{"custom reversed", statisticsQuery{period: text("custom"), from: text("2026-02-01"), to: text("2026-01-01")}, "2026-11-10", "2026-11-18", "7d"},
		{"custom garbled", statisticsQuery{period: text("custom"), from: text("x"), to: text("y")}, "2026-11-10", "2026-11-18", "7d"},
		{"unknown", statisticsQuery{period: text("1y")}, "2026-11-10", "2026-11-18", "7d"},
	}
	for _, c := range cases {
		got := resolveStatisticsPeriod(c.query, today)
		if got.from.String() != c.from || got.to.String() != c.to || got.active != c.on {
			t.Errorf("%s: %s..%s %s, want %s..%s %s", c.name, got.from, got.to, got.active, c.from, c.to, c.on)
		}
	}
}

func TestTheAllPeriodChartsNinetyDays(t *testing.T) {
	today := day(2026, time.November, 17)
	all := resolveStatisticsPeriod(statisticsQuery{period: ptr("all")}, today)
	if got := all.chartFrom(today).String(); got != "2026-08-19" {
		t.Errorf("all chart from %s", got)
	}
	week := resolveStatisticsPeriod(statisticsQuery{}, today)
	if got := week.chartFrom(today).String(); got != "2026-11-10" {
		t.Errorf("7d chart from %s", got)
	}
}

func TestFormatDBBytes(t *testing.T) {
	cases := map[int64]string{0: "0 B", 512: "512 B", 1536: "1.5 KB", 5 << 20: "5.0 MB", 3 << 30: "3.0 GB"}
	for bytes, want := range cases {
		if got := formatDBBytes(bytes); got != want {
			t.Errorf("formatDBBytes(%d) = %q, want %q", bytes, got, want)
		}
	}
}

func TestBarPercentRoundsAndFloorsAtOne(t *testing.T) {
	cases := []struct {
		count, largest int64
		want           int
	}{
		{0, 10, 0}, {5, 0, 0}, {-1, 10, 0}, {1, 1000, 1}, {1, 3, 33}, {2, 3, 67}, {10, 10, 100},
	}
	for _, c := range cases {
		if got := barPercent(c.count, c.largest); got != c.want {
			t.Errorf("barPercent(%d, %d) = %d, want %d", c.count, c.largest, got, c.want)
		}
	}
}

func TestOpenRatesListLowestFirstAndSkipThinData(t *testing.T) {
	title := func(s string) *string { return &s }
	bars := openRateBars([]models.FeedOpenRate{
		{Title: title("zeta"), Tracked: 10, Opened: 5},
		{Title: title("Alpha"), Tracked: 10, Opened: 5},
		{Title: title("thin"), Tracked: models.MinTrackedForRate - 1, Opened: 0},
		{Title: nil, Tracked: 10, Opened: 1},
	})
	var labels []string
	for _, bar := range bars {
		labels = append(labels, bar.Label)
	}
	if got := strings.Join(labels, ","); got != "(untitled feed),Alpha,zeta" {
		t.Errorf("order = %s", got)
	}
	if bars[1].Detail != "50% (5/10)" || bars[1].WidthPercent != 50 {
		t.Errorf("row = %+v", bars[1])
	}
}

func TestDailyBarsMarkOnlyTheFirstBusiestBucket(t *testing.T) {
	var daily []models.DailyReadCount
	for i := range 3 {
		daily = append(daily, models.DailyReadCount{Date: day(2026, time.January, 1+i), Count: 4})
	}
	max, bars := dailyBars(daily)
	if max != 4 || len(bars) != 3 || !bars[0].IsMax || bars[1].IsMax || bars[0].HeightPercent != 100 {
		t.Errorf("max %d bars %+v", max, bars)
	}
	if bars[0].DateLabel != "2026-01-01" || bars[0].ShortLabel != "01/01" {
		t.Errorf("labels %+v", bars[0])
	}
}

func TestStatisticsPageForAdminAndUser(t *testing.T) {
	f := newSignedIn(t)
	f.withEntries(t)
	body := f.get("/statistics?period=custom&from=2026-01-01&to=2026-12-31").Body.String()
	for _, want := range []string{`value="2026-01-01"`, `data-testid="stat-site-entries-total"`, `data-testid="stat-db-total-entries"`, "Total Entries"} {
		if !strings.Contains(body, want) {
			t.Errorf("admin page lacks %q", want)
		}
	}
	if rec := f.get("/statistics?period=custom&from=2026-01-01&to=2026-12-31&period=x"); rec.Code != 400 {
		t.Errorf("repeated key: %d, want 400", rec.Code)
	}
}
