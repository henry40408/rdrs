package server

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"image"
	"image/png"
	"mime/multipart"
	"net/http"
	"net/url"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/auth"
	"github.com/henry40408/rdrs/internal/db"
	"github.com/henry40408/rdrs/internal/imageproxy"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/secret"
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
