package entry_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func (f fixture) enableRetention(t *testing.T, days int) {
	t.Helper()
	_, err := f.database.ExecContext(context.Background(),
		`INSERT INTO user_settings (user_id, retention_read_days) VALUES ($1, $2)`, f.userID, days)
	must(t, err)
}

// addRead stores an entry read daysAgo days ago.
func (f fixture) addRead(t *testing.T, guid string, daysAgo int, starred bool) {
	t.Helper()
	ctx := context.Background()
	_, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feedID, GUID: guid, Title: ptr(guid)})
	must(t, err)
	readAt := db.FormatSpace(time.Now().UTC().AddDate(0, 0, -daysAgo))
	_, err = f.database.ExecContext(ctx, `UPDATE entry SET read_at = $1 WHERE guid = $2 AND feed_id = $3`, readAt, guid, f.feedID)
	must(t, err)
	if !starred {
		return
	}
	_, err = f.database.ExecContext(ctx,
		`UPDATE entry SET starred_at = $1 WHERE guid = $2 AND feed_id = $3`, db.FormatSpace(time.Now().UTC()), guid, f.feedID)
	must(t, err)
}

func prune(t *testing.T, database *db.DB, batch int) int64 {
	t.Helper()
	n, err := entry.PruneReadRetentionBatch(context.Background(), database, batch)
	must(t, err)
	return n
}

func assertStored(t *testing.T, f fixture, guid string, want bool) {
	t.Helper()
	found, err := entry.FindByGUIDAndFeed(context.Background(), f.database, guid, f.feedID)
	must(t, err)
	if (found != nil) != want {
		t.Errorf("%s stored = %v, want %v", guid, found != nil, want)
	}
}

func assertTombstoned(t *testing.T, f fixture, guid string) {
	t.Helper()
	result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{FeedID: f.feedID, GUID: guid})
	must(t, err)
	if result.Outcome != entry.SkippedTombstoned {
		t.Errorf("%s outcome = %v, want SkippedTombstoned", guid, result.Outcome)
	}
}

func TestPruneRespectsThresholdStarAndOptIn(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		f.addRead(t, "old", 40, false)
		f.addRead(t, "oldstar", 40, true)
		f.addRead(t, "fresh", 1, false)
		f.add(t, "unread", "unread", day(1))

		if n := prune(t, database, 500); n != 0 {
			t.Fatalf("pruned %d before opt-in", n)
		}
		f.enableRetention(t, 30)
		if n := prune(t, database, 500); n != 1 {
			t.Fatalf("pruned %d, want 1", n)
		}
		assertStored(t, f, "old", false)
		for _, guid := range []string{"oldstar", "fresh", "unread"} {
			assertStored(t, f, guid, true)
		}
		assertTombstoned(t, f, "old")
		if n := prune(t, database, 500); n != 0 {
			t.Fatalf("second prune removed %d", n)
		}
	})
}

func TestPruneBatchSizeLimitsRowsAndTombstonesEveryVictim(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		f.enableRetention(t, 1)
		for i := 0; i < 5; i++ {
			f.addRead(t, fmt.Sprint("g", i), 10, false)
		}
		for _, want := range []int64{2, 2, 1, 0} {
			if n := prune(t, database, 2); n != want {
				t.Fatalf("pruned %d, want %d", n, want)
			}
		}
		for i := 0; i < 5; i++ {
			guid := fmt.Sprint("g", i)
			assertStored(t, f, guid, false)
			assertTombstoned(t, f, guid)
		}
	})
}
