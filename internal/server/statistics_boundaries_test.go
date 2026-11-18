package server

import (
	"context"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func TestStatisticsPageEndsTheWindowOnTheReadersOwnDay(t *testing.T) {
	// 20:00 UTC on the 10th is already the 11th in Taipei.
	now := time.Date(2026, 6, 10, 20, 0, 0, 0, time.UTC)
	app := newITestAppWith(t, Options{Config: itestConfig(), Now: func() time.Time { return now }})
	user := app.SeedAccount("taipei", fixturePassword, models.RoleUser)
	app.Login("taipei")
	ctx := context.Background()
	if err := models.UpdateTimezone(ctx, app.DB, user.ID, "Asia/Taipei"); err != nil {
		t.Fatal(err)
	}
	categories, err := models.ListCategoriesByUser(ctx, app.DB, user.ID)
	if err != nil || len(categories) != 1 {
		t.Fatalf("categories = %v, %v", categories, err)
	}
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{CategoryID: categories[0].ID, URL: "https://example.com/tz.xml"})
	if err != nil {
		t.Fatal(err)
	}
	// The 11th in Taipei, still the 10th's future for a UTC reader.
	published := time.Date(2026, 6, 11, 1, 0, 0, 0, time.UTC)
	if _, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{FeedID: feed.ID, GUID: "tz-1", PublishedAt: &published}); err != nil {
		t.Fatal(err)
	}

	body := app.GetOK("/statistics")
	if !strings.Contains(body, `stats-card-value">1<`) {
		t.Errorf("an entry published on the reader's today is missing from the totals:\n%s", body)
	}
}

func TestDailyBarsMarkNothingWhenNothingWasRead(t *testing.T) {
	t.Parallel()
	var daily []models.DailyReadCount
	for i := range 3 {
		daily = append(daily, models.DailyReadCount{Date: day(2026, time.January, 1+i), Count: 0})
	}
	max, bars := dailyBars(daily)
	if max != 0 || len(bars) != 3 {
		t.Fatalf("max %d bars %+v", max, bars)
	}
	for i, bar := range bars {
		if bar.IsMax {
			t.Errorf("bar %d is marked busiest with a count of zero", i)
		}
	}
}

func TestDailyBarsMarkTheBusiestOfSeveralBuckets(t *testing.T) {
	t.Parallel()
	counts := []int64{1, 0, 5, 5, 2}
	var daily []models.DailyReadCount
	for i, count := range counts {
		daily = append(daily, models.DailyReadCount{Date: day(2026, time.January, 1+i), Count: count})
	}
	max, bars := dailyBars(daily)
	if max != 5 {
		t.Fatalf("max = %d", max)
	}
	for i, bar := range bars {
		if bar.IsMax != (i == 2) {
			t.Errorf("bar %d IsMax = %v", i, bar.IsMax)
		}
	}
}

func TestDatabaseStatsViewRoundsAndScales(t *testing.T) {
	app := defaultITestApp(t)
	app.App.AdminDBStats.Put(models.AdminDatabaseStats{
		DBSizeBytes: 1 << 20, TotalEntries: 7, AvgNewEntriesPerDay: 2.5, CoverageDays: 12.4, TombstoneCount: 3,
		Reclaimable: &models.ReclaimableSpace{Bytes: 4096, FragmentationRatio: 0.374},
	})

	view := app.App.databaseStats(context.Background())
	if view == nil || view.Reclaimable == nil {
		t.Fatalf("view = %+v", view)
	}
	if view.Reclaimable.FragPercent != 37 || view.AvgPerDay != "3" || view.Coverage != "12d" || view.TombstoneCount != 3 {
		t.Errorf("view = %+v, reclaimable %+v", view, *view.Reclaimable)
	}
}
