package models

import (
	"context"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
)

func explainSQLite(t *testing.T, database *db.DB, sqlText string) string {
	t.Helper()
	rows, err := database.QueryContext(context.Background(), "EXPLAIN QUERY PLAN "+sqlText, int64(1))
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = rows.Close() }()
	var details []string
	for rows.Next() {
		var id, parent, unused int64
		var detail string
		if err := rows.Scan(&id, &parent, &unused, &detail); err != nil {
			t.Fatal(err)
		}
		details = append(details, detail)
	}
	return strings.Join(details, "\n")
}

// Without idx_entry_feed_created_at both queries read created_at off the table
// per entry; the plan is the only observable that pins this.
func TestOpenRateQueriesAreIndexCovered(t *testing.T) {
	database := openAndMigrate(t, filepath.Join(t.TempDir(), "plan.sqlite3"), config.SQLite)
	for name, sqlText := range map[string]string{"open rates": openRatesSQL, "tracking window": trackingWindowSQL} {
		plan := explainSQLite(t, database, sqlText)
		if !strings.Contains(plan, "COVERING INDEX idx_entry_feed_created_at") {
			t.Errorf("%s: plan was:\n%s", name, plan)
		}
	}
}

func TestPercentIsSuppressedBelowTheSampleFloor(t *testing.T) {
	if _, ok := (FeedOpenRate{Tracked: MinTrackedForRate - 1, Opened: 1}).Percent(); ok {
		t.Error("a sample below the floor must not report a rate")
	}
}

func TestPercentRoundsToWholeNumbers(t *testing.T) {
	tests := []struct{ opened, tracked, want int64 }{
		{0, 10, 0}, {5, 10, 50}, {10, 10, 100},
		// 1/6 (16.7%) rounds up to 17, 4/6 (66.7%) to 67.
		{1, 6, 17}, {4, 6, 67},
		{1, MinTrackedForRate, 20},
	}
	for _, tt := range tests {
		got, ok := FeedOpenRate{Opened: tt.opened, Tracked: tt.tracked}.Percent()
		if !ok || got != tt.want {
			t.Errorf("%d/%d = %d, %v, want %d", tt.opened, tt.tracked, got, ok, tt.want)
		}
	}
}

func TestTrackedSinceTakesTheLaterOfOptInAndSurvivingData(t *testing.T) {
	enabled := time.Now().Add(-30 * 24 * time.Hour)
	oldest := time.Now().Add(-7 * 24 * time.Hour)
	earlier := enabled.Add(time.Second)
	tests := []struct {
		name   string
		window TrackingWindow
		want   *time.Time
	}{
		{"retention pruned the early entries", TrackingWindow{&enabled, &oldest}, &oldest},
		{"nothing pruned yet", TrackingWindow{&enabled, &earlier}, &earlier},
		{"opted in, nothing since", TrackingWindow{EnabledAt: &enabled}, &enabled},
		{"opted out", TrackingWindow{}, nil},
	}
	for _, tt := range tests {
		got := tt.window.TrackedSince()
		if (got == nil) != (tt.want == nil) || (got != nil && !got.Equal(*tt.want)) {
			t.Errorf("%s: got %v, want %v", tt.name, got, tt.want)
		}
	}
}

type openFixture struct {
	summaryFixture
}

func newOpenFixture(t *testing.T, database *db.DB) openFixture {
	t.Helper()
	return openFixture{newSummaryFixture(t, database, "reader")}
}

func TestRecordEntryOpenRequiresOptInAndOwnership(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newOpenFixture(t, database)
		other := newSummaryFixture(t, database, "other")
		mine := f.addEntry(t, "mine")
		theirs := other.addEntry(t, "theirs")

		if recorded, err := RecordEntryOpen(ctx, database, f.userID, mine); err != nil || recorded {
			t.Fatalf("before opt-in: %v, %v", recorded, err)
		}
		if err := UpdatePixelTracking(ctx, database, f.userID, true); err != nil {
			t.Fatal(err)
		}
		// The entry predates the opt-in, so it is outside the window.
		mustExec(t, database, `UPDATE entry SET created_at = $1`, db.FormatSpace(time.Now().UTC().Add(-time.Hour)))
		if recorded, err := RecordEntryOpen(ctx, database, f.userID, mine); err != nil || recorded {
			t.Fatalf("entry older than the opt-in: %v, %v", recorded, err)
		}
		fresh := f.addEntry(t, "fresh")
		mustExec(t, database, `UPDATE entry SET created_at = $1 WHERE id = $2`, db.FormatSpace(time.Now().UTC().Add(time.Hour)), fresh)
		if recorded, err := RecordEntryOpen(ctx, database, f.userID, fresh); err != nil || !recorded {
			t.Fatalf("first open: %v, %v", recorded, err)
		}
		if recorded, err := RecordEntryOpen(ctx, database, f.userID, fresh); err != nil || recorded {
			t.Fatalf("a re-fetch must be idempotent: %v, %v", recorded, err)
		}
		if recorded, err := RecordEntryOpen(ctx, database, f.userID, theirs); err != nil || recorded {
			t.Fatalf("another account's entry: %v, %v", recorded, err)
		}
	})
}

func TestOpenRatesCountTrackedAndOpenedPerFeed(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newOpenFixture(t, database)
		if rates, err := OpenRatesByFeed(ctx, database, f.userID); err != nil || len(rates) != 0 {
			t.Fatalf("opted out: %v, %v", rates, err)
		}
		if err := UpdatePixelTracking(ctx, database, f.userID, true); err != nil {
			t.Fatal(err)
		}
		future := db.FormatSpace(time.Now().UTC().Add(time.Hour))
		var ids []int64
		for _, guid := range []string{"a", "b", "c"} {
			id := f.addEntry(t, guid)
			mustExec(t, database, `UPDATE entry SET created_at = $1 WHERE id = $2`, future, id)
			ids = append(ids, id)
		}
		if _, err := RecordEntryOpen(ctx, database, f.userID, ids[0]); err != nil {
			t.Fatal(err)
		}
		rates, err := OpenRatesByFeed(ctx, database, f.userID)
		if err != nil || len(rates) != 1 {
			t.Fatalf("rates = %v, %v", rates, err)
		}
		if got := rates[0]; got.FeedID != f.feedID || got.Tracked != 3 || got.Opened != 1 {
			t.Errorf("rate = %+v", got)
		}

		window, err := FindTrackingWindow(ctx, database, f.userID)
		if err != nil || window.EnabledAt == nil || window.OldestTracked == nil {
			t.Fatalf("window = %+v, %v", window, err)
		}
		if since := window.TrackedSince(); since == nil || !since.After(*window.EnabledAt) {
			t.Errorf("tracked since = %v, enabled %v", since, window.EnabledAt)
		}
	})
}

func TestTrackingWindowIsEmptyWhenOptedOut(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newOpenFixture(t, database)
		window, err := FindTrackingWindow(context.Background(), database, f.userID)
		if err != nil || window != (TrackingWindow{}) {
			t.Errorf("window = %+v, %v", window, err)
		}
	})
}
