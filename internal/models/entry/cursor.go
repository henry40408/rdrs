package entry

import (
	"strconv"
	"strings"
)

// ContinuationCursor is a pagination cursor: "<ts>|<id>" (composite) or, for
// in-flight cursors from older clients, a bare id.
type ContinuationCursor interface{ isCursor() }

// CompositeCursor is (sort_ts, id); SortTS is the sort column as stored text,
// so the predicate compares against the indexed column without conversion.
type CompositeCursor struct {
	SortTS string
	ID     int64
}

// LegacyIDCursor is the old "e.id < ?" cursor; it is input only.
type LegacyIDCursor int64

func (CompositeCursor) isCursor() {}
func (LegacyIDCursor) isCursor()  {}

// ParseCursor parses a continuation token, or reports false when it is
// malformed.
func ParseCursor(s string) (ContinuationCursor, bool) {
	if s == "" {
		return nil, false
	}
	sortTS, idText, composite := strings.Cut(s, "|")
	if !composite {
		id, err := strconv.ParseInt(s, 10, 64)
		return LegacyIDCursor(id), err == nil
	}
	id, err := strconv.ParseInt(idText, 10, 64)
	if sortTS == "" || err != nil {
		return nil, false
	}
	return CompositeCursor{SortTS: sortTS, ID: id}, true
}

// EncodeCursor is the composite continuation token for a row.
func EncodeCursor(sortTS string, id int64) string {
	return sortTS + "|" + strconv.FormatInt(id, 10)
}

// ContinuationParams paginates a Google Reader style stream.
type ContinuationParams struct {
	OldestFirst  bool
	Limit        int64
	Continuation ContinuationCursor
	// OldestTS and NewestTS bound COALESCE(published_at, created_at), in epoch
	// seconds.
	OldestTS  *int64
	NewestTS  *int64
	SortOrder SortOrder
}

// WithSortTS is an entry paired with its cursor timestamp, which is what
// FetchSortTS would return for it.
type WithSortTS struct {
	WithFeed
	SortTS *string
}

// NextContinuation is the next cursor for a page fetched with pageSize+1 rows:
// built from the last kept row, or nil when the look-ahead row is absent.
func NextContinuation(rows []WithSortTS, pageSize int) *string {
	if len(rows) <= pageSize || pageSize < 1 {
		return nil
	}
	last := rows[pageSize-1]
	if last.SortTS == nil {
		return nil
	}
	token := EncodeCursor(*last.SortTS, last.ID)
	return &token
}
