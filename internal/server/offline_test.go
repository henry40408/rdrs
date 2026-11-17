package server

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Port of tests/offline_test.rs: the sync ledger, the library page, and the
// property that mirroring a reader's queue does not consume it.

func offlineUser(t *testing.T, app *itestApp, username string) *models.User {
	t.Helper()
	user, err := models.FindUserByUsername(context.Background(), app.DB, username)
	if err != nil || user == nil {
		t.Fatalf("user %s = %v, err = %v", username, user, err)
	}
	return user
}

// seedOfflineEntries gives username one feed and count entries, oldest first.
func seedOfflineEntries(t *testing.T, app *itestApp, username string, count int) []int64 {
	t.Helper()
	ctx := context.Background()
	user := offlineUser(t, app, username)
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "cat-"+username)
	if err != nil {
		t.Fatal(err)
	}
	title := "Feed"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{CategoryID: category.ID, URL: "https://example.com/" + username + ".xml", Title: &title})
	if err != nil {
		t.Fatal(err)
	}
	ids := make([]int64, 0, count)
	for i := 0; i < count; i++ {
		entryTitle, link, content := fmt.Sprintf("Entry %d", i), fmt.Sprintf("https://example.com/%s/%d", username, i), fmt.Sprintf("<p>Body %d.</p>", i)
		// Ascending published_at, so the last seeded entry is the newest.
		published := time.Now().Add(-time.Duration(count-i) * time.Minute)
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feed.ID, GUID: fmt.Sprintf("guid-%s-%d", username, i),
			Title: &entryTitle, Link: &link, Content: &content, PublishedAt: &published,
		})
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, result.ID)
	}
	return ids
}

func setOfflineKeep(t *testing.T, app *itestApp, username string, keep int64) {
	t.Helper()
	if err := models.UpdateOfflineKeep(context.Background(), app.DB, offlineUser(t, app, username).ID, keep); err != nil {
		t.Fatal(err)
	}
}

func offlineApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	app.SeedAccount("reader", fixturePassword, models.RoleAdmin)
	app.SeedAccount("other", fixturePassword, models.RoleUser)
	return app
}

type manifestBody struct {
	Keep     int64  `json:"keep"`
	CacheKey string `json:"cache_key"`
	Entries  []struct {
		ID int64 `json:"id"`
	} `json:"entries"`
}

func (m manifestBody) ids() []int64 {
	ids := []int64{}
	for _, e := range m.Entries {
		ids = append(ids, e.ID)
	}
	return ids
}

func getManifest(t *testing.T, app *itestApp) manifestBody {
	t.Helper()
	response := app.Get("/api/offline/manifest")
	wantStatus(t, response, http.StatusOK)
	var manifest manifestBody
	if err := json.Unmarshal(response.Body.Bytes(), &manifest); err != nil {
		t.Fatalf("manifest is not JSON: %v: %s", err, response.Text())
	}
	return manifest
}

func TestManifestRequiresASession(t *testing.T) {
	if defaultITestApp(t).Get("/api/offline/manifest").Code == http.StatusOK {
		t.Error("the manifest needs a session")
	}
}

func TestManifestIsEmptyUntilTheReaderOptsIn(t *testing.T) {
	app := offlineApp(t)
	seedOfflineEntries(t, app, "reader", 3)
	app.Login("reader")
	manifest := getManifest(t, app)
	if manifest.Keep != 0 || len(manifest.Entries) != 0 {
		t.Errorf("manifest = %+v", manifest)
	}
}

func TestManifestHoldsTheNewestUnreadEntriesUpToTheBudget(t *testing.T) {
	app := offlineApp(t)
	ids := seedOfflineEntries(t, app, "reader", 5)
	setOfflineKeep(t, app, "reader", 2)
	app.Login("reader")
	// Newest first, and only as many as the reader asked for.
	if got, want := getManifest(t, app).ids(), []int64{ids[4], ids[3]}; !slices.Equal(got, want) {
		t.Errorf("entries = %v, want %v", got, want)
	}
}

func TestManifestNeverNamesAnotherReadersEntries(t *testing.T) {
	app := offlineApp(t)
	mine := seedOfflineEntries(t, app, "reader", 2)
	theirs := seedOfflineEntries(t, app, "other", 2)
	setOfflineKeep(t, app, "reader", 50)
	app.Login("reader")
	got := getManifest(t, app).ids()
	for _, id := range mine {
		if !slices.Contains(got, id) {
			t.Errorf("own entry %d missing from the manifest", id)
		}
	}
	for _, id := range theirs {
		if slices.Contains(got, id) {
			t.Errorf("another reader's entry %d leaked", id)
		}
	}
}

func TestManifestCacheKeyIsOpaqueAndDiffersPerReader(t *testing.T) {
	app := offlineApp(t)
	app.Login("reader")
	mineKey := getManifest(t, app).CacheKey
	app.Login("other")
	theirsKey := getManifest(t, app).CacheKey
	if mineKey == theirsKey {
		t.Error("the cache key must differ per reader")
	}
	// A user id here would tell every page how many accounts exist.
	if mineKey == fmt.Sprint(offlineUser(t, app, "reader").ID) {
		t.Error("the cache key must not be the user id")
	}
	if len(mineKey) != 16 || strings.Trim(mineKey, "0123456789abcdefABCDEF") != "" {
		t.Errorf("expected a truncated hex tag, got %q", mineKey)
	}
}

// readAtWithin polls for read_at: the mark-read write is dispatched off the
// critical path, so a bare read straight after the response races it. The
// negative assertion is only worth something if the positive one would have
// been observed by the same wait.
func readAtWithin(t *testing.T, app *itestApp, id int64, attempts int) *time.Time {
	t.Helper()
	for i := 0; i < attempts; i++ {
		time.Sleep(20 * time.Millisecond)
		found, err := entry.FindByID(context.Background(), app.DB, id)
		if err != nil || found == nil {
			t.Fatalf("entry = %v, err = %v", found, err)
		}
		if found.ReadAt != nil {
			return found.ReadAt
		}
	}
	return nil
}

func fragmentRequest(id int64, query string) itestRequest {
	return itestRequest{Path: fmt.Sprintf("/entries/%d/fragment%s", id, query), Headers: map[string]string{"Sec-Fetch-Dest": "empty"}}
}

func TestPrefetchingAFragmentLeavesTheEntryUnread(t *testing.T) {
	app := offlineApp(t)
	ids := seedOfflineEntries(t, app, "reader", 1)
	app.Login("reader")
	wantStatus(t, app.Do(fragmentRequest(ids[0], "?offline=1")), http.StatusOK)
	if readAtWithin(t, app, ids[0], 50) != nil {
		t.Error("a sync opens every entry in the queue; marking them read would empty the backlog")
	}
}

func TestOpeningAFragmentNormallyStillMarksItRead(t *testing.T) {
	app := offlineApp(t)
	ids := seedOfflineEntries(t, app, "reader", 1)
	app.Login("reader")
	wantStatus(t, app.Do(fragmentRequest(ids[0], "")), http.StatusOK)
	if readAtWithin(t, app, ids[0], 50) == nil {
		t.Error("opening an entry must still mark it read")
	}
}

func TestLibraryListsWhatTheBrowserShouldBeHolding(t *testing.T) {
	app := offlineApp(t)
	ids := seedOfflineEntries(t, app, "reader", 3)
	setOfflineKeep(t, app, "reader", 2)
	app.Login("reader")
	body := app.GetOK("/entries/offline")
	for _, id := range ids[1:] {
		if !strings.Contains(body, fmt.Sprintf("entry-row-%d", id)) {
			t.Errorf("entry %d missing", id)
		}
	}
	if strings.Contains(body, fmt.Sprintf("entry-row-%d", ids[0])) {
		t.Error("the oldest entry is outside the budget and must not be listed")
	}
	// Nothing on this page may need the network to be useful.
	if strings.Contains(body, `id="load-more"`) || strings.Contains(body, "data-entries-search") {
		t.Error("the library must not offer Load More or search")
	}
}

func TestLibrarySaysSoWhenOfflineReadingIsOff(t *testing.T) {
	app := offlineApp(t)
	seedOfflineEntries(t, app, "reader", 2)
	app.Login("reader")
	body := app.Get("/entries/offline").Text()
	if !strings.Contains(body, "Offline reading is off") || !strings.Contains(body, "Settings") {
		t.Errorf("body = %s", body)
	}
}

// Every other list stops at a page of 50 and offers Load More, which needs the
// server; the library must list the whole budget.
func TestLibraryListsTheWholeBudgetPastTheFirstPageOfAList(t *testing.T) {
	app := offlineApp(t)
	ids := seedOfflineEntries(t, app, "reader", 55)
	setOfflineKeep(t, app, "reader", 200)
	app.Login("reader")
	if !strings.Contains(app.Get("/").Text(), `id="load-more"`) {
		t.Fatal("sanity: 55 entries is expected to leave a second page of the inbox")
	}
	body := app.Get("/entries/offline").Text()
	for _, id := range ids {
		if !strings.Contains(body, fmt.Sprintf("entry-row-%d", id)) {
			t.Errorf("entry %d is saved but the library page does not list it", id)
		}
	}
	if strings.Contains(body, `id="load-more"`) {
		t.Error("the library must not offer Load More")
	}
}

func TestLibraryRequiresASession(t *testing.T) {
	if defaultITestApp(t).Get("/entries/offline").Code == http.StatusOK {
		t.Error("the library needs a session")
	}
}

func preferencesForm(offlineKeep string) url.Values {
	return url.Values{"theme": {"system"}, "entries_per_page": {"30"}, "retention_read_days": {"0"}, "offline_keep": {offlineKeep}}
}

func storedOfflineKeep(t *testing.T, app *itestApp, username string) int64 {
	t.Helper()
	keep, err := models.OfflineKeep(context.Background(), app.DB, offlineUser(t, app, username).ID)
	if err != nil {
		t.Fatal(err)
	}
	return keep
}

func TestThePreferencesFormSavesTheBudget(t *testing.T) {
	app := offlineApp(t)
	app.Login("reader")
	response := app.PostForm("/user-settings/preferences", preferencesForm("25"))
	if response.Code < 300 || response.Code >= 400 {
		t.Fatalf("status = %d: %s", response.Code, response.Text())
	}
	if got := storedOfflineKeep(t, app, "reader"); got != 25 {
		t.Errorf("offline_keep = %d", got)
	}
}

func TestThePreferencesFormRejectsABudgetOverTheCap(t *testing.T) {
	app := offlineApp(t)
	setOfflineKeep(t, app, "reader", 25)
	app.Login("reader")
	response := app.PostForm("/user-settings/preferences", preferencesForm("5000"))
	if !strings.Contains(flashText(t, response), "offline_keep") {
		t.Errorf("the flash must name offline_keep: %s", response.Header().Get("Set-Cookie"))
	}
	if got := storedOfflineKeep(t, app, "reader"); got != 25 {
		t.Errorf("a rejected value must leave the stored one alone, got %d", got)
	}
}

// A value the form's own datalist offers must never be refused by the handler.
func TestOfflineKeepSuggestionsAreAllAccepted(t *testing.T) {
	app := offlineApp(t)
	user := offlineUser(t, app, "reader")
	for _, keep := range models.OfflineKeepSuggestions {
		if err := models.UpdateOfflineKeep(context.Background(), app.DB, user.ID, keep); err != nil {
			t.Errorf("suggestion %d rejected: %v", keep, err)
		}
	}
}

func TestSignedInPagesCarryTheCacheNameAndBudget(t *testing.T) {
	app := offlineApp(t)
	seedOfflineEntries(t, app, "reader", 1)
	setOfflineKeep(t, app, "reader", 50)
	app.Login("reader")
	body := app.Get("/").Text()
	// offline.js reads both before its first network call, so another
	// account's cached articles are gone before anything is served from them.
	for _, want := range []string{`data-offline-key="`, `data-offline-keep="50"`, "/static/js/offline.js"} {
		if !strings.Contains(body, want) {
			t.Errorf("page lacks %s", want)
		}
	}
}

func TestTheScriptlessNavigationLeadsToTheLibrary(t *testing.T) {
	app := offlineApp(t)
	seedOfflineEntries(t, app, "reader", 1)
	setOfflineKeep(t, app, "reader", 50)
	app.Login("reader")
	if !strings.Contains(app.Get("/").Text(), `href="/entries/offline"`) {
		t.Error("the scriptless nav must link to the library")
	}
}

// Its own app: the chrome is read once per session and cached.
func TestTheScriptlessNavigationOmitsTheLibraryWhileNothingIsSaved(t *testing.T) {
	app := offlineApp(t)
	seedOfflineEntries(t, app, "reader", 1)
	app.Login("reader")
	if strings.Contains(app.Get("/").Text(), "/entries/offline") {
		t.Error("a link to a page nothing is saved to invites an empty list")
	}
}

func TestTheSignInPageBootstrapsNoOfflineState(t *testing.T) {
	body := defaultITestApp(t).Get("/login").Text()
	if strings.Contains(body, "data-offline-key") || strings.Contains(body, "/static/js/offline.js") {
		t.Error("an anonymous page must not be handed a cache key")
	}
}
