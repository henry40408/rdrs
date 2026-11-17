package server

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/png"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/db"
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
