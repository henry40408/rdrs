package worker

import (
	"context"
	"log/slog"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

const (
	// RetentionInterval is how often read entries are pruned.
	RetentionInterval = 24 * time.Hour
	// retentionBatchSize entries are deleted per transaction during a drain.
	retentionBatchSize = 500
)

// Retention prunes read, aged, unstarred entries of users who opted in, then
// runs maintenance. DB should be a background handle.
type Retention struct {
	DB *db.DB
	// Interval defaults to RetentionInterval; tests shorten it.
	Interval time.Duration
}

// Run blocks until ctx ends. Cancellation is checked between batches.
func (r *Retention) Run(ctx context.Context) {
	interval := r.Interval
	if interval <= 0 {
		interval = RetentionInterval
	}
	slog.Info("retention worker started", "event", "retention.worker_started", "interval_secs", int64(interval.Seconds()))
	first := true
	everyInterval(ctx, interval, func() {
		r.pass(ctx, first)
		first = false
	})
	slog.Info("retention worker stopping", "event", "retention.worker_stopping")
	slog.Info("retention worker stopped", "event", "retention.worker_stopped")
}

// pass prunes once. The first pass skips the planner refresh because opening
// the database just did it.
func (r *Retention) pass(ctx context.Context, first bool) {
	work := context.WithoutCancel(ctx)
	total := r.drain(ctx, work)
	if total > 0 {
		slog.Info("retention pruned read entries", "event", "retention.pruned", "count", total)
		r.maintain(work)
		return
	}
	if first {
		return
	}
	// Refresh stats even when nobody opted in, or they would never refresh.
	if err := r.DB.Optimize(work); err != nil {
		slog.Error("planner statistics refresh failed", "event", "retention.optimize_failed", "error", err)
	}
}

// drain deletes batches until one comes back short, an error, or shutdown.
func (r *Retention) drain(ctx, work context.Context) int64 {
	var total int64
	for ctx.Err() == nil {
		deleted, err := entry.PruneReadRetentionBatch(work, r.DB, retentionBatchSize)
		if err != nil {
			slog.Error("retention prune failed", "event", "retention.prune_failed", "error", err)
			break
		}
		total += deleted
		if deleted < retentionBatchSize {
			break
		}
	}
	return total
}

func (r *Retention) maintain(ctx context.Context) {
	vacuumed, err := r.DB.RunMaintenance(ctx)
	if err != nil {
		slog.Error("retention maintenance failed", "event", "retention.maintenance_failed", "error", err)
		return
	}
	if vacuumed {
		slog.Info("retention maintenance ran VACUUM", "event", "retention.vacuumed")
	}
}
