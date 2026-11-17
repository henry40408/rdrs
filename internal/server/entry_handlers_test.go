package server

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Port of tests/entry_handlers_test.rs: entry-related handlers via the GReader
// endpoints, the RDRS-specific ones (neighbors, fetch-full-content,
// summarize, summary, save), and cross-user access restrictions.

const (
	readTag    = "user/-/state/com.google/read"
	starredTag = "user/-/state/com.google/starred"
	readingTag = "user/-/state/com.google/reading-list"
	testFeedID = "feed/https://example.com/feed.xml"
)

// entryData is what setup_test_data returns.
type entryData struct {
	userID, categoryID, feedID int64
	entryIDs                   []int64
}

// setupEntryData makes testuser with one category, one feed and five entries
// (entry i was published i hours ago, so entry 1 is the newest).
func setupEntryData(t *testing.T, app *itestApp) entryData {
	t.Helper()
	return setupEntryDataWith(t, app, func(i int) (time.Time, *string) {
		summary := fmt.Sprintf("Summary for entry %d", i)
		return time.Now().Add(-time.Duration(i) * time.Hour), &summary
	})
}

// setupEntryDataWith lets a test choose each entry's published time and summary.
func setupEntryDataWith(t *testing.T, app *itestApp, publish func(i int) (time.Time, *string)) entryData {
	t.Helper()
	ctx := context.Background()
	user := app.SeedAccount("testuser", fixturePassword, models.RoleAdmin)
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "Test Category")
	if err != nil {
		t.Fatal(err)
	}
	title := "Test Feed"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/feed.xml", Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	data := entryData{userID: user.ID, categoryID: category.ID, feedID: feed.ID}
	for i := 1; i <= 5; i++ {
		published, summary := publish(i)
		entryTitle, link, content := fmt.Sprintf("Entry Title %d", i), fmt.Sprintf("https://example.com/entry/%d", i), fmt.Sprintf("<p>Entry content %d</p>", i)
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feed.ID, GUID: fmt.Sprintf("guid-%d", i), Title: &entryTitle, Link: &link,
			Content: &content, Summary: summary, PublishedAt: &published,
		})
		if err != nil {
			t.Fatal(err)
		}
		data.entryIDs = append(data.entryIDs, result.ID)
	}
	return data
}

// seededEntryApp is seeded_app: setupEntryData, signed in as testuser.
func seededEntryApp(t *testing.T) (*itestApp, entryData) {
	t.Helper()
	app := defaultITestApp(t)
	data := setupEntryData(t, app)
	app.Login("testuser")
	return app, data
}

// setupSecondUserData is setup_second_user_data: otheruser with three entries.
func setupSecondUserData(t *testing.T, app *itestApp) entryData {
	t.Helper()
	ctx := context.Background()
	user := app.SeedAccount("otheruser", "password456", models.RoleUser)
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "Other User Category")
	if err != nil {
		t.Fatal(err)
	}
	title := "Other Feed"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://other.com/feed.xml", Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	data := entryData{userID: user.ID, categoryID: category.ID, feedID: feed.ID}
	for i := 1; i <= 3; i++ {
		entryTitle, link, content := fmt.Sprintf("Other Entry %d", i), fmt.Sprintf("https://other.com/entry/%d", i), fmt.Sprintf("<p>Other content %d</p>", i)
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feed.ID, GUID: fmt.Sprintf("other-guid-%d", i), Title: &entryTitle, Link: &link, Content: &content,
		})
		if err != nil {
			t.Fatal(err)
		}
		data.entryIDs = append(data.entryIDs, result.ID)
	}
	return data
}

// setupEntryWithoutLink adds an entry that has no link to the feed.
func setupEntryWithoutLink(t *testing.T, app *itestApp, feedID int64) int64 {
	t.Helper()
	title, content := "Entry Without Link", "<p>Content without link</p>"
	result, err := entry.UpsertEntry(context.Background(), app.DB, &entry.Upsert{
		FeedID: feedID, GUID: "no-link-guid", Title: &title, Content: &content,
	})
	if err != nil {
		t.Fatal(err)
	}
	return result.ID
}

func (a *itestApp) wantStatus(response itestResponse, want int) {
	a.t.Helper()
	if response.Code != want {
		a.t.Fatalf("status = %d, want %d: %s", response.Code, want, response.Text())
	}
}

// getJSONBody GETs path, expects 200 and decodes the JSON object.
func (a *itestApp) getJSONBody(path string) map[string]any {
	a.t.Helper()
	response := a.Get(path)
	a.wantStatus(response, http.StatusOK)
	return decodeJSONObject(a.t, response)
}

func decodeJSONObject(t testing.TB, response itestResponse) map[string]any {
	t.Helper()
	var body map[string]any
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatalf("body is not a JSON object: %v: %s", err, response.Text())
	}
	return body
}

func jsonItems(t testing.TB, body map[string]any) []map[string]any {
	t.Helper()
	raw, ok := body["items"].([]any)
	if !ok {
		t.Fatalf("items is not an array: %v", body["items"])
	}
	items := make([]map[string]any, len(raw))
	for i, item := range raw {
		items[i], _ = item.(map[string]any)
	}
	return items
}

func (a *itestApp) postEditForm(path string, fields url.Values) {
	a.t.Helper()
	response := a.PostForm(path, fields)
	a.wantStatus(response, http.StatusOK)
	if response.Text() != "OK" {
		a.t.Fatalf("POST %s body = %q, want OK", path, response.Text())
	}
}

// editTag adds (add) or removes the tag on the entries via edit-tag.
func (a *itestApp) editTag(add bool, tag string, entryIDs ...int64) {
	a.t.Helper()
	form := url.Values{}
	for _, id := range entryIDs {
		form.Add("i", strconv.FormatInt(id, 10))
	}
	key := "r"
	if add {
		key = "a"
	}
	form.Set(key, tag)
	a.postEditForm("/reader/api/0/edit-tag", form)
}

func (a *itestApp) markRead(entryIDs ...int64) { a.t.Helper(); a.editTag(true, readTag, entryIDs...) }
func (a *itestApp) markUnread(entryIDs ...int64) {
	a.t.Helper()
	a.editTag(false, readTag, entryIDs...)
}
func (a *itestApp) starEntry(entryIDs ...int64) {
	a.t.Helper()
	a.editTag(true, starredTag, entryIDs...)
}
func (a *itestApp) unstarEntry(entryIDs ...int64) {
	a.t.Helper()
	a.editTag(false, starredTag, entryIDs...)
}

// itemOf fetches one entry's GReader item.
func (a *itestApp) itemOf(entryID int64) map[string]any {
	a.t.Helper()
	body := a.getJSONBody(fmt.Sprintf("/reader/api/0/stream/items/contents?i=%d", entryID))
	items := jsonItems(a.t, body)
	if len(items) != 1 {
		a.t.Fatalf("items = %d, want 1", len(items))
	}
	return items[0]
}

func categoriesOf(t *testing.T, item map[string]any) []string {
	t.Helper()
	raw, ok := item["categories"].([]any)
	if !ok {
		t.Fatalf("categories is not an array: %v", item["categories"])
	}
	categories := make([]string, len(raw))
	for i, category := range raw {
		categories[i], _ = category.(string)
	}
	return categories
}

func (a *itestApp) streamItemCount(path string) int {
	a.t.Helper()
	return len(jsonItems(a.t, a.getJSONBody(path)))
}

func (a *itestApp) unreadCount() int {
	a.t.Helper()
	return a.streamItemCount("/reader/api/0/stream/contents/" + readingTag + "?xt=" + readTag)
}

func isJSONString(value any) bool { _, ok := value.(string); return ok }
func isJSONNumber(value any) bool { _, ok := value.(float64); return ok }
func isJSONBool(value any) bool   { _, ok := value.(bool); return ok }
func isJSONArray(value any) bool  { _, ok := value.([]any); return ok }
func idOf(entryID int64) float64  { return float64(entryID) }
func countFor(counts []any, id string) any {
	for _, raw := range counts {
		if count, _ := raw.(map[string]any); count["id"] == id {
			return count["count"]
		}
	}
	return nil
}

// --- Stream Contents Tests (Entry List) ---

func TestListEntriesByReadingListCategoryAndFeed(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	app.Login("testuser")

	for _, test := range []struct{ path, id string }{
		{readingTag, readingTag},
		{"user/-/label/Test%20Category", "user/-/label/Test Category"},
		{"feed/https://example.com/feed.xml", "feed/https://example.com/feed.xml"},
	} {
		body := app.getJSONBody("/reader/api/0/stream/contents/" + test.path)
		if got := len(jsonItems(t, body)); got != 5 {
			t.Errorf("%s: items = %d, want 5", test.path, got)
		}
		if body["id"] != test.id {
			t.Errorf("%s: id = %v, want %s", test.path, body["id"], test.id)
		}
	}
}

func TestListEntriesWithLimit(t *testing.T) {
	app, _ := seededEntryApp(t)

	body := app.getJSONBody("/reader/api/0/stream/contents/" + readingTag + "?n=2")
	if got := len(jsonItems(t, body)); got != 2 {
		t.Errorf("items = %d, want 2", got)
	}
	// There are more entries, so the page must continue.
	if !isJSONString(body["continuation"]) {
		t.Errorf("continuation = %v", body["continuation"])
	}
}

func TestListEntriesWithContinuation(t *testing.T) {
	app := defaultITestApp(t)
	// Continuation pagination compares ids, so id order must follow the
	// timestamp order: entry 1 is the oldest (5h ago), entry 5 the newest.
	setupEntryDataWith(t, app, func(i int) (time.Time, *string) {
		return time.Now().Add(-time.Duration(6-i) * time.Hour), nil
	})
	app.Login("testuser")

	body := app.getJSONBody("/reader/api/0/stream/contents/" + readingTag + "?n=2")
	firstPage := jsonItems(t, body)
	if len(firstPage) != 2 {
		t.Fatalf("first page = %d items", len(firstPage))
	}
	continuation, _ := body["continuation"].(string)

	query := url.Values{"n": {"2"}, "c": {continuation}}
	secondPage := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readingTag+"?"+query.Encode()))
	if len(secondPage) != 2 {
		t.Fatalf("second page = %d items", len(secondPage))
	}
	for _, item := range secondPage {
		for _, seen := range firstPage {
			if item["id"] == seen["id"] {
				t.Errorf("item %v is on both pages", item["id"])
			}
		}
	}
}

func TestGetEntryById(t *testing.T) {
	app, data := seededEntryApp(t)

	items := jsonItems(t, app.getJSONBody(fmt.Sprintf("/reader/api/0/stream/items/contents?i=%d", data.entryIDs[0])))
	if len(items) != 1 {
		t.Fatalf("items = %d, want 1", len(items))
	}
	title, _ := items[0]["title"].(string)
	summary, _ := items[0]["summary"].(map[string]any)
	if !strings.Contains(title, "Entry Title") || !isJSONString(summary["content"]) || items[0]["_entryId"] != idOf(data.entryIDs[0]) {
		t.Errorf("item = %v", items[0])
	}
}

func TestGetMultipleEntriesById(t *testing.T) {
	app, data := seededEntryApp(t)

	body := app.getJSONBody(fmt.Sprintf("/reader/api/0/stream/items/contents?i=%d&i=%d", data.entryIDs[0], data.entryIDs[1]))
	if got := len(jsonItems(t, body)); got != 2 {
		t.Errorf("items = %d, want 2", got)
	}
}

func TestGetEntriesByIdPost(t *testing.T) {
	app, data := seededEntryApp(t)

	form := url.Values{}
	for _, id := range data.entryIDs[:3] {
		form.Add("i", strconv.FormatInt(id, 10))
	}
	response := app.PostForm("/reader/api/0/stream/items/contents", form)
	app.wantStatus(response, http.StatusOK)
	if got := len(jsonItems(t, decodeJSONObject(t, response))); got != 3 {
		t.Errorf("items = %d, want 3", got)
	}
}

// --- Entry Read/Unread Tests (via edit-tag) ---

func TestMarkEntryRead(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0])

	item := app.itemOf(data.entryIDs[0])
	if !isJSONString(item["_readAt"]) {
		t.Errorf("_readAt = %v, want a string once read", item["_readAt"])
	}
	if !slices.Contains(categoriesOf(t, item), readTag) {
		t.Errorf("categories = %v lack the read tag", item["categories"])
	}
}

func TestMarkEntryUnread(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0])
	app.markUnread(data.entryIDs[0])

	item := app.itemOf(data.entryIDs[0])
	if item["_readAt"] != nil {
		t.Errorf("_readAt = %v, want null", item["_readAt"])
	}
	if slices.Contains(categoriesOf(t, item), readTag) {
		t.Errorf("categories = %v must not carry the read tag", item["categories"])
	}
}

func TestListEntriesUnreadOnlyAndReadOnly(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0], data.entryIDs[1])

	for _, test := range []struct {
		stream string
		count  int
	}{{readingTag + "?xt=" + readTag, 3}, {readTag, 2}} {
		if got := app.streamItemCount("/reader/api/0/stream/contents/" + test.stream); got != test.count {
			t.Errorf("%s: items = %d, want %d", test.stream, got, test.count)
		}
	}
}

// --- Entry Star Tests (via edit-tag) ---

func TestStarEntry(t *testing.T) {
	app, data := seededEntryApp(t)

	app.starEntry(data.entryIDs[0])

	item := app.itemOf(data.entryIDs[0])
	if !isJSONString(item["_starredAt"]) {
		t.Errorf("_starredAt = %v, want a string", item["_starredAt"])
	}
	if !slices.Contains(categoriesOf(t, item), starredTag) {
		t.Errorf("categories = %v lack the starred tag", item["categories"])
	}
}

func TestUnstarEntry(t *testing.T) {
	app, data := seededEntryApp(t)

	app.starEntry(data.entryIDs[0])
	app.unstarEntry(data.entryIDs[0])

	item := app.itemOf(data.entryIDs[0])
	if item["_starredAt"] != nil {
		t.Errorf("_starredAt = %v, want null", item["_starredAt"])
	}
	if slices.Contains(categoriesOf(t, item), starredTag) {
		t.Errorf("categories = %v must not carry the starred tag", item["categories"])
	}
}

func TestListEntriesStarredOnly(t *testing.T) {
	app, data := seededEntryApp(t)

	app.starEntry(data.entryIDs[2])

	// The starred stream and the reading list filtered to starred (it=) both
	// answer with exactly the one starred entry.
	for _, stream := range []string{starredTag, readingTag + "?it=" + starredTag} {
		items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+stream))
		if len(items) != 1 || items[0]["_entryId"] != idOf(data.entryIDs[2]) {
			t.Errorf("%s: items = %v", stream, items)
		}
	}
}

// --- Mark All Read Tests ---

func TestMarkAllReadByReadingListCategoryAndFeed(t *testing.T) {
	for _, test := range []struct{ stream, path string }{
		{readingTag, readingTag},
		{"user/-/label/Test Category", "user/-/label/Test%20Category"},
		{"feed/https://example.com/feed.xml", "feed/https://example.com/feed.xml"},
	} {
		// A fresh app per stream: once one is marked read, the others are empty.
		app := defaultITestApp(t)
		setupEntryData(t, app)
		app.Login("testuser")

		app.postEditForm("/reader/api/0/mark-all-as-read", url.Values{"s": {test.stream}})

		if got := app.streamItemCount("/reader/api/0/stream/contents/" + test.path + "?xt=" + readTag); got != 0 {
			t.Errorf("%s: %d entries still unread", test.stream, got)
		}
	}
}

func TestMarkReadBatchByIds(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0], data.entryIDs[1], data.entryIDs[2])

	if got := app.unreadCount(); got != 2 {
		t.Errorf("unread = %d, want 2", got)
	}
}

func TestEditTagNoItemsReturnsError(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	app.Login("testuser")

	app.wantStatus(app.PostForm("/reader/api/0/edit-tag", url.Values{"a": {readTag}}), http.StatusBadRequest)
}

func TestMarkReadAlreadyRead(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0])
	// Marking the same entry again is idempotent.
	app.markRead(data.entryIDs[0])

	matching := 0
	for _, item := range jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readTag)) {
		if item["_entryId"] == idOf(data.entryIDs[0]) {
			matching++
		}
	}
	if matching != 1 {
		t.Errorf("the entry appears %d times, want once", matching)
	}
}

func TestCannotMarkReadByIdsOtherUser(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)
	app.Login("testuser")

	// Another user's entries are EntryNotFound, not silently skipped.
	form := url.Values{"a": {readTag}}
	for _, id := range other.entryIDs[:2] {
		form.Add("i", strconv.FormatInt(id, 10))
	}
	app.wantStatus(app.PostForm("/reader/api/0/edit-tag", form), http.StatusNotFound)
}

// --- Unread Count Tests ---

func TestGetUnreadCountWithData(t *testing.T) {
	app, _ := seededEntryApp(t)

	counts, _ := app.getJSONBody("/reader/api/0/unread-count")["unreadcounts"].([]any)
	for _, id := range []string{testFeedID, "user/-/label/Test Category", readingTag} {
		if got := countFor(counts, id); got != float64(5) {
			t.Errorf("%s: count = %v, want 5", id, got)
		}
	}
}

func TestGetUnreadCountAfterMarkingRead(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0], data.entryIDs[1])

	counts, _ := app.getJSONBody("/reader/api/0/unread-count")["unreadcounts"].([]any)
	for _, id := range []string{testFeedID, "user/-/label/Test Category"} {
		if got := countFor(counts, id); got != float64(3) {
			t.Errorf("%s: count = %v, want 3", id, got)
		}
	}
}

// --- Entry Neighbors Tests (RDRS-specific) ---

func TestGetEntryNeighbors(t *testing.T) {
	app, data := seededEntryApp(t)

	body := app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors", data.entryIDs[2]))
	for _, key := range []string{"previous_id", "next_id"} {
		if value := body[key]; value != nil && !isJSONNumber(value) {
			t.Errorf("%s = %v, want a number or null", key, value)
		}
	}
}

func TestGetEntryPosition(t *testing.T) {
	app, data := seededEntryApp(t)

	body := app.getJSONBody(fmt.Sprintf("/api/entries/%d/position", data.entryIDs[2]))
	position, hasPosition := body["position"].(float64)
	total, hasTotal := body["total"].(float64)
	if !hasPosition || !hasTotal || position < 1 || position > total {
		t.Errorf("position %v of %v", body["position"], body["total"])
	}
	if body["limit"] != float64(1000) {
		t.Errorf("limit = %v, want 1000", body["limit"])
	}
}

func TestGetEntryNeighborsAndPositionRespectSearchQuery(t *testing.T) {
	app, data := seededEntryApp(t)
	entryID := data.entryIDs[1]

	// Only "Entry Title 2" contains "2", so no neighbor may fall outside it.
	body := app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors?q=2", entryID))
	if body["prev_id"] != nil || body["next_id"] != nil {
		t.Errorf("neighbors with q = %v", body)
	}
	body = app.getJSONBody(fmt.Sprintf("/api/entries/%d/position?q=2", entryID))
	if body["position"] != float64(1) || body["total"] != float64(1) {
		t.Errorf("position with q = %v", body)
	}

	// Without q the same entry has neighbors.
	body = app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors", entryID))
	if !isJSONNumber(body["prev_id"]) || !isJSONNumber(body["next_id"]) {
		t.Errorf("neighbors without q = %v", body)
	}
}

func TestGetEntryNeighborsSearchCombinesWithUnreadSnapshot(t *testing.T) {
	app, data := seededEntryApp(t)

	query := url.Values{"q": {"2"}, "unread_only": {"true"}, "read_after": {"2000-01-01 00:00:00"}}
	body := app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors?%s", data.entryIDs[1], query.Encode()))
	if body["prev_id"] != nil || body["next_id"] != nil {
		t.Errorf("neighbors = %v", body)
	}
}

func TestGetEntryNeighborsRejectsUnparseableSearchQuery(t *testing.T) {
	app, data := seededEntryApp(t)

	query := url.Values{"q": {"(unclosed"}}
	app.wantStatus(app.Get(fmt.Sprintf("/api/entries/%d/neighbors?%s", data.entryIDs[1], query.Encode())), http.StatusBadRequest)
}

func TestGetEntryNeighborsFirstEntry(t *testing.T) {
	app, data := seededEntryApp(t)

	// The response must be a valid JSON object.
	app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors", data.entryIDs[0]))
}

func TestGetEntryNeighborsUnreadOnly(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0], data.entryIDs[2])

	body := app.getJSONBody(fmt.Sprintf("/api/entries/%d/neighbors?unread_only=true", data.entryIDs[1]))
	for _, key := range []string{"prev_id", "next_id"} {
		if _, ok := body[key]; !ok {
			t.Errorf("response lacks %s: %v", key, body)
		}
	}
}

// --- Subscription List Tests ---

func TestSubscriptionList(t *testing.T) {
	app, _ := seededEntryApp(t)

	subscriptions, _ := app.getJSONBody("/reader/api/0/subscription/list")["subscriptions"].([]any)
	if len(subscriptions) != 1 {
		t.Fatalf("subscriptions = %d, want 1", len(subscriptions))
	}
	subscription, _ := subscriptions[0].(map[string]any)
	if subscription["title"] != "Test Feed" || subscription["id"] != testFeedID {
		t.Errorf("subscription = %v", subscription)
	}
	categories, _ := subscription["categories"].([]any)
	if len(categories) != 1 {
		t.Fatalf("categories = %v", categories)
	}
	category, _ := categories[0].(map[string]any)
	if category["label"] != "Test Category" || category["id"] != "user/-/label/Test Category" {
		t.Errorf("category = %v", category)
	}
}

func TestSubscriptionEditUpdateTitle(t *testing.T) {
	app, _ := seededEntryApp(t)

	app.postEditForm("/reader/api/0/subscription/edit", url.Values{"ac": {"edit"}, "s": {testFeedID}, "t": {"Updated Feed Title"}})

	subscriptions, _ := app.getJSONBody("/reader/api/0/subscription/list")["subscriptions"].([]any)
	first, _ := subscriptions[0].(map[string]any)
	if first["title"] != "Updated Feed Title" {
		t.Errorf("title = %v", first["title"])
	}
}

func TestSubscriptionUnsubscribe(t *testing.T) {
	app, _ := seededEntryApp(t)

	app.postEditForm("/reader/api/0/subscription/edit", url.Values{"ac": {"unsubscribe"}, "s": {testFeedID}})

	subscriptions, _ := app.getJSONBody("/reader/api/0/subscription/list")["subscriptions"].([]any)
	if len(subscriptions) != 0 {
		t.Errorf("subscriptions = %v, want none", subscriptions)
	}
}

// --- Tag List Tests ---

func tagIDs(t *testing.T, app *itestApp) []string {
	t.Helper()
	var ids []string
	tags, _ := app.getJSONBody("/reader/api/0/tag/list")["tags"].([]any)
	for _, raw := range tags {
		tag, _ := raw.(map[string]any)
		id, _ := tag["id"].(string)
		ids = append(ids, id)
	}
	return ids
}

func TestTagList(t *testing.T) {
	app, _ := seededEntryApp(t)

	ids := tagIDs(t, app)
	// System tags plus the user's labels.
	for _, want := range []string{readingTag, readTag, starredTag, "user/-/label/Test Category"} {
		if !slices.Contains(ids, want) {
			t.Errorf("tag list lacks %s: %v", want, ids)
		}
	}
}

func TestRenameTag(t *testing.T) {
	app, _ := seededEntryApp(t)

	app.postEditForm("/reader/api/0/rename-tag", url.Values{"s": {"user/-/label/Test Category"}, "dest": {"user/-/label/Renamed Category"}})

	ids := tagIDs(t, app)
	if slices.Contains(ids, "user/-/label/Test Category") || !slices.Contains(ids, "user/-/label/Renamed Category") {
		t.Errorf("tag list after rename = %v", ids)
	}
}

func TestDisableTag(t *testing.T) {
	app, _ := seededEntryApp(t)

	app.postEditForm("/reader/api/0/disable-tag", url.Values{"s": {"user/-/label/Test Category"}})

	if ids := tagIDs(t, app); slices.Contains(ids, "user/-/label/Test Category") {
		t.Errorf("tag list after disable = %v", ids)
	}
}

// --- Combined Filter Tests ---

func TestListEntriesCombinedFilters(t *testing.T) {
	app, data := seededEntryApp(t)

	app.starEntry(data.entryIDs[0], data.entryIDs[1])
	app.markRead(data.entryIDs[0])

	// Starred stream, excluding read: only the unread starred entry.
	items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+starredTag+"?xt="+readTag))
	if len(items) != 1 || items[0]["_entryId"] != idOf(data.entryIDs[1]) {
		t.Errorf("starred and unread = %v", items)
	}
	// By feed, excluding read: 4 unread.
	if got := app.streamItemCount("/reader/api/0/stream/contents/" + testFeedID + "?xt=" + readTag); got != 4 {
		t.Errorf("unread in feed = %d, want 4", got)
	}
}

func TestListEntriesOldestFirst(t *testing.T) {
	app, _ := seededEntryApp(t)

	items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readingTag+"?r=o"))
	if len(items) != 5 {
		t.Fatalf("items = %d, want 5", len(items))
	}
	for i := 1; i < len(items); i++ {
		previous, _ := items[i-1]["published"].(float64)
		current, _ := items[i]["published"].(float64)
		if current < previous {
			t.Fatal("expected ascending timestamps in oldest-first order")
		}
	}
}

// --- Mark All Read with Timestamp Tests ---

func TestMarkAllReadWithTimestamp(t *testing.T) {
	app, _ := seededEntryApp(t)

	// Entries are 1-5 hours old. older_than_days is integer division, so
	// anything under a day rounds to 0 and every entry is marked; the test only
	// confirms the ts parameter passes through.
	nowMicros := strconv.FormatInt(time.Now().Unix()*1_000_000, 10)
	app.postEditForm("/reader/api/0/mark-all-as-read", url.Values{"s": {readingTag}, "ts": {nowMicros}})
}

func TestMarkAllReadSnapshotKeepsLaterEntriesUnread(t *testing.T) {
	app, _ := seededEntryApp(t)
	if got := app.unreadCount(); got != 5 {
		t.Fatalf("unread = %d, want 5", got)
	}

	// The seeded entries were stored just now, after this earlier render.
	app.postEditForm("/reader/api/0/mark-all-as-read", url.Values{"s": {readingTag}, "snapshot": {"2020-01-01 00:00:00"}})
	if got := app.unreadCount(); got != 5 {
		t.Errorf("unread = %d, want 5 after an old snapshot", got)
	}

	// A render after they were stored covers all of them.
	later := time.Now().UTC().Add(time.Minute).Format("2006-01-02 15:04:05")
	app.postEditForm("/reader/api/0/mark-all-as-read", url.Values{"s": {readingTag}, "snapshot": {later}})
	if got := app.unreadCount(); got != 0 {
		t.Errorf("unread = %d, want 0 after a later snapshot", got)
	}
}

func TestMarkAllReadRefusesAMalformedSnapshotAndMarksNothing(t *testing.T) {
	app, _ := seededEntryApp(t)

	for _, bad := range []string{"yesterday", "2026-05-01", "2026-05-01T09:00:00Z", ""} {
		response := app.PostForm("/reader/api/0/mark-all-as-read", url.Values{"s": {readingTag}, "snapshot": {bad}})
		if response.Code != http.StatusBadRequest {
			t.Errorf("snapshot %q: status = %d, want 400", bad, response.Code)
		}
	}
	if got := app.unreadCount(); got != 5 {
		t.Errorf("unread = %d, want 5", got)
	}
}

// --- GReader Item Response Format Tests ---

func TestStreamContentsItemFormat(t *testing.T) {
	app, data := seededEntryApp(t)

	item := app.itemOf(data.entryIDs[0])
	origin, _ := item["origin"].(map[string]any)
	canonical, _ := item["canonical"].([]any)
	summary, _ := item["summary"].(map[string]any)
	firstCanonical, _ := canonical[0].(map[string]any)

	checks := []struct {
		name string
		ok   bool
	}{
		{"id", isJSONString(item["id"])},
		{"published", isJSONNumber(item["published"])},
		{"updated", isJSONNumber(item["updated"])},
		{"crawlTimeMsec", isJSONString(item["crawlTimeMsec"])},
		{"timestampUsec", isJSONString(item["timestampUsec"])},
		{"title", isJSONString(item["title"])},
		{"categories", isJSONArray(item["categories"])},
		{"summary.content", isJSONString(summary["content"])},
		{"canonical", len(canonical) > 0},
		{"canonical[0].href", isJSONString(firstCanonical["href"])},
		{"origin.streamId", isJSONString(origin["streamId"])},
		{"origin.title", isJSONString(origin["title"])},
		{"_entryId", isJSONNumber(item["_entryId"])},
		{"_feedId", isJSONNumber(item["_feedId"])},
		{"_categoryId", isJSONNumber(item["_categoryId"])},
		{"_categoryName", item["_categoryName"] == "Test Category"},
		{"_feedHasIcon", isJSONBool(item["_feedHasIcon"])},
		{"_readAt is null while unread", item["_readAt"] == nil},
		{"_starredAt is null while unstarred", item["_starredAt"] == nil},
		{"_publishedAt", isJSONString(item["_publishedAt"])},
		{"origin.streamId value", origin["streamId"] == testFeedID},
	}
	for _, check := range checks {
		if !check.ok {
			t.Errorf("item field %s is wrong: %v", check.name, item)
		}
	}
}

// --- Cross-User Access Restriction Tests ---

// Reading or acting on another user's stream or entry is a 404, never a
// glimpse of it.
func TestCannotReachOtherUserStreamsOrEntries(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)
	app.Login("testuser")

	entryID := other.entryIDs[0]
	for _, test := range []struct{ method, path string }{
		{http.MethodGet, "/reader/api/0/stream/contents/user/-/label/Other%20User%20Category"},
		{http.MethodGet, "/reader/api/0/stream/contents/feed/https://other.com/feed.xml"},
		{http.MethodGet, fmt.Sprintf("/api/entries/%d/neighbors", entryID)},
		{http.MethodPost, fmt.Sprintf("/api/entries/%d/fetch-full-content", entryID)},
		{http.MethodPost, fmt.Sprintf("/api/entries/%d/summarize", entryID)},
		{http.MethodPost, fmt.Sprintf("/api/entries/%d/save", entryID)},
		{http.MethodGet, fmt.Sprintf("/api/entries/%d/summary", entryID)},
		{http.MethodDelete, fmt.Sprintf("/api/entries/%d/summary", entryID)},
	} {
		response := app.Do(itestRequest{Method: test.method, Path: test.path})
		if response.Code != http.StatusNotFound {
			t.Errorf("%s %s = %d, want 404", test.method, test.path, response.Code)
		}
	}
}

func TestCannotGetOtherUserEntry(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)
	app.Login("testuser")

	body := app.getJSONBody(fmt.Sprintf("/reader/api/0/stream/items/contents?i=%d", other.entryIDs[0]))
	if got := len(jsonItems(t, body)); got != 0 {
		t.Errorf("items = %d, an entry not owned by the user must not appear", got)
	}
}

func TestCannotTagOtherUserEntry(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)
	app.Login("testuser")

	entryID := strconv.FormatInt(other.entryIDs[0], 10)
	for _, test := range []struct{ op, tag string }{{"a", readTag}, {"r", readTag}, {"a", starredTag}} {
		response := app.PostForm("/reader/api/0/edit-tag", url.Values{"i": {entryID}, test.op: {test.tag}})
		if response.Code != http.StatusNotFound {
			t.Errorf("%s=%s: status = %d, want 404", test.op, test.tag, response.Code)
		}
	}
}

// Every GReader mutation addressed at another user's feed or category is a
// 404, the same answer a stream that does not exist gets.
func TestCannotMutateOtherUserFeedsOrCategories(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	setupSecondUserData(t, app)
	app.Login("testuser")

	const feed, category = "feed/https://other.com/feed.xml", "user/-/label/Other User Category"
	for _, test := range []struct {
		endpoint string
		form     url.Values
	}{
		{"mark-all-as-read", url.Values{"s": {feed}}},
		{"mark-all-as-read", url.Values{"s": {category}}},
		{"subscription/edit", url.Values{"ac": {"unsubscribe"}, "s": {feed}}},
		{"subscription/edit", url.Values{"ac": {"edit"}, "s": {feed}, "t": {"Hacked Title"}}},
		{"disable-tag", url.Values{"s": {category}}},
		{"rename-tag", url.Values{"s": {category}, "dest": {"user/-/label/Hacked Category"}}},
	} {
		response := app.PostForm("/reader/api/0/"+test.endpoint, test.form)
		if response.Code != http.StatusNotFound {
			t.Errorf("%s %v: status = %d, want 404", test.endpoint, test.form, response.Code)
		}
	}
}

// --- Entry with No Link Tests (RDRS-specific) ---

func TestEntryActionsNeedALink(t *testing.T) {
	app := defaultITestApp(t)
	data := setupEntryData(t, app)
	noLinkID := setupEntryWithoutLink(t, app, data.feedID)
	app.Login("testuser")

	for _, action := range []string{"fetch-full-content", "summarize", "save"} {
		response := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/api/entries/%d/%s", noLinkID, action)})
		app.wantStatus(response, http.StatusBadRequest)
		if message, _ := decodeJSONObject(t, response)["error"].(string); !strings.Contains(message, "no link") {
			t.Errorf("%s: error = %q", action, message)
		}
	}
}

// --- Save/Summarize Without Config Tests (RDRS-specific) ---

func TestSummarizeAndSaveNeedAConfiguredService(t *testing.T) {
	app, data := seededEntryApp(t)

	for _, test := range []struct{ action, wantError string }{{"summarize", "Kagi"}, {"save", "No save services"}} {
		response := app.Do(itestRequest{Method: http.MethodPost, Path: fmt.Sprintf("/api/entries/%d/%s", data.entryIDs[0], test.action)})
		app.wantStatus(response, http.StatusBadRequest)
		if message, _ := decodeJSONObject(t, response)["error"].(string); !strings.Contains(message, test.wantError) {
			t.Errorf("%s: error = %q, want it to contain %q", test.action, message, test.wantError)
		}
	}
}

// --- Stream Item IDs Tests (item.rs coverage) ---

func TestStreamItemIds(t *testing.T) {
	app, _ := seededEntryApp(t)

	refs, _ := app.getJSONBody("/reader/api/0/stream/items/ids?s=" + readingTag)["itemRefs"].([]any)
	if len(refs) != 5 {
		t.Fatalf("itemRefs = %d, want 5", len(refs))
	}
	for _, raw := range refs {
		ref, _ := raw.(map[string]any)
		if !isJSONString(ref["id"]) || !isJSONString(ref["timestampUsec"]) {
			t.Errorf("itemRef = %v needs id and timestampUsec strings", ref)
		}
	}
}

func TestStreamItemIdsWithCount(t *testing.T) {
	app, _ := seededEntryApp(t)

	body := app.getJSONBody("/reader/api/0/stream/items/ids?s=" + readingTag + "&n=2")
	if refs, _ := body["itemRefs"].([]any); len(refs) != 2 {
		t.Errorf("itemRefs = %d, want 2", len(refs))
	}
	if !isJSONString(body["continuation"]) {
		t.Errorf("continuation = %v, want a string while entries remain", body["continuation"])
	}
}

func TestStreamItemCount(t *testing.T) {
	app, _ := seededEntryApp(t)

	for _, test := range []struct{ query, count string }{
		{"", "5"}, {"?s=feed/https://example.com/feed.xml", "5"}, {"?s=" + starredTag, "0"},
	} {
		if body := app.GetOK("/reader/api/0/stream/items/count" + test.query); body != test.count {
			t.Errorf("%q: count = %q, want %s", test.query, body, test.count)
		}
	}
}

func TestStreamContentsOldestFirst(t *testing.T) {
	app, _ := seededEntryApp(t)

	items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readingTag+"?r=o"))
	if len(items) != 5 {
		t.Fatalf("items = %d, want 5", len(items))
	}
	first, _ := items[0]["published"].(float64)
	last, _ := items[4]["published"].(float64)
	if first > last {
		t.Errorf("the first item should be the oldest: first=%v, last=%v", first, last)
	}
}

func TestStreamContentsWithCount(t *testing.T) {
	app, _ := seededEntryApp(t)

	body := app.getJSONBody("/reader/api/0/stream/contents/" + readingTag + "?n=2")
	if got := len(jsonItems(t, body)); got != 2 {
		t.Errorf("items = %d, want 2", got)
	}
	if !isJSONString(body["continuation"]) {
		t.Error("expected a continuation token when there are more items")
	}
}

func TestStreamContentsWithContinuation(t *testing.T) {
	app := defaultITestApp(t)
	// Lower ids have older published_at, which continuation paging needs.
	setupEntryDataWith(t, app, func(i int) (time.Time, *string) {
		return time.Now().Add(-time.Duration(6-i) * time.Hour), nil
	})
	app.Login("testuser")

	body := app.getJSONBody("/reader/api/0/stream/contents/" + readingTag + "?n=2")
	firstPage := jsonItems(t, body)
	if len(firstPage) != 2 {
		t.Fatalf("first page = %d items", len(firstPage))
	}
	continuation, _ := body["continuation"].(string)

	query := url.Values{"n": {"2"}, "c": {continuation}}
	secondPage := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readingTag+"?"+query.Encode()))
	if len(secondPage) != 2 {
		t.Fatalf("second page = %d items", len(secondPage))
	}
	for _, item := range secondPage {
		for _, seen := range firstPage {
			if item["_entryId"] == seen["_entryId"] {
				t.Errorf("entry %v is on both pages", item["_entryId"])
			}
		}
	}
}

func TestStreamItemsContentsPost(t *testing.T) {
	app, data := seededEntryApp(t)

	response := app.PostForm("/reader/api/0/stream/items/contents", url.Values{"i": {strconv.FormatInt(data.entryIDs[0], 10)}})
	app.wantStatus(response, http.StatusOK)
	items := jsonItems(t, decodeJSONObject(t, response))
	if len(items) != 1 || items[0]["_entryId"] != idOf(data.entryIDs[0]) {
		t.Fatalf("items = %v", items)
	}
	if title, _ := items[0]["title"].(string); !strings.Contains(title, "Entry Title") {
		t.Errorf("title = %q", title)
	}
}

func TestStreamItemsContentsEmpty(t *testing.T) {
	app, _ := seededEntryApp(t)

	// GET with no i= parameters.
	if got := app.streamItemCount("/reader/api/0/stream/items/contents"); got != 0 {
		t.Errorf("items = %d, want 0", got)
	}
}

func TestStreamContentsExcludeRead(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0])

	items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+readingTag+"?xt="+readTag))
	if len(items) != 4 {
		t.Errorf("items = %d, want 4", len(items))
	}
	for _, item := range items {
		if item["_entryId"] == idOf(data.entryIDs[0]) {
			t.Error("the read entry should be excluded from the results")
		}
	}
}

func TestStreamItemIdsDefaultStream(t *testing.T) {
	app, _ := seededEntryApp(t)

	// Without s= the stream defaults to the reading list.
	refs, _ := app.getJSONBody("/reader/api/0/stream/items/ids")["itemRefs"].([]any)
	if len(refs) != 5 {
		t.Errorf("itemRefs = %d, want 5", len(refs))
	}
}

// --- User Info Tests (user.rs coverage) ---

func TestUserInfo(t *testing.T) {
	app, _ := seededEntryApp(t)

	body := app.getJSONBody("/reader/api/0/user-info")
	if !isJSONString(body["userId"]) || !isJSONString(body["userProfileId"]) || !isJSONString(body["userEmail"]) {
		t.Errorf("user-info = %v", body)
	}
	if body["userName"] != "testuser" {
		t.Errorf("userName = %v", body["userName"])
	}
	// The email follows the pattern <username>@localhost.
	if body["userEmail"] != "testuser@localhost" {
		t.Errorf("userEmail = %v", body["userEmail"])
	}
}

func TestUnreadCountWithData(t *testing.T) {
	app, _ := seededEntryApp(t)

	body := app.getJSONBody("/reader/api/0/unread-count")
	if body["max"] != float64(1000) {
		t.Errorf("max = %v, want 1000", body["max"])
	}
	counts, _ := body["unreadcounts"].([]any)
	for _, id := range []string{testFeedID, "user/-/label/Test Category"} {
		if got, _ := countFor(counts, id).(float64); got <= 0 {
			t.Errorf("%s: count = %v, want > 0", id, got)
		}
	}
	if got := countFor(counts, readingTag); got != float64(5) {
		t.Errorf("reading-list count = %v, want 5", got)
	}
	for _, raw := range counts {
		if count, _ := raw.(map[string]any); count["id"] == readingTag && !isJSONString(count["newestItemTimestampUsec"]) {
			t.Errorf("newestItemTimestampUsec = %v, want a string", count["newestItemTimestampUsec"])
		}
	}
}

func TestUnreadCountAfterRead(t *testing.T) {
	app, data := seededEntryApp(t)

	app.markRead(data.entryIDs[0], data.entryIDs[1], data.entryIDs[2])

	counts, _ := app.getJSONBody("/reader/api/0/unread-count")["unreadcounts"].([]any)
	for _, id := range []string{testFeedID, readingTag} {
		if got := countFor(counts, id); got != float64(2) {
			t.Errorf("%s: count = %v, want 2", id, got)
		}
	}
}

// --- Star/Unstar via Stream Tests (models/entry.rs coverage) ---

func TestStarAndUnstarEntry(t *testing.T) {
	app, data := seededEntryApp(t)

	app.starEntry(data.entryIDs[1])
	items := jsonItems(t, app.getJSONBody("/reader/api/0/stream/contents/"+starredTag))
	if len(items) != 1 || items[0]["_entryId"] != idOf(data.entryIDs[1]) {
		t.Fatalf("starred = %v", items)
	}

	app.unstarEntry(data.entryIDs[1])
	if got := app.streamItemCount("/reader/api/0/stream/contents/" + starredTag); got != 0 {
		t.Errorf("starred after unstar = %d, want 0", got)
	}
}

func TestFindByIdsWithFeedEmpty(t *testing.T) {
	app, _ := seededEntryApp(t)

	// Nonexistent ids answer with no items.
	if got := app.streamItemCount("/reader/api/0/stream/items/contents?i=999999&i=999998"); got != 0 {
		t.Errorf("items = %d, want 0", got)
	}
}

func TestStreamContentsTimeFilter(t *testing.T) {
	app, _ := seededEntryApp(t)

	// Entries are now-1h .. now-5h; ot is 3.5 hours ago, so the 1h, 2h and 3h
	// entries are newer.
	oldest := time.Now().Unix() - (3*3600 + 1800)
	items := jsonItems(t, app.getJSONBody(fmt.Sprintf("/reader/api/0/stream/contents/%s?ot=%d", readingTag, oldest)))
	if len(items) < 2 || len(items) > 4 {
		t.Errorf("expected 2-4 items with the time filter, got %d", len(items))
	}
	for _, item := range items {
		if published, _ := item["published"].(float64); int64(published) < oldest {
			t.Errorf("published %v should be >= ot %d", published, oldest)
		}
	}
}

// --- Entry Summary Tests (RDRS-specific) ---

func TestGetEntrySummaryNotFound(t *testing.T) {
	app, data := seededEntryApp(t)

	response := app.Get(fmt.Sprintf("/api/entries/%d/summary", data.entryIDs[0]))
	app.wantStatus(response, http.StatusNotFound)
	if message, _ := decodeJSONObject(t, response)["error"].(string); !strings.Contains(message, "No summary") {
		t.Errorf("error = %q", message)
	}
}

func TestDeleteEntrySummary(t *testing.T) {
	app, data := seededEntryApp(t)

	response := app.Do(itestRequest{Method: http.MethodDelete, Path: fmt.Sprintf("/api/entries/%d/summary", data.entryIDs[0])})
	app.wantStatus(response, http.StatusOK)
	if decodeJSONObject(t, response)["success"] != true {
		t.Errorf("body = %s", response.Text())
	}
}

// --- Summary Fragment Tests (SSE swap endpoint) ---

func (a *itestApp) storeCompletedSummary(userID, entryID int64, html string) {
	a.t.Helper()
	ctx := context.Background()
	if _, err := models.UpsertPendingSummary(ctx, a.DB, userID, entryID); err != nil {
		a.t.Fatal(err)
	}
	if _, err := models.SetSummaryCompleted(ctx, a.DB, userID, entryID, html); err != nil {
		a.t.Fatal(err)
	}
}

func TestSummaryFragmentRendersCompletedSummary(t *testing.T) {
	app, data := seededEntryApp(t)
	app.storeCompletedSummary(data.userID, data.entryIDs[0], "<p>Test summary content</p>")

	body := app.GetOK(fmt.Sprintf("/entries/%d/summary/fragment", data.entryIDs[0]))
	if !strings.Contains(body, `data-swap-target="#rp-summary-container"`) {
		t.Error("body should contain the swap target attribute")
	}
	if !strings.Contains(body, "rp-summary-content") {
		t.Error("body should contain the completed summary blockquote class")
	}
}

// The cache is ahead of the table while a worker runs; the pane follows it.
func TestEntryFragmentReadsACachedSummaryWithoutADbRow(t *testing.T) {
	app, data := seededEntryApp(t)
	app.App.Summaries.SetCompleted(data.userID, data.entryIDs[0], "<p>Cached summary text</p>")

	if body := app.GetOK(fmt.Sprintf("/entries/%d/fragment", data.entryIDs[0])); !strings.Contains(body, "Cached summary text") {
		t.Errorf("pane lost it: %s", body)
	}
}

func TestEntryFragmentShowsAnInFlightCachedSummaryAsPending(t *testing.T) {
	app, data := seededEntryApp(t)
	app.App.Summaries.SetProcessing(data.userID, data.entryIDs[0])

	if body := app.GetOK(fmt.Sprintf("/entries/%d/fragment", data.entryIDs[0])); !strings.Contains(body, "data-summary-pending") {
		t.Errorf("pane: %s", body)
	}
}

// Kagi writes the summary from a page nobody here controls. Sanitizing on
// read, not on write, lets a row already in the table come back clean.
func TestSummaryFragmentSanitizesAHostileSummary(t *testing.T) {
	app, data := seededEntryApp(t)
	app.storeCompletedSummary(data.userID, data.entryIDs[0],
		`<style>body{background:url(//evil.tld/x)}</style><p>Real <a href="https://evil.tld/login">summary</a> <img src="//evil.tld/beacon.gif"></p>`)

	body := app.GetOK(fmt.Sprintf("/entries/%d/summary/fragment", data.entryIDs[0]))
	wantAbsent(t, body, "evil.tld", "<style")
	wantContains(t, body, "Real", "summary")
}

func TestSummaryFragment404ForOtherUsersEntry(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)
	app.Login("testuser")

	app.wantStatus(app.Get(fmt.Sprintf("/entries/%d/summary/fragment", other.entryIDs[0])), http.StatusNotFound)
}

// --- Fetched full content is persisted ---
//
// These store through the model rather than the handler, because the fetch
// runs every URL through the SSRF guard, which blocks loopback. The handler's
// failure arm is covered in nojs_test.go.

// upsertFetchable adds an entry with a link and the given feed body.
func upsertFetchable(t *testing.T, app *itestApp, feedID int64, guid, title, content string) int64 {
	t.Helper()
	link := "https://example.com/article"
	result, err := entry.UpsertEntry(context.Background(), app.DB, &entry.Upsert{
		FeedID: feedID, GUID: guid, Title: &title, Link: &link, Content: &content,
	})
	if err != nil {
		t.Fatal(err)
	}
	return result.ID
}

func setFullContent(t *testing.T, app *itestApp, userID, entryID int64, html string) {
	t.Helper()
	if err := entry.SetFullContentForUser(context.Background(), app.DB, userID, entryID, html); err != nil {
		t.Fatal(err)
	}
}

// The article "Fetch Full Content" retrieves used to live only inside the
// response that fetched it: a refresh lost it, and a reader without JavaScript
// could never see it.
func TestStoredFullContentSurvivesAReRenderAndToggles(t *testing.T) {
	app := defaultITestApp(t)
	data := setupEntryData(t, app)
	entryID := upsertFetchable(t, app, data.feedID, "full-content-guid", "Fetchable", "<p>Just the feed summary.</p>")
	app.Login("testuser")
	fragment := fmt.Sprintf("/entries/%d/fragment", entryID)

	// Before anything is fetched the pane shows the feed's body and offers to fetch.
	before := app.GetOK(fragment)
	wantContains(t, before, "Just the feed summary.", `aria-label="Fetch Full Content"`)

	setFullContent(t, app, data.userID, entryID, "<p>The whole article, fetched.</p>")

	// A fresh render has it, because it was written down.
	again := app.GetOK(fragment)
	if !strings.Contains(again, "The whole article, fetched.") {
		t.Errorf("the stored article must survive a re-render:\n%s", again)
	}
	if !strings.Contains(again, `aria-label="Show Original"`) {
		t.Error("and the pane must offer the way back")
	}

	// ?view=original returns the feed's own body without discarding the stored
	// article, and links back without a second request to the source.
	original := app.GetOK(fragment + "?view=original")
	wantContains(t, original, "Just the feed summary.")
	wantAbsent(t, original, "The whole article, fetched.")
	if !strings.Contains(original, `aria-label="Show Full Content"`) {
		t.Errorf("going back must be a link, not a re-fetch:\n%s", original)
	}

	// The deep-linked list page, which a scriptless reader lands on, renders it too.
	if deepLinked := app.GetOK(fmt.Sprintf("/entries?entry=%d", entryID)); !strings.Contains(deepLinked, "The whole article, fetched.") {
		t.Error("a full page render must show the stored article")
	}
}

// Storing again replaces what was there, so the fetch button stays a refresh.
func TestStoringFullContentAgainReplacesIt(t *testing.T) {
	app := defaultITestApp(t)
	data := setupEntryData(t, app)
	entryID := upsertFetchable(t, app, data.feedID, "refetch-guid", "Refetchable", "<p>Feed summary.</p>")
	app.Login("testuser")

	setFullContent(t, app, data.userID, entryID, "<p>First version.</p>")
	setFullContent(t, app, data.userID, entryID, "<p>Second version.</p>")

	after := app.GetOK(fmt.Sprintf("/entries/%d/fragment", entryID))
	if !strings.Contains(after, "Second version.") {
		t.Errorf("a re-fetch must replace what was stored:\n%s", after)
	}
	wantAbsent(t, after, "First version.")
}

// The write is scoped to the owning user like every other per-user entry
// write, so one account cannot plant an article in another's entry.
func TestStoringFullContentIsScopedToTheOwner(t *testing.T) {
	app := defaultITestApp(t)
	setupEntryData(t, app)
	other := setupSecondUserData(t, app)

	// A user id that owns nothing, against the second user's entry.
	setFullContent(t, app, other.userID+999, other.entryIDs[0], "<p>Planted.</p>")

	var stored *string
	if err := app.DB.QueryRowContext(context.Background(), "SELECT full_content FROM entry WHERE id = $1", other.entryIDs[0]).Scan(&stored); err != nil {
		t.Fatal(err)
	}
	if stored != nil {
		t.Errorf("a non-owner write must touch nothing, got %q", *stored)
	}
}
