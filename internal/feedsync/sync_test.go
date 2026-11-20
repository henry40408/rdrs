package feedsync

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/fetch"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
)

const rssBody = `<?xml version="1.0"?><rss version="2.0"><channel><title>T</title>
<lastBuildDate>Mon, 04 Mar 2024 08:00:00 GMT</lastBuildDate>
<item><guid>a</guid><title>A</title><link>https://x.example/a</link><description>da</description><pubDate>Tue, 05 Mar 2024 10:00:00 GMT</pubDate></item>
<item><guid>b</guid><title>B</title><description>db</description></item></channel></rss>`

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

type fixture struct {
	syncer *Syncer
	feed   *models.Feed
}

func openDB(t *testing.T) *db.DB {
	t.Helper()
	return openBackend(t, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite)
}

// eachBackend runs fn against SQLite and, when TEST_DATABASE_URL is set,
// against a freshly migrated PostgreSQL schema.
func eachBackend(t *testing.T, fn func(t *testing.T, database *db.DB)) {
	t.Helper()
	t.Run("sqlite", func(t *testing.T) { fn(t, openDB(t)) })
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		return
	}
	t.Run("postgres", func(t *testing.T) { fn(t, openBackend(t, url, config.Postgres)) })
}

func openBackend(t *testing.T, target string, backend config.Backend) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, target, backend)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = database.Shutdown(ctx) })
	if backend == config.Postgres {
		if _, err := database.ExecContext(ctx, "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"); err != nil {
			t.Fatal(err)
		}
	}
	if err := migrate.Run(ctx, database); err != nil {
		t.Fatal(err)
	}
	return database
}

func newFixture(t *testing.T, handler http.HandlerFunc) *fixture {
	t.Helper()
	return newFixtureOn(t, openDB(t), handler)
}

func newFixtureOn(t *testing.T, database *db.DB, handler http.HandlerFunc) *fixture {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	policy, err := fetch.ParsePolicy("127.0.0.1")
	if err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	user, err := models.CreateUser(ctx, database, "u", "hash", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	category, err := models.CreateCategory(ctx, database, user.ID, "c")
	if err != nil {
		t.Fatal(err)
	}
	feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: server.URL})
	if err != nil {
		t.Fatal(err)
	}
	syncer := &Syncer{DB: database, Fetcher: fetch.NewFetcher(policy), DefaultUserAgent: "default-ua"}
	return &fixture{syncer: syncer, feed: feed}
}

func (f *fixture) reload(t *testing.T) *models.Feed {
	t.Helper()
	feed, err := models.FindFeedByID(context.Background(), f.syncer.DB, f.feed.ID)
	if err != nil || feed == nil {
		t.Fatalf("feed: %v, %v", feed, err)
	}
	return feed
}

func serveFeed(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("ETag", `"v1"`)
	w.Header().Set("Last-Modified", "Wed, 06 Mar 2024 12:00:00 GMT")
	w.Header().Set("Content-Type", "application/rss+xml")
	_, _ = w.Write([]byte(rssBody))
}

func TestRefreshStoresEntriesAndValidators(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixtureOn(t, database, serveFeed)
		result, err := f.syncer.Refresh(context.Background(), f.feed.ID)
		if err != nil || result.NewEntries != 2 || result.UpdatedEntries != 0 {
			t.Fatalf("result = %+v, %v", result, err)
		}
		feed := f.reload(t)
		if feed.ETag == nil || *feed.ETag != `"v1"` || feed.FetchError != nil || feed.FetchedAt == nil {
			t.Errorf("feed = %+v", feed)
		}
		want := time.Date(2024, 3, 6, 12, 0, 0, 0, time.UTC)
		if feed.FeedUpdatedAt == nil || !feed.FeedUpdatedAt.Equal(want) {
			t.Errorf("feed_updated_at = %v, want the newest signal %v", feed.FeedUpdatedAt, want)
		}
		again, err := f.syncer.Refresh(context.Background(), f.feed.ID)
		if err != nil || again.NewEntries != 0 || again.UpdatedEntries != 0 {
			t.Errorf("second refresh = %+v, %v", again, err)
		}
	})
}

func TestRefreshSendsConditionalHeadersAndKeepsThemOn304(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		var seen atomic.Pointer[http.Header]
		f := newFixtureOn(t, database, func(w http.ResponseWriter, r *http.Request) {
			if r.Header.Get("If-None-Match") == `"v1"` {
				seen.Store(&r.Header)
				w.WriteHeader(http.StatusNotModified)
				return
			}
			serveFeed(w, r)
		})
		ctx := context.Background()
		if _, err := f.syncer.Refresh(ctx, f.feed.ID); err != nil {
			t.Fatal(err)
		}
		result, err := f.syncer.Refresh(ctx, f.feed.ID)
		if err != nil || result != (Result{}) {
			t.Fatalf("304 refresh = %+v, %v", result, err)
		}
		header := seen.Load()
		if header == nil || header.Get("If-Modified-Since") != "Wed, 06 Mar 2024 12:00:00 GMT" || header.Get("User-Agent") != "default-ua" {
			t.Fatalf("headers = %v", header)
		}
		if feed := f.reload(t); feed.ETag == nil || *feed.ETag != `"v1"` {
			t.Errorf("etag lost: %+v", feed)
		}
	})
}

func TestRefreshUsesTheFeedUserAgent(t *testing.T) {
	var agent atomic.Value
	f := newFixture(t, func(w http.ResponseWriter, r *http.Request) {
		agent.Store(r.Header.Get("User-Agent"))
		serveFeed(w, r)
	})
	custom := "custom-ua"
	f.feed.CustomUserAgent = &custom
	_, err := models.UpdateFeed(context.Background(), f.syncer.DB, models.UpdateFeedParams{
		ID: f.feed.ID, CategoryID: f.feed.CategoryID, NewCategoryID: f.feed.CategoryID, URL: f.feed.URL, CustomUserAgent: &custom,
	})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil {
		t.Fatal(err)
	}
	if agent.Load() != "custom-ua" {
		t.Errorf("user agent = %v", agent.Load())
	}
}

func TestRefreshRecordsAnHTTPError(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, _ *http.Request) { http.NotFound(w, nil) })
	_, err := f.syncer.Refresh(context.Background(), f.feed.ID)
	if err == nil || err.Error() != "Failed to fetch: HTTP 404 Not Found" {
		t.Fatalf("error = %v", err)
	}
	if feed := f.reload(t); feed.FetchError == nil || *feed.FetchError != "HTTP 404 Not Found" {
		t.Errorf("fetch_error = %v", feed.FetchError)
	}
}

func TestRefreshRecordsAParseError(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("not a feed")) })
	_, err := f.syncer.Refresh(context.Background(), f.feed.ID)
	var syncErr *Error
	if !errors.As(err, &syncErr) || syncErr.Kind != KindParse {
		t.Fatalf("error = %v", err)
	}
	if feed := f.reload(t); feed.FetchError == nil {
		t.Error("parse error was not recorded")
	}
}

func TestRefreshRechecksTheURLPolicy(t *testing.T) {
	f := newFixture(t, serveFeed)
	f.syncer.Fetcher = fetch.NewFetcher(fetch.Policy{})
	_, err := f.syncer.Refresh(context.Background(), f.feed.ID)
	var syncErr *Error
	if !errors.As(err, &syncErr) || syncErr.Kind != KindInvalidURL || err.Error() != "Invalid URL" {
		t.Fatalf("error = %v", err)
	}
	if feed := f.reload(t); feed.FetchError == nil || !strings.Contains(*feed.FetchError, fetch.ErrBlockedHost.Error()) {
		t.Errorf("fetch_error = %v", deref(feed.FetchError))
	}
}

func TestRefreshUnknownFeed(t *testing.T) {
	f := newFixture(t, serveFeed)
	if _, err := f.syncer.Refresh(context.Background(), 9999); !errors.Is(err, models.ErrFeedNotFound) {
		t.Errorf("error = %v", err)
	}
}

func TestIconIsFetchedOnlyWhenStale(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixtureOn(t, database, serveFeed)
		var calls atomic.Int32
		f.syncer.FetchIcon = func(_ context.Context, c IconCandidates) (*FetchedIcon, error) {
			calls.Add(1)
			if c.UserAgent != "default-ua" {
				t.Errorf("user agent = %q", c.UserAgent)
			}
			return &FetchedIcon{Data: []byte("png"), ContentType: "image/png", SourceURL: "https://x.example/i.png"}, nil
		}
		ctx := context.Background()
		for range 2 {
			if _, err := f.syncer.Refresh(ctx, f.feed.ID); err != nil {
				t.Fatal(err)
			}
		}
		image, err := models.FindImage(ctx, f.syncer.DB, models.EntityFeed, f.feed.ID)
		if err != nil || image == nil || string(image.Data) != "png" || calls.Load() != 1 {
			t.Fatalf("image = %+v, %v, calls = %d", image, err, calls.Load())
		}
	})
}

func TestIconFailureDoesNotFailTheSync(t *testing.T) {
	f := newFixture(t, serveFeed)
	f.syncer.FetchIcon = func(context.Context, IconCandidates) (*FetchedIcon, error) {
		return nil, errors.New("boom")
	}
	if result, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil || result.NewEntries != 2 {
		t.Fatalf("result = %+v, %v", result, err)
	}
}

// The icon host hangs until the sync's own deadline passes. The entries are
// written before the icon is tried, so the sync still succeeds.
func TestAHangingIconHostDoesNotFailTheSync(t *testing.T) {
	f := newFixture(t, serveFeed)
	f.syncer.FetchIcon = func(ctx context.Context, _ IconCandidates) (*FetchedIcon, error) {
		<-ctx.Done()
		return nil, ctx.Err()
	}
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()

	result, err := f.syncer.Refresh(ctx, f.feed.ID)

	if err != nil || result.NewEntries != 2 {
		t.Fatalf("result = %+v, %v", result, err)
	}
}

func TestTheIconRefreshHasItsOwnTimeout(t *testing.T) {
	f := newFixture(t, serveFeed)
	f.syncer.IconTimeout = 20 * time.Millisecond
	f.syncer.FetchIcon = func(ctx context.Context, _ IconCandidates) (*FetchedIcon, error) {
		<-ctx.Done()
		return nil, ctx.Err()
	}
	started := time.Now()

	result, err := f.syncer.Refresh(context.Background(), f.feed.ID)

	if err != nil || result.NewEntries != 2 {
		t.Fatalf("result = %+v, %v", result, err)
	}
	if elapsed := time.Since(started); elapsed > 5*time.Second {
		t.Errorf("sync took %v; the icon timeout did not apply", elapsed)
	}
}

func TestLatestPicksTheNewestNonNil(t *testing.T) {
	early, late := time.Unix(100, 0), time.Unix(200, 0)
	if got := latest(nil, &early, &late, nil); got != &late {
		t.Errorf("got %v", got)
	}
	if latest(nil, nil) != nil {
		t.Error("expected nil")
	}
}

func TestRefreshBucketReportsEachFeed(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		f := newFixtureOn(t, database, serveFeed)
		ctx := context.Background()
		broken, err := models.CreateFeed(ctx, f.syncer.DB, models.CreateFeedParams{CategoryID: f.feed.CategoryID, URL: "http://127.0.0.1:1/x"})
		if err != nil {
			t.Fatal(err)
		}
		f.syncer.Retry = fetch.RetryConfig{}
		if _, err := f.syncer.DB.ExecContext(ctx, "UPDATE feed SET bucket = $1 WHERE id = $2", *f.feed.Bucket, broken.ID); err != nil {
			t.Fatal(err)
		}
		results := f.syncer.RefreshBucket(ctx, *f.feed.Bucket)
		if len(results) != 2 {
			t.Fatalf("results = %+v", results)
		}
		failed := 0
		for _, r := range results {
			if r.Err != "" {
				failed++
			}
		}
		if failed != 1 {
			t.Errorf("results = %+v", results)
		}
	})
}

func TestRefreshRejectsAnOversizedBody(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(strings.Repeat("x", maxFeedBytes+1)))
	})
	_, err := f.syncer.Refresh(context.Background(), f.feed.ID)
	var syncErr *Error
	if !errors.As(err, &syncErr) || syncErr.Kind != KindFetch || !strings.Contains(err.Error(), "8 MiB") {
		t.Fatalf("error = %v", err)
	}
	if feed := f.reload(t); feed.FetchError == nil {
		t.Error("oversized body was not recorded")
	}
}

func TestStatusTextUsesTheHTTPCratePhrases(t *testing.T) {
	want := map[int]string{
		404: "HTTP 404 Not Found", 413: "HTTP 413 Payload Too Large", 414: "HTTP 414 URI Too Long",
		416: "HTTP 416 Range Not Satisfiable", 599: "HTTP 599 <unknown status code>",
	}
	for code, text := range want {
		if got := statusText(code); got != text {
			t.Errorf("statusText(%d) = %q, want %q", code, got, text)
		}
	}
}

func TestNonASCIIValidatorsAreNotStored(t *testing.T) {
	f := newFixture(t, func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("ETag", "\"caf\xe9\"")
		serveFeedBody(w, r)
	})
	if _, err := f.syncer.Refresh(context.Background(), f.feed.ID); err != nil {
		t.Fatal(err)
	}
	if feed := f.reload(t); feed.ETag != nil {
		t.Errorf("etag = %q, want none", *feed.ETag)
	}
	header := make(http.Header)
	header.Set("ETag", `"ok"`)
	if got := headerPtr(header, "ETag"); got == nil || *got != `"ok"` {
		t.Errorf("headerPtr = %v", got)
	}
}

func serveFeedBody(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "application/rss+xml")
	_, _ = w.Write([]byte(rssBody))
}

func deref(s *string) string {
	if s == nil {
		return "<nil>"
	}
	return *s
}
