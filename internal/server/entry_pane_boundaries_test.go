package server

import (
	"context"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func TestSummaryOfByStatus(t *testing.T) {
	t.Parallel()
	text, failure := "the summary", "the failure"
	cases := []struct {
		status       models.SummaryStatus
		inFlight     bool
		wantText     bool
		wantFailure  bool
		wantReported models.SummaryStatus
	}{
		{models.SummaryPending, true, false, false, models.SummaryPending},
		{models.SummaryProcessing, true, false, false, models.SummaryProcessing},
		{models.SummaryCompleted, false, true, false, models.SummaryCompleted},
		{models.SummaryFailed, false, false, true, models.SummaryFailed},
	}
	for _, tc := range cases {
		got := summaryOf(tc.status, &text, &failure)
		if got.inFlight != tc.inFlight || (got.text != nil) != tc.wantText ||
			(got.failure != nil) != tc.wantFailure || got.status != tc.wantReported {
			t.Errorf("%s: %+v", tc.status, got)
		}
	}
	if got := summaryOf(models.SummaryCompleted, nil, &failure); got.text != nil {
		t.Errorf("a completed summary without text = %q", *got.text)
	}
	if got := summaryOf(models.SummaryFailed, &text, nil); got.failure != nil {
		t.Errorf("a failed summary without a message = %q", *got.failure)
	}
}

func TestBuildReadingPaneReportsReadAndStarredState(t *testing.T) {
	app := defaultITestApp(t)
	userID, entryID := app.SeedUserWithEntry("pane", fixturePassword)
	found, err := entry.FindByIDForUser(context.Background(), app.DB, userID, entryID)
	if err != nil || found == nil {
		t.Fatalf("entry = %v, %v", found, err)
	}
	stamp := time.Date(2026, 1, 2, 3, 4, 5, 0, time.UTC)
	cases := []struct {
		name          string
		readAt        *time.Time
		starredAt     *time.Time
		read, starred bool
	}{
		{"neither", nil, nil, false, false},
		{"read", &stamp, nil, true, false},
		{"starred", nil, &stamp, false, true},
		{"both", &stamp, &stamp, true, true},
	}
	for _, tc := range cases {
		copied := *found
		copied.ReadAt, copied.StarredAt = tc.readAt, tc.starredAt
		built, err := app.App.buildReadingPane(context.Background(), paneRequest{
			userID: userID, entry: &copied, purpose: readerRender,
		})
		if err != nil {
			t.Fatalf("%s: %v", tc.name, err)
		}
		if built.pane.IsRead != tc.read || built.pane.IsStarred != tc.starred {
			t.Errorf("%s: IsRead=%v IsStarred=%v", tc.name, built.pane.IsRead, built.pane.IsStarred)
		}
	}
}
