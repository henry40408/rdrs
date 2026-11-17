package server

import (
	"context"
	"database/sql"
	"fmt"
	"html"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/henry40408/rdrs/internal/models"
	"github.com/henry40408/rdrs/internal/models/entry"
	"github.com/henry40408/rdrs/internal/save"
)

// Port of tests/no_js_test.rs (named nojs: a _js suffix is a GOOS build tag): the paths a browser takes with JavaScript
// disabled. The app never gets an X-CSRF-Token header and never posts JSON to
// /api/*, so only what the markup carries is sent.

const noJSUsername = "testuser"

// renderedCSRF is the first server-rendered _csrf value of a page.
func renderedCSRF(t testing.TB, page string) string {
	t.Helper()
	const marker = `<input type="hidden" name="_csrf" value="`
	_, rest, found := strings.Cut(page, marker)
	if !found {
		t.Fatalf("no _csrf field rendered in:\n%s", page)
	}
	value, _, found := strings.Cut(rest, `"`)
	if !found {
		t.Fatal("unterminated _csrf value")
	}
	return value
}

func (a *itestApp) noJSSetup() {
	a.t.Helper()
	page := a.Get("/setup")
	wantStatus(a.t, page, http.StatusOK)
	created := a.PostForm("/setup", url.Values{
		"_csrf": {renderedCSRF(a.t, page.Text())}, "username": {noJSUsername},
		"password": {fixturePassword}, "confirm-password": {fixturePassword},
	})
	wantStatus(a.t, created, http.StatusSeeOther)
}

func (a *itestApp) noJSLoginForm(path string, extra url.Values) itestResponse {
	a.t.Helper()
	page := a.Get(path)
	wantStatus(a.t, page, http.StatusOK)
	form := url.Values{"_csrf": {renderedCSRF(a.t, page.Text())}, "username": {noJSUsername}, "password": {fixturePassword}}
	for name, values := range extra {
		form[name] = values
	}
	return a.PostForm("/login", form)
}

// noJSApp is set up and signed in with native form POSTs only.
func noJSApp(t *testing.T) *itestApp {
	t.Helper()
	app := defaultITestApp(t)
	app.noJSSetup()
	wantStatus(t, app.noJSLoginForm("/login", nil), http.StatusSeeOther)
	return app
}

func location(response itestResponse) string { return response.Header().Get("Location") }

func TestSetupAndLoginWorkAsNativeFormPosts(t *testing.T) {
	wantStatus(t, noJSApp(t).Get("/"), http.StatusOK)
}

func TestLoginFormCarriesActionAndMethod(t *testing.T) {
	page := defaultITestApp(t).Get("/login").Text()
	// Without both, a submit becomes GET /login?password=... in the history.
	if !strings.Contains(page, `method="post"`) || !strings.Contains(page, `action="/login"`) {
		t.Errorf("login form must post to /login:\n%s", page)
	}
}

func TestFailedLoginReRendersTheFormWithAnError(t *testing.T) {
	app := defaultITestApp(t)
	app.noJSSetup()
	page := app.Get("/login")
	response := app.PostForm("/login", url.Values{
		"_csrf": {renderedCSRF(t, page.Text())}, "username": {noJSUsername}, "password": {"wrong-password-entirely"},
	})
	wantStatus(t, response, http.StatusOK)
	if !strings.Contains(response.Text(), "Invalid credentials") || !strings.Contains(response.Text(), `action="/login"`) {
		t.Errorf("want the generic failure message and the form back:\n%s", response.Text())
	}
}

func TestSigningInReturnsToThePageThatAskedForIt(t *testing.T) {
	app := defaultITestApp(t)
	app.noJSSetup()
	refused := app.Get("/feeds?category=3&filter=stale")
	wantStatus(t, refused, http.StatusSeeOther)
	loginURL := location(refused)
	if loginURL != "/login?next=%2Ffeeds%3Fcategory%3D3%26filter%3Dstale" {
		t.Fatalf("login url = %q", loginURL)
	}
	page := app.Get(loginURL).Text()
	_, rest, found := strings.Cut(page, `name="next" value="`)
	if !found {
		t.Fatalf("the login form must carry next:\n%s", page)
	}
	next, _, _ := strings.Cut(rest, `"`)
	signedIn := app.PostForm("/login", url.Values{
		"_csrf": {renderedCSRF(t, page)}, "username": {noJSUsername}, "password": {fixturePassword},
		"next": {html.UnescapeString(next)},
	})
	wantStatus(t, signedIn, http.StatusSeeOther)
	if location(signedIn) != "/feeds?category=3&filter=stale" {
		t.Errorf("location = %q", location(signedIn))
	}
}

func TestLoginRedirectRemembersOnlyPageGets(t *testing.T) {
	app := defaultITestApp(t)
	app.noJSSetup()
	if got := location(app.Get("/")); got != "/login" {
		t.Errorf("the home page is the default anyway: %q", got)
	}
	post := app.PostForm("/feeds/1/refresh", url.Values{"_csrf": {"x"}})
	if post.Code == http.StatusOK {
		t.Error("a POST must not succeed")
	}
	if got := location(post); got != "" && got != "/login" {
		t.Errorf("a POST can't be replayed as a GET: %q", got)
	}
}

func TestLoginIgnoresANextThatLeavesTheApp(t *testing.T) {
	for _, evil := range []string{"https://evil.example/", "//evil.example/", `/\evil.example/`, "/login", "/api/session"} {
		app := defaultITestApp(t)
		app.noJSSetup()
		page := app.Get("/login?" + url.Values{"next": {evil}}.Encode()).Text()
		if strings.Contains(page, `name="next"`) || strings.Contains(page, "data-next") {
			t.Errorf("%s rendered", evil)
		}
		// Posted directly, it is still refused.
		signedIn := app.noJSLoginForm("/login", url.Values{"next": {evil}})
		wantStatus(t, signedIn, http.StatusSeeOther)
		if location(signedIn) != "/" {
			t.Errorf("%s: location = %q", evil, location(signedIn))
		}
	}
}

func TestLoggedInMutationSucceedsWithOnlyTheFormField(t *testing.T) {
	app := noJSApp(t)
	page := app.Get("/categories")
	wantStatus(t, page, http.StatusOK)
	response := app.PostForm("/categories", url.Values{"_csrf": {renderedCSRF(t, page.Text())}, "name": {"From a scriptless browser"}})
	// Redirect, not 403: the guard accepted the rendered field.
	wantStatus(t, response, http.StatusSeeOther)
	if !strings.Contains(app.Get("/categories").Text(), "From a scriptless browser") {
		t.Error("the category should have been created")
	}
}

// createCategoryAndFollow posts a category and returns the landing page.
func createCategoryAndFollow(t *testing.T, app *itestApp, name string) string {
	t.Helper()
	page := app.Get("/categories")
	created := app.PostForm("/categories", url.Values{"_csrf": {renderedCSRF(t, page.Text())}, "name": {name}})
	return followRedirect(t, app, created)
}

// followRedirect follows a 303 the way a browser would, carrying the flash.
func followRedirect(t *testing.T, app *itestApp, response itestResponse) string {
	t.Helper()
	wantStatus(t, response, http.StatusSeeOther)
	landing := app.Get(location(response))
	wantStatus(t, landing, http.StatusOK)
	return landing.Text()
}

func TestAMutationConfirmsItselfWithAServerRenderedBanner(t *testing.T) {
	page := createCategoryAndFollow(t, noJSApp(t), "Announced")
	for _, want := range []string{`data-testid="flash-message"`, "Category created.", "<rdrs-flash"} {
		if !strings.Contains(page, want) {
			t.Errorf("page lacks %s:\n%s", want, page)
		}
	}
	if strings.Contains(page, "rdrs-flash-bootstrap") {
		t.Error("the JSON bootstrap is gone; SSR is the only path now")
	}
}

// With nothing to say the mount must render childless: .banner-stack:empty
// counts text nodes, so a stray newline would be a permanent invisible band.
func TestAnEmptyFlashMountHasNoChildrenAtAll(t *testing.T) {
	page := noJSApp(t).Get("/categories").Text()
	if !strings.Contains(page, "<rdrs-flash") || !strings.Contains(page, "tabindex=\"-1\"></rdrs-flash>") {
		t.Errorf("an empty flash mount must close immediately:\n%s", page)
	}
}

func TestAServerRenderedBannerTimestampsItselfUnambiguously(t *testing.T) {
	page := createCategoryAndFollow(t, noJSApp(t), "Timed")
	if !strings.Contains(page, `data-testid="flash-time"`) {
		t.Error("the banner must show a time")
	}
	if !strings.Contains(page, "<time class=\"banner-time\" datetime=\"") {
		t.Error("the time must be an RFC 3339 datetime the client can localize")
	}
}

func TestMutationWithoutTheFormFieldIsStillRejected(t *testing.T) {
	app := noJSApp(t)
	wantStatus(t, app.PostForm("/categories", url.Values{"name": {"No token at all"}}), http.StatusForbidden)
}

// seedNoJSEntry gives the signed-in user a category, a feed and one unread
// entry, straight in the database.
func seedNoJSEntry(t *testing.T, app *itestApp) (feedID, entryID int64) {
	t.Helper()
	ctx := context.Background()
	user, err := models.FindUserByUsername(ctx, app.DB, noJSUsername)
	if err != nil || user == nil {
		t.Fatalf("user = %v, err = %v", user, err)
	}
	category, err := models.CreateCategory(ctx, app.DB, user.ID, "T")
	if err != nil {
		t.Fatal(err)
	}
	title := "No-JS Feed"
	feed, err := models.CreateFeed(ctx, app.DB, models.CreateFeedParams{CategoryID: category.ID, URL: "https://x/no-js-feed", Title: &title})
	if err != nil {
		t.Fatal(err)
	}
	entryTitle, link := "E", "https://x/n"
	result, err := entry.UpsertEntry(ctx, app.DB, &entry.Upsert{FeedID: feed.ID, GUID: "guid-no-js", Title: &entryTitle, Link: &link})
	if err != nil {
		t.Fatal(err)
	}
	return feed.ID, result.ID
}

// waitForRead polls for the entry's read_at: the write may be detached.
func waitForRead(t *testing.T, app *itestApp, entryID int64) bool {
	t.Helper()
	for i := 0; i < 100; i++ {
		var readAt sql.NullString
		if err := app.DB.QueryRowContext(context.Background(), "SELECT read_at FROM entry WHERE id = $1", entryID).Scan(&readAt); err != nil {
			t.Fatal(err)
		}
		if readAt.Valid {
			return true
		}
		time.Sleep(10 * time.Millisecond)
	}
	return false
}

func documentRequest(method, path, referer string, form url.Values) itestRequest {
	request := itestRequest{Method: method, Path: path, Headers: map[string]string{"Sec-Fetch-Dest": "document"}}
	if referer != "" {
		request.Headers["Referer"] = "http://localhost" + referer
	}
	if form != nil {
		request.Body = form.Encode()
		request.Headers["Content-Type"] = "application/x-www-form-urlencoded"
	}
	return request
}

// A scriptless entry action is a top-level navigation; answering with the
// bare <template> fragment would render a blank page.
func TestEntryActionsRedirectAScriptlessFormPost(t *testing.T) {
	app := noJSApp(t)
	feedID, entryID := seedNoJSEntry(t, app)
	listURL := fmt.Sprintf("/feeds/%d/entries?status=unread", feedID)
	page := app.Get(listURL)
	wantStatus(t, page, http.StatusOK)
	token := renderedCSRF(t, page.Text())

	response := app.Do(documentRequest(http.MethodPost, fmt.Sprintf("/entries/%d/read", entryID), listURL, url.Values{"_csrf": {token}}))
	// Back to the exact list, filters intact and without ?entry=.
	wantStatus(t, response, http.StatusSeeOther)
	if location(response) != listURL {
		t.Errorf("location = %q, want %q", location(response), listURL)
	}
	if !waitForRead(t, app, entryID) {
		t.Error("a scriptless /read must still mark the entry read")
	}

	// From the reading pane the pane stays open across the redirect.
	paneURL := fmt.Sprintf("/entries?entry=%d", entryID)
	pane := app.Get(paneURL)
	wantStatus(t, pane, http.StatusOK)
	response = app.Do(documentRequest(http.MethodPost, fmt.Sprintf("/entries/%d/star", entryID), paneURL, url.Values{"_csrf": {renderedCSRF(t, pane.Text())}}))
	wantStatus(t, response, http.StatusSeeOther)
	if location(response) != paneURL {
		t.Errorf("location = %q, want %q", location(response), paneURL)
	}

	// No usable referrer: fall back to All Entries with the entry open.
	response = app.Do(documentRequest(http.MethodPost, fmt.Sprintf("/entries/%d/unstar", entryID), "", url.Values{"_csrf": {token}}))
	wantStatus(t, response, http.StatusSeeOther)
	if location(response) != paneURL {
		t.Errorf("location = %q, want %q", location(response), paneURL)
	}
}

func TestOpeningAnEntryMarksItReadWithoutJavascript(t *testing.T) {
	app := noJSApp(t)
	feedID, entryID := seedNoJSEntry(t, app)
	listURL := fmt.Sprintf("/feeds/%d/entries?status=unread", feedID)
	response := app.Do(documentRequest(http.MethodGet, fmt.Sprintf("/entries/%d/fragment", entryID), listURL, nil))
	wantStatus(t, response, http.StatusSeeOther)
	if want := fmt.Sprintf("%s&entry=%d", listURL, entryID); location(response) != want {
		t.Errorf("location = %q, want %q", location(response), want)
	}
	if !waitForRead(t, app, entryID) {
		t.Error("opening an entry with scripting off must mark it read")
	}
	landed := app.Get(fmt.Sprintf("%s&entry=%d", listURL, entryID))
	wantStatus(t, landed, http.StatusOK)
	if !strings.Contains(landed.Text(), fmt.Sprintf(`action="/entries/%d/unread"`, entryID)) {
		t.Error("the row must come back offering Mark Unread")
	}
}

// postEntryAction posts an entry action as a scriptless form navigation.
func postEntryAction(app *itestApp, path, csrf string) itestResponse {
	return app.Do(documentRequest(http.MethodPost, path, "", url.Values{"_csrf": {csrf}}))
}

// entryPaneCSRF opens the reading pane and returns its rendered token.
func entryPaneCSRF(t *testing.T, app *itestApp, entryID int64) string {
	t.Helper()
	page := app.Get(fmt.Sprintf("/entries?entry=%d", entryID))
	wantStatus(t, page, http.StatusOK)
	return renderedCSRF(t, page.Text())
}

func TestAScriptlessEntryActionReportsItself(t *testing.T) {
	app := noJSApp(t)
	_, entryID := seedNoJSEntry(t, app)
	csrf := entryPaneCSRF(t, app, entryID)
	// "Marked as unread." is only raised when the call changes state.
	wantStatus(t, postEntryAction(app, fmt.Sprintf("/entries/%d/read", entryID), csrf), http.StatusSeeOther)
	// The read write is detached; unread only counts as a change once it landed.
	if !waitForRead(t, app, entryID) {
		t.Fatal("the read write never landed")
	}
	page := followRedirect(t, app, postEntryAction(app, fmt.Sprintf("/entries/%d/unread", entryID), csrf))
	if !strings.Contains(page, "Marked as unread.") || !strings.Contains(page, `data-testid="flash-message"`) {
		t.Errorf("the message must survive the redirect as a banner:\n%s", page)
	}
}

func TestASelfEvidentActionStaysQuiet(t *testing.T) {
	app := noJSApp(t)
	_, entryID := seedNoJSEntry(t, app)
	csrf := entryPaneCSRF(t, app, entryID)
	page := followRedirect(t, app, postEntryAction(app, fmt.Sprintf("/entries/%d/star", entryID), csrf))
	if strings.Contains(page, `data-testid="flash-message"`) {
		t.Errorf("starring speaks for itself:\n%s", page)
	}
}

func TestAFailedSaveReachesAScriptlessReader(t *testing.T) {
	app := noJSApp(t)
	_, entryID := seedNoJSEntry(t, app)
	user, err := models.FindUserByUsername(context.Background(), app.DB, noJSUsername)
	if err != nil || user == nil {
		t.Fatalf("user = %v, err = %v", user, err)
	}
	services, err := save.ServicesConfig{Linkding: &save.LinkdingConfig{APIURL: "http://127.0.0.1:1/linkding", APIToken: "t"}}.JSON()
	if err != nil {
		t.Fatal(err)
	}
	// Sealed the way the handler would seal it (a configured secret).
	key, _ := app.App.Config.ServiceTokenKey()
	if _, err := models.UpdateSaveServices(context.Background(), app.DB, user.ID, services, key); err != nil {
		t.Fatal(err)
	}
	csrf := entryPaneCSRF(t, app, entryID)
	page := followRedirect(t, app, postEntryAction(app, fmt.Sprintf("/entries/%d/save", entryID), csrf))
	if !strings.Contains(page, "Linkding") {
		t.Errorf("the save failure must be reported, not swallowed:\n%s", page)
	}
	if !strings.Contains(page, "banner--error") {
		t.Error("and it is an error, not a success")
	}
}

func TestFetchFullContentReportsAFailureToAScriptlessReader(t *testing.T) {
	app := noJSApp(t)
	_, entryID := seedNoJSEntry(t, app)
	csrf := entryPaneCSRF(t, app, entryID)
	page := followRedirect(t, app, postEntryAction(app, fmt.Sprintf("/entries/%d/fetch-full-content", entryID), csrf))
	if !strings.Contains(page, "Failed to fetch full content") {
		t.Errorf("the attempt's failure must reach the reader:\n%s", page)
	}
	if strings.Contains(page, "needs JavaScript") {
		t.Error("the feature no longer needs JavaScript")
	}
	if strings.Contains(page, `aria-label="Show Original"`) {
		t.Error("a failed fetch must not leave the pane claiming full content")
	}
}

func TestEveryLoggedInPageOffersNavigationWithoutJavascript(t *testing.T) {
	app := noJSApp(t)
	feedID, entryID := seedNoJSEntry(t, app)
	paths := []string{
		"/", "/entries", "/entries/starred", "/categories", "/feeds", "/statistics", "/settings",
		fmt.Sprintf("/feeds/%d/entries", feedID), fmt.Sprintf("/entries?entry=%d", entryID),
	}
	for _, path := range paths {
		page := app.Get(path).Text()
		if !strings.Contains(page, `<nav class="nav-fallback"`) {
			t.Errorf("%s: no scriptless navigation", path)
		}
		for _, target := range []string{`href="/entries"`, `href="/entries/starred"`, `href="/categories"`, `href="/feeds"`, `href="/settings"`} {
			if !strings.Contains(page, target) {
				t.Errorf("%s: missing %s", path, target)
			}
		}
	}
}

// The fallback lives inside <noscript>, which a scripted browser does not
// parse into elements.
func TestTheNavigationFallbackStaysInsideNoscript(t *testing.T) {
	page := noJSApp(t).Get("/").Text()
	opened := strings.Index(page, "<noscript>")
	if opened < 0 {
		t.Fatal("the fallback is in a noscript")
	}
	closed := strings.Index(page[opened:], "</noscript>")
	if closed < 0 {
		t.Fatal("that noscript closes")
	}
	closed += opened
	nav := strings.Index(page, `<nav class="nav-fallback"`)
	if nav <= opened || nav >= closed {
		t.Error("the nav must sit inside the noscript, or a scripted browser renders it too")
	}
	if !strings.Contains(page, "<rdrs-sidebar") || !strings.Contains(page, `id="rdrs-sidebar-bootstrap"`) {
		t.Error("the CSR sidebar and its bootstrap stay")
	}
	if strings.Contains(page, `<div class="sidebar"`) {
		t.Error("the sidebar's own markup must not be server-rendered")
	}
}

func TestTheNavigationFallbackCarriesTheUnreadTotal(t *testing.T) {
	app := noJSApp(t)
	_, entryID := seedNoJSEntry(t, app)
	page := app.Get("/").Text()
	if !strings.Contains(page, `<span class="nav-fallback-count">1</span>`) {
		t.Errorf("one unread entry must show as a count:\n%s", page)
	}
	// Read it through the app: a direct write leaves the sidebar cache stale.
	wantStatus(t, postEntryAction(app, fmt.Sprintf("/entries/%d/read", entryID), renderedCSRF(t, page)), http.StatusSeeOther)
	if strings.Contains(app.Get("/").Text(), "nav-fallback-count") {
		t.Error("an empty inbox shows no badge at all")
	}
}

func TestAScriptlessReaderCanSignOut(t *testing.T) {
	app := noJSApp(t)
	page := app.Get("/").Text()
	if !strings.Contains(page, `action="/logout"`) {
		t.Fatal("the scriptless nav must offer a sign-out form")
	}
	out := app.PostForm("/logout", url.Values{"_csrf": {renderedCSRF(t, page)}})
	wantStatus(t, out, http.StatusSeeOther)
	if location(out) != "/login" {
		t.Errorf("location = %q", location(out))
	}
	wantStatus(t, app.Get("/"), http.StatusSeeOther)
}

func TestSigningOutStillNeedsTheCSRFToken(t *testing.T) {
	app := noJSApp(t)
	wantStatus(t, app.Do(itestRequest{Method: http.MethodPost, Path: "/logout"}), http.StatusForbidden)
	wantStatus(t, app.Get("/"), http.StatusOK)
}

func TestEntryListRowsRenderTheToken(t *testing.T) {
	app := noJSApp(t)
	page := app.Get("/feeds")
	wantStatus(t, page, http.StatusOK)
	if strings.Count(page.Text(), `name="_csrf"`) < 1 {
		t.Errorf("the feeds page renders forms and must carry the token:\n%s", page.Text())
	}
}
