package e2e

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// world is the state shared by the steps of one scenario: a server of its own,
// a throwaway account, and a browser.
type world struct {
	server  *harness.Server
	browser *harness.Browser
	api     *harness.API
	seed    *harness.Seed
	// user is registered lazily by the sign-in step, so sign-up scenarios can
	// claim the name themselves.
	user   harness.Credentials
	userID int64
	// invitePath is the one-time link issued for this account.
	invitePath string
	// otherUsername is a second account, for admin-table rows that are not the
	// scenario's own.
	otherUsername string
	// network intercepts requests; attached on first use since it costs a
	// WebSocket.
	network *harness.Network
	// Held fragment, full-content and summarize responses.
	delayedFragment, delayedFullContent, delayedSummarize *harness.RouteHandle
	// heldSummaryFragment holds the summary fragment response; summarizePosts
	// counts summarize re-queues.
	heldSummaryFragment, summarizePosts *harness.RouteHandle
	// clientInfoPuts counts the browser reports the page sends.
	clientInfoPuts *harness.RouteHandle
	// paneStamp is the list pane's render stamp when it was tagged.
	paneStamp string
	// actionBar is the recorded reading-pane action bar layout.
	actionBar []actionBarControl
	// unreadBefore is the sidebar unread count before an out-of-band change.
	unreadBefore int
	// seededEntries are the ids from the latest feed seeded for the account.
	seededEntries []int64
}

// initializeScenario runs once per scenario, so every scenario gets its own
// world through the closure.
func initializeScenario(sc *godog.ScenarioContext) {
	w := &world{}
	sc.Before(w.start)
	sc.After(w.cleanUp)
	registerAuthSteps(sc, w)
	registerAdminSteps(sc, w)
	registerOrganizeSteps(sc, w)
	registerOnboardingSteps(sc, w)
	registerPWASteps(sc, w)
	registerKeyboardSteps(sc, w)
	registerSearchSteps(sc, w)
	registerSettingsSteps(sc, w)
	registerSidebarSteps(sc, w)
	registerMorphSteps(sc, w)
	registerScopedSearchSteps(sc, w)
	registerEntrySteps(sc, w)
	registerReadingPaneSteps(sc, w)
	registerTriageSteps(sc, w)
	registerResponsiveSteps(sc, w)
	registerSSESteps(sc, w)
	registerSummarizerSteps(sc, w)
}

// start boots the server and the browser of the scenario.
func (w *world) start(ctx context.Context, _ *godog.Scenario) (context.Context, error) {
	server, err := harness.Start()
	if err != nil {
		return ctx, err
	}
	w.server = server
	w.api = harness.NewAPI(server)
	w.user = harness.Credentials{Username: "e2e-" + harness.RandomSlug(), Password: harness.Password}
	if w.seed, err = harness.OpenSeed(server.DBPath); err != nil {
		return ctx, err
	}
	if w.browser, err = harness.Open(); err != nil {
		return ctx, fmt.Errorf("could not open a browser session: %w", err)
	}
	return ctx, nil
}

// cleanUp runs on every outcome, so a failed scenario leaves no Chrome and no
// server behind.
func (w *world) cleanUp(ctx context.Context, _ *godog.Scenario, _ error) (context.Context, error) {
	if w.browser != nil {
		w.browser.Close()
	}
	if w.seed != nil {
		w.seed.Close()
	}
	if w.server != nil {
		w.server.Stop()
	}
	*w = world{}
	return ctx, nil
}

// currentUserID is the row id of this scenario's account, resolved on first
// use.
func (w *world) currentUserID() (int64, error) {
	if w.userID != 0 {
		return w.userID, nil
	}
	id, err := w.seed.UserID(w.user.Username)
	if err != nil {
		return 0, err
	}
	w.userID = id
	return id, nil
}

// goto navigates to a path on the server under test.
func (w *world) goto_(path string) error {
	return w.browser.Goto(w.server.BaseURL + path)
}

// expectPath waits for the browser's path and query to equal expected.
func (w *world) expectPath(expected string) error {
	return harness.EventuallyEq("the URL is "+expected, expected, w.browser.PathAndQuery)
}

// seedFeed gives the account one category and one feed of count entries.
func (w *world) seedFeed(title string, count int) ([]int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return nil, err
	}
	categoryID, err := w.seed.CreateCategory(userID, "Cat-"+w.user.Username)
	if err != nil {
		return nil, err
	}
	feedURL := fmt.Sprintf("https://example.com/%s.xml", w.user.Username)
	feedID, err := w.seed.CreateFeed(categoryID, feedURL, title)
	if err != nil {
		return nil, err
	}
	ids, err := w.seed.SeedTestEntries(feedID, count)
	w.seededEntries = ids
	return ids, err
}

// userClient does not follow redirects: only the status and the side effect
// of a request made as the user matter.
var userClient = &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error {
	return http.ErrUseLastResponse
}}

// requestAsUser sends a request with the browser's session, outside the page.
func (w *world) requestAsUser(method, path string) (*http.Response, error) {
	cookies, csrf, err := w.browser.CookieHeader()
	if err != nil {
		return nil, err
	}
	request, err := http.NewRequest(method, w.server.BaseURL+path, nil)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Cookie", cookies)
	request.Header.Set("X-CSRF-Token", csrf)
	return userClient.Do(request)
}

// getAsUser returns the body of a GET made with the browser's session, for
// downloads the browser cannot be asked for.
func (w *world) getAsUser(path string) (string, error) {
	response, err := w.requestAsUser(http.MethodGet, path)
	if err != nil {
		return "", err
	}
	defer func() { _ = response.Body.Close() }()
	if response.StatusCode < 200 || response.StatusCode > 299 {
		return "", fmt.Errorf("GET %s answered %s", path, response.Status)
	}
	body, err := io.ReadAll(response.Body)
	return string(body), err
}

// route adds an interception rule, attaching the interceptor on first use.
func (w *world) route(pattern string, action harness.Action) (*harness.RouteHandle, error) {
	return w.routeMethod(pattern, "", action)
}

// routeMethod is route restricted to one HTTP method.
func (w *world) routeMethod(pattern, method string, action harness.Action) (*harness.RouteHandle, error) {
	if w.network == nil {
		network, err := w.browser.AttachNetwork()
		if err != nil {
			return nil, err
		}
		w.network = network
	}
	return w.network.RouteMethod(pattern, method, action)
}

// delayRequests holds every request matching pattern for delay, then lets it
// through.
func (w *world) delayRequests(pattern string, delay time.Duration) (*harness.RouteHandle, error) {
	return w.route(pattern, harness.Action{Kind: harness.Delay, Delay: delay})
}
