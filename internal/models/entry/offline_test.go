package entry_test

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func offlineIDs(t *testing.T, f fixture, keep int64) []int64 {
	t.Helper()
	set, err := entry.ListOfflineSet(context.Background(), f.database, f.userID, keep)
	must(t, err)
	ids := make([]int64, len(set))
	for i := range set {
		ids[i] = set[i].ID
	}
	return ids
}

func TestOfflineSetIsEmptyWhenTheBudgetIsZero(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		f.addSeries(t, 3)
		if ids := offlineIDs(t, f, 0); len(ids) != 0 {
			t.Errorf("ids = %v", ids)
		}
	})
}

func TestOfflineSetTakesTheNewestUnreadFirst(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 5)
		assertIDs(t, "offline", offlineIDs(t, f, 2), ids[4], ids[3])
	})
}

func TestOfflineSetFillsTheRemainingBudgetWithStarred(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		ids := f.addSeries(t, 4)
		// The oldest is read but starred, so it fills the leftover budget.
		_, err := entry.MarkAsRead(ctx, database, ids[0])
		must(t, err)
		_, err = entry.StarEntry(ctx, database, ids[0])
		must(t, err)
		assertIDs(t, "offline", offlineIDs(t, f, 4), ids[3], ids[2], ids[1], ids[0])
	})
}

func TestOfflineSetNeverRepeatsAnEntryThatIsBoth(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixture(t, database)
		ids := f.addSeries(t, 2)
		_, err := entry.StarEntry(context.Background(), database, ids[0])
		must(t, err)
		assertIDs(t, "offline", offlineIDs(t, f, 10), ids[1], ids[0])
	})
}

func TestOfflineSetIsScopedToTheReader(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		mine := newFixture(t, database)
		other, err := models.CreateUser(ctx, database, "other", "hash", models.RoleUser)
		must(t, err)
		category, err := models.CreateCategory(ctx, database, other.ID, "Tech")
		must(t, err)
		feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/b"})
		must(t, err)
		theirs := fixture{database: database, userID: other.ID, feedID: feed.ID}
		myIDs := mine.addSeries(t, 2)
		theirs.addSeries(t, 2)
		assertIDs(t, "offline", offlineIDs(t, mine, 50), myIDs[1], myIDs[0])
	})
}

func TestUnreadCountsKeepZeroFeedsAndEmptyCategories(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		emptyCategory, err := models.CreateCategory(ctx, database, f.userID, "Empty")
		must(t, err)
		quiet, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: 1, URL: "https://example.com/quiet"})
		must(t, err)
		ids := f.addSeries(t, 3)
		_, err = entry.MarkAsRead(ctx, database, ids[0])
		must(t, err)

		byFeed, err := entry.CountUnreadByFeed(ctx, database, f.userID)
		must(t, err)
		if byFeed[f.feedID] != 2 || byFeed[quiet.ID] != 0 {
			t.Errorf("by feed = %v", byFeed)
		}
		if _, ok := byFeed[quiet.ID]; !ok {
			t.Error("a feed without entries must keep a zero count")
		}
		inCategory, err := entry.CountUnreadByFeedInCategory(ctx, database, f.userID, 1)
		must(t, err)
		if len(inCategory) != 2 || inCategory[f.feedID] != 2 {
			t.Errorf("in category = %v", inCategory)
		}
		byCategory, err := entry.CountUnreadByCategory(ctx, database, f.userID)
		must(t, err)
		if _, ok := byCategory[emptyCategory.ID]; !ok || byCategory[1] != 2 {
			t.Errorf("by category = %v", byCategory)
		}
		foreign, err := entry.CountUnreadByFeedInCategory(ctx, database, f.userID+999, 1)
		must(t, err)
		if len(foreign) != 0 {
			t.Errorf("another account read %v", foreign)
		}
	})
}
