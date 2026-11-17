package server

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/cache"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Port of tests/statistics_test.rs: the SSR /statistics page and the shared
// /api/sidebar payload.

// statisticsApp is app_signed_in_as: both fixture accounts, one signed in.
func statisticsApp(t *testing.T, username string) (app *itestApp, adminID, userID int64) {
	t.Helper()
	app = defaultITestApp(t)
	adminID, userID = app.SetupUsers()
	app.Login(username)
	return app, adminID, userID
}

// seedStatisticsEntries gives the admin one feed with five entries, three
// read and one starred.
func seedStatisticsEntries(t *testing.T, app *itestApp, adminID int64) {
	t.Helper()
	ctx := context.Background()
	category, err := models.CreateCategory(ctx, app.DB, adminID, "Tech")
	if err != nil {
		t.Fatal(err)
	}
	title := "Test Feed"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/feed", Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	published := time.Date(2026, 3, 15, 10, 0, 0, 0, time.UTC)
	var entryIDs []int64
	for i := 1; i <= 5; i++ {
		entryTitle := fmt.Sprintf("Entry %d", i)
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feed.ID, GUID: fmt.Sprintf("guid-%d", i), Title: &entryTitle, PublishedAt: &published,
		})
		if err != nil {
			t.Fatal(err)
		}
		entryIDs = append(entryIDs, result.ID)
	}
	for _, id := range entryIDs[:3] {
		execStatistics(t, app, "UPDATE entry SET read_at = '2026-03-15T12:00:00Z' WHERE id = $1", id)
	}
	execStatistics(t, app, "UPDATE entry SET starred_at = '2026-03-15T14:00:00Z' WHERE id = $1", entryIDs[0])
}

func execStatistics(t *testing.T, app *itestApp, query string, args ...any) {
	t.Helper()
	if _, err := app.DB.ExecContext(context.Background(), query, args...); err != nil {
		t.Fatal(err)
	}
}

func wantContains(t *testing.T, body string, wants ...string) {
	t.Helper()
	for _, want := range wants {
		if !strings.Contains(body, want) {
			t.Errorf("body lacks %q", want)
		}
	}
}

func wantAbsent(t *testing.T, body string, unwanted ...string) {
	t.Helper()
	for _, text := range unwanted {
		if strings.Contains(body, text) {
			t.Errorf("body must not contain %q", text)
		}
	}
}

// --- SSR /statistics ---

func TestStatisticsPageRequiresLogin(t *testing.T) {
	if got := defaultITestApp(t).Get("/statistics").Code; got != http.StatusSeeOther {
		t.Errorf("status = %d, want 303", got)
	}
}

func TestStatisticsPageRendersSsrContent(t *testing.T) {
	app, adminID, _ := statisticsApp(t, "admin")
	seedStatisticsEntries(t, app, adminID)

	body := app.GetOK("/statistics?period=all")
	wantContains(t, body, "stats-period-btn", "Total Entries", "Daily Read Articles", "Entries by Category", "Top Feeds",
		`class="stats-period-btn active">All`)
	// Legacy client-side rendering markers must be gone.
	wantAbsent(t, body, "<rdrs-statistics-page>", "/static/js/pages/statistics.js")
}

func TestStatisticsPageMarksTheSelectedPeriodActive(t *testing.T) {
	app, _, _ := statisticsApp(t, "admin")

	// No period and an unknown one both fall back to the 7d default.
	for _, test := range []struct{ query, active string }{{"", "7d"}, {"?period=30d", "30d"}, {"?period=invalid", "7d"}} {
		response := app.Get("/statistics" + test.query)
		if response.Code != http.StatusOK {
			t.Fatalf("%q: status = %d", test.query, response.Code)
		}
		if want := `class="stats-period-btn active">` + test.active; !strings.Contains(response.Text(), want) {
			t.Errorf("%q: lacks %s", test.query, want)
		}
	}
}

func TestStatisticsPageAdminSeesSitewide(t *testing.T) {
	app, _, _ := statisticsApp(t, "admin")

	body := app.GetOK("/statistics")
	wantContains(t, body, "Site-wide Statistics", "Total Users")
	// SQLite can measure free space, so the Reclaimable card is present; it is
	// omitted on PostgreSQL, which reports nothing rather than a zero.
	if !strings.Contains(body, "Reclaimable") {
		t.Error("SQLite must render the Reclaimable card")
	}
}

// The site-wide database figures are memoized, so a second render inside the
// TTL must not re-run the COUNT(*)s behind them. Staleness is the observable
// side: entries added between the two renders stay hidden until the slot is
// dropped.
func TestAdminDatabaseStatsAreServedFromTheCache(t *testing.T) {
	app, adminID, _ := statisticsApp(t, "admin")
	const zeroEntries, fiveEntries = `data-testid="stat-db-total-entries">0<`, `data-testid="stat-db-total-entries">5<`

	if !strings.Contains(app.GetOK("/statistics"), zeroEntries) {
		t.Fatal("no entries seeded yet")
	}
	seedStatisticsEntries(t, app, adminID)
	if !strings.Contains(app.GetOK("/statistics"), zeroEntries) {
		t.Error("the cached slot must still be serving the pre-seed count")
	}

	app.App.AdminDBStats = cache.NewAdminDBStats()
	if body := app.GetOK("/statistics"); !strings.Contains(body, fiveEntries) {
		t.Errorf("dropping the slot must recompute, body was:\n%s", body)
	}
}

func TestStatisticsPageRendersAndLogsWhenQueriesFail(t *testing.T) {
	app, _, _ := statisticsApp(t, "admin")
	execStatistics(t, app, "DROP TABLE entry")

	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, nil)))
	t.Cleanup(func() { slog.SetDefault(previous) })

	if !strings.Contains(app.GetOK("/statistics"), "Total Entries") {
		t.Error("page must still render")
	}
	logged := logs.String()
	for _, want := range []string{"statistics.query_failed", "overview"} {
		if !strings.Contains(logged, want) {
			t.Errorf("log lacks %q: %s", want, logged)
		}
	}
}

func TestStatisticsPageUserNoSitewide(t *testing.T) {
	app, _, _ := statisticsApp(t, "user")
	wantAbsent(t, app.GetOK("/statistics"), "Site-wide Statistics")
}

func TestStatisticsPageCustomPeriod(t *testing.T) {
	app, _, _ := statisticsApp(t, "admin")

	response := app.Get("/statistics?period=custom&from=2026-03-01&to=2026-03-31")
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d", response.Code)
	}
	// Custom dates are reflected in the form's date inputs.
	wantContains(t, response.Text(), `value="2026-03-01"`, `value="2026-03-31"`)
}

func TestStatisticsPageInvalidCustomRangeFallsBack(t *testing.T) {
	app, _, _ := statisticsApp(t, "admin")

	response := app.Get("/statistics?period=custom&from=2026-12-01&to=2026-01-01")
	if response.Code != http.StatusOK {
		t.Fatalf("status = %d", response.Code)
	}
	wantContains(t, response.Text(), `class="stats-period-btn active">7d`)
}

func TestStatisticsPageMasqueradeHidesAdminSection(t *testing.T) {
	app, _, userID := statisticsApp(t, "admin")

	started := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/admin/users/%d/masquerade", userID)})
	if started.Code != http.StatusSeeOther {
		t.Fatalf("masquerade status = %d", started.Code)
	}
	wantAbsent(t, app.GetOK("/statistics"), "Site-wide Statistics")
}

func TestStatisticsPageEmbedsSidebarAndRendersCounts(t *testing.T) {
	app, adminID, _ := statisticsApp(t, "admin")
	seedStatisticsEntries(t, app, adminID)

	// The sidebar payload is inlined so it paints without a round trip.
	wantContains(t, app.GetOK("/statistics"),
		`id="rdrs-sidebar-bootstrap"`, `"username":"admin"`, `"is_admin":true`, `"name":"Tech"`)

	// Seeded data: 5 total entries, 3 read, 1 starred.
	wantContains(t, app.GetOK("/statistics?period=all"), "Total Entries", ">5</div>", ">3</div>", ">1</div>")
}

func TestStatisticsPageDirectLabelsSingleMaxDay(t *testing.T) {
	app, adminID, _ := statisticsApp(t, "admin")
	// Entries 1..3 are read on 2026-03-15, so the daily-read chart has one
	// busiest bucket; ties must not spam a number on every column.
	seedStatisticsEntries(t, app, adminID)

	body := app.GetOK("/statistics?period=custom&from=2026-03-01&to=2026-03-31")
	if labels := strings.Count(body, "stats-bar-value"); labels != 1 {
		t.Errorf("direct labels = %d, want exactly one busiest column", labels)
	}
	wantContains(t, body, `<span class="stats-bar-value">3</span>`)
}

func TestApiSidebarReturnsCategoriesWithUnread(t *testing.T) {
	app, adminID, _ := statisticsApp(t, "admin")
	seedStatisticsEntries(t, app, adminID)

	response := app.Get("/api/sidebar")
	wantStatus(t, response, http.StatusOK)
	if got := jsonField(t, response, "username"); got != "admin" {
		t.Errorf("username = %v", got)
	}
	categories, _ := jsonField(t, response, "categories").([]any)
	if len(categories) != 1 {
		t.Fatalf("categories = %v", categories)
	}
	category, _ := categories[0].(map[string]any)
	// 5 seeded, 3 marked read, so 2 unread.
	if category["name"] != "Tech" || category["unread_count"] != float64(2) {
		t.Errorf("category = %v", category)
	}
	if got := jsonField(t, response, "total_unread"); got != float64(2) {
		t.Errorf("total_unread = %v", got)
	}
}

func TestApiSidebarTotalSummarized(t *testing.T) {
	app := defaultITestApp(t)
	adminID, _ := app.SetupUsers()
	seedStatisticsEntries(t, app, adminID)

	// Seed before the first sidebar read so the cache is cold. A pending row
	// must exist first: completing without one is a no-op.
	var entryID int64
	if err := app.DB.QueryRowContext(context.Background(), "SELECT id FROM entry ORDER BY id LIMIT 1").Scan(&entryID); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	if _, err := models.UpsertPendingSummary(ctx, app.DB, adminID, entryID); err != nil {
		t.Fatal(err)
	}
	if _, err := models.SetSummaryCompleted(ctx, app.DB, adminID, entryID, "summary text"); err != nil {
		t.Fatal(err)
	}
	app.Login("admin")

	if got := jsonField(t, app.Get("/api/sidebar"), "total_summarized"); got != float64(1) {
		t.Errorf("total_summarized = %v", got)
	}
}
