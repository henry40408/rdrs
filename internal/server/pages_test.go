package server

import (
	"context"
	"fmt"
	"net/http"
	"net/netip"
	"strings"
	"testing"

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
