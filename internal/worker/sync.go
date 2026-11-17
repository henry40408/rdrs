package worker

import (
	"context"
	"log/slog"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/events"
	"github.com/henry40408/rdrs/internal/feedsync"
	"github.com/henry40408/rdrs/internal/models"
)

// SyncInterval is how often one bucket of feeds is refreshed.
const SyncInterval = time.Minute

// bucketCount feeds are spread over this many buckets, one per tick.
const bucketCount = 60

// Sync refreshes one bucket of feeds every minute. Syncer.DB and DB should be
// background handles so the work yields to interactive writes on SQLite.
type Sync struct {
	Syncer  *feedsync.Syncer
	DB      *db.DB
	Sidebar *cache.Sidebar
	Events  *events.Bus
	// Interval defaults to SyncInterval; tests shorten it.
	Interval time.Duration
}

// Run blocks until ctx ends. A bucket in progress is finished first, as the
// Rust worker did, so shutdown waits for it.
func (s *Sync) Run(ctx context.Context) {
	slog.Info("background sync task started", "event", "sync.worker_started")
	interval := s.Interval
	if interval <= 0 {
		interval = SyncInterval
	}
	everyInterval(ctx, interval, func() { s.runBucket(context.WithoutCancel(ctx), time.Now()) })
	slog.Info("background sync stopping", "event", "sync.worker_stopping")
	slog.Info("background sync stopped", "event", "sync.worker_stopped")
}

// bucketAt is the minute of the hour, which is how feeds are assigned.
func bucketAt(now time.Time) int64 {
	return now.Unix() / 60 % bucketCount
}

func (s *Sync) runBucket(ctx context.Context, now time.Time) {
	bucket := bucketAt(now)
	slog.Debug("running background sync for bucket", "event", "sync.tick", "bucket", bucket)
	results := s.Syncer.RefreshBucket(ctx, bucket)
	if len(results) == 0 {
		return
	}
	failed := 0
	var changed []int64
	for _, r := range results {
		if r.Err != "" {
			failed++
			slog.Error("background sync failed for feed", "event", "feed.sync_failed", "feed_id", r.FeedID, "error", r.Err)
			continue
		}
		if r.Result.NewEntries > 0 {
			changed = append(changed, r.FeedID)
		}
	}
	slog.Info("background sync bucket finished", "event", "sync.bucket_finished",
		"bucket", bucket, "succeeded", len(results)-failed, "failed", failed)
	s.notifyOwners(ctx, changed)
}

// notifyOwners busts each owner's sidebar cache and nudges their open tabs.
func (s *Sync) notifyOwners(ctx context.Context, feedIDs []int64) {
	if len(feedIDs) == 0 {
		return
	}
	userIDs, err := models.OwnerUserIDsForFeeds(ctx, s.DB, feedIDs)
	if err != nil {
		slog.Error("sidebar owner lookup failed", "event", "sync.owner_lookup_failed", "error", err)
		return
	}
	for _, userID := range userIDs {
		s.Sidebar.Bust(userID)
		s.Events.EmitSidebar(userID)
	}
}
