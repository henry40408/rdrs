package entry

import (
	"context"
	"sort"
	"strconv"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// ListQuery says which entries ListByUser returns, in what order, and which
// page of them.
type ListQuery struct {
	UserID    int64
	Filter    *Filter
	SortOrder SortOrder
	Limit     int64
	Offset    int64
}

func collectWithFeed(rows *db.Rows, err error) ([]WithFeed, error) {
	return collect(rows, err, func(s scanner) (WithFeed, error) { return scanWithFeed(s) })
}

// ListByUser lists a user's entries, newest first by the sort column.
func ListByUser(ctx context.Context, e db.Executor, lq ListQuery) ([]WithFeed, error) {
	q := newUserQuery(e.Dialect(), lq.UserID)
	q.applyFilter(lq.Filter)

	// Force the published-order index: SQLite otherwise walks
	// category -> feed -> entry and sorts every row.
	hint := ""
	if lq.SortOrder == SortPublishedAt {
		hint = e.Dialect().IndexedBy(publishedSortHint(lq.Filter))
	}
	limit, offset := q.bind(lq.Limit), q.bind(lq.Offset)
	rows, err := e.QueryContext(ctx,
		`SELECT `+withFeedColumnsJoin+` FROM entry e`+hint+sortJoin(lq.SortOrder)+`
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
		 WHERE `+q.whereClause()+`
		 ORDER BY `+sortColumn(lq.SortOrder)+` DESC
		 LIMIT `+limit+` OFFSET `+offset, q.binds...)
	return collectWithFeed(rows, err)
}

// ListOfflineSet is the entries a client mirrors for offline reading, newest
// first. keep is the total budget: unread first, then starred fill the rest.
// keep <= 0 returns nothing without touching the database.
func ListOfflineSet(ctx context.Context, e db.Executor, userID, keep int64) ([]WithFeed, error) {
	if keep <= 0 {
		return nil, nil
	}
	set, err := ListByUser(ctx, e, ListQuery{
		UserID: userID, Filter: &Filter{UnreadOnly: true}, SortOrder: SortPublishedAt, Limit: keep,
	})
	if err != nil {
		return nil, err
	}
	if remaining := keep - int64(len(set)); remaining > 0 {
		starred, err := ListByUser(ctx, e, ListQuery{
			UserID: userID, Filter: &Filter{StarredOnly: true}, SortOrder: SortStarredAt, Limit: remaining,
		})
		if err != nil {
			return nil, err
		}
		set = appendUnseen(set, starred)
	}
	sort.SliceStable(set, func(a, b int) bool { return newerFirst(&set[a], &set[b]) })
	return set, nil
}

// appendUnseen adds extra entries not already in set. An unread entry can also
// be starred, so the set may fall short of the budget by the overlap.
func appendUnseen(set, extra []WithFeed) []WithFeed {
	seen := make(map[int64]bool, len(set))
	for i := range set {
		seen[set[i].ID] = true
	}
	for i := range extra {
		if !seen[extra[i].ID] {
			set = append(set, extra[i])
		}
	}
	return set
}

// newerFirst orders by (published_at or created_at, id) descending.
func newerFirst(a, b *WithFeed) bool {
	ta, tb := sortInstant(&a.Entry), sortInstant(&b.Entry)
	if !ta.Equal(tb) {
		return ta.After(tb)
	}
	return a.ID > b.ID
}

// CountByUser counts the entries a filter matches.
func CountByUser(ctx context.Context, e db.Executor, userID int64, f *Filter) (int64, error) {
	q := newUserQuery(e.Dialect(), userID)
	q.applyFilter(f)

	// Without a hint SQLite scans every entry to count one day. The covering
	// (feed_id, sort_ts) index lets category -> feed -> entry count each
	// feed's day range without reading entry rows. A snapshot's read_at >= ?
	// is outside the partial index, so skip it then.
	hint := ""
	if f.PublishedBetween != nil && f.UnreadOnly && f.ReadAfter == nil {
		hint = e.Dialect().IndexedBy(" INDEXED BY idx_entry_unread_feed_sort")
	}
	return scalar(ctx, e,
		`SELECT COUNT(*) FROM entry e`+hint+`
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE `+q.whereClause(), q.binds...)
}

// CountUnreadByUser counts a user's unread entries.
func CountUnreadByUser(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	return scalar(ctx, e,
		`SELECT COUNT(*) FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1 AND e.read_at IS NULL`, userID)
}

func scalar(ctx context.Context, e db.Executor, query string, args ...any) (int64, error) {
	var n int64
	err := e.QueryRowContext(ctx, query, args...).Scan(&n)
	return n, err
}

// unreadCountSubquery is the unread count of feed f. The read_at IS NULL test
// sits in a WHERE, not a LEFT JOIN ... ON: in an ON clause SQLite does not
// treat the partial index as covering it and reads the table row of every
// unread entry, where this reads only the index.
func unreadCountSubquery(d db.Dialect) string {
	hint := d.IndexedBy(" INDEXED BY idx_entry_unread_feed")
	return "(SELECT COUNT(*) FROM entry e" + hint + " WHERE e.feed_id = f.id AND e.read_at IS NULL)"
}

func countMap(ctx context.Context, e db.Executor, query string, args ...any) (map[int64]int64, error) {
	rows, err := e.QueryContext(ctx, query, args...)
	pairs, err := collect(rows, err, func(s scanner) ([2]int64, error) {
		var pair [2]int64
		err := s.Scan(&pair[0], &pair[1])
		return pair, err
	})
	if err != nil {
		return nil, err
	}
	counts := make(map[int64]int64, len(pairs))
	for _, pair := range pairs {
		counts[pair[0]] = pair[1]
	}
	return counts, nil
}

// CountUnreadByFeed maps feed id to unread count for a user.
func CountUnreadByFeed(ctx context.Context, e db.Executor, userID int64) (map[int64]int64, error) {
	return countMap(ctx, e,
		`SELECT f.id, `+unreadCountSubquery(e.Dialect())+` FROM feed f
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1`, userID)
}

// CountUnreadByFeedInCategory is CountUnreadByFeed for one category, which
// keeps lazy loading off the all-feeds aggregate. userID stays in the WHERE so
// a guessed category id cannot read another account's counts.
func CountUnreadByFeedInCategory(ctx context.Context, e db.Executor, userID, categoryID int64) (map[int64]int64, error) {
	return countMap(ctx, e,
		`SELECT f.id, `+unreadCountSubquery(e.Dialect())+` FROM feed f
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1 AND f.category_id = $2`, userID, categoryID)
}

// CountUnreadByCategory maps category id to unread count for a user. SUM is
// numeric on PostgreSQL, so it is cast back to BIGINT.
func CountUnreadByCategory(ctx context.Context, e db.Executor, userID int64) (map[int64]int64, error) {
	return countMap(ctx, e,
		`SELECT c.id, CAST(COALESCE(SUM(`+unreadCountSubquery(e.Dialect())+`), 0) AS BIGINT) FROM category c
		 LEFT JOIN feed f ON f.category_id = c.id
		 WHERE c.user_id = $1 GROUP BY c.id`, userID)
}

// IDTimestamp is an entry id with the epoch microseconds of its sort time.
type IDTimestamp struct {
	ID          int64
	TimestampUS int64
}

// ListIDsByUser lists (entry id, timestamp) pairs with continuation paging.
func ListIDsByUser(ctx context.Context, e db.Executor, userID int64, f *Filter, p *ContinuationParams) ([]IDTimestamp, error) {
	q := continuationQuery(e.Dialect(), userID, f, p)
	limit := q.bind(p.Limit)
	// The "no more pages" sentinel; epoch extraction dialect-forks.
	epochUS := e.Dialect().Epoch(publishedSortColumn)
	rows, err := e.QueryContext(ctx,
		`SELECT e.id, `+epochUS+` * 1000000 FROM entry e`+sortJoin(p.SortOrder)+`
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE `+q.whereClause()+`
		 ORDER BY `+orderBy(p.SortOrder, p.OldestFirst)+`
		 LIMIT `+limit, q.binds...)
	return collect(rows, err, func(s scanner) (IDTimestamp, error) {
		var item IDTimestamp
		err := s.Scan(&item.ID, &item.TimestampUS)
		return item, err
	})
}

func continuationQuery(d db.Dialect, userID int64, f *Filter, p *ContinuationParams) *query {
	q := newUserQuery(d, userID)
	q.applyFilter(f)
	q.applyTimeBounds(p)
	q.applyContinuation(p)
	return q
}

// orderBy is the ORDER BY of a paged sort: the sort column, then e.id as the
// tie-breaker.
func orderBy(order SortOrder, oldestFirst bool) string {
	direction := "DESC"
	if oldestFirst {
		direction = "ASC"
	}
	return sortColumn(order) + " " + direction + ", e.id " + direction
}

// continuationSQL builds a continuation page query; extraSelect is appended
// to the SELECT list.
func continuationSQL(d db.Dialect, userID int64, f *Filter, p *ContinuationParams, extraSelect string) (string, []any) {
	q := continuationQuery(d, userID, f, p)
	// Page 0 only: without the hint the planner walks category -> feed -> entry
	// and sorts the whole corpus. With a continuation predicate the sort index
	// is already used, so leave that plan alone.
	hint := ""
	if p.SortOrder == SortPublishedAt && p.Continuation == nil {
		hint = d.IndexedBy(publishedSortHint(f))
	}
	limit := q.bind(p.Limit)
	sql := `SELECT ` + withFeedColumnsJoin + extraSelect + ` FROM entry e` + hint + sortJoin(p.SortOrder) + `
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
		 WHERE ` + q.whereClause() + `
		 ORDER BY ` + orderBy(p.SortOrder, p.OldestFirst) + `
		 LIMIT ` + limit
	return sql, q.binds
}

// ListByUserWithContinuation lists entries for Google Reader stream/contents.
func ListByUserWithContinuation(ctx context.Context, e db.Executor, userID int64, f *Filter, p *ContinuationParams) ([]WithFeed, error) {
	query, binds := continuationSQL(e.Dialect(), userID, f, p, "")
	rows, err := e.QueryContext(ctx, query, binds...)
	return collectWithFeed(rows, err)
}

// ListByUserWithContinuationTS is ListByUserWithContinuation with each row's
// cursor timestamp, so the next cursor needs no second lookup.
func ListByUserWithContinuationTS(ctx context.Context, e db.Executor, userID int64, f *Filter, p *ContinuationParams) ([]WithSortTS, error) {
	tsExpr := e.Dialect().CursorTS(sortColumn(p.SortOrder))
	query, binds := continuationSQL(e.Dialect(), userID, f, p, ", "+tsExpr+" AS sort_ts")
	rows, err := e.QueryContext(ctx, query, binds...)
	return collect(rows, err, func(s scanner) (WithSortTS, error) {
		var item WithSortTS
		var err error
		item.WithFeed, err = scanWithFeed(s, sortTS{&item.SortTS})
		return item, err
	})
}

func itoa(n int64) string { return strconv.FormatInt(n, 10) }

// sortInstant is COALESCE(published_at, created_at).
func sortInstant(e *Entry) time.Time {
	if e.PublishedAt != nil {
		return *e.PublishedAt
	}
	return e.CreatedAt
}
