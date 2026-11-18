package server

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func TestRefreshAfterBulkReadActsOnlyWhenSomethingWasMarked(t *testing.T) {
	app := defaultITestApp(t)
	const userID, sentinelUser = 41, 42
	subscription := app.App.Events.Subscribe()
	defer subscription.Close()
	ctx := context.Background()

	cases := []struct {
		affected   int64
		wantBusted bool
	}{{0, false}, {1, true}, {5, true}}
	for _, tc := range cases {
		app.App.Sidebar.InsertIfCurrent(userID, app.App.Sidebar.BeginRead(userID), cache.Chrome{TotalUnread: 9})
		if app.App.Sidebar.Get(userID) == nil {
			t.Fatal("the cache refused the seed")
		}
		app.App.refreshAfterBulkRead(userID, tc.affected)
		if busted := app.App.Sidebar.Get(userID) == nil; busted != tc.wantBusted {
			t.Errorf("affected %d: busted = %v", tc.affected, busted)
		}
		// Emit is synchronous, so the sentinel arrives after any event the call sent.
		app.App.Events.EmitSidebar(sentinelUser)
		event, err := subscription.Next(ctx)
		if err != nil {
			t.Fatal(err)
		}
		gotEvent := event.UserID == userID
		if gotEvent != tc.wantBusted {
			t.Errorf("affected %d: event for the user = %v", tc.affected, gotEvent)
		}
		if gotEvent {
			if event, err = subscription.Next(ctx); err != nil || event.UserID != sentinelUser || event.Kind != events.Sidebar {
				t.Fatalf("sentinel = %+v, %v", event, err)
			}
		}
	}
}

// seedPublishedEntries adds entries published at the given instant to the
// user's only feed. The label keeps the GUIDs of separate calls apart.
func seedPublishedEntries(t *testing.T, app *itestApp, userID int64, label string, count int, publishedAt time.Time) {
	t.Helper()
	ctx := context.Background()
	feeds, err := models.ListFeedsByUser(ctx, app.DB, userID)
	if err != nil || len(feeds) != 1 {
		t.Fatalf("feeds = %v, %v", feeds, err)
	}
	for i := 0; i < count; i++ {
		title := "Needle entry"
		if _, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feeds[0].ID, GUID: fmt.Sprintf("%s-%d", label, i), Title: &title, PublishedAt: &publishedAt,
		}); err != nil {
			t.Fatal(err)
		}
	}
}

// snapshotAfterNow is a snapshot later than every row the test just wrote.
func snapshotAfterNow() time.Time { return time.Now().UTC().Add(time.Minute).Truncate(time.Second) }

func wantFlashMessage(t *testing.T, response itestResponse, want string) {
	t.Helper()
	if response.Code != http.StatusSeeOther {
		t.Fatalf("status %d, want 303", response.Code)
	}
	if got := flashText(t, response); !strings.Contains(got, `"level":"success"`) || !strings.Contains(got, want) {
		t.Errorf("flash = %s, want success %q", got, want)
	}
}

func TestTodayMarkReadSaysHowManyEntriesWereMarked(t *testing.T) {
	app := defaultITestApp(t)
	userID, _ := app.SeedUserWithEntry("today", fixturePassword)
	app.Login("today")
	snapshot := snapshotAfterNow()
	form := url.Values{"snapshot": {snapshot.Format(entry.SnapshotFormat)}}

	// The seeded entry has no date, so it counts under its creation day.
	wantFlashMessage(t, app.PostForm("/entries/today/mark-read", form), "Marked 1 entry as read.")
	seedPublishedEntries(t, app, userID, "two", 2, snapshot)
	wantFlashMessage(t, app.PostForm("/entries/today/mark-read", form), "Marked 2 entries as read.")
	wantFlashMessage(t, app.PostForm("/entries/today/mark-read", form), "Marked 0 entries as read.")
}

func TestScopedMarkReadSaysHowManyMatchingEntriesWereMarked(t *testing.T) {
	app := defaultITestApp(t)
	userID, _ := app.SeedUserWithEntry("scoped", fixturePassword)
	app.Login("scoped")
	snapshot := snapshotAfterNow()
	seedPublishedEntries(t, app, userID, "match", 3, snapshot)
	form := url.Values{"q": {"Needle"}, "snapshot": {snapshot.Format(entry.SnapshotFormat)}}

	wantFlashMessage(t, app.PostForm("/entries/mark-read", form), "Marked 3 matching entries as read.")
	wantFlashMessage(t, app.PostForm("/entries/mark-read", form), "Marked 0 matching entries as read.")
}
