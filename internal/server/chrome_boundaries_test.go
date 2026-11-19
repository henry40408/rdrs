package server

import (
	"context"
	"errors"
	"log/slog"
	"strings"
	"testing"

	"github.com/henry40408/rdrs/internal/models"
)

func TestReadChromeDoesNotCacheTheEmptyState(t *testing.T) {
	app := defaultITestApp(t)
	user := app.SeedAccount("empty", fixturePassword, models.RoleUser)

	chrome := app.App.readChrome(context.Background(), user.ID, nil)
	if chrome.TotalUnread != 0 {
		t.Fatalf("TotalUnread = %d", chrome.TotalUnread)
	}
	if cached := app.App.Sidebar.Get(user.ID); cached != nil {
		t.Errorf("an account with no feeds and no unread entries was cached: %+v", cached)
	}
}

func TestReadChromeCachesAnAccountWithAFeedButNoUnreadEntries(t *testing.T) {
	app := defaultITestApp(t)
	user := app.SeedAccount("feedonly", fixturePassword, models.RoleUser)
	categories, err := models.ListCategoriesByUser(context.Background(), app.DB, user.ID)
	if err != nil || len(categories) != 1 {
		t.Fatalf("categories = %v, %v", categories, err)
	}
	if _, err := models.CreateFeed(context.Background(), app.DB, models.CreateFeedParams{
		CategoryID: categories[0].ID, URL: "https://example.com/feedonly.xml",
	}); err != nil {
		t.Fatal(err)
	}

	chrome := app.App.readChrome(context.Background(), user.ID, nil)
	if chrome.TotalUnread != 0 {
		t.Fatalf("TotalUnread = %d", chrome.TotalUnread)
	}
	if app.App.Sidebar.Get(user.ID) == nil {
		t.Error("an account with a feed was not cached")
	}
}

func TestReadChromeCachesAnAccountWithUnreadEntries(t *testing.T) {
	app := defaultITestApp(t)
	userID, _ := app.SeedUserWithEntry("reader", fixturePassword)

	chrome := app.App.readChrome(context.Background(), userID, nil)
	if chrome.TotalUnread != 1 {
		t.Fatalf("TotalUnread = %d", chrome.TotalUnread)
	}
	if cached := app.App.Sidebar.Get(userID); cached == nil || cached.TotalUnread != 1 {
		t.Errorf("cached = %+v", cached)
	}
}

func TestLogQueryFailedLogsOnlyAnError(t *testing.T) {
	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelWarn})))
	t.Cleanup(func() { slog.SetDefault(previous) })

	logQueryFailed("quiet", nil)
	if logs.String() != "" {
		t.Fatalf("a nil error was logged: %s", logs.String())
	}
	logQueryFailed("left", context.Canceled)
	if logs.String() != "" {
		t.Fatalf("a canceled request was logged: %s", logs.String())
	}
	logQueryFailed("loud", errors.New("disk on fire"))
	for _, want := range []string{"event=page.query_failed", "query=loud", "disk on fire"} {
		if !strings.Contains(logs.String(), want) {
			t.Errorf("log lacks %q: %s", want, logs.String())
		}
	}
}

func TestReadChromeDoesNotCacheAfterAFailedQuery(t *testing.T) {
	logs := &lockedBuffer{}
	previous := slog.Default()
	slog.SetDefault(slog.New(slog.NewTextHandler(logs, &slog.HandlerOptions{Level: slog.LevelWarn})))
	t.Cleanup(func() { slog.SetDefault(previous) })
	app := defaultITestApp(t)
	userID, _ := app.SeedUserWithEntry("failing", fixturePassword)
	canceled, cancel := context.WithCancel(context.Background())
	cancel()

	app.App.readChrome(canceled, userID, nil)
	for _, query := range []string{"settings", "categories", "feeds", "unread_by_feed", "summaries", "today_count"} {
		if want := "query=" + query; !strings.Contains(logs.String(), want) {
			t.Errorf("log lacks %q: %s", want, logs.String())
		}
	}
	if cached := app.App.Sidebar.Get(userID); cached != nil {
		t.Fatalf("a chrome built from failed queries was cached: %+v", cached)
	}

	chrome := app.App.readChrome(context.Background(), userID, nil)
	if len(chrome.Categories) == 0 {
		t.Error("categories missing after the queries recovered")
	}
}
