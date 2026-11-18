package server

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Port of benches/entry_pages.rs through the real handler stack. The Rust
// bench counts SQLite page-cache misses with sqlite3_db_status, which modernc
// does not expose (plan section 7, decision 5, option a): this reports ns/op
// and allocs/op per route instead. Run with
//
//	go test -run '^$' -bench BenchmarkEntryPages -benchmem -count 5 ./internal/server
//
// RDRS_BENCH_ENTRIES (default 5000) sizes the seeded library.

type benchLibrary struct {
	app                         *itestApp
	feedID, categoryID, entryID int64
}

func seedBenchLibrary(b *testing.B, entries int) benchLibrary {
	b.Helper()
	app := defaultITestApp(b)
	user := app.SeedAccount("bench", fixturePassword, models.RoleAdmin)
	ctx := context.Background()
	library := benchLibrary{app: app}
	var feeds []int64
	for c := range 4 {
		category, err := models.CreateCategory(ctx, app.DB, user.ID, fmt.Sprintf("Category %d", c))
		if err != nil {
			b.Fatal(err)
		}
		for f := range 5 {
			title := fmt.Sprintf("Feed %d-%d", c, f)
			feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
				CategoryID: category.ID, URL: fmt.Sprintf("https://feed%d-%d.example.com/rss", c, f), Title: &title,
			})
			if err != nil {
				b.Fatal(err)
			}
			if len(feeds) == 0 {
				library.feedID, library.categoryID = feed.ID, category.ID
			}
			feeds = append(feeds, feed.ID)
		}
	}
	content := strings.Repeat("<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>", 12)
	summary, now := "A short summary.", time.Now()
	var read []int64
	for i := range entries {
		title, link, published := fmt.Sprintf("Entry %d", i), fmt.Sprintf("https://example.com/%d", i), now.Add(-time.Duration(i)*time.Minute)
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feeds[i%len(feeds)], GUID: fmt.Sprintf("guid-%d", i), Title: &title, Link: &link,
			Content: &content, Summary: &summary, PublishedAt: &published,
		})
		if err != nil {
			b.Fatal(err)
		}
		// A read entry: opening an unread one spawns a detached mark-read write.
		if i == entries/2-entries/2%3 {
			library.entryID = result.ID
		}
		if i%3 == 0 {
			read = append(read, result.ID)
		}
		if i%10 == 0 {
			if _, err := entry.SetStarredForUser(ctx, app.DB, user.ID, result.ID, true); err != nil {
				b.Fatal(err)
			}
		}
		if i%20 == 0 {
			if _, err := models.UpsertPendingSummary(ctx, app.DB, user.ID, result.ID); err != nil {
				b.Fatal(err)
			}
			if _, err := models.SetSummaryCompleted(ctx, app.DB, user.ID, result.ID, "Summary."); err != nil {
				b.Fatal(err)
			}
		}
	}
	if _, err := entry.MarkReadByIDs(ctx, app.DB, user.ID, read); err != nil {
		b.Fatal(err)
	}
	if err := app.DB.Optimize(ctx); err != nil {
		b.Fatal(err)
	}
	app.Login("bench")
	return library
}

func BenchmarkEntryPages(b *testing.B) {
	library := seedBenchLibrary(b, benchEntryCount())
	feed, category, mid := library.feedID, library.categoryID, library.entryID
	routes := []struct{ name, path string }{
		{"unread", "/"},
		{"entries", "/entries"},
		{"read", "/entries/read"},
		{"starred", "/entries/starred"},
		{"summarized", "/entries/summarized"},
		{"feed", fmt.Sprintf("/feeds/%d/entries", feed)},
		{"category", fmt.Sprintf("/categories/%d/entries", category)},
		{"search", "/search?q=Entry"},
		{"search content", "/search?q=adipiscing"},
		{"feeds", "/feeds"},
		{"categories", "/categories"},
		{"statistics", "/statistics"},
		{"pane fragment", fmt.Sprintf("/entries/%d/fragment", mid)},
		{"sidebar api", "/api/sidebar"},
		{"neighbors", fmt.Sprintf("/api/entries/%d/neighbors", mid)},
		{"position", fmt.Sprintf("/api/entries/%d/position", mid)},
	}
	for _, route := range routes {
		b.Run(route.name, func(b *testing.B) {
			if library.app.Get(route.path).Code != http.StatusOK {
				b.Fatalf("GET %s is not 200", route.path)
			}
			b.ReportAllocs()
			for b.Loop() {
				library.app.Get(route.path)
			}
		})
	}
}

func benchEntryCount() int {
	count := 5000
	if raw := os.Getenv("RDRS_BENCH_ENTRIES"); raw != "" {
		if parsed, err := strconv.Atoi(raw); err == nil && parsed > 0 {
			count = parsed
		}
	}
	return count
}
