package feedsync

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sync"

	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
)

// bucketConcurrency feeds are refreshed at once; the next group starts when
// the whole previous group is done, as in Rust.
const bucketConcurrency = 4

// BucketResult is one feed's outcome; Err is the text shown to the user.
type BucketResult struct {
	FeedID int64
	Result Result
	Err    string
}

// RefreshBucket refreshes every feed of a bucket and returns one result per
// feed that finished or timed out.
func (s *Syncer) RefreshBucket(ctx context.Context, bucket int64) []BucketResult {
	feeds, err := models.ListFeedsByBucket(ctx, s.DB, bucket)
	if err != nil {
		slog.Error("failed to list feeds for bucket", "event", "sync.bucket_list_failed", "bucket", bucket, "error", err)
		return nil
	}
	if len(feeds) == 0 {
		slog.Debug("no feeds in bucket", "event", "sync.bucket_empty", "bucket", bucket)
		return nil
	}
	slog.Info("refreshing feeds in bucket", "event", "sync.bucket_started", "bucket", bucket, "count", len(feeds))
	var results []BucketResult
	for start := 0; start < len(feeds); start += bucketConcurrency {
		group := feeds[start:min(start+bucketConcurrency, len(feeds))]
		results = append(results, s.refreshGroup(ctx, group)...)
	}
	return results
}

func (s *Syncer) refreshGroup(ctx context.Context, group []models.Feed) []BucketResult {
	slots := make([]*BucketResult, len(group))
	var wg sync.WaitGroup
	for i := range group {
		wg.Add(1)
		go func() {
			defer wg.Done()
			slots[i] = s.refreshOne(ctx, group[i].ID)
		}()
	}
	wg.Wait()
	var results []BucketResult
	for _, slot := range slots {
		if slot != nil {
			results = append(results, *slot)
		}
	}
	return results
}

// refreshOne never panics out: a panic is logged and the feed gets no result.
func (s *Syncer) refreshOne(ctx context.Context, feedID int64) (result *BucketResult) {
	defer func() {
		if r := recover(); r != nil {
			slog.Error("feed sync task panicked", "event", "sync.task_panicked", "feed_id", feedID, "error", r)
			result = nil
		}
	}()
	ctx, cancel := context.WithTimeout(ctx, fetch.FeedSyncTimeout)
	defer cancel()
	synced, err := s.Refresh(ctx, feedID)
	switch {
	case err != nil && errors.Is(ctx.Err(), context.DeadlineExceeded):
		slog.Warn("feed sync timed out", "event", "feed.sync_timeout", "feed_id", feedID,
			"timeout_s", int(fetch.FeedSyncTimeout.Seconds()))
		return &BucketResult{FeedID: feedID, Err: fmt.Sprintf("Feed sync timed out after %ds", int(fetch.FeedSyncTimeout.Seconds()))}
	case err != nil:
		slog.Warn("feed sync failed", "event", "feed.sync_failed", "feed_id", feedID, "error", err)
		return &BucketResult{FeedID: feedID, Err: err.Error()}
	}
	slog.Debug("feed synced", "event", "feed.synced", "feed_id", feedID,
		"new_entries", synced.NewEntries, "updated_entries", synced.UpdatedEntries)
	return &BucketResult{FeedID: feedID, Result: synced}
}
