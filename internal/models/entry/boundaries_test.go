package entry_test

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func withSortTS(id int64, sortTS string) entry.WithSortTS {
	row := entry.WithSortTS{SortTS: &sortTS}
	row.ID = id
	return row
}

func TestNextContinuationNeedsTheLookAheadRow(t *testing.T) {
	t.Parallel()
	rows := []entry.WithSortTS{
		withSortTS(1, "2025-01-03 00:00:00"), withSortTS(2, "2025-01-02 00:00:00"), withSortTS(3, "2025-01-01 00:00:00"),
	}
	cases := []struct {
		name     string
		rows     []entry.WithSortTS
		pageSize int
		want     string
	}{
		{"exactly a page", rows, 3, ""},
		{"one look-ahead row", rows, 2, "2025-01-02 00:00:00|2"},
		{"a page of one", rows[:2], 1, "2025-01-03 00:00:00|1"},
		{"no page size", rows, 0, ""},
		{"negative page size", rows, -1, ""},
	}
	for _, tc := range cases {
		got := entry.NextContinuation(tc.rows, tc.pageSize)
		switch {
		case tc.want == "" && got != nil:
			t.Errorf("%s: cursor %q, want none", tc.name, *got)
		case tc.want != "" && (got == nil || *got != tc.want):
			t.Errorf("%s: cursor %v, want %q", tc.name, got, tc.want)
		}
	}
}

func TestIsOwnedByAnswersForTheOwnerAndNobodyElse(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))
		ctx := context.Background()
		for _, tc := range []struct {
			userID, entryID int64
			want            bool
		}{{f.userID, id, true}, {f.userID + 1, id, false}, {f.userID, id + 1, false}} {
			got, err := entry.IsOwnedBy(ctx, database, tc.entryID, tc.userID)
			if err != nil || got != tc.want {
				t.Errorf("IsOwnedBy(entry %d, user %d) = %v, %v; want %v", tc.entryID, tc.userID, got, err, tc.want)
			}
		}
	})
}

func TestIsOwnedByReportsFalseWithTheError(t *testing.T) {
	t.Parallel()
	got, err := entry.IsOwnedBy(context.Background(), closedDatabase(t), 1, 1)
	if err == nil || got {
		t.Errorf("IsOwnedBy on a closed database = %v, %v; want false and an error", got, err)
	}
}

func TestMarkingAnEntryReturnsTheUpdatedEntry(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))
		ctx := context.Background()

		read, err := entry.MarkAsRead(ctx, database, id)
		if err != nil || read == nil || read.ID != id || read.ReadAt == nil {
			t.Fatalf("MarkAsRead = %+v, %v", read, err)
		}
		starred, err := entry.StarEntry(ctx, database, id)
		if err != nil || starred == nil || starred.StarredAt == nil {
			t.Fatalf("StarEntry = %+v, %v", starred, err)
		}
		unstarred, err := entry.UnstarEntry(ctx, database, id)
		if err != nil || unstarred == nil || unstarred.StarredAt != nil {
			t.Fatalf("UnstarEntry = %+v, %v", unstarred, err)
		}
	})
}

func TestMarkingAMissingEntryIsNotFound(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		newFixture(t, database)
		ctx := context.Background()
		if _, err := entry.MarkAsRead(ctx, database, 999); !errors.Is(err, entry.ErrNotFound) {
			t.Errorf("MarkAsRead = %v, want ErrNotFound", err)
		}
		if _, err := entry.StarEntry(ctx, database, 999); !errors.Is(err, entry.ErrNotFound) {
			t.Errorf("StarEntry = %v, want ErrNotFound", err)
		}
		if _, err := entry.UnstarEntry(ctx, database, 999); !errors.Is(err, entry.ErrNotFound) {
			t.Errorf("UnstarEntry = %v, want ErrNotFound", err)
		}
	})
}

func TestSettingStateForTheOwnerReportsWhetherAnythingChanged(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))
		ctx := context.Background()

		first, err := entry.SetReadForUser(ctx, database, f.userID, id, true)
		if err != nil || first == nil || !first.Changed || first.Entry.ReadAt == nil {
			t.Fatalf("first read = %+v, %v", first, err)
		}
		again, err := entry.SetReadForUser(ctx, database, f.userID, id, true)
		if err != nil || again == nil || again.Changed || again.Entry.ReadAt == nil {
			t.Fatalf("repeated read = %+v, %v", again, err)
		}
		unread, err := entry.SetReadForUser(ctx, database, f.userID, id, false)
		if err != nil || unread == nil || !unread.Changed || unread.Entry.ReadAt != nil {
			t.Fatalf("unread = %+v, %v", unread, err)
		}
		starred, err := entry.SetStarredForUser(ctx, database, f.userID, id, true)
		if err != nil || starred == nil || !starred.Changed || starred.Entry.StarredAt == nil {
			t.Fatalf("star = %+v, %v", starred, err)
		}
		unstarred, err := entry.SetStarredForUser(ctx, database, f.userID, id, false)
		if err != nil || unstarred == nil || !unstarred.Changed || unstarred.Entry.StarredAt != nil {
			t.Fatalf("unstar = %+v, %v", unstarred, err)
		}
	})
}

func TestSettingStateOfSomeoneElsesOrMissingEntryYieldsNothing(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))
		ctx := context.Background()
		for _, tc := range []struct{ userID, entryID int64 }{{f.userID + 1, id}, {f.userID, id + 100}} {
			got, err := entry.SetStarredForUser(ctx, database, tc.userID, tc.entryID, true)
			if err != nil || got != nil {
				t.Errorf("user %d entry %d = %+v, %v; want nil, nil", tc.userID, tc.entryID, got, err)
			}
		}
	})
}

func TestUpsertAndLoadReturnsTheStoredEntry(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		upsert := &entry.Upsert{FeedID: f.feedID, GUID: "g1", Title: ptr("one")}

		stored, isNew, err := entry.UpsertAndLoad(ctx, database, upsert)
		if err != nil || stored == nil || !isNew || stored.GUID != "g1" {
			t.Fatalf("first = %+v, %v, %v", stored, isNew, err)
		}
		stored, isNew, err = entry.UpsertAndLoad(ctx, database, upsert)
		if err != nil || stored == nil || isNew {
			t.Fatalf("second = %+v, %v, %v", stored, isNew, err)
		}
	})
}

func TestOfflineSetIgnoresTheBudgetItDoesNotHave(t *testing.T) {
	t.Parallel()
	// A zero budget must not reach the database, which is closed here.
	set, err := entry.ListOfflineSet(context.Background(), closedDatabase(t), 1, 0)
	if err != nil || set != nil {
		t.Errorf("keep 0 = %v, %v; want nil, nil", set, err)
	}
}

func TestOfflineSetStarredFillsOnlyWhatUnreadLeavesFree(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		unread := f.add(t, "unread", "Unread", day(2))
		starredRead := f.add(t, "starred", "Starred", day(1))
		_, err := entry.StarEntry(ctx, database, starredRead)
		must(t, err)
		_, err = entry.MarkAsRead(ctx, database, starredRead)
		must(t, err)

		full, err := entry.ListOfflineSet(ctx, database, f.userID, 1)
		must(t, err)
		if len(full) != 1 || full[0].ID != unread {
			t.Errorf("a budget the unread entry fills = %v", idsOf(full))
		}
		roomy, err := entry.ListOfflineSet(ctx, database, f.userID, 2)
		must(t, err)
		if len(roomy) != 2 || roomy[0].ID != unread || roomy[1].ID != starredRead {
			t.Errorf("a budget with a free slot = %v", idsOf(roomy))
		}
	})
}

func TestOfflineSetIsNewestFirstByPublishedOrStoredTime(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		older := f.add(t, "older", "Older", day(2))
		newer := f.add(t, "newer", "Newer", day(3))
		// No published time: it sorts by when it was stored, which is today.
		undated, err := entry.UpsertEntry(ctx, database, &entry.Upsert{FeedID: f.feedID, GUID: "undated", Title: ptr("Undated")})
		must(t, err)

		set, err := entry.ListOfflineSet(ctx, database, f.userID, 10)
		must(t, err)
		got := idsOf(set)
		want := []int64{undated.ID, newer, older}
		if len(got) != len(want) || got[0] != want[0] || got[1] != want[1] || got[2] != want[2] {
			t.Errorf("order = %v, want %v", got, want)
		}
	})
}

func idsOf(rows []entry.WithFeed) []int64 {
	ids := make([]int64, len(rows))
	for i := range rows {
		ids[i] = rows[i].ID
	}
	return ids
}

func TestCountingADaysUnreadWithASnapshotStillWorks(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		f.add(t, "a", "A", day(1))
		readThisView := f.add(t, "b", "B", day(1))
		_, err := entry.MarkAsRead(ctx, database, readThisView)
		must(t, err)

		snapshot := time.Now().UTC().Add(-time.Hour).Format("2006-01-02 15:04:05")
		filter := &entry.Filter{
			UnreadOnly: true, ReadAfter: &snapshot,
			PublishedBetween: &entry.TimeRange{Start: day(1).Add(-12 * time.Hour), End: day(1).Add(12 * time.Hour)},
		}
		got, err := entry.CountByUser(ctx, database, f.userID, filter)
		if err != nil || got != 2 {
			t.Errorf("count = %d, %v; want both entries (the read one is inside the snapshot)", got, err)
		}
	})
}

func TestEntryKnowsWhetherItsFeedHasAnIcon(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ctx := context.Background()
		id := f.add(t, "a", "A", day(1))

		before, err := entry.FindByIDWithFeed(ctx, database, id)
		if err != nil || before == nil || before.FeedHasIcon {
			t.Fatalf("without an icon = %+v, %v", before, err)
		}
		must(t, models.UpsertImage(ctx, database, models.ImageUpsert{
			EntityType: models.EntityFeed, EntityID: f.feedID, Data: []byte{1}, ContentType: "image/png",
		}))
		after, err := entry.FindByIDWithFeed(ctx, database, id)
		if err != nil || after == nil || !after.FeedHasIcon {
			t.Fatalf("with an icon = %+v, %v", after, err)
		}
	})
}
