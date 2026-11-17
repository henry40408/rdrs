package server

import (
	"context"
	"fmt"
	"net/http"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/models"
)

// Port of tests/summary_cancel.rs: POST /entries/{id}/summarize/cancel.
// test_cancel_removes_inflight_token lives in internal/summarize
// (TestCancelStopsARunningJobWithoutWriting, TestCancelsUnregisterWhenTheJobEnds):
// the registry's register step is unexported, so only the summarize package can
// seed a running job.

func TestCancelClearsFailedSummary(t *testing.T) {
	app := defaultITestApp(t)
	userID, entryID := app.SeedUserWithEntry("user1", fixturePassword)
	app.Login("user1")
	ctx := context.Background()
	if _, err := models.UpsertPendingSummary(ctx, app.DB, userID, entryID); err != nil {
		t.Fatal(err)
	}
	if _, err := models.SetSummaryFailed(ctx, app.DB, userID, entryID, "API error"); err != nil {
		t.Fatal(err)
	}

	response := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/entries/%d/summarize/cancel", entryID)})
	if response.Code != http.StatusOK {
		t.Fatalf("status %d: %s", response.Code, response.Text())
	}

	gone, err := models.FindEntrySummary(ctx, app.DB, userID, entryID)
	if err != nil || gone != nil {
		t.Fatalf("expected the summary record to be deleted, got %v, %v", gone, err)
	}
}

func TestCancelNonOwnerReturns404(t *testing.T) {
	app := defaultITestApp(t)
	_, entryID := app.SeedUserWithEntry("owner", fixturePassword)
	app.SeedAccount("attacker", "password456-vulture", models.RoleUser)
	app.LoginWith("attacker", "password456-vulture")

	response := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/entries/%d/summarize/cancel", entryID)})
	if response.Code != http.StatusNotFound {
		t.Fatalf("status %d, want 404", response.Code)
	}
}

func TestSummarizeEmitsPendingEvent(t *testing.T) {
	app := defaultITestApp(t)
	subscription := app.App.Events.Subscribe()
	defer subscription.Close()
	userID, entryID := app.SeedUserWithEntry("pendinguser", fixturePassword)
	app.Login("pendinguser")

	response := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/entries/%d/summarize", entryID)})
	if response.Code != http.StatusOK {
		t.Fatalf("status %d: %s", response.Code, response.Text())
	}

	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	event, err := subscription.Next(ctx)
	if err != nil {
		t.Fatalf("no event emitted: %v", err)
	}
	if event.UserID != userID || event.Kind != events.Summary || event.Status == nil || *event.Status != models.SummaryPending {
		t.Fatalf("event = %+v", event)
	}
}
