package server

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"image"
	"image/png"
	"log/slog"
	"mime/multipart"
	"net/http"
	"net/http/httptest"
	"net/url"
	"reflect"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/audit"
	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/imageproxy"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/secret"
	"github.com/henry40408/rdrs/internal/server/middleware"
	"github.com/henry40408/rdrs/internal/version"
)

// Port of tests/handlers_test.rs: the GReader API, feed, entry, user-settings
// and page handlers.

// authedApp is authed_server: one account, "testuser", signed in.
func authedApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("testuser", nil), http.StatusCreated)
	app.Login("testuser")
	return app
}

func (a *itestApp) request(method, path string) itestResponse {
	a.t.Helper()
	return a.Do(itestRequest{Method: method, Path: path})
}

// postForm sends alternating name, value pairs as a form body.
func (a *itestApp) postPairs(path string, pairs ...string) itestResponse {
	a.t.Helper()
	form := url.Values{}
	for i := 0; i+1 < len(pairs); i += 2 {
		form.Add(pairs[i], pairs[i+1])
	}
	return a.PostForm(path, form)
}

func (a *itestApp) postText(path, body string) itestResponse {
	a.t.Helper()
	return a.Do(itestRequest{Method: http.MethodPost, Path: path, Body: body, Headers: map[string]string{"Content-Type": "text/plain"}})
}

func (a *itestApp) signOut() {
	a.t.Helper()
	wantStatus(a.t, a.request(http.MethodDelete, "/api/session"), http.StatusOK)
}

func grCreateCategory(t *testing.T, app *itestApp, name string) {
	t.Helper()
	label := "user/-/label/" + name
	wantStatus(t, app.postPairs("/reader/api/0/rename-tag", "s", label, "dest", label), http.StatusOK)
}

func grTags(t *testing.T, app *itestApp) []any {
	t.Helper()
	response := app.Get("/reader/api/0/tag/list")
	wantStatus(t, response, http.StatusOK)
	tags, _ := jsonOf(t, response.ResponseRecorder)["tags"].([]any)
	return tags
}

// grFolderTagNames are the folder labels of tag/list, without the built-in
// state tags.
func grFolderTagNames(t *testing.T, app *itestApp) []string {
	t.Helper()
	names := []string{}
	for _, tag := range grTags(t, app) {
		fields := tag.(map[string]any)
		if fields["type"] == "folder" {
			names = append(names, strings.TrimPrefix(fields["id"].(string), "user/-/label/"))
		}
	}
	return names
}

func grSubscriptions(t *testing.T, app *itestApp) []any {
	t.Helper()
	response := app.Get("/reader/api/0/subscription/list")
	wantStatus(t, response, http.StatusOK)
	subscriptions, _ := jsonOf(t, response.ResponseRecorder)["subscriptions"].([]any)
	return subscriptions
}

// opmlOf is a body with one category holding the given feeds' xmlUrl values.
func opmlOf(category string, feedURLs ...string) string {
	var outlines strings.Builder
	for i, feedURL := range feedURLs {
		fmt.Fprintf(&outlines, `<outline type="rss" text="Feed %d" xmlUrl="%s"/>`, i, feedURL)
	}
	return fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?><opml version="2.0"><body><outline text="%s">%s</outline></body></opml>`, category, outlines.String())
}

func grImport(t *testing.T, app *itestApp, opml string) {
	t.Helper()
	wantStatus(t, app.postText("/reader/api/0/subscription/import", opml), http.StatusOK)
}

// --- Categories (through the GReader tag endpoints) ---

func TestCreateCategory(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/Tech News", "dest", "user/-/label/Tech News")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
	if !slices.Contains(grFolderTagNames(t, app), "Tech News") {
		t.Error("category not listed")
	}
}

func TestCreateCategoryEmptyName(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/", "dest", "user/-/label/")

	wantStatus(t, response, http.StatusBadRequest)
}

func TestCreateCategoryNameTooLong(t *testing.T) {
	app := authedApp(t)
	label := "user/-/label/" + strings.Repeat("a", 101)

	response := app.postPairs("/reader/api/0/rename-tag", "s", label, "dest", label)

	// The GReader layer has no explicit 100-character limit.
	if response.Code != http.StatusOK && response.Code != http.StatusBadRequest {
		t.Errorf("status = %d, want 200 or 400", response.Code)
	}
}

func TestListCategories(t *testing.T) {
	app := authedApp(t)
	for _, name := range []string{"Tech", "News", "Sports"} {
		grCreateCategory(t, app, name)
	}

	// 3 created + the "Uncategorized" category seeded at registration.
	if got := len(grFolderTagNames(t, app)); got != 4 {
		t.Errorf("folder tags = %d, want 4", got)
	}
}

// No cookie jar: a stored anonymous session would put the CSRF guard in front
// of the auth check this is about.
func TestApiMutationsRequireAuthentication(t *testing.T) {
	app := defaultITestApp(t)

	for _, c := range []struct{ method, path string }{
		{http.MethodGet, "/reader/api/0/stream/contents/user/-/state/com.google/reading-list"},
		{http.MethodPost, "/reader/api/0/subscription/import"},
		{http.MethodPost, "/api/session/reauth"},
		{http.MethodPost, "/api/passkey/register/start"},
		{http.MethodPut, "/api/passkeys/1"},
		{http.MethodDelete, "/api/passkeys/1"},
	} {
		response := app.Do(itestRequest{Method: c.method, Path: c.path, NoCookies: true})
		if response.Code != http.StatusUnauthorized {
			t.Errorf("%s %s = %d, want 401", c.method, c.path, response.Code)
		}
	}
}

func TestApiRequiresAuthentication(t *testing.T) {
	app := defaultITestApp(t)

	for _, path := range []string{
		"/reader/api/0/tag/list", "/reader/api/0/subscription/list", "/reader/api/0/subscription/export",
		"/api/feeds/1/icon", "/api/passkeys",
	} {
		if got := app.Get(path).Code; got != http.StatusUnauthorized {
			t.Errorf("%s = %d, want 401", path, got)
		}
	}
}

func TestGetCategory(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "Test Category")

	if !slices.Contains(grFolderTagNames(t, app), "Test Category") {
		t.Error("category not listed")
	}
}

func TestUpdateCategory(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "Old Name")

	response := app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/Old Name", "dest", "user/-/label/New Name")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
	names := grFolderTagNames(t, app)
	if slices.Contains(names, "Old Name") || !slices.Contains(names, "New Name") {
		t.Errorf("names = %v", names)
	}
}

func TestUpdateCategoryEmptyName(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "Test")

	wantStatus(t, app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/Test", "dest", "user/-/label/"), http.StatusBadRequest)
}

func TestDeleteCategory(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "To Delete")

	response := app.postPairs("/reader/api/0/disable-tag", "s", "user/-/label/To Delete")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
	if slices.Contains(grFolderTagNames(t, app), "To Delete") {
		t.Error("category still listed")
	}
}

// --- Feeds (through the GReader subscription endpoints) ---

func TestListFeedsEmpty(t *testing.T) {
	app := authedApp(t)

	if got := grSubscriptions(t, app); len(got) != 0 {
		t.Errorf("subscriptions = %v", got)
	}
}

func TestSubscriptionEditOfAnUnknownFeedIsNotFound(t *testing.T) {
	app := authedApp(t)
	feed := "feed/https://nonexistent.com/feed.xml"

	for _, pairs := range [][]string{
		{"ac", "edit", "s", feed, "t", "Test Feed"},
		{"ac", "edit", "s", feed},
		{"ac", "unsubscribe", "s", feed},
	} {
		if got := app.postPairs("/reader/api/0/subscription/edit", pairs...).Code; got != http.StatusNotFound {
			t.Errorf("%v = %d, want 404", pairs, got)
		}
	}
}

func TestGetFeedIconNotFound(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.Get("/api/feeds/9999/icon"), http.StatusNotFound)
}

func TestCreateFeedEmptyUrl(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.postPairs("/reader/api/0/subscription/edit", "ac", "subscribe", "s", "feed/"), http.StatusBadRequest)
}

func TestCreateFeedWhitespaceUrl(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/subscription/edit", "ac", "subscribe", "s", "feed/   ")

	// A whitespace URL is not a valid feed.
	if response.Code != http.StatusBadRequest && response.Code != http.StatusInternalServerError {
		t.Errorf("status = %d, want an error", response.Code)
	}
}

func TestMoveFeedToDifferentCategory(t *testing.T) {
	app := authedApp(t)
	grImport(t, app, opmlOf("MoveCategory1", "https://move.example.com/feed.xml"))

	response := app.postPairs("/reader/api/0/subscription/edit",
		"ac", "edit", "s", "feed/https://move.example.com/feed.xml", "a", "user/-/label/MoveNewCategory")
	wantStatus(t, response, http.StatusOK)

	subscriptions := grSubscriptions(t, app)
	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	categories := subscriptions[0].(map[string]any)["categories"].([]any)
	if label := categories[0].(map[string]any)["label"]; label != "MoveNewCategory" {
		t.Errorf("label = %v", label)
	}
}

// Categories are per user: user 2 sees only their own feed.
func TestUpdateFeedToOtherUserCategory(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("movefeeduser1", nil), http.StatusCreated)
	app.Login("movefeeduser1")
	grCreateCategory(t, app, "MoveFeedUser1 Category")
	app.signOut()
	app.SeedAccount("movefeeduser2", fixturePassword, "user")
	app.Login("movefeeduser2")

	grImport(t, app, opmlOf("MoveFeedUser2 Category", "https://movefeeduser2.example.com/feed.xml"))

	subscriptions := grSubscriptions(t, app)
	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	if id := subscriptions[0].(map[string]any)["id"]; id != "feed/https://movefeeduser2.example.com/feed.xml" {
		t.Errorf("id = %v", id)
	}
}

func TestDeleteFeedOtherUser(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("feeddeluser1", nil), http.StatusCreated)
	app.Login("feeddeluser1")
	grImport(t, app, opmlOf("DelTestCat", "https://deltest.example.com/feed.xml"))
	app.signOut()
	app.SeedAccount("feeddeluser2", fixturePassword, "user")
	app.Login("feeddeluser2")

	response := app.postPairs("/reader/api/0/subscription/edit", "ac", "unsubscribe", "s", "feed/https://deltest.example.com/feed.xml")

	wantStatus(t, response, http.StatusNotFound)
}

// --- OPML (through GReader subscription/export and subscription/import) ---

func TestExportOpmlEmpty(t *testing.T) {
	app := authedApp(t)

	body := app.GetOK("/reader/api/0/subscription/export")

	if !strings.Contains(body, "<?xml") && !strings.Contains(body, "<opml") {
		t.Errorf("not OPML: %s", body)
	}
}

func TestExportOpmlWithFeeds(t *testing.T) {
	app := authedApp(t)
	grImport(t, app, opmlOf("ExportTestCategory", "https://export.example.com/feed.xml"))

	wantContains(t, app.GetOK("/reader/api/0/subscription/export"), "ExportTestCategory", "export.example.com")
}

func TestImportOpmlValid(t *testing.T) {
	app := authedApp(t)
	opml := `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test Subscriptions</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Example Feed" title="Example Feed"
               xmlUrl="https://example.com/feed.xml" htmlUrl="https://example.com"/>
    </outline>
  </body>
</opml>`

	response := app.postText("/reader/api/0/subscription/import", opml)

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
	if got := len(grSubscriptions(t, app)); got != 1 {
		t.Errorf("subscriptions = %d, want 1", got)
	}
	if !slices.Contains(grFolderTagNames(t, app), "Tech") {
		t.Error("Tech folder missing")
	}
}

func TestImportOpmlInvalid(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.postText("/reader/api/0/subscription/import", "not valid xml"), http.StatusBadRequest)
}

func TestImportOpmlDuplicateFeedsSkipped(t *testing.T) {
	app := authedApp(t)
	opml := opmlOf("Tech", "https://example.com/feed.xml")

	grImport(t, app, opml)
	grImport(t, app, opml)

	if got := len(grSubscriptions(t, app)); got != 1 {
		t.Errorf("subscriptions = %d, want 1", got)
	}
}

func TestImportOpmlMultipleCategories(t *testing.T) {
	app := authedApp(t)
	opml := `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Subscriptions</title></head>
  <body>
    <outline text="MultiTech" title="MultiTech">
      <outline type="rss" text="Feed 1" xmlUrl="https://multi.example.com/feed1.xml"/>
    </outline>
    <outline text="MultiNews" title="MultiNews">
      <outline type="rss" text="Feed 2" xmlUrl="https://multi.example.com/feed2.xml"/>
      <outline type="rss" text="Feed 3" xmlUrl="https://multi.example.com/feed3.xml"/>
    </outline>
  </body>
</opml>`

	grImport(t, app, opml)

	if got := len(grSubscriptions(t, app)); got != 3 {
		t.Errorf("subscriptions = %d, want 3", got)
	}
	// 2 imported folders + the seeded "Uncategorized".
	if got := len(grFolderTagNames(t, app)); got != 3 {
		t.Errorf("folder tags = %d, want 3", got)
	}
}

// --- Entries (through GReader stream/contents and edit-tag) ---

// stream/items/contents with a non-existent id is a 200 with no items, not a 404.
func TestEmptyReadingListAndUnknownItemHaveNoItems(t *testing.T) {
	app := authedApp(t)

	for _, path := range []string{
		"/reader/api/0/stream/contents/user/-/state/com.google/reading-list",
		"/reader/api/0/stream/items/contents?i=9999",
	} {
		response := app.Get(path)
		wantStatus(t, response, http.StatusOK)
		if items, _ := jsonOf(t, response.ResponseRecorder)["items"].([]any); len(items) != 0 {
			t.Errorf("%s: items = %v", path, items)
		}
	}
}

func TestListEntriesWithPagination(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?n=10")

	wantStatus(t, response, http.StatusOK)
	if _, ok := jsonOf(t, response.ResponseRecorder)["items"].([]any); !ok {
		t.Error("items is not an array")
	}
}

func TestListEntriesWithFilters(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.Get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?xt=user/-/state/com.google/read"), http.StatusOK)
	wantStatus(t, app.Get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list?it=user/-/state/com.google/starred"), http.StatusOK)
}

func TestStreamOfAnUnknownCategoryOrFeedIsNotFound(t *testing.T) {
	app := authedApp(t)

	for _, stream := range []string{"user/-/label/NonExistent", "feed/https://nonexistent.com/feed.xml"} {
		if got := app.Get("/reader/api/0/stream/contents/" + stream).Code; got != http.StatusNotFound {
			t.Errorf("%s = %d, want 404", stream, got)
		}
	}
}

func TestEditTagOnAnUnknownEntryIsNotFound(t *testing.T) {
	app := authedApp(t)

	for _, c := range [][2]string{
		{"a", "user/-/state/com.google/read"},
		{"r", "user/-/state/com.google/read"},
		{"a", "user/-/state/com.google/starred"},
	} {
		if got := app.postPairs("/reader/api/0/edit-tag", "i", "9999", c[0], c[1]).Code; got != http.StatusNotFound {
			t.Errorf("%s=%s = %d, want 404", c[0], c[1], got)
		}
	}
}

func TestEntryActionsOnAnUnknownEntryAreNotFound(t *testing.T) {
	app := authedApp(t)

	for _, c := range []struct{ method, action string }{
		{http.MethodGet, "neighbors"}, {http.MethodPost, "fetch-full-content"},
		{http.MethodPost, "summarize"}, {http.MethodPost, "save"},
	} {
		if got := app.request(c.method, "/api/entries/9999/"+c.action).Code; got != http.StatusNotFound {
			t.Errorf("%s = %d, want 404", c.action, got)
		}
	}
}

func TestGetUnreadStats(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/reader/api/0/unread-count")

	wantStatus(t, response, http.StatusOK)
	if _, ok := jsonOf(t, response.ResponseRecorder)["unreadcounts"].([]any); !ok {
		t.Error("unreadcounts is not an array")
	}
}

func TestMarkAllReadAll(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/mark-all-as-read", "s", "user/-/state/com.google/reading-list")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
}

func TestMarkAllReadOlderThanDays(t *testing.T) {
	app := authedApp(t)
	// The timestamp is in microseconds: seven days ago.
	sevenDaysAgo := fmt.Sprint((time.Now().Unix() - 7*86400) * 1_000_000)

	response := app.postPairs("/reader/api/0/mark-all-as-read", "s", "user/-/state/com.google/reading-list", "ts", sevenDaysAgo)

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "OK" {
		t.Errorf("body = %q", response.Text())
	}
}

func TestMarkAllReadOfAnUnknownStreamIsNotFound(t *testing.T) {
	app := authedApp(t)

	for _, stream := range []string{"user/-/label/NonExistent", "feed/https://nonexistent.com/feed.xml"} {
		if got := app.postPairs("/reader/api/0/mark-all-as-read", "s", stream).Code; got != http.StatusNotFound {
			t.Errorf("%s = %d, want 404", stream, got)
		}
	}
}

func TestEntriesFilterByValidCategory(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "TestCategory")

	// Empty, but a 200.
	wantStatus(t, app.Get("/reader/api/0/stream/contents/user/-/label/TestCategory"), http.StatusOK)
}

func TestEmptyPasskeyNameIsABadRequest(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.PutJSON("/api/passkeys/1", map[string]string{"name": ""}), http.StatusBadRequest)
}

// --- Pages ---

func TestCategoriesPage(t *testing.T) {
	app := authedApp(t)

	body := app.GetOK("/categories")

	wantAbsent(t, body, "<rdrs-categories-page>", "/static/js/pages/categories.js")
	wantContains(t, body, "<h1>Categories</h1>", `<form method="post" action="/categories">`, `data-testid="categories-table"`)
}

func TestPagesRedirectWhenSignedOut(t *testing.T) {
	app := defaultITestApp(t)

	for _, path := range []string{"/categories", "/feeds", "/entries", "/entries/1", "/user-settings", "/settings"} {
		if got := app.Get(path).Code; got != http.StatusSeeOther {
			t.Errorf("%s = %d, want 303", path, got)
		}
	}
}

func TestFeedsPage(t *testing.T) {
	app := authedApp(t)

	body := app.GetOK("/feeds")

	wantContains(t, body, "<h1>Feeds</h1>", `<form method="post" action="/feeds">`,
		`data-testid="feeds-table"`, `data-testid="feed-url-input"`)
	wantAbsent(t, body, "<rdrs-feeds-page>", "/static/js/pages/feeds.js")
}

func TestEntriesPage(t *testing.T) {
	wantStatus(t, authedApp(t).Get("/entries"), http.StatusOK)
}

// The entry page redirects to the list page with an ?entry= parameter.
func TestEntryPage(t *testing.T) {
	wantStatus(t, authedApp(t).Get("/entries/1"), http.StatusSeeOther)
}

func TestUserSettingsPage(t *testing.T) {
	body := authedApp(t).GetOK("/user-settings")

	if !strings.Contains(body, "Settings") && !strings.Contains(body, "settings") {
		t.Error("no settings heading")
	}
}

func TestSettingsPage(t *testing.T) {
	body := authedApp(t).GetOK("/settings")

	wantAbsent(t, body, "<rdrs-settings-page>", "/static/js/pages/settings.js")
	wantContains(t, body, "<h1>App</h1>")
}

// --- Cross-user isolation ---

func TestCategoryIsolationBetweenUsers(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("user1", nil), http.StatusCreated)
	app.Login("user1")
	grCreateCategory(t, app, "User1 Category")
	app.signOut()
	app.SeedAccount("user2", fixturePassword, "user")
	app.Login("user2")

	// User 2 sees only their own seeded "Uncategorized".
	names := grFolderTagNames(t, app)
	if !slices.Equal(names, []string{"Uncategorized"}) {
		t.Errorf("names = %v", names)
	}
}

// --- Edge cases ---

func TestCreateCategoryWithWhitespaceName(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/ Trimmed Name ", "dest", "user/-/label/ Trimmed Name ")

	wantStatus(t, response, http.StatusOK)
	if len(grFolderTagNames(t, app)) == 0 {
		t.Error("no folder tag")
	}
}

func TestUpdateCategoryWithWhitespaceName(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "Original")

	wantStatus(t, app.postPairs("/reader/api/0/rename-tag", "s", "user/-/label/Original", "dest", "user/-/label/ Updated "), http.StatusOK)

	if slices.Contains(grFolderTagNames(t, app), "Original") {
		t.Error("the original name should be gone")
	}
}

// expectedStaticCacheControl: dirty builds serve no-cache so development sees
// fresh assets; production builds keep the immutable header.
func expectedStaticCacheControl() string {
	if version.IsDirty() {
		return "no-cache"
	}
	return "public, max-age=31536000, immutable"
}

// insertTestFeed stores a category and a feed for the first account.
func insertTestFeed(t *testing.T, app *itestApp, categoryName, feedURL string) (categoryID, feedID int64) {
	t.Helper()
	owner := libraryOwner{app: app, userID: 1}
	categoryID = owner.category(t, categoryName)
	feedID = owner.feed(t, categoryID, feedURL, "Test Feed")
	return categoryID, feedID
}

func TestGetFeedIconNoIcon(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("iconuser", fixturePassword)
	app.Login("iconuser")
	grImport(t, app, opmlOf("IconTestCat", "https://noicon.example.com/feed.xml"))
	var feedID int64
	if err := app.DB.QueryRowContext(context.Background(), "SELECT id FROM feed WHERE url = $1", "https://noicon.example.com/feed.xml").Scan(&feedID); err != nil {
		t.Fatal(err)
	}

	// A feed that exists but has no icon is a 404.
	wantStatus(t, app.Get(fmt.Sprintf("/api/feeds/%d/icon", feedID)), http.StatusNotFound)
	subscriptions := grSubscriptions(t, app)
	if icon := subscriptions[0].(map[string]any)["iconUrl"]; icon != "" {
		t.Errorf("iconUrl = %v, want empty", icon)
	}
}

// The icons are compiled into the binary, so they change with the build: only
// the ?v=-stamped links may be pinned long-term.
func TestFaviconCacheControlIsVersionGated(t *testing.T) {
	app := defaultITestApp(t)

	bare := app.Get("/favicon.ico")
	wantStatus(t, bare, http.StatusOK)
	if got := bare.Header().Get("Cache-Control"); got != "public, max-age=3600" {
		t.Errorf("bare Cache-Control = %q", got)
	}
	stamped := app.Get("/favicon.ico?v=" + version.Version())
	wantStatus(t, stamped, http.StatusOK)
	if got := stamped.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("stamped Cache-Control = %q, want %q", got, expectedStaticCacheControl())
	}
}

// The handler sets its own Cache-Control, so the directive itself must keep
// this auth-scoped response out of shared storage.
func TestGetFeedIconIsPrivatelyCached(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "IconCacheCat", "https://icon.example.com/feed.xml")
	err := models.UpsertImage(context.Background(), app.DB, models.ImageUpsert{
		EntityType: models.EntityFeed, EntityID: feedID, Data: []byte("\x89PNG\r\n\x1a\n"), ContentType: "image/png",
	})
	if err != nil {
		t.Fatal(err)
	}

	response := app.Get(fmt.Sprintf("/api/feeds/%d/icon", feedID))

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != "private, max-age=86400" {
		t.Errorf("Cache-Control = %q", got)
	}
}

// --- Passkeys ---

// useSessionCookie signs in with a session created straight in the database,
// for configurations where the login endpoint is unavailable.
func useSessionCookie(app *itestApp, key []byte, sessionToken string) {
	app.cookies["session_token"] = secret.SignSession(key, sessionToken)
	app.headers = http.Header{"X-Csrf-Token": {secret.DeriveCSRF(key, sessionToken)}}
}

// staleAuthentication pushes every session out of the re-authentication
// window, standing in for "this browser logged in a while ago".
func staleAuthentication(t *testing.T, app *itestApp) {
	t.Helper()
	execStatistics(t, app, "UPDATE session SET last_authenticated_at = $1", db.FormatRustRFC3339(time.Now().Add(-time.Hour)))
}

func wantReauthRequired(t *testing.T, response itestResponse) {
	t.Helper()
	wantStatus(t, response, http.StatusForbidden)
	if got := jsonField(t, response, "error"); got != "Reauthentication required" {
		t.Errorf("error = %v", got)
	}
}

func insertPasskey(t *testing.T, app *itestApp, userID int64, credentialID []byte, name string, publicKey []byte, counter int64, transports *string) int64 {
	t.Helper()
	created, err := models.CreatePasskey(context.Background(), app.DB, models.NewPasskey{
		UserID: userID, CredentialID: credentialID, PublicKey: publicKey, Counter: counter, Name: name, Transports: transports,
	})
	if err != nil {
		t.Fatal(err)
	}
	return created.ID
}

// A passkey outlives a password change, so a picked-up session must not be
// able to mint one silently.
func TestPasskeyRegisterStartRequiresRecentAuthentication(t *testing.T) {
	app := authedApp(t)

	wantStatus(t, app.request(http.MethodPost, "/api/passkey/register/start"), http.StatusOK)
	staleAuthentication(t, app)

	wantReauthRequired(t, app.request(http.MethodPost, "/api/passkey/register/start"))
}

// The window check comes before the passkey is looked up, so a non-existent id
// still reports the requirement rather than 404.
func TestPasskeyDeleteRequiresRecentAuthentication(t *testing.T) {
	app := authedApp(t)
	staleAuthentication(t, app)

	wantReauthRequired(t, app.request(http.MethodDelete, "/api/passkeys/1"))
}

func TestReauthWithCorrectPasswordReopensTheWindow(t *testing.T) {
	app := authedApp(t)
	staleAuthentication(t, app)
	wantStatus(t, app.request(http.MethodPost, "/api/passkey/register/start"), http.StatusForbidden)

	wantStatus(t, app.PostJSON("/api/session/reauth", map[string]string{"password": fixturePassword}), http.StatusNoContent)

	wantStatus(t, app.request(http.MethodPost, "/api/passkey/register/start"), http.StatusOK)
}

func TestReauthWithWrongPasswordIsRejected(t *testing.T) {
	app := authedApp(t)
	staleAuthentication(t, app)

	wantStatus(t, app.PostJSON("/api/session/reauth", map[string]string{"password": "not-my-password"}), http.StatusUnauthorized)

	// A failed re-authentication must not open the window.
	wantStatus(t, app.request(http.MethodPost, "/api/passkey/register/start"), http.StatusForbidden)
}

func TestPasskeyRegisterStartAuthorized(t *testing.T) {
	app := authedApp(t)

	response := app.request(http.MethodPost, "/api/passkey/register/start")

	wantStatus(t, response, http.StatusOK)
	publicKey := jsonField(t, response, "options").(map[string]any)["publicKey"].(map[string]any)
	if _, ok := publicKey["challenge"].(string); !ok {
		t.Error("no challenge")
	}
	if _, ok := publicKey["user"].(map[string]any)["name"].(string); !ok {
		t.Error("no user name")
	}
}

func TestPasskeyRegisterFinishUnauthorized(t *testing.T) {
	app := defaultITestApp(t)

	response := app.PostJSON("/api/passkey/register/finish", map[string]any{"name": "Test Passkey", "credential": map[string]any{}})

	wantStatus(t, response, http.StatusUnauthorized)
}

// An instance with nothing enrolled must answer like one that has passkeys:
// the handler used to reject, telling any caller whether this deployment had
// accounts using them.
func TestPasskeyAuthStartNoPasskeys(t *testing.T) {
	app := defaultITestApp(t)

	response := app.request(http.MethodPost, "/api/passkey/auth/start")

	wantStatus(t, response, http.StatusOK)
	publicKey := jsonField(t, response, "options").(map[string]any)["publicKey"].(map[string]any)
	if _, ok := publicKey["challenge"].(string); !ok {
		t.Error("a challenge must be issued regardless of what is enrolled")
	}
}

// credentialJSON is a syntactically valid credential with no real signature.
func credentialJSON(responseFields map[string]string) map[string]any {
	return map[string]any{"id": "dGVzdA", "rawId": "dGVzdA", "type": "public-key", "response": responseFields}
}

func TestPasskeyAuthFinishNoChallenge(t *testing.T) {
	app := defaultITestApp(t)

	response := app.PostJSON("/api/passkey/auth/finish", map[string]any{"credential": credentialJSON(map[string]string{
		"authenticatorData": "dGVzdA", "clientDataJSON": "dGVzdA", "signature": "dGVzdA",
	})})

	wantStatus(t, response, http.StatusBadRequest)
	if got, _ := jsonField(t, response, "error").(string); !strings.Contains(got, "Challenge") {
		t.Errorf("error = %q", got)
	}
}

func TestListPasskeysEmpty(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/api/passkeys")

	wantStatus(t, response, http.StatusOK)
	if got, _ := jsonField(t, response, "passkeys").([]any); len(got) != 0 {
		t.Errorf("passkeys = %v", got)
	}
}

func TestRenamePasskeyNotFound(t *testing.T) {
	wantStatus(t, authedApp(t).PutJSON("/api/passkeys/9999", map[string]string{"name": "New Name"}), http.StatusNotFound)
}

func TestDeletePasskeyNotFound(t *testing.T) {
	wantStatus(t, authedApp(t).request(http.MethodDelete, "/api/passkeys/9999"), http.StatusNotFound)
}

// The success path: a fresh session deletes its own passkey, which is also what
// reaches the passkey.removed audit call.
func TestDeletePasskeySucceedsWithinTheWindow(t *testing.T) {
	app := authedApp(t)
	insertPasskey(t, app, 1, []byte{1, 2, 3}, "MacBook", []byte("{}"), 0, nil)

	wantStatus(t, app.request(http.MethodDelete, "/api/passkeys/1"), http.StatusNoContent)

	// And it is gone, not merely reported as deleted.
	wantStatus(t, app.request(http.MethodDelete, "/api/passkeys/1"), http.StatusNotFound)
}

// disable_local_auth leaves no password to check, so re-authentication has to
// refuse rather than verify against a hash that can never match.
func TestReauthRefusedWhenLocalAuthDisabled(t *testing.T) {
	cfg := itestConfig()
	cfg.DisableLocalAuth = true
	app := newITestApp(t, cfg)
	app.createUserDirectly("testuser", fixturePassword)
	user, err := models.FindUserByUsername(context.Background(), app.DB, "testuser")
	if err != nil || user == nil {
		t.Fatalf("user = %v, err = %v", user, err)
	}
	session, err := models.CreateSession(context.Background(), app.DB, models.NewSession{UserID: user.ID, UserAgent: "test-agent", IPAddress: "127.0.0.1"})
	if err != nil {
		t.Fatal(err)
	}
	useSessionCookie(app, cfg.Secret, session.SessionToken)

	wantStatus(t, app.PostJSON("/api/session/reauth", map[string]string{"password": fixturePassword}), http.StatusForbidden)
}

// Starting the ceremony reads no stored credentials, so an unparseable row
// cannot take the sign-in page down; it can only fail at the finish step.
func TestPasskeyAuthStartWithInvalidPasskeyData(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("testuser", fixturePassword)
	insertPasskey(t, app, 1, []byte{1, 2, 3}, "Test Passkey", []byte("invalid json"), 0, nil)

	response := app.request(http.MethodPost, "/api/passkey/auth/start")

	wantStatus(t, response, http.StatusOK)
	publicKey := jsonField(t, response, "options").(map[string]any)["publicKey"].(map[string]any)
	if _, ok := publicKey["challenge"].(string); !ok {
		t.Error("no challenge")
	}
}

func TestPasskeyRegisterFinishEmptyName(t *testing.T) {
	app := authedApp(t)
	app.request(http.MethodPost, "/api/passkey/register/start")

	// Validation of the name comes before the credential is checked.
	response := app.PostJSON("/api/passkey/register/finish", map[string]any{"name": "", "credential": credentialJSON(map[string]string{
		"attestationObject": "dGVzdA", "clientDataJSON": "dGVzdA",
	})})

	wantStatus(t, response, http.StatusBadRequest)
	if got, _ := jsonField(t, response, "error").(string); !strings.Contains(got, "name") {
		t.Errorf("error = %q", got)
	}
}

func TestPasskeyRegisterFinishNoChallenge(t *testing.T) {
	app := authedApp(t)

	response := app.PostJSON("/api/passkey/register/finish", map[string]any{"name": "Test Passkey", "credential": credentialJSON(map[string]string{
		"attestationObject": "dGVzdA", "clientDataJSON": "dGVzdA",
	})})

	wantStatus(t, response, http.StatusBadRequest)
	if got, _ := jsonField(t, response, "error").(string); !strings.Contains(got, "Challenge") {
		t.Errorf("error = %q", got)
	}
}

func TestListPasskeysWithData(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("testuser", fixturePassword)
	transports := "usb,nfc"
	insertPasskey(t, app, 1, []byte{1, 2, 3}, "My Passkey", []byte("{}"), 5, &transports)
	app.Login("testuser")

	response := app.Get("/api/passkeys")

	wantStatus(t, response, http.StatusOK)
	passkeys, _ := jsonField(t, response, "passkeys").([]any)
	if len(passkeys) != 1 || passkeys[0].(map[string]any)["name"] != "My Passkey" {
		t.Errorf("passkeys = %v", passkeys)
	}
}

func TestRenamePasskeySuccess(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("testuser", fixturePassword)
	passkeyID := insertPasskey(t, app, 1, []byte{1, 2, 3}, "Old Name", []byte("{}"), 0, nil)
	app.Login("testuser")

	wantStatus(t, app.PutJSON(fmt.Sprintf("/api/passkeys/%d", passkeyID), map[string]string{"name": "New Name"}), http.StatusNoContent)

	passkeys, _ := jsonField(t, app.Get("/api/passkeys"), "passkeys").([]any)
	if len(passkeys) != 1 || passkeys[0].(map[string]any)["name"] != "New Name" {
		t.Errorf("passkeys = %v", passkeys)
	}
}

func TestDeletePasskeySuccess(t *testing.T) {
	app := defaultITestApp(t)
	app.createUserDirectly("testuser", fixturePassword)
	passkeyID := insertPasskey(t, app, 1, []byte{1, 2, 3}, "Test Passkey", []byte("{}"), 0, nil)
	app.Login("testuser")

	wantStatus(t, app.request(http.MethodDelete, fmt.Sprintf("/api/passkeys/%d", passkeyID)), http.StatusNoContent)

	if passkeys, _ := jsonField(t, app.Get("/api/passkeys"), "passkeys").([]any); len(passkeys) != 0 {
		t.Errorf("passkeys = %v", passkeys)
	}
}

// otherUsersPasskey is a passkey owned by the first of two accounts, with the
// second one signed in.
func otherUsersPasskey(t *testing.T, owner, intruder string) (*itestApp, int64) {
	t.Helper()
	app := defaultITestApp(t)
	app.createUserDirectly(owner, fixturePassword)
	passkeyID := insertPasskey(t, app, 1, []byte{4, 5, 6}, "User1 Key", []byte("{}"), 0, nil)
	app.createUserDirectly(intruder, fixturePassword)
	app.Login(intruder)
	return app, passkeyID
}

func TestPasskeyRenameOtherUser(t *testing.T) {
	app, passkeyID := otherUsersPasskey(t, "pkuser1", "pkuser2")

	wantStatus(t, app.PutJSON(fmt.Sprintf("/api/passkeys/%d", passkeyID), map[string]string{"name": "Hacked Name"}), http.StatusNotFound)
}

func TestPasskeyDeleteOtherUser(t *testing.T) {
	app, passkeyID := otherUsersPasskey(t, "pkdeluser1", "pkdeluser2")

	wantStatus(t, app.request(http.MethodDelete, fmt.Sprintf("/api/passkeys/%d", passkeyID)), http.StatusNotFound)
}

// --- Icons and static assets ---

func TestIconsAndStaticFilesCarryTheirContentType(t *testing.T) {
	app := defaultITestApp(t)

	for _, c := range []struct{ path, contentType string }{
		{"/favicon.ico", "image/x-icon"},
		{"/favicon.svg", "image/svg+xml"},
		{"/favicon-16x16.png", "image/png"},
		{"/favicon-32x32.png", "image/png"},
		{"/static/js/components/rdrs-sidebar.js", "application/javascript"},
	} {
		response := app.Get(c.path)
		wantStatus(t, response, http.StatusOK)
		if got := response.Header().Get("Content-Type"); got != c.contentType {
			t.Errorf("%s Content-Type = %q, want %q", c.path, got, c.contentType)
		}
	}
}

// iOS home-screen icons do not support transparency (transparent pixels render
// as black) and iOS applies its own corner mask, so the icon must be a
// fully-opaque, full-bleed square.
func TestAppleTouchIcon(t *testing.T) {
	response := defaultITestApp(t).Get("/apple-touch-icon.png")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "image/png" {
		t.Errorf("Content-Type = %q", got)
	}
	img, err := png.Decode(bytes.NewReader(response.Body.Bytes()))
	if err != nil {
		t.Fatalf("apple-touch-icon must be a decodable image: %v", err)
	}
	bounds := img.Bounds()
	if bounds.Dx() != 180 || bounds.Dy() != 180 {
		t.Fatalf("size = %dx%d, want 180x180", bounds.Dx(), bounds.Dy())
	}
	for _, corner := range []image.Point{{0, 0}, {bounds.Max.X - 1, 0}, {0, bounds.Max.Y - 1}, {bounds.Max.X - 1, bounds.Max.Y - 1}} {
		if _, _, _, alpha := img.At(corner.X, corner.Y).RGBA(); alpha != 0xffff {
			t.Errorf("corner pixel %v must be fully opaque, alpha = %#x", corner, alpha)
		}
	}
}

func TestStaticJsServesKnownFile(t *testing.T) {
	response := defaultITestApp(t).Get("/static/js/utils.js")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "application/javascript" {
		t.Errorf("Content-Type = %q", got)
	}
	if got := response.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("Cache-Control = %q", got)
	}
	if response.Body.Len() == 0 {
		t.Error("JS file should not be empty")
	}
}

func TestStaticJsNotFound(t *testing.T) {
	wantStatus(t, defaultITestApp(t).Get("/static/js/nonexistent.js"), http.StatusNotFound)
}

func TestStaticCssServesAppCss(t *testing.T) {
	response := defaultITestApp(t).Get("/static/css/app.css")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "text/css; charset=utf-8" {
		t.Errorf("Content-Type = %q", got)
	}
	if got := response.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("Cache-Control = %q", got)
	}
	wantContains(t, response.Text(), ":root")
}

func TestStaticFontServesWoff2(t *testing.T) {
	response := defaultITestApp(t).Get("/static/fonts/newsreader-latin.woff2")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "font/woff2" {
		t.Errorf("Content-Type = %q", got)
	}
	if got := response.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("Cache-Control = %q", got)
	}
	if !bytes.HasPrefix(response.Body.Bytes(), []byte("wOF2")) {
		t.Error("font must be a valid woff2 payload")
	}
}

// Feeds that grade with stars use the half-star U+2BEA, which no system font on
// macOS carries; without a face scoped to it the title renders as tofu.
func TestStaticStarFontServesAndCssScopesIt(t *testing.T) {
	app := defaultITestApp(t)
	response := app.Get("/static/fonts/noto-sans-symbols2-stars.woff2")
	wantStatus(t, response, http.StatusOK)
	if !bytes.HasPrefix(response.Body.Bytes(), []byte("wOF2")) {
		t.Error("font must be a valid woff2 payload")
	}

	css := app.GetOK("/static/css/app.css")

	// unicode-range keeps the face from displacing Newsreader for latin text.
	wantContains(t, css, "/static/fonts/noto-sans-symbols2-stars.woff2", "unicode-range: U+2605-2606, U+2BE8-2BEB;")
	// Second, right behind the stack's own face: first, WebKit sizes every line
	// box from Noto's tall metrics and shifts text off the icons beside it.
	for _, c := range []struct{ stack, face string }{
		{"--font-display", "'Newsreader'"}, {"--font-body", "'Newsreader'"},
		{"--font-ui", "'Archivo'"}, {"--font-mono", "'IBM Plex Mono'"},
	} {
		_, afterStack, ok := strings.Cut(css, c.stack+":")
		if !ok {
			t.Errorf("%s must be defined", c.stack)
			continue
		}
		declaration, _, _ := strings.Cut(afterStack, ";")
		families := strings.Split(declaration, ",")
		if len(families) < 2 || strings.TrimSpace(families[0]) != c.face || strings.TrimSpace(families[1]) != "'Noto Sans Symbols 2'" {
			t.Errorf("%s must list its own face, then the star font, got: %s", c.stack, declaration)
		}
	}
}

// Safari repaints the title link's hover color short of the CJK fallback
// glyphs' tops; the link's inline vertical padding is what grows the repainted
// rect.
func TestReadingPaneTitleLinkPadsOverCjkGlyphOverflow(t *testing.T) {
	css := defaultITestApp(t).GetOK("/static/css/app.css")

	_, afterRule, ok := strings.Cut(css, "\n.reading-pane-title a {")
	if !ok {
		t.Fatal(".reading-pane-title a rule must exist")
	}
	link, _, _ := strings.Cut(afterRule, "}")
	if !strings.Contains(link, "padding-block: 0.15em;") {
		t.Errorf("the title link must pad over the CJK glyph overflow, got: %s", link)
	}
}

func TestStaticFontNotFound(t *testing.T) {
	wantStatus(t, defaultITestApp(t).Get("/static/fonts/nonexistent.woff2"), http.StatusNotFound)
}

// --- PWA ---

func wantNoSetCookie(t *testing.T, response itestResponse, why string) {
	t.Helper()
	if cookies := response.Header().Values("Set-Cookie"); len(cookies) != 0 {
		t.Errorf("%s: Set-Cookie = %v", why, cookies)
	}
}

// Not application/javascript, which the serve table's fall-through would hand
// out: with nosniff on every response, the browser would reject the manifest
// and the app would stop being installable.
func TestManifestIsServedAsAManifest(t *testing.T) {
	response := defaultITestApp(t).Get("/static/manifest.webmanifest")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "application/manifest+json" {
		t.Errorf("Content-Type = %q", got)
	}
	if got := response.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("Cache-Control = %q", got)
	}
	wantNoSetCookie(t, response, "the manifest is publicly cacheable")
}

func TestPwaIconsAreServedAsPng(t *testing.T) {
	app := defaultITestApp(t)

	for _, path := range []string{"/static/icons/icon-192.png", "/static/icons/icon-512.png", "/static/icons/maskable-icon-512.png"} {
		response := app.Get(path)
		wantStatus(t, response, http.StatusOK)
		if got := response.Header().Get("Content-Type"); got != "image/png" {
			t.Errorf("%s Content-Type = %q", path, got)
		}
		// PNG magic proves the build rendered these rather than embedding
		// something the manifest would then fail to decode.
		if body := response.Body.Bytes(); len(body) < 4 || string(body[1:4]) != "PNG" {
			t.Errorf("%s must be a real PNG", path)
		}
	}
}

// The worker is served from the root, not /static/js/: a worker's scope is the
// directory it came from, and only a root-scoped one sees the navigations it
// exists to catch.
func TestServiceWorkerIsServedFromTheRootWithoutCookies(t *testing.T) {
	response := defaultITestApp(t).Get("/sw.js")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Content-Type"); got != "application/javascript" {
		t.Errorf("Content-Type = %q", got)
	}
	// The URL carries no ?v= stamp, so immutable would pin one build's worker
	// for a year with nothing left to invalidate.
	if got := response.Header().Get("Cache-Control"); got != "no-cache" {
		t.Errorf("Cache-Control = %q", got)
	}
	wantNoSetCookie(t, response, "the worker script must stay cookie-free to be publicly cacheable")
}

func TestOfflinePageIsPublicAndNeedsNoAuth(t *testing.T) {
	response := defaultITestApp(t).Get("/offline")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != "public, max-age=3600" {
		t.Errorf("Cache-Control = %q", got)
	}
	wantNoSetCookie(t, response, "the offline page is publicly cacheable")
	wantContains(t, response.Text(), "You are offline")
}

// The worker precaches this page from inside a signed-in session, and the
// Cache API honours no Cache-Control: a no-store response with a session cookie
// riding along would be stored on disk and replayed to the next person.
func TestOfflinePageStaysPublicForASignedInReader(t *testing.T) {
	response := authedApp(t).Get("/offline")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != "public, max-age=3600" {
		t.Errorf("a signed-in request must not make the offline page private, Cache-Control = %q", got)
	}
	wantNoSetCookie(t, response, "no session cookie may ride on a response the worker will cache")
}

func TestHealthCheck(t *testing.T) {
	response := defaultITestApp(t).Get("/health")

	wantStatus(t, response, http.StatusOK)
	if got := jsonField(t, response, "status"); got != "ok" {
		t.Errorf("status = %v", got)
	}
	if _, ok := jsonField(t, response, "git_version").(string); !ok {
		t.Error("git_version is not a string")
	}
}

func TestHealthCheckNoAuthRequired(t *testing.T) {
	wantStatus(t, defaultITestApp(t).Get("/health"), http.StatusOK)
}

// --- Subscription handler coverage ---

func TestSubscriptionListWithFeeds(t *testing.T) {
	app := authedApp(t)
	grImport(t, app, `<?xml version="1.0" encoding="UTF-8"?><opml version="2.0"><body><outline text="SubListCat">`+
		`<outline type="rss" text="SubList Feed" xmlUrl="https://sublist.example.com/feed.xml" htmlUrl="https://sublist.example.com"/>`+
		`</outline></body></opml>`)

	subscriptions := grSubscriptions(t, app)

	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	sub := subscriptions[0].(map[string]any)
	if sub["id"] != "feed/https://sublist.example.com/feed.xml" || sub["url"] != "https://sublist.example.com/feed.xml" {
		t.Errorf("id/url = %v / %v", sub["id"], sub["url"])
	}
	for _, field := range []string{"title", "sortid"} {
		if _, ok := sub[field].(string); !ok {
			t.Errorf("%s should be a string", field)
		}
	}
	if _, present := sub["iconUrl"]; !present {
		t.Error("iconUrl field should be present")
	}
	categories, _ := sub["categories"].([]any)
	if len(categories) != 1 {
		t.Fatalf("categories = %v, want one", sub["categories"])
	}
	category := categories[0].(map[string]any)
	if category["label"] != "SubListCat" || !strings.Contains(category["id"].(string), "SubListCat") {
		t.Errorf("category = %v", category)
	}
}

// ac=subscribe performs feed discovery before inserting; an unreachable URL is
// a 502 from discovery.
func TestSubscriptionEditSubscribeUnreachableUrl(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/subscription/edit", "ac", "subscribe", "s", "feed/https://unreachable.example.com/feed.xml")

	wantStatus(t, response, http.StatusBadGateway)
}

func TestSubscriptionEditUnknownAction(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/subscription/edit", "ac", "invalid", "s", "feed/https://example.com/feed.xml")

	wantStatus(t, response, http.StatusBadRequest)
}

func TestQuickaddEmptyUrl(t *testing.T) {
	app := authedApp(t)

	response := app.postPairs("/reader/api/0/subscription/quickadd", "quickadd", "")

	wantStatus(t, response, http.StatusBadRequest)
}

func TestSubscribedTrue(t *testing.T) {
	app := authedApp(t)
	grImport(t, app, opmlOf("SubdTrueCat", "https://subdtrue.example.com/feed.xml"))

	response := app.Get("/reader/api/0/subscribed?s=feed/https://subdtrue.example.com/feed.xml")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "true" {
		t.Errorf("body = %q, want true", response.Text())
	}
}

func TestSubscribedFalse(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/reader/api/0/subscribed?s=feed/https://nonexistent.com/feed.xml")

	wantStatus(t, response, http.StatusOK)
	if response.Text() != "false" {
		t.Errorf("body = %q, want false", response.Text())
	}
}

func TestSubscribedInvalidStream(t *testing.T) {
	app := authedApp(t)

	// "invalid" does not start with "feed/" so it fails validation.
	wantStatus(t, app.Get("/reader/api/0/subscribed?s=invalid"), http.StatusBadRequest)
}

func TestExportOpmlContentType(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/reader/api/0/subscription/export")

	wantStatus(t, response, http.StatusOK)
	if contentType := response.Header().Get("Content-Type"); !strings.Contains(contentType, "application/xml") {
		t.Errorf("Content-Type should be application/xml, got: %s", contentType)
	}
	disposition := response.Header().Get("Content-Disposition")
	if !strings.Contains(disposition, "attachment") || !strings.Contains(disposition, "subscriptions.opml") {
		t.Errorf("Content-Disposition should name the attachment subscriptions.opml, got: %s", disposition)
	}
}

func TestImportOpmlWithExistingCategory(t *testing.T) {
	app := authedApp(t)
	grCreateCategory(t, app, "PreExistingCat")

	grImport(t, app, opmlOf("PreExistingCat", "https://existingcat.example.com/feed.xml"))

	// No duplicate category: still exactly one folder tag with this name.
	matching := 0
	for _, name := range grFolderTagNames(t, app) {
		if name == "PreExistingCat" {
			matching++
		}
	}
	if matching != 1 {
		t.Errorf("want exactly 1 PreExistingCat category, got %d", matching)
	}
	subscriptions := grSubscriptions(t, app)
	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	categories := subscriptions[0].(map[string]any)["categories"].([]any)
	if categories[0].(map[string]any)["label"] != "PreExistingCat" {
		t.Errorf("categories = %v", categories)
	}
}

func TestSubscriptionEditEditTitle(t *testing.T) {
	app := authedApp(t)
	grImport(t, app, opmlOf("EditTitleCat", "https://edittitle.example.com/feed.xml"))

	response := app.postPairs("/reader/api/0/subscription/edit",
		"ac", "edit", "s", "feed/https://edittitle.example.com/feed.xml", "t", "NewTitle")

	wantStatus(t, response, http.StatusOK)
	subscriptions := grSubscriptions(t, app)
	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	if title := subscriptions[0].(map[string]any)["title"]; title != "NewTitle" {
		t.Errorf("title = %v, want NewTitle", title)
	}
}

// --- Auth handler coverage (ClientLogin, token, preference, friend) ---

func clientLogin(app *itestApp, email, password string) itestResponse {
	return app.postPairs("/accounts/ClientLogin", "Email", email, "Passwd", password)
}

func TestClientLoginSuccess(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("testuser", nil), http.StatusCreated)

	response := clientLogin(app, "testuser", fixturePassword)

	wantStatus(t, response, http.StatusOK)
	for _, want := range []string{"Auth=", "SID=", "LSID="} {
		if !strings.Contains(response.Text(), want) {
			t.Errorf("response should contain %s, got: %s", want, response.Text())
		}
	}
}

func TestClientLoginWrongPassword(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("testuser", nil), http.StatusCreated)

	wantStatus(t, clientLogin(app, "testuser", "wrongpassword"), http.StatusUnauthorized)
}

func TestClientLoginNonexistentUser(t *testing.T) {
	app := defaultITestApp(t)

	wantStatus(t, clientLogin(app, "nouser", fixturePassword), http.StatusUnauthorized)
}

func TestGreaderAuthHeader(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("testuser", nil), http.StatusCreated)
	login := clientLogin(app, "testuser", fixturePassword)
	wantStatus(t, login, http.StatusOK)
	var authToken string
	for _, line := range strings.Split(login.Text(), "\n") {
		if token, found := strings.CutPrefix(line, "Auth="); found {
			authToken = token
		}
	}
	if authToken == "" {
		t.Fatalf("no Auth= line in %q", login.Text())
	}

	response := app.Do(itestRequest{
		Path:    "/reader/api/0/user-info",
		Headers: map[string]string{"Authorization": "GoogleLogin auth=" + authToken},
	})

	wantStatus(t, response, http.StatusOK)
	if name := jsonOf(t, response.ResponseRecorder)["userName"]; name != "testuser" {
		t.Errorf("userName = %v", name)
	}
}

func TestGreaderInvalidAuthHeader(t *testing.T) {
	app := defaultITestApp(t)

	response := app.Do(itestRequest{
		Path:    "/reader/api/0/user-info",
		Headers: map[string]string{"Authorization": "GoogleLogin auth=invalidtoken"},
	})

	wantStatus(t, response, http.StatusUnauthorized)
}

func TestGetPostToken(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/reader/api/0/token")

	wantStatus(t, response, http.StatusOK)
	// The token format is "<timestamp>/<hmac_hex>".
	if token := response.Text(); token == "" || !strings.Contains(token, "/") {
		t.Errorf("POST token should be non-empty with a '/' separator, got: %q", token)
	}
}

func TestGreaderStubListsAreEmpty(t *testing.T) {
	app := authedApp(t)
	for path, want := range map[string]string{
		"preference/list":        `{"prefs":[]}`,
		"preference/stream/list": `{"streamprefs":{}}`,
		"friend/list":            `{"friends":[]}`,
	} {
		response := app.Get("/reader/api/0/" + path)

		wantStatus(t, response, http.StatusOK)
		if !jsonEqual(t, response.Text(), want) {
			t.Errorf("%s = %s, want %s", path, response.Text(), want)
		}
	}
}

// jsonEqual compares two JSON documents by value.
func jsonEqual(t *testing.T, got, want string) bool {
	t.Helper()
	var gotValue, wantValue any
	if err := json.Unmarshal([]byte(got), &gotValue); err != nil {
		t.Fatalf("not JSON: %s", got)
	}
	if err := json.Unmarshal([]byte(want), &wantValue); err != nil {
		t.Fatal(err)
	}
	return reflect.DeepEqual(gotValue, wantValue)
}

// --- Form-action handlers for the SSR /user-settings page. Each accepts a
// urlencoded body and answers 303 with a flash cookie plus Location. ---

func wantSeeOther(t *testing.T, response itestResponse, location string) {
	t.Helper()
	wantStatus(t, response, http.StatusSeeOther)
	if got := response.Header().Get("Location"); got != location {
		t.Errorf("Location = %q, want %q", got, location)
	}
}

func passwordChangeForm(app *itestApp, current, confirm string) itestResponse {
	return app.postPairs("/user-settings/password",
		"current_password", current, "new_password", "heron-lantern-53-drift", "confirm_password", confirm)
}

func TestChangePasswordForm(t *testing.T) {
	app := authedApp(t)

	// A mismatch changes nothing and stays on the page; the real change signs
	// the session out. In that order, so the first leaves the second something
	// to change.
	wantSeeOther(t, passwordChangeForm(app, fixturePassword, "differentvalue"), "/user-settings")
	wantSeeOther(t, passwordChangeForm(app, fixturePassword, "heron-lantern-53-drift"), "/login")
}

// Settings forms answer 303 back to /user-settings whether they saved or, as
// with entries_per_page=5 (below the minimum of 10), refused.
func TestSettingsFormsRedirectBackToThePage(t *testing.T) {
	app := authedApp(t)
	for _, form := range []struct {
		path  string
		pairs []string
	}{
		{"preferences", []string{"theme", "dark", "entries_per_page", "50", "retention_read_days", "0"}},
		{"preferences", []string{"theme", "system", "entries_per_page", "5", "retention_read_days", "0"}},
		{"linkding", []string{"api_url", "https://linkding.example.com", "api_token", "secret-token"}},
		{"kagi", []string{"session_link", "https://kagi.com/search?token=mysessiontoken", "language", "EN"}},
	} {
		wantSeeOther(t, app.postPairs("/user-settings/"+form.path, form.pairs...), "/user-settings")
	}
}

func postPreferences(app *itestApp, extra ...string) itestResponse {
	pairs := []string{"entries_per_page", "30", "retention_read_days", "0"}
	return app.postPairs("/user-settings/preferences", append(pairs, extra...)...)
}

func TestUpdatePreferencesFormStoresOnlyLightOrDarkTheme(t *testing.T) {
	app := authedApp(t)
	for _, c := range []struct{ submitted, stored string }{
		{"light", "light"}, {"dark", "dark"}, {"system", ""}, {"neon", ""},
	} {
		wantSeeOther(t, app.postPairs("/user-settings/preferences",
			"theme", c.submitted, "entries_per_page", "50", "retention_read_days", "0"), "/user-settings")

		// The layout renders the stored theme as a data-theme override.
		page := app.GetOK("/user-settings")
		rendered := ""
		for _, theme := range []string{"light", "dark"} {
			if strings.Contains(page, ` data-theme="`+theme+`"`) {
				rendered = theme
				break
			}
		}
		if rendered != c.stored {
			t.Errorf("%s: rendered theme %q, want %q", c.submitted, rendered, c.stored)
		}
	}
}

func TestUpdatePreferencesFormSetsTheTimezone(t *testing.T) {
	app := authedApp(t)
	shows := func(page, name string) bool {
		return strings.Contains(page, `name="timezone" value="`+name+`"`)
	}

	if !shows(app.GetOK("/user-settings"), "UTC") {
		t.Error("want UTC until the user chooses")
	}

	wantSeeOther(t, postPreferences(app, "theme", "system", "timezone", "Asia/Taipei"), "/user-settings")
	if !shows(app.GetOK("/user-settings"), "Asia/Taipei") {
		t.Error("want Asia/Taipei after choosing it")
	}

	// A form that omits the field leaves it alone.
	wantSeeOther(t, postPreferences(app, "theme", "system"), "/user-settings")
	if !shows(app.GetOK("/user-settings"), "Asia/Taipei") {
		t.Error("omitting the field changed the timezone")
	}

	// A name that is not a zone is refused, and the old choice survives.
	wantSeeOther(t, postPreferences(app, "theme", "system", "timezone", "Mars/Olympus_Mons"), "/user-settings")
	page := app.GetOK("/user-settings")
	if !shows(page, "Asia/Taipei") {
		t.Error("a refused zone replaced the old choice")
	}
	if !strings.Contains(page, "is not a timezone name") {
		t.Error("the refusal should be reported")
	}

	// The refusal comes before any write: other fields stay as they were.
	wantSeeOther(t, app.postPairs("/user-settings/preferences", "theme", "system", "entries_per_page", "55",
		"retention_read_days", "0", "timezone", "Mars/Olympus_Mons"), "/user-settings")
	if strings.Contains(app.GetOK("/user-settings"), `name="entries_per_page" value="55"`) {
		t.Error("a refused form still wrote entries_per_page")
	}
}

func TestUpdatePreferencesFormSetsSidebarPrefs(t *testing.T) {
	app := authedApp(t)
	sidebar := func() map[string]any {
		return jsonOf(t, app.Get("/api/sidebar").ResponseRecorder)
	}

	// Defaults before anything is submitted.
	if got := sidebar(); got["sidebar_sort"] != "name" || got["sidebar_hide_read"] != false {
		t.Errorf("defaults = %v", got)
	}

	wantSeeOther(t, postPreferences(app, "theme", "system", "sidebar_sort", "unread", "sidebar_hide_read", "1"), "/user-settings")
	if got := sidebar(); got["sidebar_sort"] != "unread" || got["sidebar_hide_read"] != true {
		t.Errorf("after submit = %v", got)
	}

	// An unchecked checkbox sends nothing, which must clear the flag rather
	// than leave it on, and an unknown sort falls back to the default.
	wantSeeOther(t, postPreferences(app, "theme", "system", "sidebar_sort", "nonsense"), "/user-settings")
	if got := sidebar(); got["sidebar_sort"] != "name" || got["sidebar_hide_read"] != false {
		t.Errorf("after unchecked = %v", got)
	}
}

// End to end for the credential-at-rest fix: what the form stores must not be
// legible in the database, and must still come back out through the app.
func TestASavedLinkdingTokenIsEncryptedInTheDatabase(t *testing.T) {
	app := authedApp(t)

	wantSeeOther(t, app.postPairs("/user-settings/linkding",
		"api_url", "https://linkding.example.com", "api_token", "SUPERSECRETTOKEN123"), "/user-settings")

	var stored string
	if err := app.DB.QueryRowContext(context.Background(), "SELECT save_services FROM user_settings LIMIT 1").Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if strings.Contains(stored, "SUPERSECRETTOKEN123") {
		t.Errorf("the token must not be legible in the column, got: %s", stored)
	}
	// And the page still reports it as configured: the value round-trips.
	if !strings.Contains(app.GetOK("/user-settings"), "linkding.example.com") {
		t.Error("the page lost the linkding URL")
	}
}

// --- session and API-token helpers ---

func userNamed(t *testing.T, app *itestApp, username string) *models.User {
	t.Helper()
	user, err := models.FindUserByUsername(context.Background(), app.DB, username)
	if err != nil || user == nil {
		t.Fatalf("user %s: %v, %v", username, user, err)
	}
	return user
}

func newSessionFor(t *testing.T, app *itestApp, userID int64, userAgent string) *models.Session {
	t.Helper()
	session, err := models.CreateSession(context.Background(), app.DB, models.NewSession{UserID: userID, UserAgent: userAgent, IPAddress: "127.0.0.1"})
	if err != nil {
		t.Fatal(err)
	}
	return session
}

func sessionExists(t *testing.T, app *itestApp, token string) bool {
	t.Helper()
	session, err := models.FindSessionByToken(context.Background(), app.DB, token)
	if err != nil {
		t.Fatal(err)
	}
	return session != nil
}

func newGreaderToken(t *testing.T, app *itestApp, userID int64, label string) *models.APIToken {
	t.Helper()
	token, err := models.CreateAPIToken(context.Background(), app.DB, models.NewAPIToken{
		UserID: userID, Kind: "greader", Label: label, UserAgent: "test-agent", IPAddress: "127.0.0.1",
	})
	if err != nil {
		t.Fatal(err)
	}
	return token
}

func apiTokenExists(t *testing.T, app *itestApp, token string) bool {
	t.Helper()
	found, err := models.FindAPIToken(context.Background(), app.DB, token)
	if err != nil {
		t.Fatal(err)
	}
	return found != nil
}

// adminApp is setup_admin_user: the first account ("admin", id 1) signed in.
func adminApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.Login("admin")
	return app
}

// seedTarget is register_target_user: "target" (id 2), straight in the database.
func seedTarget(app *itestApp) {
	app.SeedAccount("target", fixturePassword, models.RoleUser)
}

func TestRevokeOtherSessionsFormKeepsCurrentDeletesOthers(t *testing.T) {
	app := authedApp(t)
	other := newSessionFor(t, app, userNamed(t, app, "testuser").ID, "test-agent")

	wantSeeOther(t, app.request(http.MethodPost, "/user-settings/sessions/revoke-others"), "/user-settings")

	if sessionExists(t, app, other.SessionToken) {
		t.Error("other session should be deleted")
	}
	wantStatus(t, app.Get("/api/user"), http.StatusOK)
}

// /user-settings/* is not under the CSRF skip prefixes, so a bare POST is
// refused for its missing CSRF token before the auth check would answer 401.
func TestRevokeOtherSessionsFormUnauthenticatedRejected(t *testing.T) {
	app := defaultITestApp(t)

	wantStatus(t, app.request(http.MethodPost, "/user-settings/sessions/revoke-others"), http.StatusForbidden)
}

// While masquerading the effective session belongs to the target, so
// revoke-others must refuse or an admin would delete the target's real
// sessions on their own devices.
func TestRevokeOtherSessionsFormBlockedWhileMasquerading(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	targetSession := newSessionFor(t, app, 2, "test-agent")
	started := app.request(http.MethodPost, "/admin/users/2/masquerade")
	wantStatus(t, started, http.StatusSeeOther)
	// The privilege change rotates the session token, so the CSRF header has
	// to follow the cookies this response sets.
	app.ApplyCSRF(started)

	wantSeeOther(t, app.request(http.MethodPost, "/user-settings/sessions/revoke-others"), "/user-settings")

	if !sessionExists(t, app, targetSession.SessionToken) {
		t.Error("masquerade must not delete the target's real sessions")
	}
}

func TestRevokeSessionFormDeletesOnlyTheNamedSession(t *testing.T) {
	app := authedApp(t)
	userID := userNamed(t, app, "testuser").ID
	doomed := newSessionFor(t, app, userID, "phone")
	bystander := newSessionFor(t, app, userID, "laptop")

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/user-settings/sessions/%d/revoke", doomed.ID)), "/user-settings")

	if sessionExists(t, app, doomed.SessionToken) {
		t.Error("the named session should be deleted")
	}
	if !sessionExists(t, app, bystander.SessionToken) {
		t.Error("revoking one session must not touch the user's other sessions")
	}
	wantStatus(t, app.Get("/api/user"), http.StatusOK)
}

// The settings page renders no Revoke control for the caller's own session,
// but that is a UI affordance, so a hand-crafted POST naming it must be refused.
func TestRevokeSessionFormRefusesTheCurrentSession(t *testing.T) {
	app := authedApp(t)
	sessions, err := models.ListUserSessions(context.Background(), app.DB, userNamed(t, app, "testuser").ID)
	if err != nil || len(sessions) == 0 {
		t.Fatalf("the authenticated caller has a session: %v, %v", sessions, err)
	}
	current := sessions[0]

	wantStatus(t, app.request(http.MethodPost, fmt.Sprintf("/user-settings/sessions/%d/revoke", current.ID)), http.StatusSeeOther)

	if !sessionExists(t, app, current.SessionToken) {
		t.Error("the caller's own session must survive")
	}
	wantStatus(t, app.Get("/api/user"), http.StatusOK)
}

func TestRevokeSessionFormBlockedWhileMasquerading(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	targetSession := newSessionFor(t, app, 2, "test-agent")
	started := app.request(http.MethodPost, "/admin/users/2/masquerade")
	wantStatus(t, started, http.StatusSeeOther)
	app.ApplyCSRF(started)

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/user-settings/sessions/%d/revoke", targetSession.ID)), "/user-settings")

	if !sessionExists(t, app, targetSession.SessionToken) {
		t.Error("masquerade must not delete the target's real sessions")
	}
}

func TestPasswordChangeRevokesApiTokens(t *testing.T) {
	app := authedApp(t)
	token := newGreaderToken(t, app, userNamed(t, app, "testuser").ID, "test-client")

	wantStatus(t, passwordChangeForm(app, fixturePassword, "heron-lantern-53-drift"), http.StatusSeeOther)

	if apiTokenExists(t, app, token.Token) {
		t.Error("a password change must revoke API tokens too, not just browser sessions")
	}
}

// Changing a password verifies the current one with Argon2. Once the budget is
// spent even the CORRECT password is refused, which holds only if the limiter
// runs before the verification.
func TestPasswordChangeIsRateLimited(t *testing.T) {
	app := authedApp(t)
	for range 5 {
		passwordChangeForm(app, "wrongpassword", "heron-lantern-53-drift")
	}

	passwordChangeForm(app, fixturePassword, "heron-lantern-53-drift")

	user := userNamed(t, app, "testuser")
	if !auth.VerifyPassword(fixturePassword, user.PasswordHash) {
		t.Error("a throttled request must not have changed the password")
	}
	// The change-password budget is its own: it must not spend the login budget.
	login := app.PostJSON("/api/session", map[string]string{"username": "testuser", "password": fixturePassword})
	if login.Code == http.StatusTooManyRequests {
		t.Error("the password-change bucket must not spend the login budget")
	}
}

// "Sign out other sessions" means browser sessions: a GReader client's token
// must survive it.
func TestRevokeOthersDoesNotTouchApiTokens(t *testing.T) {
	app := authedApp(t)
	token := newGreaderToken(t, app, userNamed(t, app, "testuser").ID, "test-client")

	wantStatus(t, app.request(http.MethodPost, "/user-settings/sessions/revoke-others"), http.StatusSeeOther)

	if !apiTokenExists(t, app, token.Token) {
		t.Error("revoke-other-sessions must not touch API tokens")
	}
}

// A bulk revocation has to say how much it revoked: "Signed out all other
// sessions." reads the same whether it ended six sessions or none.
func TestRevokeOthersFlashCountsTheSessionsItEnded(t *testing.T) {
	app := authedApp(t)
	userID := userNamed(t, app, "testuser").ID
	revokeOthers := func() string {
		response := app.request(http.MethodPost, "/user-settings/sessions/revoke-others")
		wantStatus(t, response, http.StatusSeeOther)
		return flashText(t, response)
	}

	if text := revokeOthers(); !strings.Contains(text, "No other sessions were signed in") {
		t.Errorf("revoking nothing must say so, got: %s", text)
	}
	for range 2 {
		newSessionFor(t, app, userID, "other-agent")
	}
	if text := revokeOthers(); !strings.Contains(text, "Signed out 2 other sessions.") {
		t.Errorf("flash must carry the affected-row count, got: %s", text)
	}
	// The caller's own session survives, so a repeat is back to the zero case.
	if text := revokeOthers(); !strings.Contains(text, "No other sessions were signed in") {
		t.Errorf("repeat = %s", text)
	}
}

// Singular wording, and the same count-or-say-nothing rule as sessions.
func TestRevokeAllApiTokensFlashCountsTheTokens(t *testing.T) {
	app := authedApp(t)
	userID := userNamed(t, app, "testuser").ID
	revokeAll := func() string {
		response := app.request(http.MethodPost, "/user-settings/api-tokens/revoke-all")
		wantStatus(t, response, http.StatusSeeOther)
		return flashText(t, response)
	}

	if text := revokeAll(); !strings.Contains(text, "There were no API tokens to revoke.") {
		t.Errorf("got: %s", text)
	}
	newGreaderToken(t, app, userID, "only-client")
	if text := revokeAll(); !strings.Contains(text, "Revoked 1 API token.") {
		t.Errorf("one token must read as singular, got: %s", text)
	}
}

func TestUpdateRoleFormPromotesUser(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)

	wantSeeOther(t, app.postPairs("/admin/users/2/role", "role", "admin"), "/admin")

	// Two admins now: the action button on a non-self admin row says "demote".
	body := app.GetOK("/admin")
	if !strings.Contains(body, "target") || !strings.Contains(body, "demote") {
		t.Error("the target row should now be an admin row with a demote button")
	}
}

func TestUpdateStatusFormDisablesUser(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)

	wantSeeOther(t, app.postPairs("/admin/users/2/status", "disabled", "true"), "/admin")

	login := app.PostJSON("/api/session", map[string]string{"username": "target", "password": fixturePassword})
	wantStatus(t, login, http.StatusForbidden)
}

func TestStartMasqueradeFormRedirectsToRoot(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)

	wantSeeOther(t, app.request(http.MethodPost, "/admin/users/2/masquerade"), "/")
}

func TestDeleteUserFormSucceeds(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)

	wantSeeOther(t, app.request(http.MethodPost, "/admin/users/2/delete"), "/admin")

	if strings.Contains(app.GetOK("/admin"), ">target<") {
		t.Error("the target row should be gone")
	}
}

func TestUpdateRoleFormSelfProtection(t *testing.T) {
	app := adminApp(t)

	// Changing your own role is blocked with an error flash, not a 400.
	wantSeeOther(t, app.postPairs("/admin/users/1/role", "role", "user"), "/admin")

	// The admin's own row carries the "(you)" marker and no role toggle.
	body := app.GetOK("/admin")
	if !strings.Contains(body, "(you)") || strings.Contains(body, "promote") {
		t.Error("the admin role should be unchanged")
	}
}

// --- /categories form-action POST endpoints ---

func queryInt(t *testing.T, app *itestApp, query string, args ...any) int64 {
	t.Helper()
	var value int64
	if err := app.DB.QueryRowContext(context.Background(), query, args...).Scan(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

func queryString(t *testing.T, app *itestApp, query string, args ...any) string {
	t.Helper()
	var value string
	if err := app.DB.QueryRowContext(context.Background(), query, args...).Scan(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

func TestCreateCategoryFormSucceeds(t *testing.T) {
	app := authedApp(t)

	wantSeeOther(t, app.postPairs("/categories", "name", "Tech"), "/categories")

	if queryInt(t, app, "SELECT COUNT(*) FROM category WHERE name = $1", "Tech") == 0 {
		t.Error("category 'Tech' should exist in the DB after creation")
	}
}

func TestCreateCategoryFormEmptyName(t *testing.T) {
	app := authedApp(t)

	// An error flash redirect: still 303 to /categories, no DB write.
	wantSeeOther(t, app.postPairs("/categories", "name", ""), "/categories")

	// Only the seeded "Uncategorized" remains.
	if count := queryInt(t, app, "SELECT COUNT(*) FROM category"); count != 1 {
		t.Errorf("categories = %d, want 1 (only the seeded default)", count)
	}
}

func TestRenameCategoryFormSucceeds(t *testing.T) {
	app := authedApp(t)
	wantSeeOther(t, app.postPairs("/categories", "name", "OldName"), "/categories")
	categoryID := queryInt(t, app, "SELECT id FROM category WHERE name = $1", "OldName")

	wantSeeOther(t, app.postPairs(fmt.Sprintf("/categories/%d/rename", categoryID), "name", "NewName"), "/categories")

	if name := queryString(t, app, "SELECT name FROM category WHERE id = $1", categoryID); name != "NewName" {
		t.Errorf("name = %q, want NewName", name)
	}
}

func TestDeleteCategoryFormSucceeds(t *testing.T) {
	app := authedApp(t)
	wantSeeOther(t, app.postPairs("/categories", "name", "ToDelete"), "/categories")
	categoryID := queryInt(t, app, "SELECT id FROM category WHERE name = $1", "ToDelete")

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/categories/%d/delete", categoryID)), "/categories")

	if queryInt(t, app, "SELECT COUNT(*) FROM category WHERE id = $1", categoryID) != 0 {
		t.Error("category should be deleted from the DB")
	}
}

// --- /feeds form-action POST endpoints ---

func TestCreateFeedFormEmptyUrl(t *testing.T) {
	app := authedApp(t)

	wantSeeOther(t, app.postPairs("/feeds", "url", "", "category_id", "1"), "/feeds")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed") != 0 {
		t.Error("an empty url must not create a feed")
	}
}

func TestCreateFeedFormInvalidCategory(t *testing.T) {
	app := authedApp(t)

	wantSeeOther(t, app.postPairs("/feeds", "url", "https://example.com/feed.xml", "category_id", "999999"), "/feeds")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed") != 0 {
		t.Error("no feed should be created when the category is invalid")
	}
}

func TestEditFeedFormSucceeds(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://example.com/feed.xml")

	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		"url", "https://example.com/feed.xml", "title", "Renamed Feed", "description", "New description",
		"site_url", "https://example.com", "category_id", fmt.Sprint(categoryID),
		"custom_user_agent", "", "custom_referrer", "")

	wantSeeOther(t, response, "/feeds")
	if title := queryString(t, app, "SELECT title FROM feed WHERE id = $1", feedID); title != "Renamed Feed" {
		t.Errorf("title = %q", title)
	}
	if description := queryString(t, app, "SELECT description FROM feed WHERE id = $1", feedID); description != "New description" {
		t.Errorf("description = %q", description)
	}
}

func TestEditFeedFormReturnsToTheFilteredList(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://return-to.example.com/feed.xml")
	back := fmt.Sprintf("/feeds?category=%d&sort=unread&filter=all", categoryID)
	edit := func(feedURL, returnTo string) itestResponse {
		return app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
			"url", feedURL, "category_id", fmt.Sprint(categoryID), "return_to", returnTo)
	}

	// Success lands on the list the edit link came from.
	wantSeeOther(t, edit("https://return-to.example.com/feed.xml", back), back)

	// An error stays on the edit page, still carrying the list.
	wantSeeOther(t, edit("", back), fmt.Sprintf(
		"/feeds/%d/edit?return_to=%%2Ffeeds%%3Fcategory%%3D%d%%26sort%%3Dunread%%26filter%%3Dall", feedID, categoryID))

	// Off-site or off-list targets fall back to the bare list.
	for _, evil := range []string{"https://evil.example/feeds", "//evil.example/feeds", "/admin"} {
		if got := edit("https://return-to.example.com/feed.xml", evil).Header().Get("Location"); got != "/feeds" {
			t.Errorf("%s: Location = %q, want /feeds", evil, got)
		}
	}
}

func TestFeedRowFormsReturnToTheFilteredList(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://row-forms.example.com/feed.xml")
	encodedBack := fmt.Sprintf("%%2Ffeeds%%3Fcategory%%3D%d%%26sort%%3Dunread%%26filter%%3Dall", categoryID)
	page := app.GetOK(fmt.Sprintf("/feeds?category=%d&sort=unread&filter=all", categoryID))
	back := fmt.Sprintf("/feeds?category=%d&amp;sort=unread&amp;filter=all", categoryID)

	if !strings.Contains(page, fmt.Sprintf(`name="return_to" value="%s"`, back)) {
		t.Error("row forms carry the filtered view")
	}
	if !strings.Contains(page, fmt.Sprintf("/feeds/%d/edit?return_to=%s", feedID, encodedBack)) {
		t.Error("the edit link carries the filtered view")
	}
	editPage := app.GetOK(fmt.Sprintf("/feeds/%d/edit?return_to=%s", feedID, encodedBack))
	if !strings.Contains(editPage, fmt.Sprintf(`<a href="%s" class="btn btn-secondary">Cancel</a>`, back)) {
		t.Error("Cancel returns to the filtered list")
	}

	returnTo := fmt.Sprintf("/feeds?category=%d&filter=errors", categoryID)
	wantSeeOther(t, app.postPairs(fmt.Sprintf("/feeds/%d/delete", feedID), "return_to", returnTo), returnTo)
}

func TestEditFeedFormChangesCategory(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", "https://example.com/feed.xml")
	other, err := models.CreateCategory(context.Background(), app.DB, 1, "Other")
	if err != nil {
		t.Fatal(err)
	}

	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		"url", "https://example.com/feed.xml", "title", "Test Feed", "description", "", "site_url", "",
		"category_id", fmt.Sprint(other.ID), "custom_user_agent", "", "custom_referrer", "")

	wantStatus(t, response, http.StatusSeeOther)
	if got := queryInt(t, app, "SELECT category_id FROM feed WHERE id = $1", feedID); got != other.ID {
		t.Errorf("category_id = %d, want %d", got, other.ID)
	}
}

func TestDeleteFeedFormSucceeds(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", "https://example.com/feed.xml")

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/delete", feedID)), "/feeds")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE id = $1", feedID) != 0 {
		t.Error("the feed should be deleted")
	}
}

// otherUsersFeed is a feed owned by a second account, made behind the router's back.
func otherUsersFeed(t *testing.T, app *itestApp) int64 {
	t.Helper()
	ctx := context.Background()
	other, err := models.CreateUser(ctx, app.DB, "other", "x", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	category, err := models.CreateCategory(ctx, app.DB, other.ID, "Other")
	if err != nil {
		t.Fatal(err)
	}
	title := "Other"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://other.example.com/feed.xml", Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	return feed.ID
}

func TestDeleteFeedFormNotOwned(t *testing.T) {
	app := authedApp(t)
	otherFeedID := otherUsersFeed(t, app)

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/delete", otherFeedID)), "/feeds")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE id = $1", otherFeedID) != 1 {
		t.Error("non-owner delete must not remove the feed")
	}
}

func TestRefreshFeedFormNotOwned(t *testing.T) {
	app := authedApp(t)
	otherFeedID := otherUsersFeed(t, app)

	// The ownership check fails first, so no network call is attempted.
	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/refresh", otherFeedID)), "/feeds")
}

func multipartUpload(t *testing.T, parts map[string]string) (body *bytes.Buffer, contentType string) {
	t.Helper()
	body = &bytes.Buffer{}
	writer := multipart.NewWriter(body)
	for name, content := range parts {
		part, err := writer.CreateFormFile(name, "subscriptions.opml")
		if err != nil {
			t.Fatal(err)
		}
		_, _ = part.Write([]byte(content))
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	return body, writer.FormDataContentType()
}

func postMultipart(app *itestApp, path string, body *bytes.Buffer, contentType string) itestResponse {
	return app.Do(itestRequest{Method: http.MethodPost, Path: path, Body: body.String(), Headers: map[string]string{"Content-Type": contentType}})
}

func TestImportOpmlFormEmpty(t *testing.T) {
	app := authedApp(t)
	body, contentType := multipartUpload(t, nil)

	wantSeeOther(t, postMultipart(app, "/feeds/import", body, contentType), "/feeds/import")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed") != 0 {
		t.Error("an empty upload must not create feeds")
	}
}

func TestImportOpmlFormSucceeds(t *testing.T) {
	app := authedApp(t)
	body, contentType := multipartUpload(t, map[string]string{"file": `<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>Test Subscriptions</title></head>
  <body>
    <outline text="Tech" title="Tech">
      <outline type="rss" text="Example Feed" title="Example Feed"
               xmlUrl="https://example.com/feed.xml" htmlUrl="https://example.com"/>
    </outline>
  </body>
</opml>`})

	wantSeeOther(t, postMultipart(app, "/feeds/import", body, contentType), "/feeds")

	if queryInt(t, app, "SELECT COUNT(*) FROM category WHERE name = 'Tech'") != 1 {
		t.Error("want one Tech category")
	}
	if queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE url = 'https://example.com/feed.xml'") != 1 {
		t.Error("want the imported feed")
	}
}

// --- GET /entries/{id}/fragment ---

// fragmentEntry is a signed-in reader who owns one entry in one feed.
func fragmentEntry(t *testing.T, username, feedURL, guid string) (owner libraryOwner, entryID int64) {
	t.Helper()
	owner = newLibraryOwner(t, username)
	feedID := owner.feed(t, owner.category(t, "T"), feedURL, "Test Feed")
	entryID = owner.entryWithContent(t, feedID, guid, "Hello World", "https://x/post", "<p>Body text here</p>")
	return owner, entryID
}

func TestEntryFragmentRendersReadingPane(t *testing.T) {
	owner, entryID := fragmentEntry(t, "alice_frag", "https://x/feed", "guid-frag-test")
	app := owner.app

	response := app.Get(fmt.Sprintf("/entries/%d/fragment", entryID))

	wantStatus(t, response, http.StatusOK)
	if contentType := response.Header().Get("Content-Type"); !strings.HasPrefix(contentType, "text/html") {
		t.Errorf("expected text/html, got: %s", contentType)
	}
	html := response.Text()
	row := fmt.Sprintf("#entry-row-%d", entryID)
	for what, want := range map[string]string{
		"reading-pane id":    `id="reading-pane"`,
		"entry title":        "Hello World",
		"entry body":         "Body text here",
		"favicon chip":       "entry-favicon-chip",
		"shared action":      `class="rp-action"`,
		"accessible name":    `aria-label="Mark Unread"`,
		"marker form swap":   fmt.Sprintf(`data-swap-target="%s .entry-marker"`, row),
		"row read directive": fmt.Sprintf(`<template data-class-target="%s" data-class-add="entry-read">`, row),
	} {
		if !strings.Contains(html, want) {
			t.Errorf("%s: %q missing", what, want)
		}
	}
	if strings.Contains(html, fmt.Sprintf(`<div id="entry-row-%d"`, entryID)) {
		t.Error("the whole row must not be re-sent, only the sub-elements that changed")
	}
	// The sidebar's counts travel over SSE, not in this response.
	if strings.Contains(html, "sidebar-unread") {
		t.Error("the unconsumed sidebar-unread payload must not be reintroduced")
	}
	// The mark-as-read write is detached, so poll until it lands.
	if !waitForRead(t, app, entryID) {
		t.Error("entry must be marked read after open")
	}
}

// A real top-level navigation to the partial-only /fragment route must
// redirect to the full entries page rather than serving bare <template>
// blocks, which render blank. It still marks the entry read on the way out.
func TestEntryFragmentRedirectsOnTopLevelNavigation(t *testing.T) {
	owner, entryID := fragmentEntry(t, "doc_nav", "https://x/feed", "guid-doc-nav")
	app := owner.app

	response := app.Do(itestRequest{
		Path:    fmt.Sprintf("/entries/%d/fragment", entryID),
		Headers: map[string]string{"Sec-Fetch-Dest": "document"},
	})

	wantSeeOther(t, response, fmt.Sprintf("/entries?entry=%d", entryID))
	if !waitForRead(t, app, entryID) {
		t.Error("a scriptless open must mark the entry read, like the fetch() path")
	}
}

// A prefetch or prerender is the browser guessing, not the reader opening
// something: it must not mark anything read, and its response must not be
// storable, or the real click could be served from the prefetch cache.
func TestEntryFragmentSpeculativeLoadDoesNotMarkRead(t *testing.T) {
	owner, entryID := fragmentEntry(t, "spec_load", "https://x/spec-feed", "guid-spec")
	app := owner.app
	path := fmt.Sprintf("/entries/%d/fragment", entryID)

	// A prerender sends "prefetch;prerender", which the substring test also has to catch.
	for _, purpose := range []string{"prefetch", "prefetch;prerender"} {
		response := app.Do(itestRequest{Path: path, Headers: map[string]string{"Sec-Purpose": purpose}})
		wantStatus(t, response, http.StatusOK)
		if got := response.Header().Get("Cache-Control"); got != "no-store" {
			t.Errorf("%s: Cache-Control = %q, want no-store", purpose, got)
		}
	}
	// The same request as a scriptless navigation covers the redirect branch.
	response := app.Do(itestRequest{Path: path, Headers: map[string]string{"Sec-Purpose": "prefetch", "Sec-Fetch-Dest": "document"}})
	wantStatus(t, response, http.StatusSeeOther)
	if got := response.Header().Get("Cache-Control"); got != "no-store" {
		t.Errorf("redirect Cache-Control = %q, want no-store", got)
	}

	// The write is detached: give it the window a real one would have had.
	time.Sleep(150 * time.Millisecond)
	if isRead(t, app, entryID) {
		t.Error("a speculative load must leave the entry unread")
	}
	// And the reader's real open still works.
	wantStatus(t, app.Get(path), http.StatusOK)
	if !waitForRead(t, app, entryID) {
		t.Error("a genuine open after a prefetch must still mark read")
	}
}

// A top-level navigation carrying a Referer from a scoped list page must
// redirect back into that scope with the pane pre-opened. An id that resolves
// to nothing still redirects rather than 404s.
func TestEntryFragmentDocumentNavPreservesRefererScope(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.setupJSON("ref_scope", nil), http.StatusCreated)
	app.Login("ref_scope")

	response := app.Do(itestRequest{
		Path: "/entries/123/fragment",
		Headers: map[string]string{
			"Sec-Fetch-Dest": "document",
			"Referer":        "https://testserver/categories/4/entries?q=rust",
		},
	})

	wantSeeOther(t, response, "/categories/4/entries?q=rust&entry=123")
}

const (
	proxyTestImageURL    = "https://example.com/a.png"
	proxyTestImageURLB64 = "aHR0cHM6Ly9leGFtcGxlLmNvbS9hLnBuZw"
)

// The media proxy serves a stable ETag (the per-URL request signature). A
// conditional request that already holds it gets a 304 with no origin fetch.
func TestProxyImage304OnIfNoneMatch(t *testing.T) {
	app := defaultITestApp(t)
	// Signed with the test config's secret: the 304 short-circuit sits behind
	// signature verification.
	signature := imageproxy.SignURL(make([]byte, 32), proxyTestImageURL)

	response := app.Do(itestRequest{
		Path:    "/api/proxy/image?url=" + proxyTestImageURLB64 + "&s=" + signature,
		Headers: map[string]string{"If-None-Match": `"` + signature + `"`},
	})

	wantStatus(t, response, http.StatusNotModified)
	if etag := response.Header().Get("ETag"); etag != `"`+signature+`"` {
		t.Errorf("ETag = %q", etag)
	}
}

// The 304 short-circuit must not answer ahead of the signature check, or
// anyone could get back a cacheable 304 whose ETag echoed their own input.
func TestProxyImage304RequiresAValidSignature(t *testing.T) {
	app := defaultITestApp(t)
	for _, signature := range []string{"sometoken", "*"} {
		response := app.Do(itestRequest{
			Path:    "/api/proxy/image?url=" + proxyTestImageURLB64 + "&s=" + url.QueryEscape(signature),
			Headers: map[string]string{"If-None-Match": "*"},
		})

		wantStatus(t, response, http.StatusBadRequest)
		if etag := response.Header().Get("ETag"); etag != "" {
			t.Errorf("a rejected request must not echo `s` back as an ETag, got %q", etag)
		}
	}
}

func TestEntryFragment404ForOtherUser(t *testing.T) {
	owner := newLibraryOwner(t, "alice_404")
	app := owner.app
	// Bob and his entry go straight into the database; he never logs in.
	bob, err := models.CreateUser(context.Background(), app.DB, "bob_404", "x", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	bobCategory, err := models.CreateCategory(context.Background(), app.DB, bob.ID, "T")
	if err != nil {
		t.Fatal(err)
	}
	bobOwner := libraryOwner{app: app, userID: bob.ID}
	bobFeed := bobOwner.feed(t, bobCategory.ID, "https://b/feed", "Bob Feed")
	bobEntry := bobOwner.entry(t, bobFeed, "guid-bob-entry", "Bob's Entry", "https://b/post")

	response := app.Get(fmt.Sprintf("/entries/%d/fragment", bobEntry))

	wantStatus(t, response, http.StatusNotFound)
}

// --- POST /entries/{id}/star, unstar, read, unread: idempotent form actions ---

// singleEntryOwner is a signed-in reader with one entry titled "E".
func singleEntryOwner(t *testing.T, username, feedURL, guid string) (app *itestApp, entryID int64) {
	t.Helper()
	owner := newLibraryOwner(t, username)
	feedID := owner.feed(t, owner.category(t, "T"), feedURL, "Test Feed")
	return owner.app, owner.entry(t, feedID, guid, "E", "https://x/p")
}

func postActionHTML(t *testing.T, app *itestApp, entryID int64, action string) string {
	t.Helper()
	response := app.request(http.MethodPost, fmt.Sprintf("/entries/%d/%s", entryID, action))
	wantStatus(t, response, http.StatusOK)
	return response.Text()
}

func TestStarEntryFormIsIdempotentMarkStarred(t *testing.T) {
	app, entryID := singleEntryOwner(t, "alice_star", "https://x/star-feed", "guid-star-test")

	// The first /star is a real state change.
	html := postActionHTML(t, app, entryID, "star")

	for what, want := range map[string]string{
		"multi-target row block":     `data-swap-target="#entry-row-`,
		"Unstar on the row toggle":   `aria-label="Unstar"`,
		"pane star form swap target": `data-swap-target="#reading-pane-star-form-`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("%s: %q missing", what, want)
		}
	}
	if strings.Contains(html, "sidebar-unread") {
		t.Error("the unconsumed sidebar-unread payload must not be reintroduced")
	}
	// The second /star is idempotent: the entry stays starred, no toggle back.
	if second := postActionHTML(t, app, entryID, "star"); !strings.Contains(second, `aria-label="Unstar"`) {
		t.Error("second /star call must be a no-op: the star toggle must still show Unstar")
	}
}

func TestUnstarEntryFormIsIdempotentMarkUnstarred(t *testing.T) {
	app, entryID := singleEntryOwner(t, "alice_unstar", "https://x/unstar-feed", "guid-unstar-test")
	// Already starred, so the first /unstar is a real state change.
	if _, err := entry.StarEntry(context.Background(), app.DB, entryID); err != nil {
		t.Fatal(err)
	}

	html := postActionHTML(t, app, entryID, "unstar")

	if strings.Contains(html, `aria-label="Unstar"`) {
		t.Error("row star toggle must show Star, not Unstar, after /unstar")
	}
	if !strings.Contains(html, ">Star<") {
		t.Error("pane-star-form swap must render the Star label after unstar")
	}
	if second := postActionHTML(t, app, entryID, "unstar"); strings.Contains(second, `aria-label="Unstar"`) {
		t.Error("second /unstar call must be a no-op: the row must still be unstarred")
	}
}

func TestReadEntryFormIsIdempotentMarkRead(t *testing.T) {
	app, entryID := singleEntryOwner(t, "alice_read", "https://x/read-feed", "guid-read-test")

	html := postActionHTML(t, app, entryID, "read")

	if !strings.Contains(html, `data-swap-target="#entry-row-`) {
		t.Error("multi-target row block must be present")
	}
	if strings.Contains(html, "sidebar-unread") {
		t.Error("the unconsumed sidebar-unread payload must not be reintroduced")
	}
	// The row's read state travels as a class directive applied to the row the
	// client already has.
	if !strings.Contains(html, `data-class-add="entry-read"`) {
		t.Error("row must be marked read via the class directive after first call")
	}
	if !strings.Contains(html, `action="/entries/`) || !strings.Contains(html, `/unread"`) {
		t.Error("the swapped marker form must now offer the inverse (unread) action")
	}
	// The second /read is idempotent: the entry stays read.
	second := postActionHTML(t, app, entryID, "read")
	if !strings.Contains(second, `data-class-add="entry-read"`) {
		t.Error("second /read call must be a no-op: the row must still be marked read")
	}
	if strings.Contains(second, `data-class-remove="entry-read"`) {
		t.Error("second /read call must not flip the row back to unread")
	}
}

func TestUnreadEntryFormIsIdempotentMarkUnread(t *testing.T) {
	app, entryID := singleEntryOwner(t, "alice_unr", "https://x/unread-feed", "guid-unread-test")
	// Already read, so the first /unread is a real state change.
	if _, err := entry.MarkAsRead(context.Background(), app.DB, entryID); err != nil {
		t.Fatal(err)
	}

	html := postActionHTML(t, app, entryID, "unread")

	if !strings.Contains(html, `data-class-remove="entry-read"`) {
		t.Error("row must be told to drop .entry-read after /unread")
	}
	if !strings.Contains(html, "Marked as unread.") {
		t.Error("the first /unread must emit the flash")
	}
	// The second /unread is a no-op and must not re-emit the flash, which
	// would spam the user on stale-label re-clicks.
	second := postActionHTML(t, app, entryID, "unread")
	if !strings.Contains(second, `data-class-remove="entry-read"`) {
		t.Error("second /unread call must be a no-op: the row must still be unread")
	}
	if strings.Contains(second, "Marked as unread.") {
		t.Error("no-op /unread must not re-emit the flash")
	}
}

// --- POST /entries/{id}/star, unstar, read, unread: cross-tenant 404 ---

// otherUsersEntry is an entry owned by a second account made behind the
// router's back, so the signed-in session stays the first account's.
func otherUsersEntry(t *testing.T, app *itestApp, username, feedURL, guid string) int64 {
	t.Helper()
	other, err := models.CreateUser(context.Background(), app.DB, username, "x", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	bob := libraryOwner{app: app, userID: other.ID}
	feedID := bob.feed(t, bob.category(t, "Bob Cat"), feedURL, "Bob Feed")
	return bob.entry(t, feedID, guid, "Bob Entry", "https://bob/entry")
}

func TestStarEntryForm404ForOtherUser(t *testing.T) {
	owner := newLibraryOwner(t, "alice_s404")
	bobEntryID := otherUsersEntry(t, owner.app, "bob_s404", "https://bob/star-feed", "guid-bob-star")

	for _, action := range []string{"star", "unstar"} {
		response := owner.app.request(http.MethodPost, fmt.Sprintf("/entries/%d/%s", bobEntryID, action))
		if response.Code != http.StatusNotFound {
			t.Errorf("cross-user %s = %d, want 404", action, response.Code)
		}
	}
}

func TestReadEntryForm404ForOtherUser(t *testing.T) {
	owner := newLibraryOwner(t, "alice_r404")
	bobEntryID := otherUsersEntry(t, owner.app, "bob_r404", "https://bob/read-feed", "guid-bob-read")

	for _, action := range []string{"read", "unread"} {
		response := owner.app.request(http.MethodPost, fmt.Sprintf("/entries/%d/%s", bobEntryID, action))
		if response.Code != http.StatusNotFound {
			t.Errorf("cross-user %s = %d, want 404", action, response.Code)
		}
	}
}

// POST /entries/{id}/summarize returns a small multi-target swap that only
// updates #rp-summary-container, so a Fetch-Full-Content view keeps its
// externally-fetched article body.
func TestSummarizeEntryFormRendersSummaryPendingFragment(t *testing.T) {
	owner := newLibraryOwner(t, "alice_sum")
	feedID := owner.feed(t, owner.category(t, "T"), "https://x/sum-feed", "Sum Feed")
	entryID := owner.entryWithContent(t, feedID, "guid-sum-test", "Summarizable Entry", "https://x/sum-post", "<p>Content to summarize</p>")

	response := owner.app.request(http.MethodPost, fmt.Sprintf("/entries/%d/summarize", entryID))

	wantStatus(t, response, http.StatusOK)
	html := response.Text()
	if !strings.Contains(html, `data-swap-target="#rp-summary-container"`) {
		t.Error("response must target the summary container, not the whole reading pane")
	}
	if !strings.Contains(html, "Summarizing") {
		t.Error("pending fragment must show a 'Summarizing…' message")
	}
	if strings.Contains(html, "reading-pane-article") {
		t.Error("response must NOT swap the article body (would reset full-content view)")
	}
}

// --- GET /entries?fragment=1&after=...: Load-More fragment ---

func TestEntriesLoadMoreReturnsRowFragments(t *testing.T) {
	owner := newLibraryOwner(t, "alice_lm")
	// Pinned rather than left to the default: this test is about append
	// semantics, and the row counts below should not move with the default.
	owner.setEntriesPerPage(t, 50)
	feedID := owner.feed(t, owner.category(t, "LoadMore Cat"), "https://lm/feed", "LM Feed")
	for i := 0; i < 75; i++ {
		owner.entry(t, feedID, fmt.Sprintf("guid-lm-%d", i), fmt.Sprintf("LM Entry %d", i), fmt.Sprintf("https://lm/%d", i))
	}

	page1 := owner.app.Get("/entries")
	wantStatus(t, page1, http.StatusOK)
	_, afterToken, found := strings.Cut(page1.Text(), `name="after" value="`)
	if !found {
		t.Fatal("page 1 should have a load-more cursor (75 entries > 50 page size)")
	}
	cursor, _, _ := strings.Cut(afterToken, `"`)

	response := owner.app.Get("/entries?fragment=1&after=" + url.QueryEscape(cursor))

	wantStatus(t, response, http.StatusOK)
	html := response.Text()
	if rows := strings.Count(html, "data-entry-row"); rows != 25 {
		t.Errorf("rows = %d, want 25: append semantics return only the new slice", rows)
	}
	if strings.Contains(html, `id="load-more"`) {
		t.Error("no more pages: the load-more form must be absent")
	}
	if !strings.Contains(html, `data-swap-target="#load-more"`) {
		t.Error("fragment must use the multi-target template swap that targets #load-more")
	}
}

// --- feed edit form: additional branches ---

// editFeedPairs is the full edit form; custom headers default to blank.
func editFeedPairs(feedURL, title string, categoryID int64, customUserAgent, customReferrer string) []string {
	return []string{
		"url", feedURL, "title", title, "description", "", "site_url", "",
		"category_id", fmt.Sprint(categoryID),
		"custom_user_agent", customUserAgent, "custom_referrer", customReferrer,
	}
}

func queryNullString(t *testing.T, app *itestApp, query string, args ...any) sql.NullString {
	t.Helper()
	var value sql.NullString
	if err := app.DB.QueryRowContext(context.Background(), query, args...).Scan(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

func TestEditFeedFormEmptyUrl(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://empty-url-test.example.com/feed.xml")

	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID), editFeedPairs("", "Test Feed", categoryID, "", "")...)

	wantSeeOther(t, response, fmt.Sprintf("/feeds/%d/edit", feedID))
	if got := queryString(t, app, "SELECT url FROM feed WHERE id = $1", feedID); got != "https://empty-url-test.example.com/feed.xml" {
		t.Errorf("url = %q", got)
	}
}

// Editing the URL used to write whatever was typed straight into the feed
// table, with no scheme check and no SSRF guard.
func TestEditFeedFormRefusesAPrivateUrl(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://private-url-test.example.com/feed.xml")

	for _, feedURL := range []string{
		"http://169.254.169.254/latest/meta-data/",
		"http://192.168.1.5/feed.xml",
		"file:///etc/passwd",
	} {
		response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID), editFeedPairs(feedURL, "Test Feed", categoryID, "", "")...)

		wantSeeOther(t, response, fmt.Sprintf("/feeds/%d/edit", feedID))
		if stored := queryString(t, app, "SELECT url FROM feed WHERE id = $1", feedID); stored != "https://private-url-test.example.com/feed.xml" {
			t.Errorf("%s must not reach the feed table, stored %q", feedURL, stored)
		}
	}
}

func TestEditFeedFormNotFound(t *testing.T) {
	app := authedApp(t)
	categoryID, _ := insertTestFeed(t, app, "Tech", "https://notfound-test.example.com/feed.xml")

	response := app.postPairs("/feeds/999999/edit", editFeedPairs("https://notfound-test.example.com/feed.xml", "Test", categoryID, "", "")...)

	wantSeeOther(t, response, "/feeds/999999/edit")
}

func TestEditFeedFormOtherUsersFeed(t *testing.T) {
	app := authedApp(t)
	otherFeedID := otherUsersFeed(t, app)

	// The handler checks ownership through the feed's category, which belongs
	// to the other account.
	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", otherFeedID),
		editFeedPairs("https://other.example.com/feed.xml", "Hacked", 1, "", "")...)

	if response.Code != http.StatusSeeOther {
		t.Errorf("status = %d, want 303", response.Code)
	}
	if title := queryString(t, app, "SELECT title FROM feed WHERE id = $1", otherFeedID); title != "Other" {
		t.Errorf("the other user's feed title = %q, want unchanged", title)
	}
}

func TestEditFeedFormCategoryNotOwned(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "OwnedCat", "https://cat-not-owned.example.com/feed.xml")
	other, err := models.CreateUser(context.Background(), app.DB, "other_catowner", "x", models.RoleUser)
	if err != nil {
		t.Fatal(err)
	}
	otherCategory, err := models.CreateCategory(context.Background(), app.DB, other.ID, "NotMyCategory")
	if err != nil {
		t.Fatal(err)
	}

	// Moving the feed into the other account's category must be refused.
	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		editFeedPairs("https://cat-not-owned.example.com/feed.xml", "Test Feed", otherCategory.ID, "", "")...)

	if response.Code != http.StatusSeeOther {
		t.Errorf("status = %d, want 303", response.Code)
	}
	if got := queryInt(t, app, "SELECT category_id FROM feed WHERE id = $1", feedID); got != categoryID {
		t.Errorf("category_id = %d, want unchanged %d", got, categoryID)
	}
}

// A blank custom_user_agent / custom_referrer erases the stored value, the
// same way a blank description or site_url does.
func TestEditFeedFormBlankHttpSettingsClearThem(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://clear-ua.example.com/feed.xml")
	setFeedHTTPSettings(t, app, feedID)

	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		editFeedPairs("https://clear-ua.example.com/feed.xml", "Test Feed", categoryID, "", "   ")...)

	wantSeeOther(t, response, "/feeds")
	if ua := queryNullString(t, app, "SELECT custom_user_agent FROM feed WHERE id = $1", feedID); ua.Valid {
		t.Errorf("custom_user_agent should be NULL after a blank field, got %q", ua.String)
	}
	// Whitespace-only counts as blank, matching the trim the other fields use.
	if referrer := queryNullString(t, app, "SELECT custom_referrer FROM feed WHERE id = $1", feedID); referrer.Valid {
		t.Errorf("custom_referrer should be NULL after whitespace, got %q", referrer.String)
	}
}

func setFeedHTTPSettings(t *testing.T, app *itestApp, feedID int64) {
	t.Helper()
	if _, err := app.DB.ExecContext(context.Background(),
		"UPDATE feed SET custom_user_agent = 'MyBot/1.0', custom_referrer = 'https://ref.example.com' WHERE id = $1", feedID); err != nil {
		t.Fatal(err)
	}
}

// The counterpart: a non-empty submission round-trips unchanged, so an
// ordinary save never disturbs the overrides.
func TestEditFeedFormKeepsResubmittedHttpSettings(t *testing.T) {
	app := authedApp(t)
	categoryID, feedID := insertTestFeed(t, app, "Tech", "https://keep-ua.example.com/feed.xml")
	setFeedHTTPSettings(t, app, feedID)

	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		editFeedPairs("https://keep-ua.example.com/feed.xml", "Renamed Feed", categoryID, "MyBot/1.0", "https://ref.example.com")...)

	if response.Code != http.StatusSeeOther {
		t.Errorf("status = %d, want 303", response.Code)
	}
	if ua := queryNullString(t, app, "SELECT custom_user_agent FROM feed WHERE id = $1", feedID); ua.String != "MyBot/1.0" {
		t.Errorf("custom_user_agent = %v", ua)
	}
	if referrer := queryNullString(t, app, "SELECT custom_referrer FROM feed WHERE id = $1", feedID); referrer.String != "https://ref.example.com" {
		t.Errorf("custom_referrer = %v", referrer)
	}
}

// Omitting an optional field is not the same as sending it blank: a partial
// POST that carries only the fields it means to change must leave everything
// else alone.
func TestEditFeedFormOmittedFieldsAreLeftAlone(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", "https://omitted.example.com/feed.xml")
	execStatistics(t, app, "UPDATE feed SET description = 'Kept description', site_url = 'https://site.example.com', custom_user_agent = 'MyBot/1.0', custom_referrer = 'https://ref.example.com' WHERE id = $1", feedID)
	otherCategoryID := libraryOwner{app: app, userID: 1}.category(t, "Other")

	// Only the two required fields plus the category: everything optional is
	// absent from the body entirely.
	response := app.postPairs(fmt.Sprintf("/feeds/%d/edit", feedID),
		"url", "https://omitted.example.com/feed.xml", "category_id", fmt.Sprint(otherCategoryID))

	if response.Code != http.StatusSeeOther {
		t.Errorf("status = %d, want 303", response.Code)
	}
	if got := queryInt(t, app, "SELECT category_id FROM feed WHERE id = $1", feedID); got != otherCategoryID {
		t.Errorf("category_id = %d, the category change should have applied", got)
	}
	for column, want := range map[string]string{
		"description": "Kept description", "site_url": "https://site.example.com",
		"custom_user_agent": "MyBot/1.0", "custom_referrer": "https://ref.example.com",
	} {
		if got := queryNullString(t, app, "SELECT "+column+" FROM feed WHERE id = $1", feedID); got.String != want {
			t.Errorf("%s = %v, want %q", column, got, want)
		}
	}
}

func TestDeleteFeedFormNotFound(t *testing.T) {
	app := authedApp(t)

	wantSeeOther(t, app.request(http.MethodPost, "/feeds/999999/delete"), "/feeds")
}

func TestImportOpmlFormInvalid(t *testing.T) {
	app := authedApp(t)
	body, contentType := multipartUpload(t, map[string]string{"file": "<not valid opml"})

	wantSeeOther(t, postMultipart(app, "/feeds/import", body, contentType), "/feeds/import")

	if queryInt(t, app, "SELECT COUNT(*) FROM feed") != 0 {
		t.Error("no feeds may be created from a parse error")
	}
}

func opmlWithFeeds(title, category string, feedURLs ...string) string {
	var feeds strings.Builder
	for i, feedURL := range feedURLs {
		fmt.Fprintf(&feeds, "      <outline type=\"rss\" text=\"Feed %d\" xmlUrl=\"%s\"/>\n", i, feedURL)
	}
	return fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<opml version="2.0">
  <head><title>%s</title></head>
  <body>
    <outline text="%s" title="%s">
%s    </outline>
  </body>
</opml>`, title, category, category, feeds.String())
}

func TestImportOpmlFormDuplicateSkipped(t *testing.T) {
	app := authedApp(t)
	feedURL := "https://dup-import.example.com/feed.xml"
	insertTestFeed(t, app, "DupCat", feedURL)
	body, contentType := multipartUpload(t, map[string]string{"file": opmlWithFeeds("Dup Test", "DupCat", feedURL)})

	wantSeeOther(t, postMultipart(app, "/feeds/import", body, contentType), "/feeds")

	if count := queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE url = $1", feedURL); count != 1 {
		t.Errorf("count = %d, a duplicate import must not create a second feed row", count)
	}
}

// "OPML imported." cannot distinguish a 300-feed haul from a re-import that
// added nothing, which is what a user wants to know after uploading a file.
func TestImportOpmlFormFlashReportsCounts(t *testing.T) {
	app := authedApp(t)
	existing := "https://already.example.com/feed.xml"
	insertTestFeed(t, app, "MixCat", existing)
	body, contentType := multipartUpload(t, map[string]string{"file": opmlWithFeeds("Mixed", "MixCat",
		existing, "https://new-a.example.com/feed.xml", "https://new-b.example.com/feed.xml")})

	response := postMultipart(app, "/feeds/import", body, contentType)

	wantSeeOther(t, response, "/feeds")
	if text := flashText(t, response); !strings.Contains(text, "OPML imported: 2 feeds added, 1 already subscribed.") {
		t.Errorf("flash must break the import down by outcome, got: %s", text)
	}
}

// --- feed forms that fetch: against a stub feed server ---

const mockRSSFixture = `<?xml version="1.0"?><rss version="2.0"><channel>
  <title>Mock Feed</title><description>Mock Desc</description><link>https://e</link>
  <item><guid>m1</guid><title>One</title><link>https://e/1</link><description>c1</description></item>
</channel></rss>`

// mockFeedServer serves mockRSSFixture at every path.
func mockFeedServer(t *testing.T) string {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "application/rss+xml")
		_, _ = w.Write([]byte(mockRSSFixture))
	}))
	t.Cleanup(server.Close)
	return server.URL
}

func TestCreateFeedFormSuccess(t *testing.T) {
	app := authedApp(t)
	categoryID := libraryOwner{app: app, userID: 1}.category(t, "Tech")
	feedURL := mockFeedServer(t)

	wantSeeOther(t, app.postPairs("/feeds", "url", feedURL, "category_id", fmt.Sprint(categoryID)), "/feeds")

	if count := queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE url = $1", feedURL); count != 1 {
		t.Errorf("count = %d, the feed should have been created", count)
	}
}

func TestCreateFeedFormDuplicate(t *testing.T) {
	app := authedApp(t)
	feedURL := mockFeedServer(t)
	categoryID, _ := insertTestFeed(t, app, "Tech", feedURL)

	wantSeeOther(t, app.postPairs("/feeds", "url", feedURL, "category_id", fmt.Sprint(categoryID)), "/feeds")

	if count := queryInt(t, app, "SELECT COUNT(*) FROM feed WHERE url = $1", feedURL); count != 1 {
		t.Errorf("count = %d, a duplicate create must not add a second feed row", count)
	}
}

func TestRefreshFeedFormSuccess(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", mockFeedServer(t))

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/refresh", feedID)), "/feeds")

	if count := queryInt(t, app, "SELECT COUNT(*) FROM entry WHERE feed_id = $1", feedID); count < 1 {
		t.Errorf("refresh should have synced at least 1 entry, got %d", count)
	}
}

func TestFetchMetadataFormSuccess(t *testing.T) {
	app := authedApp(t)
	_, feedID := insertTestFeed(t, app, "Tech", mockFeedServer(t))

	wantSeeOther(t, app.request(http.MethodPost, fmt.Sprintf("/feeds/%d/fetch-metadata", feedID)), fmt.Sprintf("/feeds/%d/edit", feedID))

	if title := queryString(t, app, "SELECT title FROM feed WHERE id = $1", feedID); title != "Mock Feed" {
		t.Errorf("title = %q", title)
	}
	if description := queryNullString(t, app, "SELECT description FROM feed WHERE id = $1", feedID); description.String != "Mock Desc" {
		t.Errorf("description = %v", description)
	}
}

// --- SSE /events auth gate and cache-control wiring ---

func TestEventsEndpointRequiresAuth(t *testing.T) {
	app := defaultITestApp(t)

	// PageAuthUser redirects every unauthenticated request to /login.
	wantStatus(t, app.Get("/events"), http.StatusSeeOther)
}

func TestAuthenticatedPageIsNoStore(t *testing.T) {
	app := authedApp(t)

	response := app.Get("/")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != "no-store" {
		t.Errorf("Cache-Control = %q: an authenticated page must never be retained by a disk cache or shared proxy", got)
	}
	if got := response.Header().Get("Vary"); got != "Cookie" {
		t.Errorf("Vary = %q: Vary: Cookie must accompany no-store so a proxy cannot conflate the variants", got)
	}
}

// Regression guard: the cache-control layer only fills in a header when the
// response has none, so the long-lived static-asset directive must survive.
func TestStaticAssetKeepsItsCacheControl(t *testing.T) {
	app := defaultITestApp(t)

	response := app.Get("/static/css/app.css")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != expectedStaticCacheControl() {
		t.Errorf("Cache-Control = %q", got)
	}
}

// proxyNotModified requests the signed image URL with a matching If-None-Match,
// the SSRF-free 304 path that carries the handler's own Cache-Control. A real
// upstream fetch cannot be exercised: the proxy's SSRF guard rejects loopback.
func proxyNotModified(app *itestApp) itestResponse {
	signature := imageproxy.SignURL(make([]byte, 32), proxyTestImageURL)
	return app.Do(itestRequest{
		Path:    "/api/proxy/image?url=" + proxyTestImageURLB64 + "&s=" + signature,
		Headers: map[string]string{"If-None-Match": `"` + signature + `"`},
	})
}

// Authenticated on purpose: that is where the cache-control layer could
// plausibly override the proxy's own header.
func TestImageProxyKeepsUpstreamCacheControl(t *testing.T) {
	response := proxyNotModified(authedApp(t))

	wantStatus(t, response, http.StatusNotModified)
	if got := response.Header().Get("Cache-Control"); got != "public, max-age=86400" {
		t.Errorf("Cache-Control = %q: the proxy's own header must pass through untouched", got)
	}
}

// Regression test for session-cookie cache poisoning: a shared cache that
// stores this publicly-cacheable response must never receive a live session or
// CSRF cookie riding along on it.
func TestAuthenticatedCacheableResponseCarriesNoSetCookie(t *testing.T) {
	response := proxyNotModified(authedApp(t))

	wantStatus(t, response, http.StatusNotModified)
	wantNoSetCookie(t, response, "a publicly-cacheable authenticated response")
}

func TestHealthEndpointStaysCacheable(t *testing.T) {
	app := defaultITestApp(t)

	response := app.Get("/health")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Cache-Control"); got != "" {
		t.Errorf("Cache-Control = %q: an unauthenticated request must not get a forced no-store", got)
	}
}

// --- Strict-Transport-Security (HSTS) ---

// httpsConfig derives HSTS the real way, from RDRS_PUBLIC_BASE_URL's scheme
// through config.Load, instead of a hand-set flag.
func httpsConfig(t *testing.T) config.Config {
	t.Helper()
	loaded, err := config.Load(func(name string) (string, bool) {
		if name != "RDRS_PUBLIC_BASE_URL" {
			return "", false
		}
		return "https://rdrs.example.com", true
	})
	if err != nil {
		t.Fatal(err)
	}
	cfg := itestConfig()
	cfg.HSTS, cfg.HSTSMaxAge, cfg.HSTSIncludeSubdomains = loaded.HSTS, loaded.HSTSMaxAge, loaded.HSTSIncludeSubdomains
	return cfg
}

const wantHSTS = "max-age=31536000; includeSubDomains"

func TestHstsHeaderPresentWhenPublicBaseUrlIsHttps(t *testing.T) {
	app := newITestApp(t, httpsConfig(t))

	if got := app.Get("/health").Header().Get("Strict-Transport-Security"); got != wantHSTS {
		t.Errorf("Strict-Transport-Security = %q", got)
	}
}

func TestHstsHeaderAbsentByDefault(t *testing.T) {
	// HSTS is sticky and cannot be retracted quickly, so a plain-HTTP
	// deployment must never receive it by accident.
	cfg := itestConfig()
	cfg.HSTS = false
	app := newITestApp(t, cfg)

	response := app.Get("/health")

	wantStatus(t, response, http.StatusOK)
	if got := response.Header().Get("Strict-Transport-Security"); got != "" {
		t.Errorf("a plain-HTTP deployment must never receive Strict-Transport-Security, got %q", got)
	}
}

// HSTS is a declaration about the host, not any one response, so there is no
// skip list.
func TestHstsHeaderOnStaticAndHealth(t *testing.T) {
	app := newITestApp(t, httpsConfig(t))

	for _, path := range []string{"/health", "/static/css/app.css"} {
		response := app.Get(path)
		wantStatus(t, response, http.StatusOK)
		if got := response.Header().Get("Strict-Transport-Security"); got != wantHSTS {
			t.Errorf("%s: Strict-Transport-Security = %q", path, got)
		}
	}
}

// The realistic source of a pre-existing header is a TLS-terminating reverse
// proxy, so the middleware is wired around a handler standing in for one.
func TestExistingHstsHeaderIsNotOverwritten(t *testing.T) {
	proxy := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Strict-Transport-Security", "max-age=1")
	})
	recorder := httptest.NewRecorder()

	middleware.HSTS(wantHSTS)(proxy).ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/probe", nil))

	if got := recorder.Header().Get("Strict-Transport-Security"); got != "max-age=1" {
		t.Errorf("Strict-Transport-Security = %q", got)
	}
}

// crossSitePost is a request the CSRF origin guard refuses before any handler.
func crossSitePost(app *itestApp) itestResponse {
	return app.Do(itestRequest{
		Method: http.MethodPost, Path: "/api/session", Headers: map[string]string{"Sec-Fetch-Site": "cross-site"},
	})
}

// Regression test: HSTS must be the outermost layer, because the CSRF guard
// short-circuits with a 403 without calling next.
func TestHstsIsSentOnACsrfRejectedResponse(t *testing.T) {
	response := crossSitePost(newITestApp(t, httpsConfig(t)))

	wantStatus(t, response, http.StatusForbidden)
	if got := response.Header().Get("Strict-Transport-Security"); got != wantHSTS {
		t.Errorf("Strict-Transport-Security = %q", got)
	}
}

// --- Fixed security headers ---

// expectedSecurityHeaders pairs each header the fixed layer owns with the
// exact value it must carry, so a test asserts the whole set.
var expectedSecurityHeaders = map[string]string{
	"Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; " +
		"img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; " +
		"base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
	"X-Content-Type-Options": "nosniff",
	"Referrer-Policy":        "strict-origin-when-cross-origin",
	"Permissions-Policy": "accelerometer=(), autoplay=(), camera=(), display-capture=(), encrypted-media=(), " +
		"geolocation=(), gyroscope=(), magnetometer=(), microphone=(), midi=(), payment=(), " +
		"usb=(), xr-spatial-tracking=()",
	"X-Frame-Options":            "DENY",
	"Cross-Origin-Opener-Policy": "same-origin",
}

func wantSecurityHeaders(t *testing.T, response itestResponse, context string) {
	t.Helper()
	for name, want := range expectedSecurityHeaders {
		if got := response.Header().Get(name); got != want {
			t.Errorf("%s: wrong or missing %s: %q", context, name, got)
		}
	}
}

// Unlike HSTS these are unconditional: none of them depends on the transport.
func TestSecurityHeadersPresentOnTheDefaultConfig(t *testing.T) {
	response := defaultITestApp(t).Get("/health")

	wantStatus(t, response, http.StatusOK)
	wantSecurityHeaders(t, response, "/health")
}

func TestSecurityHeadersOnStaticAndAPage(t *testing.T) {
	app := defaultITestApp(t)

	for _, path := range []string{"/static/css/app.css", "/login"} {
		response := app.Get(path)
		wantStatus(t, response, http.StatusOK)
		wantSecurityHeaders(t, response, path)
	}
}

// A rejected response is exactly the one an attacker sees, so it must carry
// the policy too.
func TestSecurityHeadersAreSentOnACsrfRejectedResponse(t *testing.T) {
	response := crossSitePost(defaultITestApp(t))

	wantStatus(t, response, http.StatusForbidden)
	wantSecurityHeaders(t, response, "csrf-rejected /api/session")
}

func TestExistingSecurityHeaderIsNotOverwritten(t *testing.T) {
	proxy := http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Security-Policy", "default-src 'none'")
	})
	recorder := httptest.NewRecorder()

	middleware.SecurityHeaders(proxy).ServeHTTP(recorder, httptest.NewRequest(http.MethodGet, "/probe", nil))

	if got := recorder.Header().Get("Content-Security-Policy"); got != "default-src 'none'" {
		t.Errorf("Content-Security-Policy = %q", got)
	}
	if got := recorder.Header().Get("X-Content-Type-Options"); got != "nosniff" {
		t.Errorf("X-Content-Type-Options = %q: the headers the proxy did not set are still filled in", got)
	}
}

// --- Admin re-authentication ---

// Every route that changes another account is behind the same window that
// guards passkey enrolment; a lapsed session must be refused before it touches
// the account.
func TestAdminAccountChangesRequireRecentAuthentication(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	staleAuthentication(t, app)

	for path, pairs := range map[string][]string{
		"/admin/users/2/role":       {"role", "admin"},
		"/admin/users/2/status":     {"disabled", "true"},
		"/admin/users/2/masquerade": nil,
		"/admin/users/2/delete":     nil,
	} {
		wantSeeOther(t, app.postPairs(path, pairs...), "/admin")
	}

	target := userNamed(t, app, "target")
	if target.Role != models.RoleUser || target.IsDisabled() {
		t.Errorf("target = %+v: still a plain, enabled user", target)
	}
	if admin := userNamed(t, app, "admin"); !admin.IsAdmin() {
		t.Error("the admin was dragged into a masquerade on the way")
	}
}

func TestConfirmingThePasswordReopensTheAdminWindow(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	staleAuthentication(t, app)

	wantStatus(t, app.postPairs("/admin/reauth", "password", fixturePassword), http.StatusSeeOther)
	wantStatus(t, app.postPairs("/admin/users/2/role", "role", "admin"), http.StatusSeeOther)

	if target := userNamed(t, app, "target"); target.Role != models.RoleAdmin {
		t.Errorf("role = %v: the action refused a moment ago must now land", target.Role)
	}
}

// The confirmation is a real credential check, not a click-through. Both
// responses are the same 303, so the account state afterwards is what counts.
func TestAWrongPasswordDoesNotReopenTheAdminWindow(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	staleAuthentication(t, app)

	wantStatus(t, app.postPairs("/admin/reauth", "password", "not-the-password"), http.StatusSeeOther)
	wantStatus(t, app.postPairs("/admin/users/2/role", "role", "admin"), http.StatusSeeOther)

	if target := userNamed(t, app, "target"); target.Role != models.RoleUser {
		t.Errorf("role = %v: a failed confirmation must not let the change through", target.Role)
	}
}

// While masquerading, the password that would be asked for belongs to the
// impersonated account, so requiring one would strand the admin there.
func TestStoppingAMasqueradeNeverRequiresReauthentication(t *testing.T) {
	app := adminApp(t)
	seedTarget(app)
	started := app.request(http.MethodPost, "/admin/users/2/masquerade")
	wantStatus(t, started, http.StatusSeeOther)
	app.ApplyCSRF(started)
	staleAuthentication(t, app)

	wantStatus(t, app.request(http.MethodPost, "/api/admin/unmasquerade"), http.StatusOK)
}

// The form is rendered ahead of time, so an admin learns the window lapsed
// before clicking "delete" rather than after.
func TestTheAdminPageAsksForConfirmationOnlyOnceTheWindowLapses(t *testing.T) {
	app := adminApp(t)

	fresh := app.Get("/admin")
	wantStatus(t, fresh, http.StatusOK)
	if strings.Contains(fresh.Text(), "admin-reauth-form") {
		t.Error("a session inside the window has nothing to confirm")
	}
	staleAuthentication(t, app)

	stale := app.Get("/admin")
	wantStatus(t, stale, http.StatusOK)
	if !strings.Contains(stale.Text(), "admin-reauth-form") {
		t.Error("a lapsed window must surface the confirmation form")
	}
}

// The browser's minlength/maxlength hints come from the constants the handler
// validates against, so the two cannot drift.
func TestPasswordFieldsAdvertiseTheServerSidePolicy(t *testing.T) {
	wantHints := func(body string) {
		t.Helper()
		wantContains(t, body,
			fmt.Sprintf(`minlength="%d"`, auth.PasswordMinLength),
			fmt.Sprintf(`maxlength="%d"`, auth.PasswordMaxLength))
	}
	// Two apps on purpose: /setup is only served while no account exists, and
	// loading any page first mints an anonymous CSRF cookie.
	setup := defaultITestApp(t).Get("/setup")
	wantStatus(t, setup, http.StatusOK)
	wantHints(setup.Text())

	settings := adminApp(t).Get("/user-settings")
	wantStatus(t, settings, http.StatusOK)
	wantHints(settings.Text())
}

// --- Admin-created accounts and the one-time link that activates them ---

const (
	newcomerPassword = "heron-lantern-53-drift"
	createNewcomer   = "newcomer"
)

// invitePathFrom reads the /invite/{token} path off the flash the create-account
// redirect sets, as an admin has to: the link is shown once and never stored
// in recoverable form.
func invitePathFrom(t *testing.T, response itestResponse) string {
	t.Helper()
	raw := strings.ReplaceAll(flashText(t, response), "%2F", "/")
	_, tail, found := strings.Cut(raw, "/invite/")
	if !found {
		t.Fatalf("expected a link in the flash, got %q", raw)
	}
	// The token is base64url; the path ends where the encoded JSON resumes.
	token := tail
	end := strings.IndexFunc(token, func(r rune) bool {
		return !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-' || r == '_')
	})
	if end >= 0 {
		token = token[:end]
	}
	return "/invite/" + token
}

func createAccount(app *itestApp, username string) itestResponse {
	return app.postPairs("/admin/users", "username", username, "role", "user")
}

func redeemInvite(app *itestApp, invite, password string) itestResponse {
	return app.postPairs(invite, "password", password, "confirm_password", password)
}

func signInAs(app *itestApp, username, password string) itestResponse {
	return app.PostJSON("/api/session", map[string]string{"username": username, "password": password})
}

func TestAnAdminCreatedAccountIsActivatedThroughItsLink(t *testing.T) {
	app := adminApp(t)
	created := createAccount(app, createNewcomer)
	wantStatus(t, created, http.StatusSeeOther)
	invite := invitePathFrom(t, created)

	// The account exists but cannot be signed into: no password has been set.
	wantStatus(t, signInAs(app, createNewcomer, fixturePassword), http.StatusUnauthorized)
	// The link names the account it opens...
	page := app.Get(invite)
	wantStatus(t, page, http.StatusOK)
	if !strings.Contains(page.Text(), createNewcomer) {
		t.Error("the link page must name the account")
	}
	// ...and redeeming it sets the password.
	wantSeeOther(t, redeemInvite(app, invite, newcomerPassword), "/login")
	wantStatus(t, signInAs(app, createNewcomer, newcomerPassword), http.StatusOK)
}

func TestAnAdminCannotCreateAnAccountWithAnInvalidUsername(t *testing.T) {
	app := adminApp(t)

	for _, bad := range []string{"   ", "a\tb"} {
		refused := createAccount(app, bad)
		wantStatus(t, refused, http.StatusSeeOther)
		if strings.Contains(flashText(t, refused), "/invite/") {
			t.Errorf("%q: a link was issued", bad)
		}
		if found, _ := models.FindUserByUsername(context.Background(), app.DB, strings.TrimSpace(bad)); found != nil {
			t.Errorf("%q: the account was created", bad)
		}
	}
}

// renameInviteTable breaks and restores the last step of account creation: the
// account and its category are written before the invite.
func renameInviteTable(t *testing.T, app *itestApp, from, to string) {
	t.Helper()
	execStatistics(t, app, "ALTER TABLE "+from+" RENAME TO "+to)
}

func TestAFailedAccountCreationLeavesNoAccountBehind(t *testing.T) {
	app := adminApp(t)
	renameInviteTable(t, app, "user_invite", "user_invite_off")

	failed := createAccount(app, createNewcomer)

	wantStatus(t, failed, http.StatusSeeOther)
	if !strings.Contains(flashText(t, failed), "Failed to create the account.") {
		t.Error("the flash must say the creation failed")
	}
	if found, _ := models.FindUserByUsername(context.Background(), app.DB, createNewcomer); found != nil {
		t.Error("a failed creation must not hold the username")
	}
	renameInviteTable(t, app, "user_invite_off", "user_invite")
	if retried := createAccount(app, createNewcomer); !strings.HasPrefix(invitePathFrom(t, retried), "/invite/") {
		t.Error("the admin can retry with the same username")
	}
}

// capturedLogs keeps everything a logger wrote so a test can assert on it.
type capturedLogs struct {
	mutex  sync.Mutex
	buffer bytes.Buffer
}

func (c *capturedLogs) Write(data []byte) (int, error) {
	c.mutex.Lock()
	defer c.mutex.Unlock()
	return c.buffer.Write(data)
}

func (c *capturedLogs) count(needle string) int {
	c.mutex.Lock()
	defer c.mutex.Unlock()
	return strings.Count(c.buffer.String(), needle)
}

// An audit event claims the account or link exists, so it must follow the
// commit: a rolled-back attempt must leave no trace that says otherwise.
func TestAFailedInviteOrAccountCreationEmitsNoAuditEvent(t *testing.T) {
	logs := &capturedLogs{}
	cfg := itestConfig()
	app := newITestAppWith(t, Options{
		Config: cfg, Audit: audit.New(slog.New(slog.NewTextHandler(logs, nil)), cfg.Secret),
	})
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.Login("admin")
	// Setup already audits the first admin's own account.
	accountsBefore, invitesBefore := logs.count("account.created"), logs.count("invite.issued")

	renameInviteTable(t, app, "user_invite", "user_invite_off")
	failed := createAccount(app, createNewcomer)
	if !strings.Contains(flashText(t, failed), "Failed to create the account.") {
		t.Error("the creation should have failed")
	}
	wantAuditCounts(t, logs, accountsBefore, invitesBefore)

	renameInviteTable(t, app, "user_invite_off", "user_invite")
	created := createAccount(app, createNewcomer)
	if !strings.HasPrefix(invitePathFrom(t, created), "/invite/") {
		t.Error("the retry should have succeeded")
	}
	wantAuditCounts(t, logs, accountsBefore+1, invitesBefore+1)

	renameInviteTable(t, app, "user_invite", "user_invite_off")
	failedReissue := app.request(http.MethodPost, "/admin/users/2/invite")
	if !strings.Contains(flashText(t, failedReissue), "Failed to issue a link.") {
		t.Error("the reissue should have failed")
	}
	wantAuditCounts(t, logs, accountsBefore+1, invitesBefore+1)

	renameInviteTable(t, app, "user_invite_off", "user_invite")
	reissued := app.request(http.MethodPost, "/admin/users/2/invite")
	if !strings.HasPrefix(invitePathFrom(t, reissued), "/invite/") {
		t.Error("the second reissue should have succeeded")
	}
	wantAuditCounts(t, logs, accountsBefore+1, invitesBefore+2)
}

func wantAuditCounts(t *testing.T, logs *capturedLogs, accounts, invites int) {
	t.Helper()
	if got := logs.count("account.created"); got != accounts {
		t.Errorf("account.created events = %d, want %d", got, accounts)
	}
	if got := logs.count("invite.issued"); got != invites {
		t.Errorf("invite.issued events = %d, want %d", got, invites)
	}
}

func TestALinkWorksExactlyOnce(t *testing.T) {
	app := adminApp(t)
	invite := invitePathFrom(t, createAccount(app, createNewcomer))
	wantStatus(t, redeemInvite(app, invite, newcomerPassword), http.StatusSeeOther)

	// A second submission must not replace the password the first person chose.
	replayed := redeemInvite(app, invite, "badger-kestrel-19-plume")

	wantStatus(t, replayed, http.StatusOK)
	if !strings.Contains(replayed.Text(), "not valid") {
		t.Error("a spent link must say it is not valid")
	}
	wantStatus(t, signInAs(app, createNewcomer, newcomerPassword), http.StatusOK)
}

// Unknown, revoked and already-spent links must be indistinguishable, and none
// may name an account: otherwise the page becomes the enumeration oracle that
// removing registration was meant to close.
func TestEveryBadTokenGetsTheSameAnswer(t *testing.T) {
	app := adminApp(t)
	invite := invitePathFrom(t, createAccount(app, createNewcomer))
	unknown := app.Get("/invite/completely-made-up-token")
	wantStatus(t, unknown, http.StatusOK)
	wantStatus(t, app.request(http.MethodPost, "/admin/users/2/invite/revoke"), http.StatusSeeOther)

	revoked := app.Get(invite)

	wantStatus(t, revoked, http.StatusOK)
	if unknown.Text() != revoked.Text() {
		t.Error("an unknown and a revoked token must get the same page")
	}
	if strings.Contains(revoked.Text(), createNewcomer) {
		t.Error("a revoked link must not name the account")
	}
}

// The unusable hash has to hold for the GReader client protocol too, not just
// the browser form.
func TestAPendingAccountCannotSignInByAnyPath(t *testing.T) {
	app := adminApp(t)
	wantStatus(t, createAccount(app, createNewcomer), http.StatusSeeOther)

	wantStatus(t, signInAs(app, createNewcomer, "!"), http.StatusUnauthorized)
	wantStatus(t, clientLogin(app, createNewcomer, "!"), http.StatusUnauthorized)
}

func TestRedemptionEnforcesThePasswordPolicy(t *testing.T) {
	app := adminApp(t)
	invite := invitePathFrom(t, createAccount(app, createNewcomer))

	// Too short; the link survives so the person can try again.
	short := redeemInvite(app, invite, "short")
	wantStatus(t, short, http.StatusOK)
	if !strings.Contains(short.Text(), "at least") {
		t.Error("a short password must name the minimum length")
	}
	// Built out of the username, which only the estimator can catch.
	derived := redeemInvite(app, invite, "newcomernewcomer")
	wantStatus(t, derived, http.StatusOK)
	if !strings.Contains(derived.Text(), "invite-error") {
		t.Error("a username-derived password must be refused")
	}
	wantStatus(t, redeemInvite(app, invite, newcomerPassword), http.StatusSeeOther)
}

// rdrs has no self-service recovery, so this is the reset path; issuing one
// must not lock the account's owner out on its own.
func TestAnAdminIssuedResetLeavesTheOldPasswordWorkingUntilRedeemed(t *testing.T) {
	app := adminApp(t)
	app.SeedAccount("forgetful", fixturePassword, models.RoleUser)
	issued := app.request(http.MethodPost, "/admin/users/2/invite")
	wantStatus(t, issued, http.StatusSeeOther)
	invite := invitePathFrom(t, issued)

	// Signing in as the target replaces the session cookie, so the CSRF header
	// is refreshed from the response, exactly as a browser does on its own.
	signedIn := signInAs(app, "forgetful", fixturePassword)
	wantStatus(t, signedIn, http.StatusOK)
	app.ApplyCSRF(signedIn)
	wantStatus(t, redeemInvite(app, invite, newcomerPassword), http.StatusSeeOther)

	wantStatus(t, signInAs(app, "forgetful", fixturePassword), http.StatusUnauthorized)
	wantStatus(t, signInAs(app, "forgetful", newcomerPassword), http.StatusOK)
}

func TestCreatingAnAccountRequiresRecentAuthentication(t *testing.T) {
	app := adminApp(t)
	staleAuthentication(t, app)

	wantStatus(t, createAccount(app, createNewcomer), http.StatusSeeOther)

	if found, _ := models.FindUserByUsername(context.Background(), app.DB, createNewcomer); found != nil {
		t.Error("a stale session must not be able to create an account")
	}
}

func TestASingleUserInstanceRefusesASecondAccount(t *testing.T) {
	cfg := itestConfig()
	cfg.MultiUser = false
	app := newITestApp(t, cfg)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.Login("admin")

	wantStatus(t, createAccount(app, createNewcomer), http.StatusSeeOther)

	if found, _ := models.FindUserByUsername(context.Background(), app.DB, createNewcomer); found != nil {
		t.Error("a single-user instance must refuse a second account")
	}
}

// The link is the only copy that will ever exist, so it belongs where it can be
// read and copied, not in a banner that fades, and exactly once on the page.
func TestTheOneTimeLinkIsShownInItsOwnBlockNotABanner(t *testing.T) {
	app := adminApp(t)
	created := createAccount(app, createNewcomer)
	wantStatus(t, created, http.StatusSeeOther)
	invite := invitePathFrom(t, created)

	page := app.Get("/admin")

	wantStatus(t, page, http.StatusOK)
	body := page.Text()
	if !strings.Contains(body, "admin-invite-link") {
		t.Error("the link should render in its own block")
	}
	if count := strings.Count(body, invite); count != 1 {
		t.Errorf("the link appears %d times, want once: not in the block and the banner", count)
	}
	// Dropping the message entirely would leave the flash cookie uncleared and
	// the link would come back on the next load.
	if !strings.Contains(body, "Account link ready.") {
		t.Error("the banner must still say something happened")
	}
}

// An admin pastes this into a chat window, so a bare path is close to useless:
// RDRS_PUBLIC_BASE_URL when set, never the client-supplied Host header.
func TestTheOneTimeLinkIsAbsoluteWhenAPublicBaseUrlIsConfigured(t *testing.T) {
	cfg := itestConfig()
	cfg.PublicBaseURL = "https://reader.example.com"
	app := newITestApp(t, cfg)
	wantStatus(t, app.setupJSON("admin", nil), http.StatusCreated)
	app.Login("admin")
	wantStatus(t, createAccount(app, createNewcomer), http.StatusSeeOther)

	body := app.Get("/admin").Text()

	if !strings.Contains(body, "https://reader.example.com/invite/") {
		t.Errorf("expected an absolute link, got: %s", body)
	}
}
