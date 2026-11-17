package entry

import (
	"context"
	"strconv"

	"github.com/henry40408/rdrs/internal/db"
)

// PositionCountLimit is where counting stops, so the cost stays flat on huge
// lists.
const PositionCountLimit int64 = 1000

// Position is the 1-based place of an entry in a filtered list and the list's
// size. Each is capped at Limit+1; a value above Limit means "more than Limit".
type Position struct {
	Position int64
	Total    int64
	Limit    int64
}

// countSlice is one disjoint part of the filtered list, counted on its own.
type countSlice struct {
	filter Filter
	// readSince counts only entries read at or after this instant.
	readSince *string
}

// countSlices splits the snapshot predicate (read_at IS NULL OR read_at >= ?),
// which defeats every partial index, into its two disjoint halves that each
// have an index.
func countSlices(f *Filter) []countSlice {
	if !f.UnreadOnly || f.ReadAfter == nil {
		return []countSlice{{filter: *f}}
	}
	unread := *f
	unread.ReadAfter = nil
	read := *f
	read.UnreadOnly, read.ReadAfter = false, nil
	return []countSlice{{filter: unread}, {filter: read, readSince: f.ReadAfter}}
}

type positionCounter struct {
	userID  int64
	entryID int64
	slices  []countSlice
}

// sliceSQL builds one capped count. $1 is the user id and, when cmp is set,
// $2 the entry whose sort key cmp compares against; the filter's binds follow.
// unreadIndex is the partial unread index to pin for a strict unread slice:
// with several to choose from the planner flips on statistics.
func (p positionCounter) sliceSQL(dialect db.Dialect, cmp string, slice countSlice, unreadIndex string) (string, []any) {
	q := newUserQuery(dialect, p.userID)
	if cmp != "" {
		q.bind(p.entryID)
	}
	q.applyFilter(&slice.filter)

	hint, join := "", "INNER JOIN"
	switch {
	case slice.readSince != nil:
		// Entries read since the snapshot are few; the read_at range finds them
		// where the planner would otherwise walk every read entry.
		q.where(q.readSince(*slice.readSince))
		hint, join = dialect.IndexedBy(" INDEXED BY idx_entry_read_at"), pinnedJoin(dialect)
	case slice.filter.UnreadOnly:
		hint = dialect.IndexedBy(unreadIndex)
	}
	where := q.whereClause()
	if cmp != "" {
		where += cmp
	}
	return `SELECT COUNT(*) FROM (
		 SELECT 1 FROM entry e` + hint + `
		 ` + join + ` feed f ON e.feed_id = f.id
		 ` + join + ` category c ON f.category_id = c.id
		 WHERE ` + where + `
		 LIMIT ` + strconv.FormatInt(PositionCountLimit+1, 10) + `
		 ) capped`, q.binds
}

// cappedCount sums the rows matching cmp over all slices, capped at
// PositionCountLimit+1.
func (p positionCounter) cappedCount(ctx context.Context, e db.Executor, cmp, unreadIndex string) (int64, error) {
	var sum int64
	for _, slice := range p.slices {
		sqlText, args := p.sliceSQL(e.Dialect(), cmp, slice, unreadIndex)
		n, err := scalar(ctx, e, sqlText, args...)
		if err != nil {
			return 0, err
		}
		sum += n
		if sum > PositionCountLimit {
			break
		}
	}
	return min(sum, PositionCountLimit+1), nil
}

// FindPosition says where entryID sits in the "published DESC, id DESC" order
// under f: the entries ahead of it, itself included, against the same sort key
// FindNeighbors walks.
func FindPosition(ctx context.Context, e db.Executor, userID, entryID int64, f *Filter) (Position, error) {
	counter := positionCounter{userID: userID, entryID: entryID, slices: countSlices(f)}

	ts := publishedSortColumn
	ownTS := "(SELECT COALESCE(s.published_at, s.created_at) FROM entry s WHERE s.id = $2)"

	// total has no sort key, so feed_id alone covers it; the range wants
	// (feed_id, ts); the tie-break is an equality on ts that no feed scopes.
	total, err := counter.cappedCount(ctx, e, "", " INDEXED BY idx_entry_unread_feed")
	if err != nil {
		return Position{}, err
	}
	// Splitting the tie-break from the range lets the sort index serve the
	// range half; an OR across both would force a scan.
	ahead, err := counter.cappedCount(ctx, e, " AND "+ts+" > "+ownTS, " INDEXED BY idx_entry_unread_feed_sort")
	if err != nil {
		return Position{}, err
	}
	tied, err := counter.cappedCount(ctx, e,
		" AND "+ts+" = "+ownTS+" AND e.id >= $2", " INDEXED BY idx_entry_unread_sort")
	if err != nil {
		return Position{}, err
	}
	return Position{Position: min(ahead+tied, PositionCountLimit+1), Total: total, Limit: PositionCountLimit}, nil
}
