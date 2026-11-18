package entry

import (
	"context"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/db"
)

// The statements stamp with datetime('now') rather than a bound time so
// updated_at, read_at and starred_at keep the column-DEFAULT text format the
// string-compared cursor needs; the db layer maps it to now() on PostgreSQL.

const (
	setReadSQL   = "read_at = datetime('now'), updated_at = datetime('now')"
	clearReadSQL = "read_at = NULL, updated_at = datetime('now')"
	setStarSQL   = "starred_at = datetime('now'), updated_at = datetime('now')"
	clearStarSQL = "starred_at = NULL, updated_at = datetime('now')"
	updateEntry  = "UPDATE entry SET "
)

func exec(ctx context.Context, e db.Executor, query string, args ...any) (int64, error) {
	result, err := e.ExecContext(ctx, query, args...)
	if err != nil {
		return 0, err
	}
	return result.RowsAffected()
}

// MarkAsReadQuietly marks an unread entry read without re-reading it, for a
// caller that already holds the entry.
func MarkAsReadQuietly(ctx context.Context, e db.Executor, id int64) error {
	_, err := exec(ctx, e, updateEntry+setReadSQL+" WHERE id = $1 AND read_at IS NULL", id)
	return err
}

// MarkAsRead marks an unread entry read and returns it.
func MarkAsRead(ctx context.Context, e db.Executor, id int64) (*Entry, error) {
	if err := MarkAsReadQuietly(ctx, e, id); err != nil {
		return nil, err
	}
	return mustFind(ctx, e, id)
}

// StarEntry sets starred_at unless it is already set, and returns the entry.
func StarEntry(ctx context.Context, e db.Executor, id int64) (*Entry, error) {
	if _, err := exec(ctx, e, updateEntry+setStarSQL+" WHERE id = $1 AND starred_at IS NULL", id); err != nil {
		return nil, err
	}
	return mustFind(ctx, e, id)
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

func mustFind(ctx context.Context, e db.Executor, id int64) (*Entry, error) {
	entry, err := FindByID(ctx, e, id)
	if err != nil {
		return nil, err
	}
	if entry == nil {
		return nil, ErrNotFound
	}
	return entry, nil
}

// Changed is an entry after a state change, and whether the change did
// anything.
type Changed struct {
	Entry   *WithFeed
	Changed bool
}

// SetStarredForUser sets the starred state, scoped to the owner. It is
// idempotent. It returns nil when the entry is missing or not owned.
func SetStarredForUser(ctx context.Context, e db.Executor, userID, entryID int64, starred bool) (*Changed, error) {
	return setStateForUser(ctx, e, userID, entryID, stateChange{
		current: func(w *WithFeed) bool { return w.StarredAt != nil },
		desired: starred,
		set:     setStarSQL,
		clear:   clearStarSQL,
	})
}

// SetReadForUser sets the read state, scoped to the owner. It is idempotent.
// It returns nil when the entry is missing or not owned.
func SetReadForUser(ctx context.Context, e db.Executor, userID, entryID int64, read bool) (*Changed, error) {
	return setStateForUser(ctx, e, userID, entryID, stateChange{
		current: func(w *WithFeed) bool { return w.ReadAt != nil },
		desired: read,
		set:     setReadSQL,
		clear:   clearReadSQL,
	})
}

type stateChange struct {
	current func(*WithFeed) bool
	desired bool
	set     string
	clear   string
}

func setStateForUser(ctx context.Context, e db.Executor, userID, entryID int64, change stateChange) (*Changed, error) {
	before, err := FindByIDForUser(ctx, e, userID, entryID)
	if err != nil || before == nil {
		return nil, err
	}
	changed := change.current(before) != change.desired
	if changed {
		assignment := change.clear
		if change.desired {
			assignment = change.set
		}
		if _, err := exec(ctx, e, updateEntry+assignment+" WHERE id = $1", entryID); err != nil {
			return nil, err
		}
	}
	after, err := FindByIDForUser(ctx, e, userID, entryID)
	if err != nil || after == nil {
		return nil, err
	}
	return &Changed{Entry: after, Changed: changed}, nil
}

// SetFullContentForUser stores the raw "Fetch Full Content" HTML, scoped to
// the owner, overwriting any previous fetch. An entry the user does not own
// updates nothing rather than failing.
func SetFullContentForUser(ctx context.Context, e db.Executor, userID, entryID int64, html string) error {
	_, err := exec(ctx, e,
		`UPDATE entry SET full_content = $1, updated_at = datetime('now')
		 WHERE id = $2 AND feed_id IN (
		     SELECT f.id FROM feed f JOIN category c ON c.id = f.category_id
		     WHERE c.user_id = $3
		 )`, html, entryID, userID)
	return err
}

// MarkAllRead limits which unread entries a bulk "mark all as read" touches,
// beyond its scope.
type MarkAllRead struct {
	// OlderThanDays keeps entries published more than this many days ago.
	OlderThanDays *int64
	// StoredBy keeps entries stored strictly before this instant: when the
	// page the user clicked on was rendered. Strict, because the instant is
	// truncated to the second, so an entry stored within it may postdate the
	// list query.
	StoredBy *time.Time
}

// conditions is the extra " AND ..." text; it binds after the scope's own
// binds in q.
func (m MarkAllRead) conditions(q *query) string {
	var extra strings.Builder
	if m.OlderThanDays != nil {
		// The days value is an int64, so it cannot inject SQL.
		cutoff := q.dialect.DaysAgo(itoa(*m.OlderThanDays))
		extra.WriteString(" AND COALESCE(published_at, created_at) < " + cutoff)
	}
	if m.StoredBy != nil {
		extra.WriteString(" AND created_at < " + q.bind(q.dialect.TimeBound(*m.StoredBy)))
	}
	return extra.String()
}

// MarkAllReadByFeed marks a feed's unread entries read within limits and
// returns the rows affected.
func MarkAllReadByFeed(ctx context.Context, e db.Executor, feedID int64, limits MarkAllRead) (int64, error) {
	q := &query{dialect: e.Dialect(), binds: []any{feedID}}
	extra := limits.conditions(q)
	return exec(ctx, e,
		"UPDATE entry SET "+setReadSQL+" WHERE feed_id = $1 AND read_at IS NULL"+extra, q.binds...)
}

// MarkAllReadByUser marks all of a user's unread entries read within limits.
func MarkAllReadByUser(ctx context.Context, e db.Executor, userID int64, limits MarkAllRead) (int64, error) {
	q := &query{dialect: e.Dialect(), binds: []any{userID}}
	extra := limits.conditions(q)
	return exec(ctx, e,
		`UPDATE entry SET `+setReadSQL+` WHERE read_at IS NULL`+extra+` AND feed_id IN (
		     SELECT f.id FROM feed f
		     INNER JOIN category c ON f.category_id = c.id
		     WHERE c.user_id = $1
		 )`, q.binds...)
}

// MarkAllReadByCategory marks a category's unread entries read within limits.
func MarkAllReadByCategory(ctx context.Context, e db.Executor, categoryID int64, limits MarkAllRead) (int64, error) {
	q := &query{dialect: e.Dialect(), binds: []any{categoryID}}
	extra := limits.conditions(q)
	return exec(ctx, e,
		`UPDATE entry SET `+setReadSQL+` WHERE read_at IS NULL`+extra+` AND feed_id IN (
		     SELECT id FROM feed WHERE category_id = $1
		 )`, q.binds...)
}

// MarkReadByFilter marks every unread entry a filter matches, owned by the
// user, as read, reusing the list query's filter builder.
func MarkReadByFilter(ctx context.Context, e db.Executor, userID int64, f *Filter) (int64, error) {
	q := newUserQuery(e.Dialect(), userID)
	q.applyFilter(f)
	return exec(ctx, e,
		`UPDATE entry SET `+setReadSQL+` WHERE read_at IS NULL AND id IN (
		     SELECT e.id FROM entry e
		     INNER JOIN feed f ON e.feed_id = f.id
		     INNER JOIN category c ON f.category_id = c.id
		     WHERE `+q.whereClause()+`
		 )`, q.binds...)
}

// bulkUpdate sets set on entryIDs owned by userID; extraWhere follows the id
// list. The user id is $1 and the ids fill $2...
func bulkUpdate(ctx context.Context, e db.Executor, userID int64, entryIDs []int64, set, extraWhere string) (int64, error) {
	if len(entryIDs) == 0 {
		return 0, nil
	}
	return exec(ctx, e,
		`UPDATE entry SET `+set+` WHERE id IN (`+placeholders(2, len(entryIDs))+`)`+extraWhere+`
		   AND feed_id IN (
		       SELECT f.id FROM feed f
		       INNER JOIN category c ON f.category_id = c.id
		       WHERE c.user_id = $1
		   )`, append([]any{userID}, int64Args(entryIDs)...)...)
}

// MarkReadByIDs marks currently unread entries read, scoped to the user.
func MarkReadByIDs(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (int64, error) {
	return bulkUpdate(ctx, e, userID, entryIDs, setReadSQL, " AND read_at IS NULL")
}

// MarkUnreadByIDs marks entries unread, scoped to the user.
func MarkUnreadByIDs(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (int64, error) {
	return bulkUpdate(ctx, e, userID, entryIDs, clearReadSQL, "")
}

// StarByIDs stars entries that are not starred yet, scoped to the user.
func StarByIDs(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (int64, error) {
	return bulkUpdate(ctx, e, userID, entryIDs, setStarSQL, " AND starred_at IS NULL")
}

// UnstarByIDs unstars entries, scoped to the user.
func UnstarByIDs(ctx context.Context, e db.Executor, userID int64, entryIDs []int64) (int64, error) {
	return bulkUpdate(ctx, e, userID, entryIDs, clearStarSQL, "")
}
