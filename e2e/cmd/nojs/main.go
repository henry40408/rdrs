// Command nojs walks the whole app with scripting off and every .js request
// aborted, failing on anything a scriptless reader cannot do (it found #489:
// sign-out existed only as a fetch DELETE).
//
// It is a gate: each check is a guarantee from the no-JS series (#480-#491).
// Findings print before the exit code is decided.
//
//	cd e2e && go run -mod=vendor ./cmd/nojs
//
// Scripting is disabled and Network aborts the scripts, so the pages must work
// with scripts never delivered, not merely not run.
package main

import (
	"fmt"
	"os"
	"sort"
	"strings"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

const (
	walkerName = "walker"
	// scriptURLs matches anything whose URL ends in .js, with or without a
	// query string.
	scriptURLs = `\.js($|\?)`
)

// findings are printed together at the end, so one run reports everything.
type findings []string

func (f *findings) note(where, what string) {
	*f = append(*f, where+": "+what)
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	server, err := harness.Start()
	if err != nil {
		return err
	}
	defer server.Stop()
	browser, err := harness.OpenWith(harness.ScriptsOff)
	if err != nil {
		return err
	}
	defer browser.Close()
	network, err := browser.AttachNetwork()
	if err != nil {
		return err
	}
	if _, err := network.Route(scriptURLs, harness.Action{Kind: harness.Abort}); err != nil {
		return err
	}

	var found findings
	walker := &walk{server: server, browser: browser, network: network, found: &found}
	// A crash is itself a finding; print what came before it.
	if err := walker.all(); err != nil {
		found.note("walkthrough", fmt.Sprintf("aborted: %v", err))
	}

	fmt.Println("\n=== no-JS walkthrough findings ===")
	if len(found) == 0 {
		fmt.Println("(none)")
	}
	for _, finding := range found {
		fmt.Println("- " + finding)
	}
	fmt.Println("=== end ===")
	if len(found) > 0 {
		return fmt.Errorf("the app must stay usable with JavaScript disabled (%d finding(s))", len(found))
	}
	return nil
}

type walk struct {
	server  *harness.Server
	browser *harness.Browser
	network *harness.Network
	found   *findings
}

func (w *walk) all() error {
	steps := []func() error{
		w.setup, w.signIn, w.navigation, w.subscribe, w.filterBar,
		w.categories, w.seedEntries, w.entries, w.search, w.preferences, w.signOut,
	}
	for _, step := range steps {
		if err := step(); err != nil {
			return err
		}
	}
	return nil
}

func (w *walk) goTo(path string) error {
	return w.browser.Goto(w.server.BaseURL + path)
}

func (w *walk) url() (string, error) {
	current, err := w.browser.URL()
	if err != nil {
		return "", err
	}
	return current.String(), nil
}

// present reports whether selector matches, noting the finding when not.
func (w *walk) present(where, selector, missing string) (bool, error) {
	element, err := w.browser.CSSOpt(selector)
	if err != nil {
		return false, err
	}
	if element == nil {
		w.found.note(where, missing)
		return false, nil
	}
	return true, nil
}

// expectURLContains notes a finding when the current URL lacks needle.
func (w *walk) expectURLContains(where, needle, message string) error {
	current, err := w.url()
	if err != nil {
		return err
	}
	if !strings.Contains(current, needle) {
		w.found.note(where, fmt.Sprintf("%s: %s", message, current))
	}
	return nil
}

func (w *walk) fillAll(fields [][2]string) error {
	for _, field := range fields {
		if err := w.browser.FillCSS(field[0], field[1]); err != nil {
			return err
		}
	}
	return nil
}

// 1. First-run setup.
func (w *walk) setup() error {
	if err := w.goTo("/setup"); err != nil {
		return err
	}
	fields := [][2]string{
		{"#setup-token", w.server.SetupToken},
		{"#username", walkerName},
		{"#password", harness.Password},
	}
	if err := w.fillAll(fields); err != nil {
		return err
	}
	confirm, err := w.browser.CSSOpt("#confirm-password")
	if err != nil {
		return err
	}
	if confirm != nil {
		if err := w.browser.FillCSS("#confirm-password", harness.Password); err != nil {
			return err
		}
	}
	if err := w.browser.SubmitCSS(`form button[type="submit"]`); err != nil {
		return err
	}
	return w.expectURLContains("setup", "/login", "expected to land on /login, got")
}

// 2. Sign in.
func (w *walk) signIn() error {
	if err := w.fillAll([][2]string{{"#username", walkerName}, {"#password", harness.Password}}); err != nil {
		return err
	}
	if err := w.browser.SubmitCSS(`form[action="/login"] button[type="submit"]`); err != nil {
		return err
	}
	current, err := w.url()
	if err != nil {
		return err
	}
	if strings.Contains(current, "/login") {
		title, err := harness.Eval[string](w.browser, "document.title")
		if err != nil {
			return err
		}
		w.found.note("login", "still on /login after submitting: "+title)
	}
	return nil
}

// 3. Navigation exists at all, and every destination answers 200.
func (w *walk) navigation() error {
	nav, err := w.browser.CSSOpt("nav.nav-fallback")
	if err != nil {
		return err
	}
	if nav == nil {
		w.found.note("nav", "no scriptless navigation on the landing page")
		return nil
	}
	links, err := w.browser.CSSAll("nav.nav-fallback a")
	if err != nil {
		return err
	}
	hrefs := map[string]bool{}
	for _, link := range links {
		href, ok, err := link.Attr("href")
		if err != nil {
			return err
		}
		if ok {
			hrefs[href] = true
		}
	}
	sorted := make([]string, 0, len(hrefs))
	for href := range hrefs {
		sorted = append(sorted, href)
	}
	sort.Strings(sorted)
	for _, href := range sorted {
		if err := w.visitDestination(href); err != nil {
			return err
		}
	}
	return nil
}

func (w *walk) visitDestination(href string) error {
	if err := w.goTo(href); err != nil {
		return err
	}
	status, ok := w.network.DocumentStatus(href)
	if !ok || status != 200 {
		w.found.note("nav", fmt.Sprintf("%s returned %d (seen: %t)", href, status, ok))
	}
	_, err := w.present("nav", "nav.nav-fallback", href+" renders without the navigation")
	return err
}

// 4. Subscribe to a feed.
func (w *walk) subscribe() error {
	const form = `form[action="/feeds"][method="post"]`
	if err := w.goTo("/feeds"); err != nil {
		return err
	}
	if ok, err := w.present("feeds", form, "no add-feed form"); err != nil || !ok {
		return err
	}
	if err := w.browser.FillCSS(form+` input[name="url"]`, w.server.FeedURL+"/feed.xml"); err != nil {
		return err
	}
	if err := w.browser.SubmitCSS(form + ` button[type="submit"]`); err != nil {
		return err
	}
	return w.expectConfirmation("feeds", "adding a feed produced no visible confirmation")
}

func (w *walk) expectConfirmation(where, missing string) error {
	visible, err := w.browser.IsVisible("flash-message")
	if err != nil {
		return err
	}
	if !visible {
		w.found.note(where, missing)
	}
	return nil
}

// 5. The filter bar.
func (w *walk) filterBar() error {
	if err := w.goTo("/feeds"); err != nil {
		return err
	}
	apply, err := w.browser.TestIDOpt("feed-filter-apply")
	if err != nil {
		return err
	}
	if apply == nil {
		w.found.note("feeds", "filter bar has no way to submit without scripting")
		return nil
	}
	if err := w.selectByValue("#sort-by", "unread"); err != nil {
		return err
	}
	if err := w.browser.Submit("feed-filter-apply"); err != nil {
		return err
	}
	return w.expectURLContains("feeds", "sort=unread", "Apply did not carry the sort")
}

// selectByValue is a native select change; no page script is needed for it.
func (w *walk) selectByValue(selector, value string) error {
	_, err := harness.Eval[bool](w.browser, fmt.Sprintf(
		"(() => { document.querySelector(%s).value = %s; return true })()",
		harness.JSLiteral(selector), harness.JSLiteral(value)))
	return err
}

// 6. Categories: create, and see it.
func (w *walk) categories() error {
	const form = `form[action="/categories"][method="post"]`
	if err := w.goTo("/categories"); err != nil {
		return err
	}
	if ok, err := w.present("categories", form, "no create form"); err != nil || !ok {
		return err
	}
	if err := w.browser.FillCSS(form+` input[name="name"]`, "Walkthrough"); err != nil {
		return err
	}
	if err := w.browser.SubmitCSS(form + ` button[type="submit"]`); err != nil {
		return err
	}
	// The name comes back inside the rename form's input, not as text.
	inputs, err := w.browser.CSSAll(`form.cat-rename input[name="name"]`)
	if err != nil {
		return err
	}
	names := make([]string, 0, len(inputs))
	for _, input := range inputs {
		value, err := input.Value()
		if err != nil {
			return err
		}
		names = append(names, value)
	}
	if !contains(names, "Walkthrough") {
		w.found.note("categories", fmt.Sprintf("the created category is not listed afterwards (%q)", names))
	}
	return w.expectConfirmation("categories", "creating a category produced no confirmation")
}

func contains(values []string, wanted string) bool {
	for _, value := range values {
		if value == wanted {
			return true
		}
	}
	return false
}

// seedEntries adds items directly: the mock feed has none, and the reading
// flow needs something to read.
func (w *walk) seedEntries() error {
	seed, err := harness.OpenSeed(w.server.DBPath)
	if err != nil {
		return err
	}
	defer seed.Close()
	userID, err := seed.UserID(walkerName)
	if err != nil {
		return err
	}
	categoryID, err := seed.CreateCategory(userID, "Walk")
	if err != nil {
		return err
	}
	feedID, err := seed.CreateFeed(categoryID, "https://example.invalid/walk", "Walk Feed")
	if err != nil {
		return err
	}
	entries := make([]harness.NewEntry, 0, 3)
	for i := range 3 {
		entries = append(entries, harness.NewEntry{
			FeedID:  feedID,
			GUID:    fmt.Sprintf("walk-%d", i),
			Title:   fmt.Sprintf("Walk entry %d", i),
			Link:    fmt.Sprintf("https://example.invalid/walk/%d", i),
			Content: fmt.Sprintf("<p>Body %d</p>", i),
		})
	}
	_, err = seed.InsertEntries(entries)
	return err
}

// 7. Reading: open an entry, star it, toggle read.
func (w *walk) entries() error {
	if err := w.goTo("/entries"); err != nil {
		return err
	}
	title, err := w.browser.TestIDOpt("entry-title-link")
	if err != nil {
		return err
	}
	if title == nil {
		w.found.note("entries", "no entries to read (the mock feed may be empty) — skipped")
		return nil
	}
	if err := w.browser.Submit("entry-title-link"); err != nil {
		return err
	}
	if err := w.expectURLContains("entries", "entry=", "opening an entry did not land on a pane URL"); err != nil {
		return err
	}
	if _, err := w.present("entries", "#reading-pane", "the reading pane did not render after opening an entry"); err != nil {
		return err
	}
	if err := w.star(); err != nil {
		return err
	}
	return w.toggleRead()
}

func (w *walk) star() error {
	star, err := w.browser.TestIDOpt("entry-star-action")
	if err != nil {
		return err
	}
	if star == nil {
		w.found.note("entries", "no star control")
		return nil
	}
	if err := w.browser.Submit("entry-star-action"); err != nil {
		return err
	}
	_, err = w.present("entries", ".entry-star.starred", "starring did not visibly take effect")
	return err
}

func (w *walk) toggleRead() error {
	toggle, err := w.browser.TestIDOpt("entry-read-toggle")
	if err != nil {
		return err
	}
	if toggle == nil {
		w.found.note("entries", "no read/unread toggle")
		return nil
	}
	return w.browser.Submit("entry-read-toggle")
}

// 8. Search. The form is scoped by :has(): a bare `form button[type=submit]`
// hits the sign-out form and ends the walk early.
func (w *walk) search() error {
	const (
		form   = `form:has(input[name="q"])`
		field  = `form input[name="q"]`
		submit = `form:has(input[name="q"]) button[type="submit"]`
	)
	if err := w.goTo("/search"); err != nil {
		return err
	}
	if ok, err := w.present("search", form, "no search form"); err != nil || !ok {
		return err
	}
	if err := w.syntaxHelp(); err != nil {
		return err
	}
	if err := w.browser.FillCSS(field, "test"); err != nil {
		return err
	}
	if ok, err := w.present("search", submit, "search form has no submit control"); err != nil || !ok {
		return err
	}
	if err := w.browser.SubmitCSS(submit); err != nil {
		return err
	}
	return w.expectURLContains("search", "q=test", "search did not navigate")
}

// syntaxHelp is a popover, so it opens without a script.
func (w *walk) syntaxHelp() error {
	if ok, err := w.present("search", "[data-search-help]", "no syntax help button"); err != nil || !ok {
		return err
	}
	if err := w.browser.ClickCSS("[data-search-help]"); err != nil {
		return err
	}
	open, err := harness.Eval[bool](w.browser, "document.getElementById('search-syntax-popover').matches(':popover-open')")
	if err != nil {
		return err
	}
	if !open {
		w.found.note("search", "syntax help did not open")
	}
	// It is centred over the button; close it for the form below.
	_, err = harness.Eval[bool](w.browser, "(() => { document.getElementById('search-syntax-popover').hidePopover(); return true })()")
	return err
}

// 9. Preferences.
func (w *walk) preferences() error {
	const form = `form[action="/user-settings/preferences"]`
	if err := w.goTo("/user-settings"); err != nil {
		return err
	}
	if ok, err := w.present("preferences", form, "no preferences form"); err != nil || !ok {
		return err
	}
	_, err := w.present("preferences", form+` button[type="submit"]`, "preferences form has no submit control")
	return err
}

// 10. Sign out, and confirm the session is actually gone.
func (w *walk) signOut() error {
	const logout = `form[action="/logout"] button[type="submit"]`
	if err := w.goTo("/"); err != nil {
		return err
	}
	if ok, err := w.present("logout", logout, "no way to sign out"); err != nil || !ok {
		return err
	}
	if err := w.browser.SubmitCSS(logout); err != nil {
		return err
	}
	if err := w.expectURLContains("logout", "/login", "did not land on /login"); err != nil {
		return err
	}
	// The session must really be over, not just redirected away from.
	if err := w.goTo("/"); err != nil {
		return err
	}
	current, err := w.url()
	if err != nil {
		return err
	}
	if !strings.Contains(current, "/login") {
		w.found.note("logout", "still signed in after signing out")
	}
	return nil
}
