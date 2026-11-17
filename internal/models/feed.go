package models

import (
	"context"
	"strconv"
	"strings"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/siphash"
)

// bucketCount is how many refresh buckets feeds are spread over, one per minute.
const bucketCount = 60

// Feed is a row of the feed table.
type Feed struct {
	ID              int64
	CategoryID      int64
	URL             string
	Title           *string
	Description     *string
	SiteURL         *string
	FeedUpdatedAt   *time.Time
	FetchedAt       *time.Time
	FetchError      *string
	ETag            *string
	LastModified    *string
	CustomUserAgent *string
	HTTP2Disabled   bool
	CustomReferrer  *string
	Bucket          *int64
	CreatedAt       time.Time
	UpdatedAt       time.Time
}

const feedColumns = "id, category_id, url, title, description, site_url, " +
	"feed_updated_at, fetched_at, fetch_error, etag, last_modified, " +
	"custom_user_agent, http2_disabled, custom_referrer, bucket, created_at, updated_at"

// feedColumnsAliased is feedColumns qualified with the alias "f" for joins.
var feedColumnsAliased = "f." + strings.ReplaceAll(feedColumns, ", ", ", f.")

func scanFeed(row scanner) (Feed, error) {
	var f Feed
	err := row.Scan(&f.ID, &f.CategoryID, &f.URL, &f.Title, &f.Description, &f.SiteURL,
		db.NullTimeDest(&f.FeedUpdatedAt), db.NullTimeDest(&f.FetchedAt), &f.FetchError, &f.ETag,
		&f.LastModified, &f.CustomUserAgent, &f.HTTP2Disabled, &f.CustomReferrer, &f.Bucket,
		db.TimeDest(&f.CreatedAt), db.TimeDest(&f.UpdatedAt))
	return f, err
}

// URLToBucket is the refresh bucket of a feed URL. It reproduces the Rust
// build's DefaultHasher so existing feed.bucket values stay valid.
func URLToBucket(url string) int64 {
	return int64(siphash.RustStrHash(url) % bucketCount)
}

// CreateFeedParams is the input to CreateFeed.
type CreateFeedParams struct {
	CategoryID      int64
	URL             string
	Title           *string
	Description     *string
	SiteURL         *string
	CustomUserAgent *string
	HTTP2Disabled   bool
	CustomReferrer  *string
}

// CreateFeed inserts a feed; a duplicate is ErrFeedExists.
func CreateFeed(ctx context.Context, e db.Executor, p CreateFeedParams) (*Feed, error) {
	row := e.QueryRowContext(ctx,
		`INSERT INTO feed (category_id, url, title, description, site_url,
		     custom_user_agent, http2_disabled, custom_referrer, bucket)
		 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING `+feedColumns,
		p.CategoryID, p.URL, p.Title, p.Description, p.SiteURL,
		p.CustomUserAgent, p.HTTP2Disabled, p.CustomReferrer, URLToBucket(p.URL))
	feed, err := scanFeed(row)
	if db.IsUniqueViolation(err) {
		return nil, ErrFeedExists
	}
	if err != nil {
		return nil, err
	}
	return &feed, nil
}

func findFeed(ctx context.Context, e db.Executor, query string, args ...any) (*Feed, error) {
	feed, err := scanFeed(e.QueryRowContext(ctx, query, args...))
	return orNil(&feed, err)
}

// FindFeedByID looks a feed up by id.
func FindFeedByID(ctx context.Context, e db.Executor, id int64) (*Feed, error) {
	return findFeed(ctx, e, `SELECT `+feedColumns+` FROM feed WHERE id = $1`, id)
}

// FindFeedByURLAndCategory looks a feed up by URL within one category.
func FindFeedByURLAndCategory(ctx context.Context, e db.Executor, url string, categoryID int64) (*Feed, error) {
	return findFeed(ctx, e, `SELECT `+feedColumns+` FROM feed WHERE url = $1 AND category_id = $2`, url, categoryID)
}

// FindFeedByURLForUser finds a feed by URL across all of a user's categories.
func FindFeedByURLForUser(ctx context.Context, e db.Executor, url string, userID int64) (*Feed, error) {
	return findFeed(ctx, e,
		`SELECT `+feedColumnsAliased+` FROM feed f INNER JOIN category c ON f.category_id = c.id
		 WHERE f.url = $1 AND c.user_id = $2`, url, userID)
}

// ListFeedsByUser returns every feed of a user, ordered by title.
func ListFeedsByUser(ctx context.Context, e db.Executor, userID int64) ([]Feed, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+feedColumnsAliased+` FROM feed f INNER JOIN category c ON f.category_id = c.id
		 WHERE c.user_id = $1 ORDER BY f.title ASC`, userID)
	return collect(rows, err, scanFeed)
}

// CountFeedsByUser counts a user's feeds across all categories.
func CountFeedsByUser(ctx context.Context, e db.Executor, userID int64) (int64, error) {
	return countScalar(ctx, e,
		`SELECT COUNT(*) FROM feed f INNER JOIN category c ON f.category_id = c.id WHERE c.user_id = $1`, userID)
}

// ListFeedsByCategory returns a category's feeds, ordered by title.
func ListFeedsByCategory(ctx context.Context, e db.Executor, categoryID int64) ([]Feed, error) {
	rows, err := e.QueryContext(ctx,
		`SELECT `+feedColumns+` FROM feed WHERE category_id = $1 ORDER BY title ASC`, categoryID)
	return collect(rows, err, scanFeed)
}

// ListFeedsByBucket returns the feeds due in one refresh bucket.
func ListFeedsByBucket(ctx context.Context, e db.Executor, bucket int64) ([]Feed, error) {
	rows, err := e.QueryContext(ctx, `SELECT `+feedColumns+` FROM feed WHERE bucket = $1`, bucket)
	return collect(rows, err, scanFeed)
}

// UpdateFeedParams is the input to UpdateFeed. CategoryID is the current
// category (the ownership check), NewCategoryID where the feed moves to.
type UpdateFeedParams struct {
	ID              int64
	CategoryID      int64
	NewCategoryID   int64
	URL             string
	Title           *string
	Description     *string
	SiteURL         *string
	CustomUserAgent *string
	HTTP2Disabled   bool
	CustomReferrer  *string
}

// UpdateFeed rewrites a feed and recomputes its bucket; no matching row is
// ErrFeedNotFound and a duplicate is ErrFeedExists.
func UpdateFeed(ctx context.Context, e db.Executor, p UpdateFeedParams) (*Feed, error) {
	row := e.QueryRowContext(ctx,
		`UPDATE feed SET category_id = $1, url = $2, title = $3, description = $4,
		     site_url = $5, custom_user_agent = $6, http2_disabled = $7, custom_referrer = $8,
		     bucket = $9, updated_at = $10 WHERE id = $11 AND category_id = $12
		 RETURNING `+feedColumns,
		p.NewCategoryID, p.URL, p.Title, p.Description, p.SiteURL, p.CustomUserAgent,
		p.HTTP2Disabled, p.CustomReferrer, URLToBucket(p.URL), stamp(time.Now()), p.ID, p.CategoryID)
	feed, err := scanFeed(row)
	if db.IsUniqueViolation(err) {
		return nil, ErrFeedExists
	}
	found, err := orNil(&feed, err)
	if err != nil {
		return nil, err
	}
	if found == nil {
		return nil, ErrFeedNotFound
	}
	return found, nil
}

// DeleteFeed removes a feed and its icon.
func DeleteFeed(ctx context.Context, e db.Executor, id, categoryID int64) error {
	affected, err := rowsAffected(e.ExecContext(ctx, "DELETE FROM feed WHERE id = $1 AND category_id = $2", id, categoryID))
	if err != nil {
		return err
	}
	if affected == 0 {
		return ErrFeedNotFound
	}
	return DeleteImageByEntity(ctx, e, EntityFeed, id)
}

// FetchResult is the outcome of one fetch attempt.
type FetchResult struct {
	ID            int64
	FetchedAt     time.Time
	FetchError    *string
	ETag          *string
	LastModified  *string
	FeedUpdatedAt *time.Time
}

// UpdateFetchResult records a fetch attempt. A nil FeedUpdatedAt keeps the
// stored value. Pass a transaction so readers never see a half-applied sync.
func UpdateFetchResult(ctx context.Context, e db.Executor, r FetchResult) error {
	_, err := e.ExecContext(ctx,
		`UPDATE feed SET fetched_at = $1, fetch_error = $2, etag = $3, last_modified = $4,
		     feed_updated_at = COALESCE($5, feed_updated_at), updated_at = $6 WHERE id = $7`,
		stamp(r.FetchedAt), r.FetchError, r.ETag, r.LastModified, stampPtr(r.FeedUpdatedAt),
		stamp(time.Now()), r.ID)
	return err
}

// OwnerUserIDsForFeeds returns the distinct owners of the given feeds.
func OwnerUserIDsForFeeds(ctx context.Context, e db.Executor, feedIDs []int64) ([]int64, error) {
	if len(feedIDs) == 0 {
		return nil, nil
	}
	placeholders := make([]string, len(feedIDs))
	args := make([]any, len(feedIDs))
	for i, id := range feedIDs {
		placeholders[i] = "$" + strconv.Itoa(i+1)
		args[i] = id
	}
	rows, err := e.QueryContext(ctx,
		`SELECT DISTINCT c.user_id FROM feed f JOIN category c ON c.id = f.category_id
		 WHERE f.id IN (`+strings.Join(placeholders, ", ")+`)`, args...)
	return collect(rows, err, func(row scanner) (int64, error) {
		var id int64
		return id, row.Scan(&id)
	})
}
