package server

import (
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"net/http"
	"net/netip"
	"net/url"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/config"
	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
)

// Port of tests/pages_test.rs: page handlers and edge cases (template
// rendering, masquerading, flash messages).

// signedInAs is app_signed_in_as: admin and user exist, username is signed in.
func signedInAs(t *testing.T, username string) (app *itestApp, adminID, userID int64) {
	t.Helper()
	app = defaultITestApp(t)
	adminID, userID = app.SetupUsers()
	app.Login(username)
	return app, adminID, userID
}

// usersApp is create_test_app + setup_users, not yet signed in.
func usersApp(t *testing.T, config config.Config) (app *itestApp, adminID, userID int64) {
	t.Helper()
	app = newITestApp(t, config)
	adminID, userID = app.SetupUsers()
	return app, adminID, userID
}

func masquerade(t *testing.T, app *itestApp, userID int64) {
	t.Helper()
	wantStatus(t, app.PostForm(fmt.Sprintf("/admin/users/%d/masquerade", userID), nil), http.StatusSeeOther)
}

// seedEntryWithText seeds one category, feed and entry owned by username and
// returns the entry id.
func seedEntryWithText(t *testing.T, app *itestApp, username, slug, title, body string) int64 {
	t.Helper()
	ctx := context.Background()
	user, err := models.FindUserByUsername(ctx, app.DB, username)
	if err != nil || user == nil {
		t.Fatalf("user %q = %v, err = %v", username, user, err)
	}
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "cat-"+slug)
	if err != nil {
		t.Fatal(err)
	}
	feedTitle := "Feed " + slug
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/" + slug + ".xml", Title: &feedTitle,
	})
	if err != nil {
		t.Fatal(err)
	}
	link := "https://example.com/" + slug
	result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{
		FeedID: feed.ID, GUID: "guid-" + slug, Title: &title, Link: &link, Content: &body,
	})
	if err != nil {
		t.Fatal(err)
	}
	return result.ID
}

func seedOneEntry(t *testing.T, app *itestApp, username, slug string) int64 {
	t.Helper()
	return seedEntryWithText(t, app, username, slug, "Title for "+slug, "<p>Body for "+slug+".</p>")
}

// --- Page rendering ---

func TestUnreadPageRendersSsrLayout(t *testing.T) {
	app, adminID, _ := usersApp(t, itestConfig())
	// A feed without entries makes the empty list the genuine "all caught up"
	// case rather than the no-feeds onboarding case.
	category, err := models.CreateCategory(context.Background(), app.DB, adminID, "Tech")
	if err != nil {
		t.Fatal(err)
	}
	title := "Test Feed"
	if _, err := models.CreateFeed(context.Background(), app.DB, models.CreateFeedParams{
		CategoryID: category.ID, URL: "https://example.com/feed.xml", Title: &title,
	}); err != nil {
		t.Fatal(err)
	}
	app.Login("admin")

	body := app.GetOK("/")

	wantAbsent(t, body, "<rdrs-entries-page>", "/static/js/pages/entries.js")
	wantContains(t, body, "data-entries-list", `id="reading-pane"`, "Select an entry",
		`class="empty-state-quiet"`, `class="empty-state-quiet-title"`, "All caught up")
}

func TestUnreadPageShowsOnboardingWhenNoFeeds(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/")

	wantContains(t, body, `data-testid="onboarding-guide"`, "Add your first feed", "Import OPML")
	wantAbsent(t, body, "All caught up")
}

func TestUnreadPageWhileMasquerading(t *testing.T) {
	app, adminID, userID := signedInAs(t, "admin")
	masquerade(t, app, userID)

	response := app.Get("/")
	wantStatus(t, response, http.StatusOK)
	body := response.Text()
	wantAbsent(t, body, "<rdrs-entries-page>")
	// The bootstrap marks the original admin so the sidebar can still show the
	// Admin link under masquerade.
	wantContains(t, body, `id="rdrs-sidebar-bootstrap"`, `"is_admin":true`, `"is_masquerading":true`)

	api := app.Get("/api/user")
	wantStatus(t, api, http.StatusOK)
	if got := jsonField(t, api, "username"); got != "user" {
		t.Errorf("username = %v, want user", got)
	}
	if got := jsonField(t, api, "id"); got != float64(userID) {
		t.Errorf("id = %v, want %d", got, userID)
	}
	if adminID == userID {
		t.Error("admin and user ids must differ")
	}
}

// Simplified text carries lang="zh-Hans" on the row title and the pane, so a
// Traditional-locale browser resolves the whole string in one font.
func TestSimplifiedEntryIsTaggedZhHans(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	entryID := seedEntryWithText(t, app, "admin", "hans",
		"《控制》新作体验：新怪谈游戏的祖师爷，又杀回来了", "<p>这款游戏的敌人远比想象中稳定。</p>")
	app.Login("admin")

	body := app.GetOK(fmt.Sprintf("/?entry=%d", entryID))

	if !strings.Contains(body, `data-testid="entry-title-link" lang="zh-Hans"`) {
		t.Errorf("simplified row title must be tagged:\n%s", body)
	}
	if !strings.Contains(body, `data-testid="reading-pane-title" lang="zh-Hans"`) {
		t.Errorf("simplified pane headline must be tagged:\n%s", body)
	}
	if !strings.Contains(body, `lang="zh-Hans">&#60;p&#62;这款游戏`) && !strings.Contains(body, `lang="zh-Hans"><p>这款游戏`) {
		t.Errorf("simplified article body must be tagged:\n%s", body)
	}
}

// Traditional text stays untagged: forcing it onto the Simplified cascade
// would restyle every glyph.
func TestTraditionalEntryIsNotTagged(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	entryID := seedEntryWithText(t, app, "admin", "hant",
		"《控制》新作體驗：新怪談遊戲的祖師爺，又殺回來了", "<p>這款遊戲的敵人遠比想像中穩定。</p>")
	app.Login("admin")

	body := app.GetOK(fmt.Sprintf("/?entry=%d", entryID))

	if strings.Contains(body, `lang="zh-Hans"`) {
		t.Errorf("traditional entry must not be tagged:\n%s", body)
	}
}

func TestUnreadPageEntryQueryPopulatesReadingPane(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	entryID := seedOneEntry(t, app, "admin", "deep-link-ok")
	app.Login("admin")

	body := app.GetOK(fmt.Sprintf("/?entry=%d", entryID))

	wantAbsent(t, body, "reading-pane-empty")
	wantContains(t, body, `data-testid="reading-pane-title"`, "Title for deep-link-ok", "Body for deep-link-ok")
	// An unsummarized row must render a truly empty status span, or the
	// .entry-status:empty rule never matches.
	if !strings.Contains(body, `<span class="entry-status"></span>`) {
		t.Errorf("unsummarized entry row must render an empty .entry-status span:\n%s", body)
	}
}

func TestUnreadPageEntryQueryInvalidIdFallsBackToEmptyPane(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/?entry=99999")

	wantContains(t, body, "reading-pane-empty")
	wantAbsent(t, body, `data-testid="reading-pane-title"`)
}

func TestUnreadPageEntryQueryOtherUserFallsBackToEmptyPane(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	entryID := seedOneEntry(t, app, "user", "cross-user")
	app.Login("admin")

	body := app.GetOK(fmt.Sprintf("/?entry=%d", entryID))

	wantContains(t, body, "reading-pane-empty")
	wantAbsent(t, body, "Title for cross-user")
}

func TestStarredEntriesPageEntryQueryPopulatesReadingPane(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	entryID := seedOneEntry(t, app, "admin", "starred-deep-link")
	app.Login("admin")

	body := app.GetOK(fmt.Sprintf("/entries/starred?entry=%d", entryID))

	wantAbsent(t, body, "reading-pane-empty")
	wantContains(t, body, "Title for starred-deep-link")
}

func TestAdminPageWhileMasquerading(t *testing.T) {
	app, _, userID := signedInAs(t, "admin")
	masquerade(t, app, userID)

	wantContains(t, app.GetOK("/admin"), "Admin")
}

func TestUserSettingsPageRendersSsrContent(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/user-settings")

	wantAbsent(t, body, "<rdrs-user-settings-page>", "/static/js/pages/user-settings.js")
	wantContains(t, body, "<h1>Settings</h1>", "Account Information",
		`<form method="post" action="/user-settings/password">`,
		`<form method="post" action="/user-settings/preferences">`,
		`<form method="post" action="/user-settings/linkding">`,
		`<form method="post" action="/user-settings/kagi">`,
		"<rdrs-passkeys>", "/static/js/passkey.js",
		// Sidebar display preferences default to the pre-existing behaviour.
		`name="sidebar_sort"`, `<option value="name" selected>`, `name="sidebar_hide_read"`)
	if strings.Contains(body, `name="sidebar_hide_read" value="1" checked`) {
		t.Error("hide-read must be off by default")
	}
}

// The render half of the <datalist> contract: the page must offer every
// suggestion, and both fields keep min/max.
func TestUserSettingsPageOffersNumberFieldSuggestions(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/user-settings")

	for _, pair := range [][2]string{
		{"entries-per-page", "entries-per-page-list"},
		{"retention-read-days", "retention-read-days-list"},
	} {
		inputID, listID := pair[0], pair[1]
		if !strings.Contains(body, fmt.Sprintf(`id="%s"`, inputID)) {
			t.Errorf("%s input missing", inputID)
		}
		if !strings.Contains(body, fmt.Sprintf(`list="%s"`, listID)) {
			t.Errorf("%s does not reference %s", inputID, listID)
		}
		if !strings.Contains(body, fmt.Sprintf(`<datalist id="%s">`, listID)) {
			t.Errorf("%s not rendered", listID)
		}
	}
	for _, value := range append(append([]int64{}, models.EntriesPerPageSuggestions...), models.RetentionReadDaysSuggestions...) {
		if !strings.Contains(body, fmt.Sprintf(`<option value="%d">`, value)) {
			t.Errorf("suggestion %d not offered", value)
		}
	}
	wantContains(t, body, `min="10" max="100"`, `min="0" max="3650"`)
}

func TestUserSettingsListsApiTokens(t *testing.T) {
	app, adminID, _ := signedInAs(t, "admin")
	if _, err := models.CreateAPIToken(context.Background(), app.DB, models.NewAPIToken{
		UserID: adminID, Kind: "greader", Label: "FeedMe/1.0", UserAgent: "FeedMe/1.0", IPAddress: "203.0.113.5",
	}); err != nil {
		t.Fatal(err)
	}

	wantContains(t, app.GetOK("/user-settings"),
		"GReader API Tokens", "FeedMe/1.0", "203.0.113.5", "/user-settings/api-tokens/revoke-all")
}

func TestApiTokenRevokeIsUserScoped(t *testing.T) {
	app, adminID, _ := usersApp(t, itestConfig())
	token, err := models.CreateAPIToken(context.Background(), app.DB, models.NewAPIToken{
		UserID: adminID, Kind: "greader", Label: "admin-client", UserAgent: "test-agent", IPAddress: "127.0.0.1",
	})
	if err != nil {
		t.Fatal(err)
	}
	app.Login("user")

	wantStatus(t, app.PostForm(fmt.Sprintf("/user-settings/api-tokens/%d/revoke", token.ID), nil), http.StatusSeeOther)

	found, err := models.FindAPIToken(context.Background(), app.DB, token.Token)
	if err != nil || found == nil {
		t.Errorf("user B must not be able to revoke user A's API token (found %v, err %v)", found, err)
	}
}

// Without ?v= the long-lived header on the icons would have no URL left to
// change across an upgrade.
func TestFaviconLinksCarryTheBuildStamp(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.Get("/user-settings").Text()

	for _, icon := range []string{"/favicon.ico", "/favicon.svg", "/apple-touch-icon.png"} {
		if !strings.Contains(body, icon+"?v="+gitVersion()) {
			t.Errorf("%s must be version-stamped", icon)
		}
	}
}

func TestPagesLinkTheManifestAndDeclareAThemeColor(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.Get("/user-settings").Text()

	if !strings.Contains(body, "/static/manifest.webmanifest?v="+gitVersion()) {
		t.Error("the manifest link must be version-stamped")
	}
	if !strings.Contains(body, "/static/js/pwa.js?v="+gitVersion()) {
		t.Error("signed-in pages must load the service-worker registration")
	}
	// One theme-color per scheme: the manifest's single value cannot follow it.
	for _, scheme := range []string{"light", "dark"} {
		if !strings.Contains(body, "(prefers-color-scheme: "+scheme+")") {
			t.Errorf("a theme-color is missing for the %s scheme", scheme)
		}
	}
}

// Without crossorigin a manifest is fetched with credentials omitted, so a
// cookie-authenticating proxy would answer it with a login redirect.
func TestManifestLinkIsFetchedWithCredentials(t *testing.T) {
	body := defaultITestApp(t).Get("/login").Text()

	var link string
	for _, tag := range strings.Split(body, "<") {
		if strings.HasPrefix(tag, "link") && strings.Contains(tag, `rel="manifest"`) {
			link = tag
			break
		}
	}
	if link == "" {
		t.Fatal("every page links the manifest")
	}
	if !strings.Contains(link, `crossorigin="use-credentials"`) {
		t.Errorf("the manifest link must request credentials, got: <%s", link)
	}
}

func TestLoginPageRegistersNoServiceWorker(t *testing.T) {
	body := defaultITestApp(t).Get("/login").Text()

	if strings.Contains(body, "pwa.js") {
		t.Error("the sign-in page must not register a service worker")
	}
	if !strings.Contains(body, "/static/manifest.webmanifest") {
		t.Error("the manifest belongs on every page, signed in or not")
	}
}

func TestSessionRevokeIsUserScoped(t *testing.T) {
	app, adminID, _ := usersApp(t, itestConfig())
	victim, err := models.CreateSession(context.Background(), app.DB, models.NewSession{
		UserID: adminID, UserAgent: "test-agent", IPAddress: "10.0.0.1",
	})
	if err != nil {
		t.Fatal(err)
	}
	app.Login("user")

	wantStatus(t, app.PostForm(fmt.Sprintf("/user-settings/sessions/%d/revoke", victim.ID), nil), http.StatusSeeOther)

	found, err := models.FindSessionByToken(context.Background(), app.DB, victim.SessionToken)
	if err != nil || found == nil {
		t.Errorf("user B must not be able to revoke user A's session (found %v, err %v)", found, err)
	}
}

func TestUserSettingsRendersSessionCards(t *testing.T) {
	app, adminID, _ := signedInAs(t, "admin")
	other, err := models.CreateSession(context.Background(), app.DB, models.NewSession{
		UserID: adminID, UserAgent: "Mozilla/5.0 (X11; Linux x86_64) OtherDevice/1.0", IPAddress: "198.51.100.7",
	})
	if err != nil {
		t.Fatal(err)
	}

	wantContains(t, app.GetOK("/user-settings"),
		// The full User-Agent, not a truncated or tooltip-only copy.
		"Mozilla/5.0 (X11; Linux x86_64) OtherDevice/1.0", "198.51.100.7",
		fmt.Sprintf(`action="/user-settings/sessions/%d/revoke"`, other.ID),
		// The asking session gets the note instead of a button.
		"This device")
}

func TestSettingsPageRendersSsrContent(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/settings")

	wantAbsent(t, body, "<rdrs-settings-page>", "/static/js/pages/settings.js",
		// The standalone sub-heading is gone; one Configuration table remains.
		"Environment Variables")
	wantContains(t, body, "<h1>App</h1>", "Configuration", "DATABASE_URL", "RDRS_USER_AGENT",
		"RDRS_MULTI_USER_ENABLED", "RDRS_SECRET", "Current")
}

func TestSettingsPageReflectsCustomConfig(t *testing.T) {
	cfg := itestConfig()
	cfg.DatabaseURL = "/data/custom.sqlite3"
	cfg.ServerBind = netip.MustParseAddrPort("0.0.0.0:9090")
	cfg.UserAgent = "Custom-Agent/2.0"
	app, _, _ := usersApp(t, cfg)
	app.Login("admin")

	body := app.GetOK("/settings")

	wantContains(t, body, "Custom-Agent/2.0", "(custom)", "/data/custom.sqlite3", "9090", "Configured")
	wantAbsent(t, body, ">Auto-generated<")
	if strings.Count(body, `<span class="success-text">Yes</span>`) < 1 {
		t.Error("expected a Yes badge")
	}
}

func TestSettingsPageReflectsAutoGeneratedImageProxySecret(t *testing.T) {
	cfg := itestConfig()
	cfg.SecretGenerated = true
	app, _, _ := usersApp(t, cfg)
	app.Login("admin")

	body := app.GetOK("/settings")

	wantContains(t, body, "Auto-generated")
	wantAbsent(t, body, ">Configured<")
}

func TestSettingsPageRedactsDatabasePassword(t *testing.T) {
	cfg := itestConfig()
	cfg.DatabaseURL = "postgres://rdrs:sup3rs3cret@db.internal:5432/rdrs"
	app, _, _ := usersApp(t, cfg)
	app.Login("admin")

	body := app.GetOK("/settings")

	if strings.Contains(body, "sup3rs3cret") {
		t.Error("database password leaked into /settings")
	}
	wantContains(t, body, "postgres://rdrs:***@db.internal:5432/rdrs")
}

// Non-admins are bounced rather than shown deployment internals.
func TestAdminOnlyPagesRedirectARegularUser(t *testing.T) {
	app, _, _ := signedInAs(t, "user")

	for _, path := range []string{"/settings", "/admin"} {
		if got := app.Get(path).Code; got != http.StatusSeeOther {
			t.Errorf("%s = %d, want 303", path, got)
		}
	}
}

func TestLoginPageHidesSignupWhenDisabled(t *testing.T) {
	wantContains(t, defaultITestApp(t).GetOK("/login"), "Login")
}

// The page has exactly one purpose and it is spent: a redirect, not a
// disabled form, and nothing about the existing accounts is disclosed.
func TestSetupPageRedirectsOnceTheInstanceHasAnAccount(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())

	response := app.Get("/setup")

	wantStatus(t, response, http.StatusSeeOther)
	if got := response.Header().Get("Location"); got != "/login" {
		t.Errorf("Location = %q, want /login", got)
	}
}

func TestPagesRenderAPendingFlash(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	for _, c := range []struct{ path, level, message string }{
		{"/categories", "success", "Category created successfully"},
		{"/feeds", "error", "Failed to add feed"},
		{"/entries", "info", "Entries refreshed"},
		{"/user-settings", "success", "Settings saved"},
	} {
		payload := fmt.Sprintf(`[{"level":"%s","message":"%s"}]`, c.level, c.message)
		app.cookies["flash"] = app.signedFlash(payload)
		response := app.Get(c.path)
		delete(app.cookies, "flash")
		wantStatus(t, response, http.StatusOK)
		// Banners inside <rdrs-flash> are visible on first paint without JavaScript.
		if !strings.Contains(response.Text(), `data-testid="flash-message"`) || !strings.Contains(response.Text(), c.message) {
			t.Errorf("%s: the pending flash is not rendered", c.path)
		}
	}
}

func TestEntryListPagesRenderSsrLayout(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	for _, path := range []string{"/entries", "/entries/summarized"} {
		body := app.GetOK(path)
		if strings.Contains(body, "<rdrs-entries-page>") || strings.Contains(body, "/static/js/pages/entries.js") {
			t.Errorf("%s: no CSR shell expected", path)
		}
		if !strings.Contains(body, `id="reading-pane"`) {
			t.Errorf("%s: reading pane missing", path)
		}
	}
}

// --- Regular user permissions ---

func TestRegularUserUnreadPageNoAdminLink(t *testing.T) {
	app, _, _ := signedInAs(t, "user")

	body := app.GetOK("/")

	wantContains(t, body, "user")
	wantAbsent(t, body, `data-testid="nav-admin"`)
}

// --- Archive entry pages ---

func TestArchiveEntryPages(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	for _, c := range []struct{ path, heading, word string }{
		{"/entries/read", "Read Entries", "read"},
		{"/entries/starred", "Starred Entries", "starred"},
		{"/entries/summarized", "Summarized Entries", "summarized"},
	} {
		body := app.GetOK(c.path)
		if strings.Contains(body, "<rdrs-entries-page>") {
			t.Errorf("%s: no CSR shell expected", c.path)
		}
		if !strings.Contains(body, c.heading) && !strings.Contains(body, c.word) {
			t.Errorf("%s: neither %q nor %q", c.path, c.heading, c.word)
		}
	}
}

func TestSearchPage(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/search")

	wantContains(t, body, "<h1>Search</h1>", `<form method="get" action="/search"`,
		`data-testid="search-input"`, "Search your library", `class="empty-state"`)
	// The old editorial empty-state class is retired.
	wantAbsent(t, body, "search-status", "<rdrs-entries-page>", "/static/js/pages/entries.js")
}

func TestSearchPageWithResults(t *testing.T) {
	app, _, _ := usersApp(t, itestConfig())
	execStatistics(t, app, "INSERT INTO category (user_id, name) VALUES ($1, $2)", 1, "Search Cat")
	execStatistics(t, app, "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)", 1, "https://example.com/search-feed.xml", "Search Feed")
	execStatistics(t, app, "INSERT INTO entry (feed_id, guid, title, link, content) VALUES ($1, $2, $3, $4, $5)",
		1, "search-guid-1", "Quokka Discovery in Western Australia", "https://example.com/quokka", "<p>The quokka is a small marsupial.</p>")
	execStatistics(t, app, "INSERT INTO entry (feed_id, guid, title, link, content) VALUES ($1, $2, $3, $4, $5)",
		1, "search-guid-2", "Other Topic", "https://example.com/other", "<p>Unrelated article.</p>")
	app.Login("admin")

	body := app.GetOK("/search?q=Quokka")

	// Title and snippet wrap matches in <mark>; the unmatched text stays verbatim.
	wantContains(t, body, `data-testid="search-results"`, "<mark>Quokka</mark>", "Discovery in Western Australia",
		// Results link to the in-app entry route, not the article URL.
		`href="/entries/1"`, "Search Feed",
		// The match is case-insensitive and keeps the snippet's lowercase.
		"<mark>quokka</mark>", "is a small marsupial.")
	wantAbsent(t, body, "https://example.com/quokka", "Other Topic")
}

// appWithWombats is admin with n entries titled "Wombat {i}"; search pages
// hold 50.
func appWithWombats(t *testing.T, n int) *itestApp {
	t.Helper()
	app, _, _ := usersApp(t, itestConfig())
	execStatistics(t, app, "INSERT INTO category (user_id, name) VALUES ($1, $2)", 1, "Cat")
	execStatistics(t, app, "INSERT INTO feed (category_id, url, title) VALUES ($1, $2, $3)", 1, "https://example.com/wombats.xml", "Wombat Feed")
	for i := 0; i < n; i++ {
		execStatistics(t, app, "INSERT INTO entry (feed_id, guid, title, content) VALUES ($1, $2, $3, $4)",
			1, fmt.Sprintf("wombat-%d", i), fmt.Sprintf("Wombat %d", i), "<p>Burrowing.</p>")
	}
	app.Login("admin")
	return app
}

// resultIDs are the entry ids linked from a search page or fragment, in order;
// nav links such as /entries/read are skipped.
func resultIDs(html string) []int64 {
	var ids []int64
	parts := strings.Split(html, `href="/entries/`)
	for _, rest := range parts[1:] {
		end := strings.IndexByte(rest, '"')
		if end < 0 {
			continue
		}
		if id, err := strconv.ParseInt(rest[:end], 10, 64); err == nil {
			ids = append(ids, id)
		}
	}
	return ids
}

// loadMoreAfter is the Load More cursor, URL-encoded for the next request.
func loadMoreAfter(html string) (string, bool) {
	const marker = `name="after" value="`
	start := strings.Index(html, marker)
	if start < 0 {
		return "", false
	}
	start += len(marker)
	end := strings.IndexByte(html[start:], '"')
	if end < 0 {
		return "", false
	}
	return url.QueryEscape(html[start : start+end]), true
}

func TestSearchPagesThroughEveryMatchOnce(t *testing.T) {
	app := appWithWombats(t, 125)

	page := app.GetOK("/search?q=Wombat")
	ids := resultIDs(page)
	if len(ids) != 50 {
		t.Fatalf("first page has %d results, want 50", len(ids))
	}
	wantContains(t, page, `<li id="search-load-more"`, "data-swap-fragment")
	// Scriptless submits must reach the page, so the markup leaves fragment to JS.
	wantAbsent(t, page, `name="fragment"`)

	cursor, more := loadMoreAfter(page)
	for more {
		fragment := app.GetOK("/search?q=Wombat&fragment=1&after=" + cursor)
		wantContains(t, fragment, `<template data-swap-target="#search-load-more">`)
		ids = append(ids, resultIDs(fragment)...)
		cursor, more = loadMoreAfter(fragment)
	}
	unique := slices.Clone(ids)
	slices.Sort(unique)
	unique = slices.Compact(unique)
	if len(ids) != 125 {
		t.Errorf("every match once: got %d ids: %v", len(ids), ids)
	}
	if len(unique) != 125 {
		t.Errorf("no duplicates: got %d unique of %v", len(unique), ids)
	}
}

func TestSearchFragmentModesAndDocumentNavigation(t *testing.T) {
	app := appWithWombats(t, 3)

	// Live search: the whole results region, no layout.
	live := app.GetOK("/search?q=Wombat&fragment=1")
	wantContains(t, live, `<template data-swap-target="[data-search-results]">`)
	wantAbsent(t, live, "<h1>Search</h1>")
	if got := len(resultIDs(live)); got != 3 {
		t.Errorf("live results = %d, want 3", got)
	}

	// A cleared field goes back to the empty state.
	wantContains(t, app.GetOK("/search?q=&fragment=1"), "Search your library")

	// A bookmarked or scriptless fragment URL still gets the page.
	document := app.Do(documentRequest(http.MethodGet, "/search?q=Wombat&fragment=1", "", nil))
	wantStatus(t, document, http.StatusOK)
	wantContains(t, document.Text(), "<h1>Search</h1>")
	wantAbsent(t, document.Text(), "<template")
}

func TestSearchLoadMoreWithoutScriptRendersTheNextPage(t *testing.T) {
	app := appWithWombats(t, 55)

	page := app.GetOK("/search?q=Wombat")
	cursor, ok := loadMoreAfter(page)
	if !ok {
		t.Fatal("a second page is expected")
	}
	next := app.GetOK("/search?q=Wombat&after=" + cursor)

	wantContains(t, next, "<h1>Search</h1>")
	first, second := resultIDs(page), resultIDs(next)
	if len(second) != 5 {
		t.Errorf("second page = %d results, want 5", len(second))
	}
	for _, id := range second {
		if slices.Contains(first, id) {
			t.Errorf("entry %d appears on both pages", id)
		}
	}
	wantAbsent(t, next, `id="search-load-more"`)
}

func TestSearchPageNoResults(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/search?q=zzznotfoundzzz")

	wantContains(t, body, "Nothing matched", "zzznotfoundzzz", "No matches", `data-testid="search-empty"`)
}

func TestSearchPageInvalidQueryShowsErrorNoResults(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	// "(rust OR": unbalanced parenthesis.
	body := app.GetOK("/search?q=%28rust%20OR")

	wantContains(t, body, "Search syntax error", `data-testid="search-error"`)
	wantAbsent(t, body, `data-testid="search-results"`)
}

func TestSearchPageValidStructuredQueryRendersWithoutError(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	wantAbsent(t, app.GetOK("/search?q=is%3Aunread"), "Search syntax error", `data-testid="search-error"`)
}

// The syntax reference is the shared help dialog, opened from a button; the
// old <details> panel is gone.
func TestSearchPageOpensSyntaxHelpFromAButton(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	body := app.GetOK("/search")

	wantContains(t, body, "data-search-help",
		// Without scripts the same button opens a server-rendered popover.
		`popovertarget="search-syntax-popover"`,
		`id="search-syntax-popover" class="search-syntax-popover" popover`,
		"<code>is:starred</code>")
	wantAbsent(t, body, "search-syntax-help")
}

// Every entries list takes the inline search drawer, with its help button.
func TestListPagesRenderTheInlineSearchDrawer(t *testing.T) {
	app := appWithWombats(t, 3)

	for _, path := range []string{
		"/", "/entries", "/entries/read", "/entries/starred", "/entries/summarized",
		"/categories/1/entries", "/feeds/1/entries",
	} {
		body := app.GetOK(path)
		for _, marker := range []string{"data-search-drawer", "data-search-toggle", "data-search-help", "data-search-error"} {
			if !strings.Contains(body, marker) {
				t.Errorf("%s: missing %s", path, marker)
			}
		}
	}
}

// The inline search speaks the /search syntax on every list, not a substring.
func TestListSearchUsesSearchSyntax(t *testing.T) {
	app := appWithWombats(t, 3)

	for _, base := range []string{"/", "/entries", "/categories/1/entries", "/feeds/1/entries"} {
		get := func(q string) string {
			return app.GetOK(base + "?fragment=1&q=" + url.QueryEscape(q))
		}
		exact := get(`title:"Wombat 1"`)
		expect(t, base+": title: term", strings.Contains(exact, "Wombat 1"))
		expect(t, base+": title: term narrows", !strings.Contains(exact, "Wombat 2"))

		excluded := get("Wombat -2")
		expect(t, base+": -term keeps others", strings.Contains(excluded, "Wombat 1"))
		expect(t, base+": -term excludes", !strings.Contains(excluded, "Wombat 2"))

		expect(t, base+": feed: + is:", strings.Contains(get("feed:wombat is:unread"), "Wombat 0"))

		none := get("is:starred")
		expect(t, base+": is:starred", !strings.Contains(none, "Wombat 0"))
		expect(t, base+": no matches", strings.Contains(none, "scoped-search-empty"))
	}
}

func expect(t *testing.T, what string, ok bool) {
	t.Helper()
	if !ok {
		t.Error(what)
	}
}

// An unparseable query reports why and lists nothing, instead of falling back
// to the unfiltered list.
func TestListSearchSyntaxErrorListsNothing(t *testing.T) {
	app := appWithWombats(t, 3)

	for _, base := range []string{"/", "/entries/starred", "/categories/1/entries"} {
		body := app.GetOK(base + "?fragment=1&q=%28Wombat")
		expect(t, base+": "+body, strings.Contains(body, "Search syntax error (near character 1)"))
		expect(t, base+": must list nothing", !strings.Contains(body, "Wombat 0"))
		expect(t, base+": no empty-state", !strings.Contains(body, "scoped-search-empty"))
	}
}

// --- Category and feed entries pages ---

// libraryOwner is setupAndSignIn: the first account made through /api/setup,
// signed in.
type libraryOwner struct {
	app    *itestApp
	userID int64
}

func newLibraryOwner(t *testing.T, username string) libraryOwner {
	t.Helper()
	app := defaultITestApp(t)
	wantStatus(t, app.PostJSON("/api/setup", map[string]string{"username": username, "password": fixturePassword}), http.StatusCreated)
	app.Login(username)
	user, err := models.FindUserByUsername(context.Background(), app.DB, username)
	if err != nil || user == nil {
		t.Fatalf("user = %v, err = %v", user, err)
	}
	return libraryOwner{app: app, userID: user.ID}
}

func (o libraryOwner) category(t *testing.T, name string) int64 {
	t.Helper()
	created, err := models.CreateCategory(context.Background(), o.app.DB, o.userID, name)
	if err != nil {
		t.Fatal(err)
	}
	return created.ID
}

func (o libraryOwner) feed(t *testing.T, categoryID int64, feedURL, title string) int64 {
	t.Helper()
	created, err := models.CreateFeed(context.Background(), o.app.DB, models.CreateFeedParams{
		CategoryID: categoryID, URL: feedURL, Title: &title,
	})
	if err != nil {
		t.Fatal(err)
	}
	return created.ID
}

func (o libraryOwner) entry(t *testing.T, feedID int64, guid, title, link string) int64 {
	t.Helper()
	result, err := entry.UpsertEntry(context.Background(), o.app.DB, &entry.Upsert{
		FeedID: feedID, GUID: guid, Title: &title, Link: &link,
	})
	if err != nil {
		t.Fatal(err)
	}
	return result.ID
}

func TestCategoryEntriesPage(t *testing.T) {
	owner := newLibraryOwner(t, "alice_ce")
	catID := owner.category(t, "Engineering")
	feed1 := owner.feed(t, catID, "https://x/ce-feed-1", "Feed 1")
	feed2 := owner.feed(t, catID, "https://x/ce-feed-2", "Feed 2")
	entryA := owner.entry(t, feed1, "guid-ce-a", "Entry A", "https://x/ce/a")
	entryB := owner.entry(t, feed2, "guid-ce-b", "Entry B", "https://x/ce/b")

	response := owner.app.Get(fmt.Sprintf("/categories/%d/entries", catID))
	wantStatus(t, response, http.StatusOK)
	html := response.Text()

	wantContains(t, html, "Engineering",
		fmt.Sprintf(`id="entry-row-%d"`, entryA), fmt.Sprintf(`id="entry-row-%d"`, entryB),
		"Select an entry from the list to start reading.",
		`data-testid="breadcrumb"`, `<a href="/categories">Categories</a>`,
		// The sidebar highlights the current category.
		fmt.Sprintf(`active-category-id="%d"`, catID),
		// The scoped Mark-as-Read dropdown carries the category's GReader stream.
		`id="mark-read-age"`, `data-mark-read-scope="user/-/label/Engineering"`,
		`id="mark-above-read"`)
	wantAbsent(t, html, "rdrs-entries-page", "/static/js/pages/entries.js")
	if strings.Contains(html, `id="load-more"`) {
		wantContains(t, html, fmt.Sprintf(`action="/categories/%d/entries"`, catID))
	}
}

func TestNotFoundPagesRenderInsideTheAppChrome(t *testing.T) {
	app, _, _ := signedInAs(t, "admin")

	for _, c := range []struct{ path, heading string }{
		{"/categories/999999/entries", "Category not found"},
		{"/feeds/999999/entries", "Feed not found"},
		{"/feeds/999999/edit", "Feed not found"},
		{"/this-page-does-not-exist", "Page not found"},
	} {
		response := app.Get(c.path)
		if response.Code != http.StatusNotFound {
			t.Errorf("%s = %d, want 404", c.path, response.Code)
		}
		if !strings.Contains(response.Text(), c.heading) {
			t.Errorf("%s: heading %q missing", c.path, c.heading)
		}
		if !strings.Contains(response.Text(), "rdrs-sidebar") {
			t.Errorf("%s should render inside the app chrome (sidebar present)", c.path)
		}
	}
}

func TestCategoryEntriesPageOtherUser(t *testing.T) {
	app := defaultITestApp(t)
	wantStatus(t, app.PostJSON("/api/setup", map[string]string{"username": "alice_cou", "password": fixturePassword}), http.StatusCreated)
	app.SeedAccount("bob_cou", fixturePassword, models.RoleUser)
	alice, err := models.FindUserByUsername(context.Background(), app.DB, "alice_cou")
	if err != nil || alice == nil {
		t.Fatalf("alice = %v, err = %v", alice, err)
	}
	category, err := models.CreateCategory(context.Background(), app.DB, alice.ID, "Alice Cat")
	if err != nil {
		t.Fatal(err)
	}
	app.Login("bob_cou")

	if got := app.Get(fmt.Sprintf("/categories/%d/entries", category.ID)).Code; got != http.StatusNotFound {
		t.Errorf("cross-user category entries = %d, want 404", got)
	}
}

func TestCategoryEntriesPageLoadMoreFragment(t *testing.T) {
	owner := newLibraryOwner(t, "alice_cl")
	catID := owner.category(t, "LMCat")
	feedID := owner.feed(t, catID, "https://x/clm-feed", "CLM Feed")
	for i := 0; i < 3; i++ {
		owner.entry(t, feedID, fmt.Sprintf("guid-clm-%d", i), fmt.Sprintf("Entry %d", i), fmt.Sprintf("https://x/clm/%d", i))
	}

	response := owner.app.Get(fmt.Sprintf("/categories/%d/entries?fragment=1", catID))
	wantStatus(t, response, http.StatusOK)

	wantContains(t, response.Text(), "data-entry-row")
	wantAbsent(t, response.Text(), "<rdrs-sidebar", "<h1>LMCat</h1>")
}

// The category-switch fragment app.js swaps in place of a document reload: the
// whole left column, header included, plus an emptied reading pane, which stays
// empty even with ?entry= because switching closes the previous entry.
func TestCategoryEntriesPagePaneFragment(t *testing.T) {
	owner := newLibraryOwner(t, "alice_cp")
	catID := owner.category(t, "PaneCat")
	feedID := owner.feed(t, catID, "https://x/cpane-feed", "CPane Feed")
	entryID := owner.entry(t, feedID, "guid-cpane-0", "Pane Entry", "https://x/cpane/0")

	response := owner.app.Get(fmt.Sprintf("/categories/%d/entries?pane=1&entry=%d", catID, entryID))
	wantStatus(t, response, http.StatusOK)
	html := response.Text()

	wantContains(t, html, `data-swap-target="[data-list-pane]"`, `data-swap-target="#reading-pane"`,
		"<h1>PaneCat</h1>", "data-entry-row", "reading-pane-empty")
	wantAbsent(t, html, "<rdrs-sidebar", "reading-pane-article")
}

// The same in-place navigation contract for feeds, since the sidebar lists
// feeds and clicking one must not reload the document either.
func TestFeedEntriesPagePaneFragment(t *testing.T) {
	owner := newLibraryOwner(t, "alice_fp")
	catID := owner.category(t, "FPCat")
	feedID := owner.feed(t, catID, "https://x/fp-feed", "FP Feed")
	entryID := owner.entry(t, feedID, "guid-fp-0", "FP Entry", "https://x/fp/0")

	response := owner.app.Get(fmt.Sprintf("/feeds/%d/entries?pane=1&entry=%d", feedID, entryID))
	wantStatus(t, response, http.StatusOK)
	html := response.Text()

	wantContains(t, html, `data-swap-target="[data-list-pane]"`, `data-swap-target="#reading-pane"`,
		"<h1>", "FP Feed", "reading-pane-empty")
	wantAbsent(t, html, "<rdrs-sidebar", "reading-pane-article")
}

// A feed page names its feed to the sidebar, and its parent category, which is
// what makes that category's list expand at all.
func TestFeedEntriesPageMarksActiveFeed(t *testing.T) {
	owner := newLibraryOwner(t, "alice_af")
	catID := owner.category(t, "AFCat")
	feedID := owner.feed(t, catID, "https://x/af-feed", "AF Feed")

	html := owner.app.Get(fmt.Sprintf("/feeds/%d/entries", feedID)).Text()
	wantContains(t, html, fmt.Sprintf(`active-feed-id="%d"`, feedID), fmt.Sprintf(`active-category-id="%d"`, catID))

	wantAbsent(t, owner.app.Get(fmt.Sprintf("/categories/%d/entries", catID)).Text(), "active-feed-id")
}

// afterTheSeed is a render time after anything the test seeded, as a list
// page would post it.
func afterTheSeed() string {
	return time.Now().UTC().Add(30 * time.Second).Format("2006-01-02 15:04:05")
}

// backdateStoredEntries moves every entry out of the render's own second,
// which a snapshot would leave out.
func backdateStoredEntries(t *testing.T, app *itestApp) {
	t.Helper()
	execStatistics(t, app, "UPDATE entry SET created_at = '2000-01-01 00:00:00'")
}

func readAt(t *testing.T, app *itestApp, entryID int64) sql.NullString {
	t.Helper()
	var value sql.NullString
	if err := app.DB.QueryRowContext(context.Background(), "SELECT read_at FROM entry WHERE id = $1", entryID).Scan(&value); err != nil {
		t.Fatal(err)
	}
	return value
}

func postMarkRead(t *testing.T, app *itestApp, path string, fields map[string]string) itestResponse {
	t.Helper()
	form := url.Values{"snapshot": {afterTheSeed()}}
	for name, value := range fields {
		form.Set(name, value)
	}
	response := app.PostForm(path, form)
	wantStatus(t, response, http.StatusSeeOther)
	return response
}

// POST /categories/{id}/entries/mark-read marks only the entries matching the
// scoped search, and redirects back preserving ?q=.
func TestCategoryMarkReadScopedSearch(t *testing.T) {
	owner := newLibraryOwner(t, "alice_mr")
	catID := owner.category(t, "MarkReadCat")
	feedID := owner.feed(t, catID, "https://x/mr-feed", "MR Feed")
	matching := owner.entry(t, feedID, "guid-mr-match", "Widget Roundup", "https://x/mr/match")
	other := owner.entry(t, feedID, "guid-mr-other", "Something Else", "https://x/mr/other")

	response := postMarkRead(t, owner.app, fmt.Sprintf("/categories/%d/entries/mark-read", catID), map[string]string{"q": "Widget"})

	if got, want := response.Header().Get("Location"), fmt.Sprintf("/categories/%d/entries?q=Widget", catID); got != want {
		t.Errorf("redirect = %q, want %q (it must preserve the scoped-search keyword)", got, want)
	}
	if !readAt(t, owner.app, matching).Valid {
		t.Error("entry matching the scoped search must be marked read")
	}
	if readAt(t, owner.app, other).Valid {
		t.Error("entry not matching the scoped search must remain unread")
	}
}

// "Mark matching as read" takes the search syntax, and a query that does not
// parse marks nothing rather than the whole category.
func TestCategoryMarkReadScopedSearchSyntax(t *testing.T) {
	owner := newLibraryOwner(t, "alice_ms")
	catID := owner.category(t, "MarkSyntaxCat")
	feedID := owner.feed(t, catID, "https://x/ms-feed", "MS Feed")
	widget := owner.entry(t, feedID, "guid-ms-a", "Widget Roundup", "https://x/ms/guid-ms-a")
	other := owner.entry(t, feedID, "guid-ms-b", "Something Else", "https://x/ms/guid-ms-b")
	path := fmt.Sprintf("/categories/%d/entries/mark-read", catID)

	// Unparseable: nothing is marked.
	postMarkRead(t, owner.app, path, map[string]string{"q": "(Widget"})
	expect(t, "unparseable query marked the widget entry", !readAt(t, owner.app, widget).Valid)
	expect(t, "unparseable query marked the other entry", !readAt(t, owner.app, other).Valid)

	// An empty quoted phrase is a syntax error too, not "match everything".
	postMarkRead(t, owner.app, path, map[string]string{"q": `""`})
	expect(t, "empty phrase marked the widget entry", !readAt(t, owner.app, widget).Valid)
	expect(t, "empty phrase marked the other entry", !readAt(t, owner.app, other).Valid)

	// The syntax is honoured: only the entry that is not "Widget" is marked.
	postMarkRead(t, owner.app, path, map[string]string{"q": "-Widget"})
	expect(t, "-Widget marked the widget entry", !readAt(t, owner.app, widget).Valid)
	expect(t, "-Widget left the other entry unread", readAt(t, owner.app, other).Valid)
}

// The lazily-loaded feed list under the open category, with per-feed unread
// counts. Another account's category must 404 rather than leak its feeds.
func TestSidebarCategoryFeedsEndpoint(t *testing.T) {
	owner := newLibraryOwner(t, "alice_sf")
	catID := owner.category(t, "SFCat")
	otherCatID := owner.category(t, "SFOther")
	feedID := owner.feed(t, catID, "https://x/sf-feed", "SF Feed")
	otherFeedID := owner.feed(t, otherCatID, "https://x/sf-other-feed", "SF Other Feed")
	for i := 0; i < 2; i++ {
		owner.entry(t, feedID, fmt.Sprintf("guid-sf-%d", i), fmt.Sprintf("SF Entry %d", i), fmt.Sprintf("https://x/sf/%d", i))
	}
	owner.entry(t, otherFeedID, "guid-sf-other", "Other Entry", "https://x/sf/other")
	path := fmt.Sprintf("/api/sidebar/categories/%d/feeds", catID)

	response := owner.app.Get(path)
	wantStatus(t, response, http.StatusOK)
	var body struct {
		CategoryID int64 `json:"category_id"`
		Feeds      []struct {
			ID          int64  `json:"id"`
			Title       string `json:"title"`
			UnreadCount int64  `json:"unread_count"`
			HasIcon     bool   `json:"has_icon"`
		} `json:"feeds"`
	}
	if err := json.Unmarshal(response.Body.Bytes(), &body); err != nil {
		t.Fatal(err)
	}
	if body.CategoryID != catID {
		t.Errorf("category_id = %d, want %d", body.CategoryID, catID)
	}
	if len(body.Feeds) != 1 {
		t.Fatalf("only the requested category's feeds may be listed, got %d", len(body.Feeds))
	}
	feed := body.Feeds[0]
	if feed.ID != feedID || feed.Title != "SF Feed" {
		t.Errorf("feed = %+v", feed)
	}
	if feed.UnreadCount != 2 {
		t.Errorf("unread_count = %d, want 2 (scoped to the feed)", feed.UnreadCount)
	}
	if feed.HasIcon {
		t.Error("a feed with no stored icon must say so, or the sidebar renders a broken image")
	}

	sourceURL := "https://x/icon.png"
	if err := models.UpsertImage(context.Background(), owner.app.DB, models.ImageUpsert{
		EntityType: models.EntityFeed, EntityID: feedID, Data: []byte{1, 2, 3}, ContentType: "image/png", SourceURL: &sourceURL,
	}); err != nil {
		t.Fatal(err)
	}
	withIcon := owner.app.Get(path)
	if !strings.Contains(strings.ReplaceAll(withIcon.Text(), " ", ""), `"has_icon":true`) {
		t.Errorf("a stored icon must be reported so the sidebar renders it: %s", withIcon.Text())
	}

	wantStatus(t, owner.app.Get("/api/sidebar/categories/999999/feeds"), http.StatusNotFound)
}

// "Mark Above as Read" is dropped from a scoped-search render: under a search
// it would mean the matches, too close to "Mark N matching as Read".
func TestCategoryEntriesHidesMarkAboveWhileSearching(t *testing.T) {
	owner := newLibraryOwner(t, "alice_ma")
	catID := owner.category(t, "MACat")
	feedID := owner.feed(t, catID, "https://x/ma-feed", "MA Feed")
	owner.entry(t, feedID, "guid-ma-0", "Widget News", "https://x/ma/0")

	plain := owner.app.Get(fmt.Sprintf("/categories/%d/entries", catID))
	wantContains(t, plain.Text(), "mark-above-read")

	searched := owner.app.Get(fmt.Sprintf("/categories/%d/entries?q=Widget", catID)).Text()
	wantContains(t, searched, "data-entry-row")
	wantAbsent(t, searched, "mark-above-read")
}

// The feed-scoped twin of the category test, guarding against an argument-order
// bug between the two form handlers and the shared scoped helper.
func TestFeedMarkReadScopedSearch(t *testing.T) {
	owner := newLibraryOwner(t, "alice_fmr")
	catID := owner.category(t, "FeedMarkReadCat")
	feedID := owner.feed(t, catID, "https://x/fmr-feed", "FMR Feed")
	matching := owner.entry(t, feedID, "guid-fmr-match", "Widget Roundup", "https://x/fmr/match")
	other := owner.entry(t, feedID, "guid-fmr-other", "Something Else", "https://x/fmr/other")

	response := postMarkRead(t, owner.app, fmt.Sprintf("/feeds/%d/entries/mark-read", feedID), map[string]string{"q": "Widget"})

	if got, want := response.Header().Get("Location"), fmt.Sprintf("/feeds/%d/entries?q=Widget", feedID); got != want {
		t.Errorf("redirect = %q, want %q (it must preserve the scoped-search keyword)", got, want)
	}
	expect(t, "entry matching the scoped search must be marked read", readAt(t, owner.app, matching).Valid)
	expect(t, "entry not matching the scoped search must remain unread", !readAt(t, owner.app, other).Valid)
}

// On the ?status=all tab matching_count must equal what the action will mark
// (unread and matching), not what the tab lists.
func TestCategoryMatchingCountReflectsUnreadOnlyOnAllTab(t *testing.T) {
	owner := newLibraryOwner(t, "alice_mc")
	catID := owner.category(t, "MatchCountCat")
	feedID := owner.feed(t, catID, "https://x/mc-feed", "MC Feed")
	matchingUnread := owner.entry(t, feedID, "guid-mc-unread", "Widget Roundup Unread", "https://x/mc/unread")
	matchingRead := owner.entry(t, feedID, "guid-mc-read", "Widget Roundup Read", "https://x/mc/read")
	execStatistics(t, owner.app, "UPDATE entry SET read_at = CURRENT_TIMESTAMP WHERE id = $1", matchingRead)
	backdateStoredEntries(t, owner.app)

	html := owner.app.GetOK(fmt.Sprintf("/categories/%d/entries?status=all&q=Widget", catID))
	if !strings.Contains(html, "Mark 1 matching as Read") {
		t.Errorf("the count must reflect only the unread match:\n%s", html)
	}

	postMarkRead(t, owner.app, fmt.Sprintf("/categories/%d/entries/mark-read", catID), map[string]string{"q": "Widget", "status": "all"})

	expect(t, "the previously-unread match must now be marked read", readAt(t, owner.app, matchingUnread).Valid)
	expect(t, "the already-read match must stay read", readAt(t, owner.app, matchingRead).Valid)
}
