package models

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

type summaryFixture struct {
	database *db.DB
	userID   int64
	feedID   int64
}

func newSummaryFixture(t *testing.T, database *db.DB, username string) summaryFixture {
	t.Helper()
	user := mustUser(t, database, username)
	category := mustCategory(t, database, user.ID, "Tech")
	feed := mustFeed(t, database, category.ID, "https://example.com/"+username+".xml")
	return summaryFixture{database: database, userID: user.ID, feedID: feed.ID}
}

// addEntry inserts an entry with a link and returns its id.
func (f summaryFixture) addEntry(t *testing.T, guid string) int64 {
	t.Helper()
	mustExec(t, f.database, `INSERT INTO entry (feed_id, guid, title, link) VALUES ($1, $2, 'T', 'https://example.com/e')`, f.feedID, guid)
	var id int64
	err := f.database.QueryRowContext(context.Background(),
		`SELECT id FROM entry WHERE feed_id = $1 AND guid = $2`, f.feedID, guid).Scan(&id)
	if err != nil {
		t.Fatal(err)
	}
	return id
}

func (f summaryFixture) pending(t *testing.T, entryID int64) *EntrySummary {
	t.Helper()
	summary, err := UpsertPendingSummary(context.Background(), f.database, f.userID, entryID)
	if err != nil {
		t.Fatal(err)
	}
	return summary
}

func (f summaryFixture) find(t *testing.T, entryID int64) *EntrySummary {
	t.Helper()
	summary, err := FindEntrySummary(context.Background(), f.database, f.userID, entryID)
	if err != nil || summary == nil {
		t.Fatalf("find = %v, %v", summary, err)
	}
	return summary
}

func TestUpsertPendingSummaryStartsEmpty(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newSummaryFixture(t, database, "alice")
		summary := f.pending(t, f.addEntry(t, "a"))
		if summary.Status != SummaryPending || summary.SummaryText != nil || summary.ErrorMessage != nil {
			t.Errorf("summary = %+v", summary)
		}
	})
}

func TestSummaryStatusTransitions(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		f.pending(t, entryID)

		if err := SetSummaryProcessing(ctx, database, f.userID, entryID); err != nil {
			t.Fatal(err)
		}
		if got := f.find(t, entryID); got.Status != SummaryProcessing {
			t.Errorf("status = %s", got.Status)
		}
		if _, err := SetSummaryCompleted(ctx, database, f.userID, entryID, "This is the summary"); err != nil {
			t.Fatal(err)
		}
		got := f.find(t, entryID)
		if got.Status != SummaryCompleted || got.SummaryText == nil || *got.SummaryText != "This is the summary" {
			t.Errorf("got %+v", got)
		}
	})
}

func TestSetSummaryFailedStoresTheMessage(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		f.pending(t, entryID)
		if _, err := SetSummaryFailed(context.Background(), database, f.userID, entryID, "API error"); err != nil {
			t.Fatal(err)
		}
		got := f.find(t, entryID)
		if got.Status != SummaryFailed || got.ErrorMessage == nil || *got.ErrorMessage != "API error" {
			t.Errorf("got %+v", got)
		}
	})
}

func TestStatusChangesOnAMissingRowReportNotFound(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		if err := SetSummaryProcessing(ctx, database, f.userID, entryID); !errors.Is(err, ErrSummaryNotFound) {
			t.Errorf("processing err = %v", err)
		}
		if _, err := SetSummaryCompleted(ctx, database, f.userID, entryID, "x"); !errors.Is(err, ErrSummaryNotFound) {
			t.Errorf("completed err = %v", err)
		}
		if _, err := SetSummaryFailed(ctx, database, f.userID, entryID, "x"); !errors.Is(err, ErrSummaryNotFound) {
			t.Errorf("failed err = %v", err)
		}
		if found, err := FindEntrySummary(ctx, database, f.userID, entryID); err != nil || found != nil {
			t.Errorf("find = %v, %v", found, err)
		}
	})
}

func TestDeleteEntrySummaryReportsWhetherARowWentAway(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		f.pending(t, entryID)
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, entryID); !ok {
			t.Fatal("row must exist")
		}
		if deleted, err := DeleteEntrySummary(ctx, database, f.userID, entryID); err != nil || !deleted {
			t.Errorf("deleted = %v, %v", deleted, err)
		}
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, entryID); ok {
			t.Error("row survived")
		}
		if deleted, _ := DeleteEntrySummary(ctx, database, f.userID, entryID); deleted {
			t.Error("deleting nothing must report false")
		}
	})
}

func TestSummaryStatusesForListsOnlyEntriesWithASummary(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		ids := []int64{f.addEntry(t, "0"), f.addEntry(t, "1"), f.addEntry(t, "2")}
		f.pending(t, ids[0])
		f.pending(t, ids[1])
		if _, err := SetSummaryCompleted(ctx, database, f.userID, ids[1], "Summary"); err != nil {
			t.Fatal(err)
		}
		got, err := SummaryStatusesFor(ctx, database, f.userID, ids)
		if err != nil {
			t.Fatal(err)
		}
		if len(got) != 2 || got[ids[0]] != SummaryPending || got[ids[1]] != SummaryCompleted {
			t.Errorf("got %v", got)
		}
		if empty, err := SummaryStatusesFor(ctx, database, f.userID, nil); err != nil || len(empty) != 0 {
			t.Errorf("empty = %v, %v", empty, err)
		}
	})
}

func TestFindIncompleteSummariesSkipsFinishedAndLinkless(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		pending, done, processing := f.addEntry(t, "p"), f.addEntry(t, "d"), f.addEntry(t, "r")
		mustExec(t, database, `INSERT INTO entry (feed_id, guid) VALUES ($1, 'nolink')`, f.feedID)
		var noLink int64
		if err := database.QueryRowContext(ctx, `SELECT id FROM entry WHERE guid = 'nolink'`).Scan(&noLink); err != nil {
			t.Fatal(err)
		}
		for _, id := range []int64{pending, done, processing, noLink} {
			f.pending(t, id)
		}
		if _, err := SetSummaryCompleted(ctx, database, f.userID, done, "s"); err != nil {
			t.Fatal(err)
		}
		if err := SetSummaryProcessing(ctx, database, f.userID, processing); err != nil {
			t.Fatal(err)
		}
		got, err := FindIncompleteSummaries(ctx, database)
		if err != nil {
			t.Fatal(err)
		}
		if len(got) != 2 {
			t.Fatalf("got %+v", got)
		}
		for _, row := range got {
			if row.UserID != f.userID || row.Link != "https://example.com/e" || (row.EntryID != pending && row.EntryID != processing) {
				t.Errorf("row = %+v", row)
			}
		}
	})
}

func TestUpsertPendingSummaryResetsACompletedRow(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newSummaryFixture(t, database, "alice")
		entryID := f.addEntry(t, "a")
		f.pending(t, entryID)
		if _, err := SetSummaryCompleted(context.Background(), database, f.userID, entryID, "Summary"); err != nil {
			t.Fatal(err)
		}
		if got := f.pending(t, entryID); got.Status != SummaryPending || got.SummaryText != nil {
			t.Errorf("got %+v", got)
		}
	})
}

func TestParseSummaryStatus(t *testing.T) {
	for _, s := range []SummaryStatus{SummaryPending, SummaryProcessing, SummaryCompleted, SummaryFailed} {
		if got, ok := ParseSummaryStatus(string(s)); !ok || got != s {
			t.Errorf("%s -> %s, %v", s, got, ok)
		}
	}
	if _, ok := ParseSummaryStatus("invalid"); ok {
		t.Error("invalid must not parse")
	}
}

func TestCountCompletedSummariesCountsOnlyCompletedForTheUser(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		u1 := newSummaryFixture(t, database, "u1")
		u2 := newSummaryFixture(t, database, "u2")
		e1, e2, e3, e4 := u1.addEntry(t, "g1"), u1.addEntry(t, "g2"), u1.addEntry(t, "g3"), u1.addEntry(t, "g4")
		e5 := u2.addEntry(t, "g5")
		for _, id := range []int64{e1, e2, e3, e4} {
			u1.pending(t, id)
		}
		u2.pending(t, e5)
		for _, id := range []int64{e1, e2} {
			if _, err := SetSummaryCompleted(ctx, database, u1.userID, id, "s"); err != nil {
				t.Fatal(err)
			}
		}
		if _, err := SetSummaryFailed(ctx, database, u1.userID, e4, "err"); err != nil {
			t.Fatal(err)
		}
		if _, err := SetSummaryCompleted(ctx, database, u2.userID, e5, "s"); err != nil {
			t.Fatal(err)
		}
		for userID, want := range map[int64]int64{u1.userID: 2, u2.userID: 1, 99999: 0} {
			if got, err := CountCompletedSummaries(ctx, database, userID); err != nil || got != want {
				t.Errorf("user %d: %d, %v, want %d", userID, got, err, want)
			}
		}
	})
}

func TestDeleteExpiredSummariesUsesTheCreationTime(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newSummaryFixture(t, database, "alice")
		old, fresh := f.addEntry(t, "old"), f.addEntry(t, "fresh")
		f.pending(t, old)
		f.pending(t, fresh)
		backdated := db.FormatSpace(time.Now().UTC().Add(-48 * time.Hour))
		mustExec(t, database, `UPDATE entry_summary SET created_at = $1 WHERE entry_id = $2`, backdated, old)

		removed, err := DeleteExpiredSummaries(ctx, database, 24)
		if err != nil || removed != 1 {
			t.Fatalf("removed = %d, %v", removed, err)
		}
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, old); ok {
			t.Error("expired row survived")
		}
		if ok, _ := EntrySummaryExists(ctx, database, f.userID, fresh); !ok {
			t.Error("a row created seconds ago is not older than 24h")
		}
	})
}
