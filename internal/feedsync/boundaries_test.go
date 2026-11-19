package feedsync

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// A sync that timed out has a dead context, yet its error is what the feed
// must store.
func TestRecordErrorSurvivesAnExpiredDeadline(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixtureOn(t, database, serveFeed)
		expired, cancel := context.WithDeadline(context.Background(), time.Now().Add(-time.Second))
		defer cancel()

		if err := f.syncer.recordError(expired, f.feed.ID, "timed out"); err != nil {
			t.Fatal(err)
		}
		feed := f.reload(t)
		if feed.FetchError == nil || *feed.FetchError != "timed out" {
			t.Errorf("FetchError = %v", feed.FetchError)
		}
	})
}

func TestRecordErrorLogsAWriteThatFailed(t *testing.T) {
	logs := &bytes.Buffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })
	f := newFixtureOn(t, openDB(t), serveFeed)

	if err := f.syncer.recordError(context.Background(), f.feed.ID, "boom"); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(logs.String(), "feed.record_error_failed") {
		t.Fatalf("a successful write was logged: %s", logs.String())
	}
	if _, err := f.syncer.DB.ExecContext(context.Background(),
		`CREATE TRIGGER refuse_feed_update BEFORE UPDATE ON feed BEGIN SELECT RAISE(ABORT, 'refused'); END`); err != nil {
		t.Fatal(err)
	}
	if err := f.syncer.recordError(context.Background(), f.feed.ID, "boom"); err == nil {
		t.Fatal("want the refused write's error")
	}
	if !strings.Contains(logs.String(), "event=feed.record_error_failed") {
		t.Errorf("log lacks the failed write: %s", logs.String())
	}
}

// A caller that left must not leave a fetch error on a healthy feed.
func TestRecordErrorSkipsACanceledCaller(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixtureOn(t, database, serveFeed)
		canceled, cancel := context.WithCancel(context.Background())
		cancel()

		if err := f.syncer.recordError(canceled, f.feed.ID, "context canceled"); !errors.Is(err, context.Canceled) {
			t.Fatalf("err = %v", err)
		}
		if feed := f.reload(t); feed.FetchError != nil {
			t.Errorf("FetchError = %q, want none", *feed.FetchError)
		}
	})
}

func TestCountsRecordEachOutcomeOnce(t *testing.T) {
	var c Counts
	c.record(entry.Inserted)
	c.record(entry.Updated)
	c.record(entry.Unchanged)
	c.record(entry.SkippedTombstoned)
	if c != (Counts{New: 1, Updated: 1, Unchanged: 1, Skipped: 1}) {
		t.Errorf("counts = %+v", c)
	}
}

func TestVisibleASCIIBounds(t *testing.T) {
	tests := []struct {
		in   string
		want bool
	}{
		{"a\tb", true}, {"~", true}, {"a b", true}, {"a\x1fb", false}, {"a\x7fb", false}, {"café", false}, {"", true},
	}
	for _, tc := range tests {
		if got := visibleASCII(tc.in); got != tc.want {
			t.Errorf("visibleASCII(%q) = %v, want %v", tc.in, got, tc.want)
		}
	}
}

func TestRefreshAcceptsStatus299(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		w.WriteHeader(299)
		_, _ = w.Write([]byte(rssBody))
	})
	if result, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil || result.NewEntries != 2 {
		t.Fatalf("result = %+v, %v", result, err)
	}
}

func TestRefreshAcceptsABodyOfExactlyTheLimit(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		padding := maxFeedBytes - len(rssBody) - len("<!---->")
		_, _ = w.Write([]byte(rssBody + "<!--" + strings.Repeat("x", padding) + "-->"))
	})
	if result, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil || result.NewEntries != 2 {
		t.Fatalf("result = %+v, %v", result, err)
	}
}

func TestSaveIconLogsSuccess(t *testing.T) {
	var logs bytes.Buffer
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(&logs, &slog.HandlerOptions{Level: slog.LevelDebug})))
	t.Cleanup(func() { slog.SetDefault(previous) })
	f := newFixture(t, serveFeed)
	f.syncer.saveIcon(context.Background(), f.feed.ID, &FetchedIcon{Data: []byte("png"), ContentType: "image/png", SourceURL: "https://x.example/i.png"})
	if !strings.Contains(logs.String(), "feed.icon_saved") || strings.Contains(logs.String(), "icon_save_failed") {
		t.Errorf("logs = %s", logs.String())
	}
}
