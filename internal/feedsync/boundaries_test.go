package feedsync

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models/entry"
)

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
