package worker

import (
	"context"
	"log/slog"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models"
)

const (
	// CleanupInterval is how often expired rows are swept.
	CleanupInterval = time.Hour
	// SummaryTTLHours is how long a stored summary lives.
	SummaryTTLHours = 24
	// orphanImageBatchSize images are deleted per statement to keep each
	// SQLite write short.
	orphanImageBatchSize = 500
)

// Cleanup deletes expired summaries, sessions and API tokens, spent or expired
// account invites, and feed images whose feed is gone. Sessions and tokens back up the lazy per-request
// deletes, which never fire for an abandoned device. DB should be a
// background handle.
type Cleanup struct {
	DB *db.DB
	// Interval defaults to CleanupInterval; tests shorten it.
	Interval time.Duration
}

// Run blocks until ctx ends.
func (c *Cleanup) Run(ctx context.Context) {
	interval := c.Interval
	if interval <= 0 {
		interval = CleanupInterval
	}
	slog.Info("summary cleanup worker started", "event", "cleanup.worker_started",
		"interval_hours", int64(interval.Hours()), "ttl_hours", SummaryTTLHours)
	everyInterval(ctx, interval, func() { c.Sweep(context.WithoutCancel(ctx)) })
	slog.Info("summary cleanup worker stopping", "event", "cleanup.worker_stopping")
	slog.Info("summary cleanup worker stopped", "event", "cleanup.worker_stopped")
}

// Sweep runs one pass. Each sweep reports its own failure so a broken summary
// table cannot halt session and token expiry.
func (c *Cleanup) Sweep(ctx context.Context) {
	slog.Debug("running cleanup sweep", "event", "cleanup.sweep_started")
	sweeps := []struct {
		kind string
		run  func(context.Context) (int64, error)
	}{
		{"summary", func(ctx context.Context) (int64, error) {
			return models.DeleteExpiredSummaries(ctx, c.DB, SummaryTTLHours)
		}},
		{"session", func(ctx context.Context) (int64, error) { return models.DeleteExpiredSessions(ctx, c.DB) }},
		{"api_token", func(ctx context.Context) (int64, error) { return models.DeleteExpiredTokens(ctx, c.DB) }},
		{"invite", func(ctx context.Context) (int64, error) { return models.DeleteStaleInvites(ctx, c.DB) }},
		{"feed_image", c.deleteOrphanedFeedImages},
	}
	for _, sweep := range sweeps {
		count, err := sweep.run(ctx)
		if err != nil {
			slog.Error("cleanup sweep failed", "event", "cleanup.sweep_failed", "kind", sweep.kind, "error", err)
			continue
		}
		if count > 0 {
			slog.Info("cleanup swept expired rows", "event", "cleanup.swept", "kind", sweep.kind, "count", count)
		}
	}
}

func (c *Cleanup) deleteOrphanedFeedImages(ctx context.Context) (int64, error) {
	var total int64
	for {
		deleted, err := models.DeleteOrphanedFeedImages(ctx, c.DB, orphanImageBatchSize)
		total += deleted
		if err != nil || deleted < orphanImageBatchSize {
			return total, err
		}
	}
}
