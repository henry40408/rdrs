package worker

import (
	"bytes"
	"context"
	"log/slog"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// captureLogs routes the default logger to a buffer for one test. The worker
// tests do not run in parallel, so swapping the global logger is safe.
func captureLogs(t *testing.T) *bytes.Buffer {
	t.Helper()
	var buffer bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&buffer, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return &buffer
}

func (f fixture) refuse(t *testing.T, table, event string) {
	t.Helper()
	f.exec(t, "CREATE TRIGGER refuse_"+event+" BEFORE "+event+" ON "+table+" BEGIN SELECT RAISE(ABORT, 'refused'); END")
}

func expectLogged(t *testing.T, logs *bytes.Buffer, event string) {
	t.Helper()
	if !strings.Contains(logs.String(), "event="+event) {
		t.Errorf("no %s event in:\n%s", event, logs)
	}
}

func TestBackfillGivesUpOnAFailingBatchAndKeepsTheRow(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	body := "<p>x</p>"
	result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "g", Content: &body})
	must(t, err)
	f.exec(t, "UPDATE entry SET content_text = NULL WHERE id = $1", result.ID)
	f.refuse(t, "entry", "update")
	logs := captureLogs(t)

	BackfillContentText(context.Background(), f.database)

	expectLogged(t, logs, "backfill.batch_failed")
	if missing, err := entry.CountMissingContentText(context.Background(), f.database); err != nil || missing != 1 {
		t.Errorf("missing = %d, %v; the row must wait for the next start", missing, err)
	}
}

func TestBackfillReportsAnUncountableDatabase(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	must(t, f.database.Shutdown(context.Background()))
	logs := captureLogs(t)

	BackfillContentText(context.Background(), f.database)

	expectLogged(t, logs, "backfill.count_failed")
}

func TestCleanupKeepsSweepingWhenOneSweepFails(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	session, err := models.CreateSession(ctx, f.database, models.NewSession{UserID: f.user.ID})
	must(t, err)
	f.exec(t, "UPDATE session SET expires_at = '2000-01-01 00:00:00' WHERE id = $1", session.ID)
	f.exec(t, "ALTER TABLE entry_summary RENAME TO entry_summary_gone")
	logs := captureLogs(t)

	(&Cleanup{DB: f.database}).Sweep(ctx)

	expectLogged(t, logs, "cleanup.sweep_failed")
	if found, err := models.FindSessionByToken(ctx, f.database, session.SessionToken); err != nil || found != nil {
		t.Errorf("a broken summary sweep stopped session expiry: %v, %v", found, err)
	}
}

func TestRetentionStopsDrainingOnAPruneError(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	result, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "old"})
	must(t, err)
	f.exec(t, "UPDATE entry SET read_at = $1 WHERE id = $2", db.FormatSpace(time.Now().UTC().AddDate(0, 0, -40)), result.ID)
	f.exec(t, "INSERT INTO user_settings (user_id, retention_read_days) VALUES ($1, 30)", f.user.ID)
	f.refuse(t, "entry", "delete")
	logs := captureLogs(t)

	if total := (&Retention{DB: f.database}).drain(ctx, ctx); total != 0 {
		t.Errorf("drained %d despite the error", total)
	}

	expectLogged(t, logs, "retention.prune_failed")
	if found, err := entry.FindByGUIDAndFeed(ctx, f.database, "old", f.feed.ID); err != nil || found == nil {
		t.Errorf("entry lost despite the refused delete: %v, %v", found, err)
	}
}

func TestRetentionReportsFailingPlannerRefreshAndMaintenance(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	r := &Retention{DB: f.database}
	must(t, f.database.Shutdown(context.Background()))
	logs := captureLogs(t)

	r.pass(context.Background(), false)
	expectLogged(t, logs, "retention.optimize_failed")

	r.maintain(context.Background())
	expectLogged(t, logs, "retention.maintenance_failed")
}
