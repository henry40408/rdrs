package models

// Entries a reader's client rendered, and the per-feed open rate.
//
// Compare entry.created_at to the pixel_tracking_enabled_at column, column to
// column, never to a bound timestamp: SQLite stores datetime('now') text while
// a bound RFC 3339 value has a 'T', and 'T' > ' ' breaks the comparison.

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// MinTrackedForRate is how many tracked entries a feed needs before its open
// rate is shown instead of a dash; smaller samples swing too much to mean
// anything.
const MinTrackedForRate = 5

// FeedOpenRate is one feed's open counts over the tracked window.
type FeedOpenRate struct {
	FeedID int64
	Title  *string
	// Tracked is the entries created since opt-in and not yet pruned.
	Tracked int64
	// Opened is, of those, the ones a client rendered.
	Opened int64
}

// Percent is the whole-percent open rate, or false below MinTrackedForRate.
func (r FeedOpenRate) Percent() (int64, bool) {
	if r.Tracked < MinTrackedForRate {
		return 0, false
	}
	// Rounded integer division, matching bar_percent.
	return (r.Opened*100 + r.Tracked/2) / r.Tracked, true
}

// TrackingWindow is the window the open rate actually speaks about.
type TrackingWindow struct {
	EnabledAt *time.Time
	// OldestTracked is the oldest surviving entry; retention can make it far
	// newer than EnabledAt.
	OldestTracked *time.Time
}

// TrackedSince is the later of the opt-in and the oldest surviving entry, since
// pruned entries no longer count in the denominator.
func (w TrackingWindow) TrackedSince() *time.Time {
	if w.EnabledAt == nil {
		return nil
	}
	if w.OldestTracked != nil && w.OldestTracked.After(*w.EnabledAt) {
		return w.OldestTracked
	}
	return w.EnabledAt
}

// RecordEntryOpen records that entryID was rendered for userID and reports
// whether it was new.
//
// Ownership and opt-in are enforced in the statement itself; DO NOTHING makes
// re-fetches idempotent.
func RecordEntryOpen(ctx context.Context, e db.Executor, userID, entryID int64) (bool, error) {
	affected, err := rowsAffected(e.ExecContext(ctx,
		`INSERT INTO entry_open (user_id, entry_id)
		 SELECT us.user_id, e.id
		 FROM entry e
		 JOIN feed f ON f.id = e.feed_id
		 JOIN category c ON c.id = f.category_id
		 JOIN user_settings us ON us.user_id = c.user_id
		 WHERE us.user_id = $1
		   AND e.id = $2
		   AND us.pixel_tracking_enabled_at IS NOT NULL
		   AND e.created_at >= us.pixel_tracking_enabled_at
		 ON CONFLICT (user_id, entry_id) DO NOTHING`, userID, entryID))
	return affected > 0, err
}

// openRatesSQL must stay covered by idx_entry_feed_created_at; a plan test pins
// it.
const openRatesSQL = `SELECT f.id AS feed_id, f.title AS title,
		COUNT(e.id) AS tracked,
		COUNT(o.entry_id) AS opened
	 FROM feed f
	 JOIN category c ON c.id = f.category_id
	 JOIN user_settings us ON us.user_id = c.user_id
	 LEFT JOIN entry e ON e.feed_id = f.id
	      AND e.created_at >= us.pixel_tracking_enabled_at
	 LEFT JOIN entry_open o ON o.entry_id = e.id AND o.user_id = us.user_id
	 WHERE c.user_id = $1 AND us.pixel_tracking_enabled_at IS NOT NULL
	 GROUP BY f.id, f.title`

// OpenRatesByFeed counts per feed (untracked feeds as 0/0) in one aggregate to
// avoid N+1. It is empty when the user opted out, which hides the column.
func OpenRatesByFeed(ctx context.Context, e db.Executor, userID int64) ([]FeedOpenRate, error) {
	rows, err := e.QueryContext(ctx, openRatesSQL, userID)
	return collect(rows, err, func(row scanner) (FeedOpenRate, error) {
		var r FeedOpenRate
		err := row.Scan(&r.FeedID, &r.Title, &r.Tracked, &r.Opened)
		return r, err
	})
}

// Hoisted like openRatesSQL; depends on the same index.
const trackingWindowSQL = `SELECT us.pixel_tracking_enabled_at AS enabled_at,
		MIN(e.created_at) AS oldest_tracked
	 FROM user_settings us
	 LEFT JOIN category c ON c.user_id = us.user_id
	 LEFT JOIN feed f ON f.category_id = c.id
	 LEFT JOIN entry e ON e.feed_id = f.id
	      AND e.created_at >= us.pixel_tracking_enabled_at
	 WHERE us.user_id = $1 AND us.pixel_tracking_enabled_at IS NOT NULL
	 GROUP BY us.pixel_tracking_enabled_at`

// FindTrackingWindow is the opt-in date and the oldest entry still inside the
// tracked window; both are nil when the user opted out.
func FindTrackingWindow(ctx context.Context, e db.Executor, userID int64) (TrackingWindow, error) {
	var w TrackingWindow
	err := e.QueryRowContext(ctx, trackingWindowSQL, userID).Scan(
		db.NullTimeDest(&w.EnabledAt), db.NullTimeDest(&w.OldestTracked))
	if errors.Is(err, sql.ErrNoRows) {
		return TrackingWindow{}, nil
	}
	return w, err
}
