package entry_test

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// seedNonMonotonic stores six entries in date order, then four back-dated ones
// with later ids, so id order and published order diverge (#164).
func seedNonMonotonic(t *testing.T, f fixture) {
	t.Helper()
	rows := []struct{ guid, published string }{
		{"g1", "2026-04-01 10:00:00"}, {"g2", "2026-04-02 10:00:00"}, {"g3", "2026-04-03 10:00:00"},
		{"g4", "2026-04-04 10:00:00"}, {"g5", "2026-04-05 10:00:00"}, {"g6", "2026-04-06 10:00:00"},
		{"g7-bd", "2026-03-01 10:00:00"}, {"g8-bd", "2026-03-02 10:00:00"},
		{"g9-bd", "2026-03-03 10:00:00"}, {"g10-bd", "2026-03-04 10:00:00"},
	}
	for _, row := range rows {
		_, err := f.database.ExecContext(context.Background(),
			`INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)`, f.feedID, row.guid, row.published)
		must(t, err)
	}
}

// walk pages through every entry with composite cursors built from
// FetchSortTS and returns the ids in the order visited.
func walk(t *testing.T, f fixture, oldestFirst bool) []int64 {
	t.Helper()
	ctx := context.Background()
	var seen []int64
	visited := map[int64]bool{}
	params := &entry.ContinuationParams{OldestFirst: oldestFirst, Limit: 3, SortOrder: entry.SortPublishedAt}
	for len(seen) <= 100 {
		page, err := entry.ListByUserWithContinuation(ctx, f.database, f.userID, &entry.Filter{}, params)
		must(t, err)
		if len(page) == 0 {
			return seen
		}
		for i := range page {
			if visited[page[i].ID] {
				t.Fatalf("duplicate id %d", page[i].ID)
			}
			visited[page[i].ID] = true
			seen = append(seen, page[i].ID)
		}
		last := page[len(page)-1]
		sortTS, err := entry.FetchSortTS(ctx, f.database, last.ID, entry.SortPublishedAt)
		must(t, err)
		params.Continuation = entry.CompositeCursor{SortTS: *sortTS, ID: last.ID}
	}
	t.Fatal("runaway loop")
	return nil
}

func TestCompositeCursorWalksNonMonotonicDataWithoutSkip(t *testing.T) {
	for _, oldestFirst := range []bool{false, true} {
		eachBackend(t, func(t *testing.T, database *db.DB) {
			f := newFixture(t, database)
			seedNonMonotonic(t, f)
			if seen := walk(t, f, oldestFirst); len(seen) != 10 {
				t.Errorf("oldestFirst=%v visited %d entries, want 10", oldestFirst, len(seen))
			}
		})
	}
}

func TestLegacyIDCursorStillPaginates(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		params := &entry.ContinuationParams{Limit: 10, SortOrder: entry.SortPublishedAt, Continuation: entry.LegacyIDCursor(ids[4])}
		page, err := entry.ListByUserWithContinuation(context.Background(), database, f.userID, &entry.Filter{}, params)
		must(t, err)
		if len(page) != 4 {
			t.Fatalf("page = %d entries, want 4", len(page))
		}
		for i := range page {
			if page[i].ID >= ids[4] {
				t.Errorf("id %d not below the cursor", page[i].ID)
			}
		}
	})
}
