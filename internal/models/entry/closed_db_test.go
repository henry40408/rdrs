package entry_test

import (
	"context"
	"testing"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/testutil"
)

// closedDatabase returns a handle whose pool is already shut down, so every
// query fails.
func closedDatabase(t *testing.T) *db.DB {
	t.Helper()
	database := testutil.SQLite(t)
	must(t, database.Shutdown(context.Background()))
	return database
}

func TestQueriesReturnTheDatabaseError(t *testing.T) {
	database := closedDatabase(t)
	ctx := context.Background()
	filter := &entry.Filter{}
	ids := []int64{1}
	reportsError := map[string]func() error{
		"CountMissingContentText":  func() error { _, err := entry.CountMissingContentText(ctx, database); return err },
		"BackfillContentTextBatch": func() error { _, err := entry.BackfillContentTextBatch(ctx, database, 10); return err },
		"PruneReadRetentionBatch":  func() error { _, err := entry.PruneReadRetentionBatch(ctx, database, 10); return err },
		"FindByID":                 func() error { _, err := entry.FindByID(ctx, database, 1); return err },
		"FindByGUIDAndFeed":        func() error { _, err := entry.FindByGUIDAndFeed(ctx, database, "g", 1); return err },
		"FindByIDWithFeed":         func() error { _, err := entry.FindByIDWithFeed(ctx, database, 1); return err },
		"FindByIDForUser":          func() error { _, err := entry.FindByIDForUser(ctx, database, 1, 1); return err },
		"IsOwnedBy":                func() error { _, err := entry.IsOwnedBy(ctx, database, 1, 1); return err },
		"FindByIDsWithFeed":        func() error { _, err := entry.FindByIDsWithFeed(ctx, database, 1, ids); return err },
		"FetchSortTS":              func() error { _, err := entry.FetchSortTS(ctx, database, 1, entry.SortReadAt); return err },
		"ListByUser": func() error {
			_, err := entry.ListByUser(ctx, database, entry.ListQuery{UserID: 1, Filter: filter, Limit: 5})
			return err
		},
		"ListOfflineSet":              func() error { _, err := entry.ListOfflineSet(ctx, database, 1, 5); return err },
		"CountByUser":                 func() error { _, err := entry.CountByUser(ctx, database, 1, filter); return err },
		"CountUnreadByUser":           func() error { _, err := entry.CountUnreadByUser(ctx, database, 1); return err },
		"CountUnreadByFeed":           func() error { _, err := entry.CountUnreadByFeed(ctx, database, 1); return err },
		"CountUnreadByFeedInCategory": func() error { _, err := entry.CountUnreadByFeedInCategory(ctx, database, 1, 1); return err },
		"CountUnreadByCategory":       func() error { _, err := entry.CountUnreadByCategory(ctx, database, 1); return err },
		"ListIDsByUser": func() error {
			_, err := entry.ListIDsByUser(ctx, database, 1, filter, &entry.ContinuationParams{Limit: 5})
			return err
		},
		"ListByUserWithContinuation": func() error {
			_, err := entry.ListByUserWithContinuation(ctx, database, 1, filter, &entry.ContinuationParams{Limit: 5})
			return err
		},
		"ListByUserWithContinuationTS": func() error {
			_, err := entry.ListByUserWithContinuationTS(ctx, database, 1, filter, &entry.ContinuationParams{Limit: 5})
			return err
		},
		"MarkAsReadQuietly":     func() error { return entry.MarkAsReadQuietly(ctx, database, 1) },
		"MarkAsRead":            func() error { _, err := entry.MarkAsRead(ctx, database, 1); return err },
		"StarEntry":             func() error { _, err := entry.StarEntry(ctx, database, 1); return err },
		"UnstarEntry":           func() error { _, err := entry.UnstarEntry(ctx, database, 1); return err },
		"SetStarredForUser":     func() error { _, err := entry.SetStarredForUser(ctx, database, 1, 1, true); return err },
		"SetReadForUser":        func() error { _, err := entry.SetReadForUser(ctx, database, 1, 1, true); return err },
		"SetFullContentForUser": func() error { return entry.SetFullContentForUser(ctx, database, 1, 1, "<p>x</p>") },
		"MarkAllReadByFeed":     func() error { _, err := entry.MarkAllReadByFeed(ctx, database, 1, entry.MarkAllRead{}); return err },
		"MarkAllReadByUser":     func() error { _, err := entry.MarkAllReadByUser(ctx, database, 1, entry.MarkAllRead{}); return err },
		"MarkAllReadByCategory": func() error { _, err := entry.MarkAllReadByCategory(ctx, database, 1, entry.MarkAllRead{}); return err },
		"MarkReadByFilter":      func() error { _, err := entry.MarkReadByFilter(ctx, database, 1, filter); return err },
		"MarkReadByIDs":         func() error { _, err := entry.MarkReadByIDs(ctx, database, 1, ids); return err },
		"MarkUnreadByIDs":       func() error { _, err := entry.MarkUnreadByIDs(ctx, database, 1, ids); return err },
		"StarByIDs":             func() error { _, err := entry.StarByIDs(ctx, database, 1, ids); return err },
		"UnstarByIDs":           func() error { _, err := entry.UnstarByIDs(ctx, database, 1, ids); return err },
		"FindNeighbors":         func() error { _, err := entry.FindNeighbors(ctx, database, 1, 1, filter); return err },
		"FindPosition":          func() error { _, err := entry.FindPosition(ctx, database, 1, 1, filter); return err },
		"UpsertEntry": func() error {
			_, err := entry.UpsertEntry(ctx, database, &entry.Upsert{FeedID: 1, GUID: "g"})
			return err
		},
		"UpsertAndLoad": func() error {
			_, _, err := entry.UpsertAndLoad(ctx, database, &entry.Upsert{FeedID: 1, GUID: "g"})
			return err
		},
		"InsertTombstone": func() error { return entry.InsertTombstone(ctx, database, 1, "g") },
	}
	for name, call := range reportsError {
		t.Run(name, func(t *testing.T) {
			if err := call(); err == nil {
				t.Error("want an error from a closed database, got nil")
			}
		})
	}
}
