package entry_test

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/migrate"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/search"
)

func TestMain(m *testing.M) {
	slog.SetDefault(slog.New(slog.DiscardHandler))
	os.Exit(m.Run())
}

func eachBackend(t *testing.T, fn func(t *testing.T, database *db.DB)) {
	t.Helper()
	t.Run("sqlite", func(t *testing.T) {
		fn(t, open(t, filepath.Join(t.TempDir(), "test.sqlite3"), config.SQLite))
	})
	url := os.Getenv("TEST_DATABASE_URL")
	if url == "" {
		return
	}
	t.Run("postgres", func(t *testing.T) { fn(t, open(t, url, config.Postgres)) })
}

func open(t *testing.T, url string, backend config.Backend) *db.DB {
	t.Helper()
	ctx := context.Background()
	database, err := db.Open(ctx, url, backend)
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

type fixture struct {
	database *db.DB
	userID   int64
	feedID   int64
}

func newFixture(t *testing.T, database *db.DB) fixture {
	t.Helper()
	ctx := context.Background()
	user, err := models.CreateUser(ctx, database, "alice", "hash", models.RoleUser)
	must(t, err)
	category, err := models.CreateCategory(ctx, database, user.ID, "News")
	must(t, err)
	feed, err := models.CreateFeed(ctx, database, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/feed"})
	must(t, err)
	return fixture{database: database, userID: user.ID, feedID: feed.ID}
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

func ptr[T any](v T) *T { return &v }

func (f fixture) add(t *testing.T, guid, title string, published time.Time) int64 {
	t.Helper()
	result, err := entry.UpsertEntry(context.Background(), f.database, &entry.Upsert{
		FeedID: f.feedID, GUID: guid, Title: &title,
		Content: ptr("<p>body of " + title + "</p>"), PublishedAt: &published,
	})
	must(t, err)
	if result.Outcome != entry.Inserted {
		t.Fatalf("outcome = %v, want Inserted", result.Outcome)
	}
	return result.ID
}

func day(n int) time.Time { return time.Date(2025, 1, n, 12, 0, 0, 0, time.UTC) }

func TestUpsertOutcomes(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		u := &entry.Upsert{FeedID: f.feedID, GUID: "g1", Title: ptr("one"), Content: ptr("<b>x</b>")}

		first, err := entry.UpsertEntry(ctx, database, u)
		must(t, err)
		again, err := entry.UpsertEntry(ctx, database, u)
		must(t, err)
		if first.Outcome != entry.Inserted || again.Outcome != entry.Unchanged || again.ID != first.ID {
			t.Fatalf("first=%+v again=%+v", first, again)
		}

		u.Title = ptr("changed")
		changed, err := entry.UpsertEntry(ctx, database, u)
		must(t, err)
		if changed.Outcome != entry.Updated {
			t.Fatalf("outcome = %v, want Updated", changed.Outcome)
		}

		must(t, entry.InsertTombstone(ctx, database, f.feedID, "gone"))
		skipped, err := entry.UpsertEntry(ctx, database, &entry.Upsert{FeedID: f.feedID, GUID: "gone"})
		must(t, err)
		if skipped.Outcome != entry.SkippedTombstoned {
			t.Fatalf("outcome = %v, want SkippedTombstoned", skipped.Outcome)
		}
	})
}

func TestListAndFilters(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		oldest := f.add(t, "a", "Alpha rust", day(1))
		middle := f.add(t, "b", "Beta go", day(2))
		newest := f.add(t, "c", "Gamma Rust", day(3))
		_, err := entry.MarkAsRead(ctx, database, middle)
		must(t, err)

		list := func(filter *entry.Filter) []int64 {
			rows, err := entry.ListByUser(ctx, database, entry.ListQuery{
				UserID: f.userID, Filter: filter, SortOrder: entry.SortPublishedAt, Limit: 10,
			})
			must(t, err)
			ids := make([]int64, len(rows))
			for i := range rows {
				ids[i] = rows[i].ID
			}
			return ids
		}
		assertIDs(t, "all", list(&entry.Filter{}), newest, middle, oldest)
		assertIDs(t, "unread", list(&entry.Filter{UnreadOnly: true}), newest, oldest)
		assertIDs(t, "read", list(&entry.Filter{ReadOnly: true}), middle)
		assertIDs(t, "search", list(&entry.Filter{Search: ptr("RUST")}), newest, oldest)

		node, err := search.Parse(`title:rust -status:read`)
		must(t, err)
		assertIDs(t, "query", list(&entry.Filter{Query: node}), newest, oldest)

		count, err := entry.CountByUser(ctx, database, f.userID, &entry.Filter{UnreadOnly: true})
		must(t, err)
		if count != 2 {
			t.Fatalf("count = %d, want 2", count)
		}
	})
}

func assertIDs(t *testing.T, name string, got []int64, want ...int64) {
	t.Helper()
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("%s = %v, want %v", name, got, want)
	}
}

func TestContinuationPaging(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		var ids []int64
		for i := 1; i <= 5; i++ {
			ids = append(ids, f.add(t, fmt.Sprint(i), fmt.Sprint("t", i), day(i)))
		}
		// Same timestamp as ids[4]: only the id tie-breaker orders them.
		ids = append(ids, f.add(t, "tie", "tie", day(5)))

		var seen []int64
		params := &entry.ContinuationParams{Limit: 3, SortOrder: entry.SortPublishedAt}
		for page := 0; page < 4; page++ {
			rows, err := entry.ListByUserWithContinuationTS(ctx, database, f.userID, &entry.Filter{}, params)
			must(t, err)
			if len(rows) == 0 {
				break
			}
			for _, row := range rows {
				seen = append(seen, row.ID)
			}
			next := entry.NextContinuation(rows, 2)
			// Fetch limit is pageSize+1; trim back to the kept rows.
			seen = seen[:len(seen)-(len(rows)-minInt(len(rows), 2))]
			if next == nil {
				break
			}
			cursor, ok := entry.ParseCursor(*next)
			if !ok {
				t.Fatalf("bad cursor %q", *next)
			}
			params.Continuation = cursor
		}
		assertIDs(t, "paged", seen, ids[5], ids[4], ids[3], ids[2], ids[1], ids[0])
	})
}

func minInt(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func TestBulkMarksAreOwnerScoped(t *testing.T) {
	eachBackend(t, func(t *testing.T, database *db.DB) {
		ctx := context.Background()
		f := newFixture(t, database)
		id := f.add(t, "a", "A", day(1))

		n, err := entry.MarkReadByIDs(ctx, database, f.userID+999, []int64{id})
		must(t, err)
		if n != 0 {
			t.Fatalf("other user marked %d rows", n)
		}
		n, err = entry.MarkReadByIDs(ctx, database, f.userID, []int64{id})
		must(t, err)
		if n != 1 {
			t.Fatalf("owner marked %d rows, want 1", n)
		}
		unread, err := entry.CountUnreadByUser(ctx, database, f.userID)
		must(t, err)
		if unread != 0 {
			t.Fatalf("unread = %d", unread)
		}
	})
}

func TestSQLiteIndexHintsPlan(t *testing.T) {
	database := open(t, filepath.Join(t.TempDir(), "plan.sqlite3"), config.SQLite)
	f := newFixture(t, database)
	f.add(t, "a", "A", day(1))
	ctx := context.Background()
	// Hint names must exist, or SQLite rejects the statement outright.
	for _, filter := range []*entry.Filter{{}, {UnreadOnly: true}, {StarredOnly: true}, {ReadOnly: true}} {
		_, err := entry.ListByUser(ctx, database, entry.ListQuery{
			UserID: f.userID, Filter: filter, SortOrder: entry.SortPublishedAt, Limit: 5,
		})
		if err != nil {
			t.Errorf("%+v: %v", filter, err)
		}
	}
	if _, err := entry.CountUnreadByFeed(ctx, database, f.userID); err != nil {
		t.Error(err)
	}
	if !strings.Contains(entry.EncodeCursor("2025-01-01 00:00:00", 7), "|7") {
		t.Error("cursor encoding")
	}
}
