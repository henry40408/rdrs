package worker

import (
	"context"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"sync/atomic"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

type fixture struct {
	database *db.DB
	user     *models.User
	feed     *models.Feed
}

func newFixture(t *testing.T, feedURL string) fixture {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	must(t, migrate.Run(ctx, database))
	user, err := models.CreateUser(ctx, database, "u", "hash", models.RoleUser)
	must(t, err)
	category, err := models.CreateCategory(ctx, database, user.ID, "c")
	must(t, err)
	feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: feedURL})
	must(t, err)
	return fixture{database: database, user: user, feed: feed}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func (f fixture) exec(t *testing.T, query string, args ...any) {
	t.Helper()
	_, err := f.database.ExecContext(context.Background(), query, args...)
	must(t, err)
}

func TestEveryIntervalRunsAtOnceThenCatchesUp(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var calls []time.Time
	start := time.Now()
	everyInterval(ctx, 20*time.Millisecond, func() {
		calls = append(calls, time.Now())
		switch len(calls) {
		case 1:
			time.Sleep(70 * time.Millisecond) // overruns three intervals
		case 5:
			cancel()
		}
	})
	if len(calls) != 5 {
		t.Fatalf("calls = %d, want 5", len(calls))
	}
	if calls[0].Sub(start) > 15*time.Millisecond {
		t.Errorf("first tick was not immediate: %v", calls[0].Sub(start))
	}
	// The ticks missed during the first call run back to back, not 20ms apart.
	if gap := calls[3].Sub(calls[1]); gap > 15*time.Millisecond {
		t.Errorf("missed ticks were not burst: gap %v", gap)
	}
}

func TestEveryIntervalStopsWhileWaiting(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	var calls atomic.Int32
	done := make(chan struct{})
	go func() {
		everyInterval(ctx, time.Hour, func() { calls.Add(1) })
		close(done)
	}()
	time.Sleep(20 * time.Millisecond)
	cancel()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("did not stop")
	}
	if calls.Load() != 1 {
		t.Errorf("calls = %d, want 1", calls.Load())
	}
}

func TestBucketAt(t *testing.T) {
	for unix, want := range map[int64]int64{0: 0, 59: 0, 60: 1, 3599: 59, 3600: 0, 3660: 1} {
		if got := bucketAt(time.Unix(unix, 0)); got != want {
			t.Errorf("bucketAt(%d) = %d, want %d", unix, got, want)
		}
	}
}

const rssBody = `<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<item><guid>a</guid><title>A</title><description>da</description></item></channel></rss>`

func newSync(t *testing.T, f fixture) *Sync {
	t.Helper()
	policy, err := fetch.ParsePolicy("127.0.0.1")
	must(t, err)
	return &Sync{
		Syncer:  &feedsync.Syncer{DB: f.database, Fetcher: fetch.NewFetcher(policy), DefaultUserAgent: "ua"},
		DB:      f.database,
		Sidebar: cache.NewSidebar(8, time.Hour),
		Events:  events.NewBus(),
	}
}

func TestSyncBucketBustsSidebarAndNudgesOwnerOnNewEntries(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(rssBody))
	}))
	t.Cleanup(server.Close)
	f := newFixture(t, server.URL)
	s := newSync(t, f)
	now := time.Now()
	f.exec(t, "UPDATE feed SET bucket = $1 WHERE id = $2", bucketAt(now), f.feed.ID)

	since := s.Sidebar.BeginRead(f.user.ID)
	sub := s.Events.Subscribe()
	defer sub.Close()

	s.runBucket(context.Background(), now)

	s.Sidebar.InsertIfCurrent(f.user.ID, since, cache.Chrome{})
	if s.Sidebar.Get(f.user.ID) != nil {
		t.Error("sidebar cache was not busted")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	event, err := sub.Next(ctx)
	if err != nil || event.UserID != f.user.ID || event.Kind != events.Sidebar {
		t.Fatalf("event = %+v, %v", event, err)
	}

	// The same entries again change nothing, so nobody is nudged.
	since = s.Sidebar.BeginRead(f.user.ID)
	s.runBucket(context.Background(), now)
	s.Sidebar.InsertIfCurrent(f.user.ID, since, cache.Chrome{})
	if s.Sidebar.Get(f.user.ID) == nil {
		t.Error("sidebar cache was busted without new entries")
	}
}

func TestSyncRunStopsOnCancellationAfterTheFirstTick(t *testing.T) {
	f := newFixture(t, "http://127.0.0.1:1/x")
	s := newSync(t, f)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() {
		s.Run(ctx)
		close(done)
	}()
	time.Sleep(50 * time.Millisecond)
	cancel()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("sync worker did not stop")
	}
}

func TestCleanupSweepRemovesExpiredAndOrphanedRows(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	result, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "g"})
	must(t, err)
	if _, err := models.UpsertPendingSummary(ctx, f.database, f.user.ID, result.ID); err != nil {
		t.Fatal(err)
	}
	f.exec(t, "UPDATE entry_summary SET created_at = '2000-01-01 00:00:00'")
	for _, feedID := range []int64{f.feed.ID, 9999} {
		must(t, models.UpsertImage(ctx, f.database, models.ImageUpsert{
			EntityType: models.EntityFeed, EntityID: feedID, Data: []byte("x"), ContentType: "image/png",
		}))
	}

	(&Cleanup{DB: f.database}).Sweep(ctx)

	if exists, err := models.EntrySummaryExists(ctx, f.database, f.user.ID, result.ID); err != nil || exists {
		t.Errorf("expired summary survived: %v, %v", exists, err)
	}
	if image, err := models.FindImage(ctx, f.database, models.EntityFeed, 9999); err != nil || image != nil {
		t.Errorf("orphaned image survived: %v, %v", image, err)
	}
	if image, err := models.FindImage(ctx, f.database, models.EntityFeed, f.feed.ID); err != nil || image == nil {
		t.Errorf("live image was removed: %v, %v", image, err)
	}
}

func TestRetentionPassPrunesOnlyForOptedInUsers(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	result, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "old"})
	must(t, err)
	f.exec(t, "UPDATE entry SET read_at = $1 WHERE id = $2", db.FormatSpace(time.Now().UTC().AddDate(0, 0, -40)), result.ID)
	r := &Retention{DB: f.database}

	r.pass(ctx, true)
	if found, err := entry.FindByGUIDAndFeed(ctx, f.database, "old", f.feed.ID); err != nil || found == nil {
		t.Fatalf("pruned before opt-in: %v, %v", found, err)
	}
	f.exec(t, "INSERT INTO user_settings (user_id, retention_read_days) VALUES ($1, 30)", f.user.ID)
	r.pass(ctx, false)
	if found, err := entry.FindByGUIDAndFeed(ctx, f.database, "old", f.feed.ID); err != nil || found != nil {
		t.Fatalf("not pruned: %v, %v", found, err)
	}
}

func TestRetentionDrainStopsWhenCancelled(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if total := (&Retention{DB: f.database}).drain(ctx, context.Background()); total != 0 {
		t.Errorf("drained %d after cancellation", total)
	}
}

func TestBackfillContentTextFillsLegacyRows(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx := context.Background()
	body := "<p>Hel</p><p>lo</p>"
	result, err := entry.UpsertEntry(ctx, f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "g", Content: &body})
	must(t, err)
	f.exec(t, "UPDATE entry SET content_text = NULL WHERE id = $1", result.ID)

	BackfillContentText(ctx, f.database)

	if missing, err := entry.CountMissingContentText(ctx, f.database); err != nil || missing != 0 {
		t.Errorf("missing = %d, %v", missing, err)
	}
}

func TestBackfillContentTextStopsWhenCancelled(t *testing.T) {
	f := newFixture(t, "https://x.example/feed")
	ctx, cancel := context.WithCancel(context.Background())
	body := "<p>x</p>"
	result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{FeedID: f.feed.ID, GUID: "g", Content: &body})
	must(t, err)
	f.exec(t, "UPDATE entry SET content_text = NULL WHERE id = $1", result.ID)
	cancel()

	BackfillContentText(ctx, f.database)

	if missing, err := entry.CountMissingContentText(context.Background(), f.database); err != nil || missing != 1 {
		t.Errorf("missing = %d, %v; a cancelled backfill must leave the row for the next start", missing, err)
	}
}
