package feedsync

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/url"
	"time"

	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/feedparse"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/models"
)

// Syncer refreshes feeds.
type Syncer struct {
	DB               *db.DB
	Fetcher          *fetch.Fetcher
	DefaultUserAgent string
	// FetchIcon is nil until the icon package is wired; then no icon is fetched.
	FetchIcon IconFetcher
	Retry     fetch.RetryConfig
}

// Result is what a successful refresh changed.
type Result struct {
	NewEntries     int64
	UpdatedEntries int64
}

func (s *Syncer) recordError(ctx context.Context, feedID int64, message string) error {
	return models.UpdateFetchResult(ctx, s.DB, models.FetchResult{
		ID: feedID, FetchedAt: time.Now(), FetchError: &message,
	})
}

// fail stores the error text on the feed, best effort, and returns the error.
func (s *Syncer) fail(ctx context.Context, feedID int64, kind ErrorKind, detail string) error {
	_ = s.recordError(ctx, feedID, detail)
	return &Error{Kind: kind, Detail: detail}
}

func userAgentOf(feed *models.Feed, fallback string) string {
	if feed.CustomUserAgent != nil {
		return *feed.CustomUserAgent
	}
	return fallback
}

// newRequest is a conditional GET: the validators of the last fetch let the
// server answer 304.
func newRequest(ctx context.Context, feed *models.Feed, userAgent string) (*http.Request, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, feed.URL, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", userAgent)
	// A stored validator that is not valid now is skipped, as in Rust.
	if feed.ETag != nil && visibleASCII(*feed.ETag) {
		req.Header.Set("If-None-Match", *feed.ETag)
	}
	if feed.LastModified != nil && visibleASCII(*feed.LastModified) {
		req.Header.Set("If-Modified-Since", *feed.LastModified)
	}
	return req, nil
}

func statusText(code int) string {
	return fmt.Sprintf("HTTP %d %s", code, fetch.StatusReason(code))
}

// headerPtr is nil for an absent header and for a value that is not visible
// ASCII, as Rust's `to_str().ok()` gives.
func headerPtr(h http.Header, name string) *string {
	value := h.Get(name)
	if value == "" || !visibleASCII(value) {
		return nil
	}
	return &value
}

func visibleASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if (s[i] < 0x20 && s[i] != '	') || s[i] > 0x7e {
			return false
		}
	}
	return true
}

func (s *Syncer) send(ctx context.Context, feed *models.Feed) (*http.Response, error) {
	build := func(ctx context.Context) (*http.Request, error) {
		return newRequest(ctx, feed, userAgentOf(feed, s.DefaultUserAgent))
	}
	return fetch.SendWithRetry(ctx, fetch.SendParams{
		Client: s.Fetcher.Client(feed.HTTP2Disabled), Config: s.Retry, Build: build,
	})
}

// Refresh fetches one feed and stores what changed.
func (s *Syncer) Refresh(ctx context.Context, feedID int64) (Result, error) {
	feed, err := models.FindFeedByID(ctx, s.DB, feedID)
	if err != nil {
		return Result{}, err
	}
	if feed == nil {
		return Result{}, models.ErrFeedNotFound
	}
	// Checked on every refresh: older rows and OPML imports skip the
	// write-time guard.
	parsedURL, err := url.Parse(feed.URL)
	if err != nil {
		return Result{}, &Error{Kind: KindInvalidURL}
	}
	if err := s.Fetcher.Validate(parsedURL); err != nil {
		_ = s.recordError(ctx, feedID, err.Error())
		return Result{}, &Error{Kind: KindInvalidURL}
	}
	resp, err := s.send(ctx, feed)
	if err != nil {
		return Result{}, s.fail(ctx, feedID, KindFetch, err.Error())
	}
	defer func() { _ = resp.Body.Close() }()
	return s.handle(ctx, feed, resp)
}

func (s *Syncer) handle(ctx context.Context, feed *models.Feed, resp *http.Response) (Result, error) {
	if resp.StatusCode == http.StatusNotModified {
		slog.Debug("feed not modified (304)", "event", "feed.not_modified", "feed_id", feed.ID)
		err := models.UpdateFetchResult(ctx, s.DB, models.FetchResult{
			ID: feed.ID, FetchedAt: time.Now(), ETag: feed.ETag, LastModified: feed.LastModified,
		})
		return Result{}, err
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		message := statusText(resp.StatusCode)
		if err := s.recordError(ctx, feed.ID, message); err != nil {
			return Result{}, err
		}
		return Result{}, &Error{Kind: KindFetch, Detail: message}
	}
	parsed, err := s.readFeed(ctx, feed.ID, resp)
	if err != nil {
		return Result{}, err
	}
	return s.store(ctx, feed, parsed, validators{
		ETag: headerPtr(resp.Header, "ETag"), LastModified: headerPtr(resp.Header, "Last-Modified"),
	})
}

// maxFeedBytes bounds the body: parsing builds a tree that takes many times
// the body's size in memory, and Rust had no bound at all.
const maxFeedBytes = 8 << 20

func (s *Syncer) readFeed(ctx context.Context, feedID int64, resp *http.Response) (*feedparse.Feed, error) {
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxFeedBytes+1))
	if err != nil {
		return nil, s.fail(ctx, feedID, KindFetch, err.Error())
	}
	if len(body) > maxFeedBytes {
		return nil, s.fail(ctx, feedID, KindFetch, "response body is larger than 8 MiB")
	}
	decoded, err := feedparse.Decode(body, resp.Header.Get("Content-Type"))
	if err != nil {
		return nil, s.fail(ctx, feedID, KindParse, err.Error())
	}
	parsed, err := feedparse.Parse(decoded)
	if err != nil {
		return nil, s.fail(ctx, feedID, KindParse, err.Error())
	}
	return parsed, nil
}

func (s *Syncer) store(ctx context.Context, feed *models.Feed, parsed *feedparse.Feed, v validators) (Result, error) {
	if err := s.refreshIcon(ctx, feed, parsed); err != nil {
		return Result{}, err
	}
	counts, err := apply(ctx, s.DB, parsed, batch{feedID: feed.ID, feedTimestamp: parsed.Timestamp(), validators: v})
	if err != nil {
		return Result{}, err
	}
	slog.Info("feed refreshed", "event", "feed.refreshed", "feed_id", feed.ID, "new_entries", counts.New,
		"updated_entries", counts.Updated, "unchanged_entries", counts.Unchanged, "skipped_entries", counts.Skipped)
	return Result{NewEntries: counts.New, UpdatedEntries: counts.Updated}, nil
}
