package worker

import (
	"bytes"
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

type lockedBuffer struct {
	mu     sync.Mutex
	buffer bytes.Buffer
}

func (b *lockedBuffer) Write(p []byte) (int, error) {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.Write(p)
}

func (b *lockedBuffer) String() string {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.buffer.String()
}

func captureDebugLogs(t *testing.T) *lockedBuffer {
	t.Helper()
	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelDebug})))
	t.Cleanup(func() { slog.SetDefault(previous) })
	return logs
}

func TestSyncBucketLogsSucceededAndFailedCounts(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(rssBody))
	}))
	t.Cleanup(server.Close)
	f := newFixture(t, server.URL)
	_, err := models.CreateFeed(context.Background(), f.database, models.CreateFeedParams{
		CategoryID: f.feed.CategoryID, URL: "http://127.0.0.1:1/dead",
	})
	must(t, err)
	s := newSync(t, f)
	now := time.Now()
	f.exec(t, "UPDATE feed SET bucket = $1", bucketAt(now))
	logs := captureDebugLogs(t)

	s.runBucket(context.Background(), now)

	if !strings.Contains(logs.String(), "succeeded=1 failed=1") {
		t.Errorf("logs = %s", logs)
	}
}

func countTicks(logs *lockedBuffer) int {
	return strings.Count(logs.String(), "event=sync.tick")
}

func TestSyncRunHonoursAShortIntervalAndDefaultsAZeroOne(t *testing.T) {
	for name, test := range map[string]struct {
		interval time.Duration
		check    func(ticks int) bool
	}{
		"short interval ticks repeatedly": {10 * time.Millisecond, func(ticks int) bool { return ticks >= 3 }},
		"zero interval uses the default":  {0, func(ticks int) bool { return ticks == 1 }},
	} {
		t.Run(name, func(t *testing.T) {
			f := newFixture(t, "http://127.0.0.1:1/x")
			s := newSync(t, f)
			s.Interval = test.interval
			logs := captureDebugLogs(t)
			ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
			defer cancel()
			s.Run(ctx)
			if ticks := countTicks(logs); !test.check(ticks) {
				t.Errorf("ticks = %d", ticks)
			}
		})
	}
}

func TestCleanupSweepLogsOnlySweepsThatDeletedRows(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	logs := captureDebugLogs(t)
	(&Cleanup{DB: f.database}).Sweep(ctx)
	if strings.Contains(logs.String(), "event=cleanup.swept") || strings.Contains(logs.String(), "cleanup.sweep_failed") {
		t.Fatalf("an empty sweep logged: %s", logs)
	}

	must(t, models.UpsertImage(ctx, f.database, models.ImageUpsert{
		EntityType: models.EntityFeed, EntityID: 9999, Data: []byte("x"), ContentType: "image/png",
	}))
	(&Cleanup{DB: f.database}).Sweep(ctx)
	if !strings.Contains(logs.String(), "event=cleanup.swept kind=feed_image count=1") {
		t.Errorf("logs = %s", logs)
	}
}

func TestCleanupSweepReportsEachFailure(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	must(t, f.database.Shutdown(context.Background()))
	logs := captureDebugLogs(t)

	(&Cleanup{DB: f.database}).Sweep(context.Background())

	if got := strings.Count(logs.String(), "event=cleanup.sweep_failed"); got != 5 {
		t.Errorf("failed sweeps = %d, logs = %s", got, logs)
	}
	if strings.Contains(logs.String(), "event=cleanup.swept") {
		t.Errorf("a failed sweep reported a count: %s", logs)
	}
}

func insertOrphanImages(t *testing.T, database *db.DB, count int) {
	t.Helper()
	for id := range count {
		must(t, models.UpsertImage(context.Background(), database, models.ImageUpsert{
			EntityType: models.EntityFeed, EntityID: int64(10000 + id), Data: []byte("x"), ContentType: "image/png",
		}))
	}
}

func TestOrphanImageDeletionContinuesPastAFullBatch(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	insertOrphanImages(t, f.database, orphanImageBatchSize+1)

	total, err := (&Cleanup{DB: f.database}).deleteOrphanedFeedImages(context.Background())

	if err != nil || total != orphanImageBatchSize+1 {
		t.Errorf("total = %d, %v", total, err)
	}
}

func TestRetentionDrainContinuesPastAFullBatch(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	f.exec(t, "INSERT INTO user_settings (user_id, retention_read_days) VALUES ($1, 30)", f.user.ID)
	readAt := db.FormatSpace(time.Now().UTC().AddDate(0, 0, -40))
	for i := range retentionBatchSize + 1 {
		guid := "old-" + strings.Repeat("x", i%7) + string(rune('a'+i%26)) + time.Duration(i).String()
		_, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: guid})
		must(t, err)
	}
	f.exec(t, "UPDATE entry SET read_at = $1", readAt)

	total := (&Retention{DB: f.database}).drain(ctx, ctx)

	if total != retentionBatchSize+1 {
		t.Errorf("drained %d, want %d", total, retentionBatchSize+1)
	}
}

func TestBackfillContinuesPastAFullBatch(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	body := "<p>x</p>"
	for i := range backfillBatchSize + 1 {
		guid := "g-" + time.Duration(i).String()
		_, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: guid, Content: &body})
		must(t, err)
	}
	f.exec(t, "UPDATE entry SET content_text = NULL")

	BackfillContentText(ctx, f.database)

	if missing, err := entry.CountMissingContentText(ctx, f.database); err != nil || missing != 0 {
		t.Errorf("missing = %d, %v", missing, err)
	}
}
