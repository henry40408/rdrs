package entry_test

import (
	"context"
	"fmt"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// seconds is a published time n*10 seconds into a fixed day, so list order is
// the order of n.
func seconds(n int) time.Time {
	return time.Date(2025, 1, 1, 12, 0, n*10, 0, time.UTC)
}

func (f fixture) addSeries(t *testing.T, count int) []int64 {
	t.Helper()
	ids := make([]int64, count)
	for i := range ids {
		ids[i] = f.add(t, fmt.Sprint("guid-", i), fmt.Sprint("Entry ", i), seconds(i))
	}
	return ids
}

func (f fixture) setReadAt(t *testing.T, id int64, at time.Time) {
	t.Helper()
	_, err := f.database.ExecContext(context.Background(),
		`UPDATE entry SET read_at = $1 WHERE id = $2`, db.FormatSpace(at), id)
	must(t, err)
}

func assertNeighbors(t *testing.T, got entry.Neighbors, prev, next *int64) {
	t.Helper()
	if fmt.Sprint(deref(got.PrevID), deref(got.NextID)) != fmt.Sprint(deref(prev), deref(next)) {
		t.Errorf("neighbors = (%s, %s), want (%s, %s)", deref(got.PrevID), deref(got.NextID), deref(prev), deref(next))
	}
}

func deref(id *int64) string {
	if id == nil {
		return "none"
	}
	return fmt.Sprint(*id)
}

func neighbors(t *testing.T, f fixture, id int64, filter *entry.Filter) entry.Neighbors {
	t.Helper()
	got, err := entry.FindNeighbors(context.Background(), f.database, f.userID, id, filter)
	must(t, err)
	return got
}

func TestFindNeighborsStarredOnly(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		_, err := entry.StarEntry(context.Background(), database, ids[1])
		must(t, err)
		_, err = entry.StarEntry(context.Background(), database, ids[3])
		must(t, err)

		starred := &entry.Filter{StarredOnly: true}
		assertNeighbors(t, neighbors(t, f, ids[3], starred), nil, &ids[1])
		assertNeighbors(t, neighbors(t, f, ids[1], starred), &ids[3], nil)
		assertNeighbors(t, neighbors(t, f, ids[3], &entry.Filter{}), &ids[4], &ids[2])
	})
}

func TestFindNeighborsUnreadSnapshot(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		// ids[1] was read before the snapshot boundary, ids[2] inside it.
		f.setReadAt(t, ids[1], time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))
		f.setReadAt(t, ids[2], time.Date(2025, 7, 1, 0, 0, 0, 0, time.UTC))
		snapshot := "2025-06-01 00:00:00"
		filter := &entry.Filter{UnreadOnly: true, ReadAfter: &snapshot}

		// next from ids[3] lands on the just-read ids[2]: still reachable.
		assertNeighbors(t, neighbors(t, f, ids[3], filter), &ids[4], &ids[2])
		// next from ids[2] skips the pre-snapshot ids[1].
		assertNeighbors(t, neighbors(t, f, ids[2], filter), &ids[3], &ids[0])
		// Without a snapshot both read entries are skipped.
		assertNeighbors(t, neighbors(t, f, ids[3], &entry.Filter{UnreadOnly: true}), &ids[4], &ids[0])
	})
}

func TestFindNeighborsReadOnly(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 4)
		for _, i := range []int{0, 2} {
			_, err := entry.MarkAsRead(context.Background(), database, ids[i])
			must(t, err)
		}
		read := &entry.Filter{ReadOnly: true}
		assertNeighbors(t, neighbors(t, f, ids[2], read), nil, &ids[0])
		assertNeighbors(t, neighbors(t, f, ids[0], read), &ids[2], nil)
	})
}

func TestFindNeighborsTieBreaksOnID(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		same := day(1)
		first := f.add(t, "a", "A", same)
		second := f.add(t, "b", "B", same)
		got := neighbors(t, f, second, &entry.Filter{})
		assertNeighbors(t, got, nil, &first)
	})
}

func TestFindNeighborsIsOwnerScoped(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))
		got, err := entry.FindNeighbors(context.Background(), database, f.userID+999, id, &entry.Filter{})
		must(t, err)
		assertNeighbors(t, got, nil, nil)
	})
}

func position(t *testing.T, f fixture, id int64, filter *entry.Filter) entry.Position {
	t.Helper()
	got, err := entry.FindPosition(context.Background(), f.database, f.userID, id, filter)
	must(t, err)
	return got
}

func assertPosition(t *testing.T, name string, got entry.Position, position, total int64) {
	t.Helper()
	if got.Position != position || got.Total != total {
		t.Errorf("%s = (%d of %d), want (%d of %d)", name, got.Position, got.Total, position, total)
	}
}

func TestFindPositionCountsWithinFilter(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		all := &entry.Filter{}
		assertPosition(t, "newest", position(t, f, ids[4], all), 1, 5)
		assertPosition(t, "oldest", position(t, f, ids[0], all), 5, 5)

		for _, i := range []int{1, 3} {
			_, err := entry.StarEntry(context.Background(), database, ids[i])
			must(t, err)
		}
		assertPosition(t, "starred", position(t, f, ids[1], &entry.Filter{StarredOnly: true}), 2, 2)
	})
}

func TestFindPositionStopsCountingAtTheLimit(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		tx, err := database.Begin(ctx)
		must(t, err)
		defer func() { _ = tx.Rollback() }()
		count := int(entry.PositionCountLimit) + 3
		ids := make([]int64, count)
		for i := range ids {
			result, err := entry.UpsertEntry(ctx, tx, &entry.Upsert{
				FeedID: f.feedID, GUID: fmt.Sprint("g", i), PublishedAt: ptr(day(1).Add(time.Duration(i) * time.Second)),
			})
			must(t, err)
			ids[i] = result.ID
		}
		must(t, tx.Commit())

		all := &entry.Filter{}
		newest := position(t, f, ids[count-1], all)
		if newest.Position != 1 || newest.Total != entry.PositionCountLimit+1 {
			t.Errorf("newest = %+v", newest)
		}
		oldest := position(t, f, ids[0], all)
		if oldest.Position != entry.PositionCountLimit+1 || oldest.Limit != entry.PositionCountLimit {
			t.Errorf("oldest = %+v", oldest)
		}
	})
}

func TestFindPositionUnreadSnapshotCountsEntriesReadSince(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		f.setReadAt(t, ids[1], time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))
		f.setReadAt(t, ids[3], time.Date(2025, 7, 1, 0, 0, 0, 0, time.UTC))
		snapshot := "2025-06-01 00:00:00"
		filter := &entry.Filter{UnreadOnly: true, ReadAfter: &snapshot}

		// The list is 4, 3, 2, 0: ids[1] is gone, ids[3] stays.
		assertPosition(t, "newest", position(t, f, ids[4], filter), 1, 4)
		assertPosition(t, "read since", position(t, f, ids[3], filter), 2, 4)
		assertPosition(t, "oldest", position(t, f, ids[0], filter), 4, 4)
	})
}
