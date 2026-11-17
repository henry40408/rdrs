// Package entry holds the SQL for the entry table and the listing, search,
// pagination and bulk-update queries built on it. Each function takes a
// db.Executor so it runs on a pool or inside a transaction.
//
// Entry timestamps are space-format TEXT on SQLite ("2006-01-02 15:04:05",
// the column DEFAULT), so every comparison binds db.FormatSpace text: an RFC
// 3339 bind would mis-order ('T' sorts after ' '). PostgreSQL parses the same
// text as timestamptz under its pinned UTC zone, which keeps the columns
// sargable.
package entry

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/search"
)

// ErrNotFound is returned when an entry does not exist.
var ErrNotFound = errors.New("entry not found")

// SortOrder is the column a list pages by.
type SortOrder int

// The sort orders.
const (
	// SortPublishedAt is COALESCE(published_at, created_at) DESC.
	SortPublishedAt SortOrder = iota
	SortReadAt
	SortStarredAt
	// SortSummarizedAt is entry_summary.created_at DESC; queries join
	// entry_summary through sortJoin.
	SortSummarizedAt
)

// Entry is a row of the entry table.
type Entry struct {
	ID     int64
	FeedID int64
	GUID   string
	Title  *string
	Link   *string
	// Content is the feed's HTML as fetched.
	Content *string
	// FullContent is the raw HTML of "Fetch Full Content". It is sanitised per
	// render, never at write time: sanitising signs image-proxy URLs, and a
	// stored signature would outlive its key.
	FullContent *string
	Summary     *string
	Author      *string
	PublishedAt *time.Time
	ReadAt      *time.Time
	StarredAt   *time.Time
	CreatedAt   time.Time
	UpdatedAt   time.Time
}

const entryColumns = "id, feed_id, guid, title, link, content, full_content, summary, author, " +
	"published_at, read_at, starred_at, created_at, updated_at"

func (e *Entry) scanDest() []any {
	return []any{
		&e.ID, &e.FeedID, &e.GUID, &e.Title, &e.Link, &e.Content, &e.FullContent, &e.Summary, &e.Author,
		db.NullTimeDest(&e.PublishedAt), db.NullTimeDest(&e.ReadAt), db.NullTimeDest(&e.StarredAt),
		db.TimeDest(&e.CreatedAt), db.TimeDest(&e.UpdatedAt),
	}
}

// WithFeed is an entry joined with its feed and category.
type WithFeed struct {
	Entry
	FeedTitle      *string
	FeedURL        string
	SiteURL        *string
	CategoryID     int64
	CategoryName   string
	FeedHasIcon    bool
	CustomReferrer *string
}

// ContentBaseURL is the base for resolving relative URLs: the entry link, then
// the feed site, then the feed URL. It is never empty, so the sanitiser can
// drop unresolvable image URLs instead of passing them through unproxied.
func (w *WithFeed) ContentBaseURL() string {
	for _, candidate := range []*string{w.Link, w.SiteURL} {
		if candidate != nil && *candidate != "" {
			return *candidate
		}
	}
	return w.FeedURL
}

// Filter narrows an entry list. The zero value matches everything.
type Filter struct {
	FeedID      *int64
	CategoryID  *int64
	UnreadOnly  bool
	StarredOnly bool
	ReadOnly    bool
	// Search is a raw substring (not LIKE-escaped) matched against title and
	// content_text.
	Search     *string
	HasSummary *bool
	// PublishedBetween keeps COALESCE(published_at, created_at) in [Start,
	// End): the sort column, so the unread sort index can serve the range.
	PublishedBetween *TimeRange
	// StoredBefore keeps entries stored strictly before this instant, so a
	// bulk mark skips what arrived after the list it was clicked on. Strict,
	// because the instant is truncated to the second.
	StoredBefore *time.Time
	// ReadAfter, with UnreadOnly, keeps entries read at or after this UTC
	// "YYYY-MM-DD HH:MM:SS" instant counted as unread, so reading-pane
	// navigation can return to them.
	ReadAfter *string
	// Query is the boolean /search AST.
	Query search.Node
}

// TimeRange is the half-open interval [Start, End).
type TimeRange struct{ Start, End time.Time }

// scanner is the Scan method shared by db.Row and db.Rows.
type scanner interface {
	Scan(dest ...any) error
}

func orNil[T any](value *T, err error) (*T, error) {
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return value, nil
}

func collect[T any](rows *db.Rows, err error, scan func(scanner) (T, error)) ([]T, error) {
	if err != nil {
		return nil, err
	}
	defer func() { _ = rows.Close() }()
	var out []T
	for rows.Next() {
		item, err := scan(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, item)
	}
	return out, rows.Err()
}

func scanEntry(row scanner) (Entry, error) {
	var e Entry
	err := row.Scan(e.scanDest()...)
	return e, err
}

// withFeedColumns* are the SELECT list for WithFeed. Every column is aliased
// because the join has duplicate names. The Count form takes has_icon from a
// correlated COUNT, for queries without LEFT JOIN image; the Join form from an
// existing LEFT JOIN image i. Keep both and scanWithFeed in sync.
const (
	withFeedBaseColumns = "e.id AS id, e.feed_id AS feed_id, e.guid AS guid, e.title AS title, " +
		"e.link AS link, e.content AS content, e.full_content AS full_content, e.summary AS summary, " +
		"e.author AS author, e.published_at AS published_at, e.read_at AS read_at, " +
		"e.starred_at AS starred_at, e.created_at AS created_at, e.updated_at AS updated_at, " +
		"f.title AS feed_title, f.url AS feed_url, f.site_url AS site_url, " +
		"c.id AS category_id, c.name AS category_name, "
	withFeedColumnsCount = withFeedBaseColumns +
		"(SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) AS has_icon, " +
		"f.custom_referrer AS custom_referrer"
	withFeedColumnsJoin = withFeedBaseColumns +
		"CAST(CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END AS BIGINT) AS has_icon, " +
		"f.custom_referrer AS custom_referrer"
)

// scanWithFeed scans a withFeedColumns* row followed by any extra columns.
func scanWithFeed(row scanner, extra ...any) (WithFeed, error) {
	var w WithFeed
	var hasIcon int64
	dest := append(w.Entry.scanDest(),
		&w.FeedTitle, &w.FeedURL, &w.SiteURL, &w.CategoryID, &w.CategoryName, &hasIcon, &w.CustomReferrer)
	err := row.Scan(append(dest, extra...)...)
	w.FeedHasIcon = hasIcon > 0
	return w, err
}

// sortTS scans a cursor timestamp column. SQLite returns a bare column of
// declared type TIMESTAMP as time.Time, and a COALESCE of columns as the
// stored text.
type sortTS struct{ dst **string }

func (s sortTS) Scan(value any) error {
	switch v := value.(type) {
	case nil:
		*s.dst = nil
		return nil
	case string:
		*s.dst = &v
	case []byte:
		text := string(v)
		*s.dst = &text
	case time.Time:
		text := db.FormatSpace(v)
		*s.dst = &text
	default:
		return fmt.Errorf("scan cursor timestamp: unexpected %T", value)
	}
	return nil
}

// FindByID looks an entry up by id.
func FindByID(ctx context.Context, e db.Executor, id int64) (*Entry, error) {
	entry, err := scanEntry(e.QueryRowContext(ctx, `SELECT `+entryColumns+` FROM entry WHERE id = $1`, id))
	return orNil(&entry, err)
}

// FindByGUIDAndFeed looks an entry up by its feed-unique guid.
func FindByGUIDAndFeed(ctx context.Context, e db.Executor, guid string, feedID int64) (*Entry, error) {
	entry, err := scanEntry(e.QueryRowContext(ctx,
		`SELECT `+entryColumns+` FROM entry WHERE guid = $1 AND feed_id = $2`, guid, feedID))
	return orNil(&entry, err)
}

func findWithFeed(ctx context.Context, e db.Executor, query string, args ...any) (*WithFeed, error) {
	entry, err := scanWithFeed(e.QueryRowContext(ctx, query, args...))
	return orNil(&entry, err)
}

// FindByIDWithFeed looks an entry up by id, with its feed and category.
func FindByIDWithFeed(ctx context.Context, e db.Executor, id int64) (*WithFeed, error) {
	return findWithFeed(ctx, e,
		`SELECT `+withFeedColumnsCount+` FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE e.id = $1`, id)
}

// FindByIDForUser is FindByIDWithFeed scoped to the owner; a missing entry and
// one owned by someone else both come back nil.
func FindByIDForUser(ctx context.Context, e db.Executor, userID, entryID int64) (*WithFeed, error) {
	return findWithFeed(ctx, e,
		`SELECT `+withFeedColumnsJoin+` FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
		 WHERE e.id = $1 AND c.user_id = $2`, entryID, userID)
}

// IsOwnedBy reports whether entryID belongs to userID.
func IsOwnedBy(ctx context.Context, e db.Executor, userID, entryID int64) (bool, error) {
	var id int64
	err := e.QueryRowContext(ctx,
		`SELECT e.id FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE e.id = $1 AND c.user_id = $2`, entryID, userID).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return false, nil
	}
	return err == nil, err
}

// FindByIDsWithFeed loads several entries at once, verifying ownership.
func FindByIDsWithFeed(ctx context.Context, e db.Executor, userID int64, ids []int64) ([]WithFeed, error) {
	if len(ids) == 0 {
		return nil, nil
	}
	rows, err := e.QueryContext(ctx,
		`SELECT `+withFeedColumnsCount+` FROM entry e
		 INNER JOIN feed f ON e.feed_id = f.id
		 INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1 AND e.id IN (`+placeholders(2, len(ids))+`)`,
		append([]any{userID}, int64Args(ids)...)...)
	return collect(rows, err, func(s scanner) (WithFeed, error) { return scanWithFeed(s) })
}

// sortColumn is the column a sort order pages by, over the "entry e" alias.
func sortColumn(order SortOrder) string {
	switch order {
	case SortReadAt:
		return "e.read_at"
	case SortStarredAt:
		return "e.starred_at"
	case SortSummarizedAt:
		return "es.created_at"
	}
	return publishedSortColumn
}

const publishedSortColumn = "COALESCE(e.published_at, e.created_at)"

// sortJoin is the join a sort order's column needs, scoped to user $1.
// SortSummarizedAt also drops entries without a summary.
func sortJoin(order SortOrder) string {
	if order == SortSummarizedAt {
		return " INNER JOIN entry_summary es ON es.entry_id = e.id AND es.user_id = $1"
	}
	return ""
}

// FetchSortTS is the sort column of an entry as the exact cursor text, or nil
// when the entry does not exist or has no value.
func FetchSortTS(ctx context.Context, e db.Executor, entryID int64, order SortOrder) (*string, error) {
	tsExpr := e.Dialect().CursorTS(sortColumn(order))
	// $1 is the entry id, so join on it alone: an entry has one owner.
	join := ""
	if order == SortSummarizedAt {
		join = " INNER JOIN entry_summary es ON es.entry_id = e.id"
	}
	var ts *string
	err := e.QueryRowContext(ctx,
		`SELECT `+tsExpr+` FROM entry e`+join+` WHERE e.id = $1`, entryID).Scan(sortTS{&ts})
	if errors.Is(err, sql.ErrNoRows) {
		return nil, nil
	}
	return ts, err
}
