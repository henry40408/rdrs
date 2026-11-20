package entry

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/textutil"
)

// Outcome says what Upsert did.
type Outcome int

// The upsert outcomes.
const (
	Inserted Outcome = iota
	Updated
	// Unchanged means every mutable column already matched; no write ran.
	Unchanged
	// SkippedTombstoned means the guid is tombstoned, so nothing was stored.
	SkippedTombstoned
)

// UpsertResult is the outcome and, unless tombstoned, the entry id.
type UpsertResult struct {
	Outcome Outcome
	ID      int64
}

// Upsert is one entry as fetched from a feed.
type Upsert struct {
	FeedID      int64
	GUID        string
	Title       *string
	Link        *string
	Content     *string
	Summary     *string
	Author      *string
	PublishedAt *time.Time
}

// upsertSelectSQL reads only the id: UNIQUE(feed_id, guid) makes it
// index-only, and reading the comparable columns costs more than the write it
// would save, so the no-op check lives in the UPDATE's WHERE.
const upsertSelectSQL = "SELECT id FROM entry WHERE guid = $1 AND feed_id = $2"

// upsertInsertSQL is guarded by the tombstone table: no row back means the
// guid is tombstoned. On PostgreSQL the SELECT-list parameters would resolve
// to text, so the timestamp is cast explicitly.
const upsertInsertSQL = `INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at, content_text)
	SELECT $1, $2, $3, $4, $5, $6, $7, %s, $9
	WHERE NOT EXISTS (SELECT 1 FROM entry_tombstone WHERE feed_id = $1 AND guid = $2)
	RETURNING id`

// upsertUpdateSQL only writes when a column differs, so a re-served identical
// article costs no write and zero rows affected means Unchanged. The NULL-safe
// inequality forks per dialect. content_text derives from content, so it needs
// no comparison.
func upsertUpdateSQL(d db.Dialect) string {
	return "UPDATE entry SET title = $1, link = $2, content = $3, summary = $4, author = $5, " +
		"content_text = $6, updated_at = datetime('now') WHERE id = $7 AND (" +
		d.NullSafeNe("title", "$1") + " OR " + d.NullSafeNe("link", "$2") + " OR " +
		d.NullSafeNe("content", "$3") + " OR " + d.NullSafeNe("summary", "$4") + " OR " +
		d.NullSafeNe("author", "$5") + ")"
}

// UpsertEntry stores an entry without re-reading the row (the feed-sync hot
// path). The insert is tombstone-guarded.
func UpsertEntry(ctx context.Context, e db.Executor, u *Upsert) (UpsertResult, error) {
	var contentText *string
	if u.Content != nil {
		text := textutil.StripToSearchText(*u.Content)
		contentText = &text
	}

	var id int64
	err := e.QueryRowContext(ctx, upsertSelectSQL, u.GUID, u.FeedID).Scan(&id)
	if err == nil {
		return updateExisting(ctx, e, u, id, contentText)
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return UpsertResult{}, err
	}
	return insertNew(ctx, e, u, contentText)
}

func updateExisting(ctx context.Context, e db.Executor, u *Upsert, id int64, contentText *string) (UpsertResult, error) {
	rows, err := exec(ctx, e, upsertUpdateSQL(e.Dialect()),
		u.Title, u.Link, u.Content, u.Summary, u.Author, contentText, id)
	if err != nil {
		return UpsertResult{}, err
	}
	if rows > 0 {
		return UpsertResult{Outcome: Updated, ID: id}, nil
	}
	return UpsertResult{Outcome: Unchanged, ID: id}, nil
}

func insertNew(ctx context.Context, e db.Executor, u *Upsert, contentText *string) (UpsertResult, error) {
	// Seconds-truncated space-format text, the form the cursor compares as.
	var publishedAt *string
	if u.PublishedAt != nil {
		text := db.FormatSpace(*u.PublishedAt)
		publishedAt = &text
	}
	placeholder := "$8"
	if e.Dialect() == db.DialectPostgres {
		placeholder = "CAST($8 AS TIMESTAMPTZ)"
	}
	var id int64
	err := e.QueryRowContext(ctx, fmt.Sprintf(upsertInsertSQL, placeholder),
		u.FeedID, u.GUID, u.Title, u.Link, u.Content, u.Summary, u.Author, publishedAt, contentText).Scan(&id)
	if errors.Is(err, sql.ErrNoRows) {
		return UpsertResult{Outcome: SkippedTombstoned}, nil
	}
	if err != nil {
		return UpsertResult{}, err
	}
	return UpsertResult{Outcome: Inserted, ID: id}, nil
}
