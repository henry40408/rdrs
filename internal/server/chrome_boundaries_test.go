package server

import (
	"context"
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
