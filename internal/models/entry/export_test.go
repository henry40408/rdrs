package entry

import (
	"context"
	"errors"

	"github.com/henry40408/rdrs/internal/db"
)

// NeighborSQL exposes the neighbour lookup SQL so plan tests can run it.
func NeighborSQL(dialect db.Dialect, userID, entryID int64, sortTime string, next bool, f *Filter) (string, []any) {
	return neighborQuery{userID: userID, entryID: entryID, sortTime: sortTime, filter: f, next: next}.build(dialect)
}

// PositionSliceSQL exposes one position count SQL over slice of f.
func PositionSliceSQL(userID, entryID int64, f *Filter, slice int, cmp, unreadIndex string) (string, []any) {
	counter := positionCounter{userID: userID, entryID: entryID, slices: countSlices(f)}
	return counter.sliceSQL(db.DialectSQLite, cmp, counter.slices[slice], unreadIndex)
}

// ListSQL exposes the page SQL of a continuation list.
func ListSQL(dialect db.Dialect, userID int64, f *Filter, p *ContinuationParams) (string, []any) {
	return continuationSQL(dialect, userID, f, p, "")
}

// CountUnreadByUser counts a user's unread entries.
func CountUnreadByUser(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	return scalar(ctx, e,
		`SELECT COUNT(*) FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1 AND e.read_at IS NULL`, userID)
}

// UpsertAndLoad upserts and returns the stored entry plus whether it is new.
// It is an error to call it for a tombstoned guid.
func UpsertAndLoad(ctx context.Context, e db.Executor, u *Upsert) (*Entry, bool, error) {
	result, err := UpsertEntry(ctx, e, u)
	if err != nil {
		return nil, false, err
	}
	if result.Outcome == SkippedTombstoned {
		return nil, false, errors.New("entry: upsert for a tombstoned guid")
	}
	entry, err := mustFind(ctx, e, result.ID)
	return entry, result.Outcome == Inserted, err
}

// InsertTombstone records a tombstone for (feedID, guid). It is idempotent.
func InsertTombstone(ctx context.Context, e db.Executor, feedID int64, guid string) error {
	_, err := e.ExecContext(ctx,
		`INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, $2) ON CONFLICT(feed_id, guid) DO NOTHING`,
		feedID, guid)
	return err
}

// UnstarEntry clears starred_at and returns the entry.
func UnstarEntry(ctx context.Context, e db.Executor, id int64) (*Entry, error) {
	rows, err := exec(ctx, e, updateEntry+clearStarSQL+" WHERE id = $1", id)
	if err != nil {
		return nil, err
	}
	if rows == 0 {
		return nil, ErrNotFound
	}
	return mustFind(ctx, e, id)
}
