package worker

import (
	"context"
	"log/slog"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// backfillBatchSize rows are filled per transaction.
const backfillBatchSize = 500

// BackfillContentText fills entry.content_text for rows that predate its
// migration, so startup is not blocked. It is idempotent: an interrupted run
// resumes at the next start. Body search misses unfilled rows until it ends.
// database should be a background handle.
func BackfillContentText(ctx context.Context, database *db.DB) {
	work := context.WithoutCancel(ctx)
	total, err := entry.CountMissingContentText(work, database)
	if err != nil {
		slog.Error("content_text backfill could not count legacy entries", "event", "backfill.count_failed", "error", err)
		return
	}
	if total == 0 {
		return
	}
	slog.Info("content_text backfill started in background; body search over not-yet-filled rows is degraded until it completes",
		"event", "backfill.started", "total", total)
	var backfilled int64
	for {
		if ctx.Err() != nil {
			slog.Info("content_text backfill interrupted; resumes on next start",
				"event", "backfill.interrupted", "backfilled", backfilled, "total", total)
			return
		}
		n, err := entry.BackfillContentTextBatch(work, database, backfillBatchSize)
		if err != nil {
			slog.Error("content_text backfill batch failed", "event", "backfill.batch_failed", "error", err)
			return
		}
		if n == 0 {
			break
		}
		backfilled += int64(n)
		slog.Info("content_text backfill progress", "event", "backfill.progress", "backfilled", backfilled, "total", total)
	}
	slog.Info("content_text backfill complete", "event", "backfill.complete", "backfilled", backfilled)
}
