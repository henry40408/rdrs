package server

import (
	"context"
	"fmt"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/pixel"
	"github.com/henry40408/rdrs/internal/secret"
)

// Port of tests/pixel_tracking_test.rs: what the open-tracking endpoint
// records and refuses to record, and that the pixel survives the sanitiser
// and stays same-origin.

var pixelTestSecret = make([]byte, 32)

func pixelApp(t *testing.T) *itestApp {
	t.Helper()
	return pixelAppWithBaseURL(t, "")
}

func pixelAppWithBaseURL(t *testing.T, baseURL string) *itestApp {
	t.Helper()
	config := itestConfig()
	config.PublicBaseURL = baseURL
	app := newITestApp(t, config)
	app.SeedAccount("reader", fixturePassword, models.RoleAdmin)
	app.SeedAccount("other", fixturePassword, models.RoleUser)
	return app
}

func pixelUserID(t *testing.T, app *itestApp, username string) int64 {
	t.Helper()
	return offlineUser(t, app, username).ID
}

// seedPixelFeed gives username a feed titled label with count entries.
func seedPixelFeed(t *testing.T, app *itestApp, username, label string, count int) []int64 {
	t.Helper()
	ctx := context.Background()
	category, err := models.CreateCategory(ctx, app.DB, pixelUserID(t, app, username), fmt.Sprintf("cat-%s-%s", username, label))
	if err != nil {
		t.Fatal(err)
	}
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: fmt.Sprintf("https://example.com/%s-%s.xml", username, label), Title: &label,
	})
	if err != nil {
		t.Fatal(err)
	}
	ids := make([]int64, 0, count)
	for i := 0; i < count; i++ {
		title, link, content := fmt.Sprintf("Entry %d", i), fmt.Sprintf("https://example.com/%s/%s/%d", username, label, i), fmt.Sprintf("<p>Body %d.</p>", i)
		published := time.Now()
		result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
			FeedID: feed.ID, GUID: fmt.Sprintf("guid-%s-%s-%d", username, label, i),
			Title: &title, Link: &link, Content: &content, PublishedAt: &published,
		})
		if err != nil {
			t.Fatal(err)
		}
		ids = append(ids, result.ID)
	}
	return ids
}

func seedPixelEntries(t *testing.T, app *itestApp, username string, count int) []int64 {
	t.Helper()
	return seedPixelFeed(t, app, username, "Feed", count)
}

func enablePixelTracking(t *testing.T, app *itestApp, username string) {
	t.Helper()
	if err := models.UpdatePixelTracking(context.Background(), app.DB, pixelUserID(t, app, username), true); err != nil {
		t.Fatal(err)
	}
}

func openCount(t *testing.T, app *itestApp) int64 {
	t.Helper()
	var count int64
	if err := app.DB.QueryRowContext(context.Background(), "SELECT COUNT(*) FROM entry_open").Scan(&count); err != nil {
		t.Fatal(err)
	}
	return count
}

// pixelPath is the pixel URL the render paths would embed, built the same way.
func pixelPath(userID, entryID int64) string {
	return fmt.Sprintf("/p/%d-%d-%s.gif", userID, entryID, secret.PixelSig(pixelTestSecret, userID, entryID))
}

func wantPixelOK(t *testing.T, app *itestApp, path string) itestResponse {
	t.Helper()
	response := app.Get(path)
	if response.Code != 200 {
		t.Fatalf("GET %s = %d", path, response.Code)
	}
	return response
}

func wantOpenCount(t *testing.T, app *itestApp, want int64, why string) {
	t.Helper()
	if got := openCount(t, app); got != want {
		t.Errorf("entry_open rows = %d, want %d (%s)", got, want, why)
	}
}

// --- The endpoint ---

func TestAHitRecordsOneOpenAndIsIdempotent(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	path := pixelPath(pixelUserID(t, app, "reader"), entries[0])

	wantPixelOK(t, app, path)
	wantOpenCount(t, app, 1, "first hit")
	// The metric is "entries opened", not "requests served".
	wantPixelOK(t, app, path)
	wantOpenCount(t, app, 1, "a repeated hit must not inflate the count")
}

func TestTheResponseIsAnUncacheableGif(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")

	response := wantPixelOK(t, app, pixelPath(pixelUserID(t, app, "reader"), entries[0]))
	if got := response.Header().Get("Content-Type"); got != "image/gif" {
		t.Errorf("content-type = %q", got)
	}
	if !strings.Contains(response.Header().Get("Cache-Control"), "no-store") {
		t.Errorf("a cached pixel never reports again: %q", response.Header().Get("Cache-Control"))
	}
	if got := response.Body.Len(); got != 43 {
		t.Errorf("body = %d bytes, want 43", got)
	}
}

func TestABadSignatureRecordsNothingAndStillReturnsTheGif(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")

	// A 403 would tell whoever is probing which tokens are real.
	for _, path := range []string{
		fmt.Sprintf("/p/%d-%d-deadbeef.gif", userID, entries[0]),
		fmt.Sprintf("/p/%d-%d-.gif", userID, entries[0]),
		"/p/nonsense.gif",
	} {
		if got := wantPixelOK(t, app, path).Body.Len(); got != 43 {
			t.Errorf("%s: body = %d bytes", path, got)
		}
	}
	wantOpenCount(t, app, 0, "bad signatures")
}

func TestAnOptedOutReaderRecordsNothing(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)

	// Also the shape of a hit on HTML rendered before the reader opted out.
	wantPixelOK(t, app, pixelPath(pixelUserID(t, app, "reader"), entries[0]))
	wantOpenCount(t, app, 0, "tracking was never turned on")
}

func TestEntriesPredatingTheOptInAreNotRecorded(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	userID := pixelUserID(t, app, "reader")
	enablePixelTracking(t, app, "reader")

	// Push the baseline past the entry, as opting in tomorrow would.
	if _, err := app.DB.ExecContext(context.Background(),
		"UPDATE user_settings SET pixel_tracking_enabled_at = datetime('now', '+1 day') WHERE user_id = $1", userID); err != nil {
		t.Fatal(err)
	}

	wantPixelOK(t, app, pixelPath(userID, entries[0]))
	wantOpenCount(t, app, 0, "the backlog is outside the denominator")
}

func TestATokenCannotRecordAnOpenOnAnotherReadersEntry(t *testing.T) {
	app := pixelApp(t)
	otherEntries := seedPixelEntries(t, app, "other", 1)
	seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	enablePixelTracking(t, app, "other")

	// Signed for reader, but the entry belongs to other: the ownership join
	// stops one account writing into another's counts.
	wantPixelOK(t, app, pixelPath(pixelUserID(t, app, "reader"), otherEntries[0]))
	wantOpenCount(t, app, 0, "cross-account entry")
}

// --- The render path ---

func TestTheReadingPaneCarriesAnUnproxiedSameOriginPixel(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")

	response := app.Get(fmt.Sprintf("/entries/%d/fragment", entries[0]))
	if response.Code != 200 {
		t.Fatalf("status = %d", response.Code)
	}
	body := response.Text()
	// Both only hold because injection runs after sanitising.
	if expected := pixelPath(userID, entries[0]); !strings.Contains(body, expected) {
		t.Errorf("reading pane should carry %s", expected)
	}
	if proxied := fmt.Sprintf("/api/proxy/image?url=%%2Fp%%2F%d", userID); strings.Contains(body, proxied) {
		t.Error("the pixel must not be routed through the image proxy")
	}
}

func TestAnOptedOutReadingPaneCarriesNoPixel(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	app.Login("reader")

	response := app.Get(fmt.Sprintf("/entries/%d/fragment", entries[0]))
	if response.Code != 200 {
		t.Fatalf("status = %d", response.Code)
	}
	if strings.Contains(response.Text(), pixel.PathPrefix) {
		t.Error("an opted-out reading pane must carry no pixel")
	}
}

func TestGreaderContentCarriesAnAbsolutePixel(t *testing.T) {
	// External clients render the content off-origin, so a root-relative URL
	// would resolve against their own host and never reach us.
	app := pixelAppWithBaseURL(t, "https://rdrs.example.com")
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")

	response := app.Get("/reader/api/0/stream/contents/user/-/state/com.google/reading-list")
	if response.Code != 200 {
		t.Fatalf("status = %d: %s", response.Code, response.Text())
	}
	sig := secret.PixelSig(pixelTestSecret, userID, entries[0])
	want := fmt.Sprintf("https://rdrs.example.com/p/%d-%d-%s.gif", userID, entries[0], sig)
	if !strings.Contains(response.Text(), want) {
		t.Errorf("greader item content should carry an absolute pixel: %s", response.Text())
	}
}

func TestTheOfflineMirrorFragmentCarriesNoPixel(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 1)
	enablePixelTracking(t, app, "reader")
	app.Login("reader")
	fragment := fmt.Sprintf("/entries/%d/fragment", entries[0])

	// offline.js fetches every same-origin <img src> it caches, so a pixel
	// here would report an open for every entry merely queued.
	response := app.Get(fragment + "?offline=1")
	if response.Code != 200 || strings.Contains(response.Text(), pixel.PathPrefix) {
		t.Errorf("the offline mirror must not carry a pixel (status %d)", response.Code)
	}

	prefetch := app.Do(itestRequest{Path: fragment, Headers: map[string]string{"Sec-Purpose": "prefetch"}})
	if prefetch.Code != 200 || strings.Contains(prefetch.Text(), pixel.PathPrefix) {
		t.Errorf("a speculative fetch must not carry a pixel (status %d)", prefetch.Code)
	}

	// A real open still does, or the feature would record nothing at all.
	if !strings.Contains(app.Get(fragment).Text(), pixel.PathPrefix) {
		t.Error("a real open must carry a pixel")
	}
}

// --- The read-side UI ---

func TestTheFeedsColumnAppearsOnlyOnceTrackingIsOn(t *testing.T) {
	app := pixelApp(t)
	seedPixelEntries(t, app, "reader", 1)
	app.Login("reader")

	// A column of dashes would be worse than no column.
	if strings.Contains(app.Get("/feeds").Text(), "Open Rate") {
		t.Error("opted out: no Open Rate column")
	}
	enablePixelTracking(t, app, "reader")
	if !strings.Contains(app.Get("/feeds").Text(), "Open Rate") {
		t.Error("opted in: Open Rate column expected")
	}
}

func TestTheFeedsColumnSuppressesARateItCannotStandBehind(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", int(models.MinTrackedForRate)-1)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")

	app.Get(pixelPath(userID, entries[0]))

	// One open out of four is 25%, but on four entries that is noise.
	body := app.Get("/feeds").Text()
	if !strings.Contains(body, "Needs at least") {
		t.Error("a sub-floor sample must render the dash, not a percentage")
	}
	if strings.Contains(body, "25%") {
		t.Error("a sub-floor sample must not show 25%")
	}
}

func TestTheFeedsColumnReportsTheRateAndItsCounts(t *testing.T) {
	app := pixelApp(t)
	entries := seedPixelEntries(t, app, "reader", 10)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")

	for _, id := range entries[:4] {
		app.Get(pixelPath(userID, id))
	}

	// 40% of ten is a very different claim from 40% of five.
	if body := app.Get("/feeds").Text(); !strings.Contains(body, "40% (4/10)") {
		t.Errorf("missing the rate with its counts: %s", body)
	}
}

func TestTheStatisticsSectionRanksTheLeastOpenedFeedFirst(t *testing.T) {
	app := pixelApp(t)
	popular := seedPixelFeed(t, app, "reader", "Feed", 10)
	ignored := seedPixelFeed(t, app, "reader", "Ignored", 10)
	enablePixelTracking(t, app, "reader")
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")

	for _, id := range popular[:8] {
		app.Get(pixelPath(userID, id))
	}
	app.Get(pixelPath(userID, ignored[0]))

	body := app.Get("/statistics").Text()
	if !strings.Contains(body, "Feeds by Open Rate") || !strings.Contains(body, "Tracked since") {
		t.Fatalf("the open-rate section is missing: %s", body)
	}
	ignoredAt, popularAt := strings.Index(body, "10% (1/10)"), strings.Index(body, "80% (8/10)")
	if ignoredAt < 0 || popularAt < 0 {
		t.Fatalf("both feeds must be listed (ignored at %d, popular at %d)", ignoredAt, popularAt)
	}
	if ignoredAt > popularAt {
		t.Error("the unsubscribe candidate belongs at the top")
	}
}

func TestTheStatisticsSectionIsAbsentWhileOptedOut(t *testing.T) {
	app := pixelApp(t)
	seedPixelEntries(t, app, "reader", 10)
	app.Login("reader")

	if strings.Contains(app.Get("/statistics").Text(), "Feeds by Open Rate") {
		t.Error("opted out: no open-rate section")
	}
}

// --- The settings toggle ---

func TestThePreferencesFormTogglesTrackingWithoutMovingTheBaseline(t *testing.T) {
	app := pixelApp(t)
	userID := pixelUserID(t, app, "reader")
	app.Login("reader")
	form := func(tracking bool) url.Values {
		fields := url.Values{
			"theme": {"system"}, "entries_per_page": {"30"}, "retention_read_days": {"0"},
			"sidebar_sort": {"name"}, "offline_keep": {"0"},
		}
		if tracking {
			fields.Set("pixel_tracking", "1")
		}
		return fields
	}
	enabledAt := func() *time.Time {
		at, err := models.PixelTrackingEnabledAt(context.Background(), app.DB, userID)
		if err != nil {
			t.Fatal(err)
		}
		return at
	}

	app.PostForm("/user-settings/preferences", form(true))
	first := enabledAt()
	if first == nil {
		t.Fatal("the checkbox turns tracking on")
	}

	// Saving any other preference re-submits this form; the baseline must not
	// move or the denominator silently resets.
	app.PostForm("/user-settings/preferences", form(true))
	if got := enabledAt(); got == nil || !got.Equal(*first) {
		t.Errorf("baseline moved: %v -> %v", first, got)
	}

	// An unchecked checkbox sends no field at all.
	app.PostForm("/user-settings/preferences", form(false))
	if got := enabledAt(); got != nil {
		t.Errorf("tracking should be off, enabled_at = %v", got)
	}
}
