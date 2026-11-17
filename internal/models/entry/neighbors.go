package entry

import (
	"context"
	"database/sql"
	"errors"

	"github.com/henry40408/rdrs/internal/db"
)

// Neighbors are the entries around one entry in a filtered list. Prev is the
// newer one and Next the older one; either is nil at the end of the list.
type Neighbors struct {
	PrevID *int64
	NextID *int64
}

// FindNeighbors finds the entries next to entryID by
// COALESCE(published_at, created_at) DESC, honouring every filter condition.
func FindNeighbors(ctx context.Context, e db.Executor, userID, entryID int64, f *Filter) (Neighbors, error) {
	sortTime, err := fetchSortTime(ctx, e, userID, entryID)
	if err != nil || sortTime == nil {
		return Neighbors{}, err
	}
	prev, err := neighborID(ctx, e, neighborQuery{userID: userID, entryID: entryID, sortTime: *sortTime, filter: f, next: false})
	if err != nil {
		return Neighbors{}, err
	}
	next, err := neighborID(ctx, e, neighborQuery{userID: userID, entryID: entryID, sortTime: *sortTime, filter: f, next: true})
	if err != nil {
		return Neighbors{}, err
	}
	return Neighbors{PrevID: prev, NextID: next}, nil
}

func fetchSortTime(ctx context.Context, e db.Executor, userID, entryID int64) (*string, error) {
	var ts *string
	err := e.QueryRowContext(ctx,
		`SELECT `+e.Dialect().CursorTS(publishedSortColumn)+`
		 FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE e.id = $1 AND c.user_id = $2`, entryID, userID).Scan(sortTS{&ts})
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return ts, err
}

type neighborQuery struct {
	userID   int64
	entryID  int64
	sortTime string
	filter   *Filter
	// next selects the older neighbour; otherwise the newer one.
	next bool
}

func neighborID(ctx context.Context, e db.Executor, n neighborQuery) (*int64, error) {
	sqlText, args := n.build(e.Dialect())
	var id int64
	err := e.QueryRowContext(ctx, sqlText, args...).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return &id, nil
}

func (n neighborQuery) build(dialect db.Dialect) (string, []any) {
	q := newUserQuery(dialect, n.userID)

	// On PostgreSQL bind a timestamp so the lookup is an index range scan.
	cmpTS := dialect.CursorTS(publishedSortColumn)
	var bound any = n.sortTime
	if parsed, ok := parseCursorTS(n.sortTime); ok && dialect == db.DialectPostgres {
		cmpTS, bound = publishedSortColumn, db.FormatSpace(parsed)
	}
	ts := q.bind(bound)
	order := "ASC"
	if n.next {
		idPlaceholder := q.bind(n.entryID)
		q.where("(" + cmpTS + " < " + ts + " OR (" + cmpTS + " = " + ts + " AND e.id < " + idPlaceholder + "))")
		order = "DESC, e.id DESC"
	} else {
		q.where(cmpTS + " > " + ts)
	}
	q.applyFilter(n.filter)

	hint, join := n.indexHint(dialect)
	return `SELECT e.id FROM entry e` + hint + `
		 ` + join + ` feed f ON e.feed_id = f.id
		 ` + join + ` category c ON f.category_id = c.id
		 WHERE ` + q.whereClause() + `
		 ORDER BY ` + publishedSortColumn + ` ` + order + `
		 LIMIT 1`, q.binds
}

// pinnedJoin is the join keyword that makes SQLite keep the written join
// order. PostgreSQL's CROSS JOIN takes no ON clause, so it uses INNER JOIN.
func pinnedJoin(dialect db.Dialect) string {
	if dialect == db.DialectPostgres {
		return "INNER JOIN"
	}
	return "CROSS JOIN"
}

// indexHint pins the sort index with entry driving the join so the lookup
// stops at LIMIT 1. A snapshot's (read_at IS NULL OR read_at >= ?) otherwise
// gets a multi-index OR plan that is O(table); the strict unread path must
// stay unpinned so it uses the partial idx_entry_unread_feed.
func (n neighborQuery) indexHint(dialect db.Dialect) (hint, join string) {
	if n.filter.UnreadOnly && n.filter.ReadAfter != nil {
		return dialect.IndexedBy(" INDEXED BY idx_entry_sort_ts"), pinnedJoin(dialect)
	}
	return dialect.IndexedBy(publishedSortHint(n.filter)), "INNER JOIN"
}
