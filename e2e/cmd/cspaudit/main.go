// Command cspaudit walks the app in a real browser and fails on any Content
// Security Policy violation.
//
// The static scan in the server's security-headers middleware cannot see
// runtime-only violations (cross-origin @import, innerHTML markup, <style> in
// shadow roots); only a browser enforcing the header can.
//
//	cd e2e && go run -mod=vendor ./cmd/cspaudit
package main

import (
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// collector gathers securitypolicyviolation events (structured, and fired for
// inline style= too). It is injected as an init script, which is exempt from
// the page's CSP, so the policy cannot silence it.
const collector = `
window.__cspViolations = [];
document.addEventListener('securitypolicyviolation', (e) => {
  window.__cspViolations.push({
    directive: e.effectiveDirective || e.violatedDirective,
    blockedURI: e.blockedURI,
    sourceFile: e.sourceFile,
    line: e.lineNumber,
    sample: (e.sample || '').slice(0, 80),
  });
});
`

// drainScript empties the page-side buffer.
const drainScript = `(() => { const v = window.__cspViolations || []; window.__cspViolations = []; return v; })()`

// settle lets deferred modules and swaps run so their violations are not missed.
const settle = 250 * time.Millisecond

const positiveControl = "positive control"

// violation is one reported violation, plus where the walk was when it fired.
type violation struct {
	Where      string `json:"-"`
	Directive  string `json:"directive"`
	BlockedURI string `json:"blockedURI"`
	SourceFile string `json:"sourceFile"`
	Line       int    `json:"line"`
	Sample     string `json:"sample"`
}

func (v violation) String() string {
	blocked := v.BlockedURI
	if blocked == "" {
		blocked = "(inline)"
	}
	text := fmt.Sprintf("  [%s] %s blocked %s", v.Where, v.Directive, blocked)
	if v.SourceFile != "" {
		text += fmt.Sprintf(" from %s:%d", v.SourceFile, v.Line)
	}
	if v.Sample != "" {
		text += " — sample: " + v.Sample
	}
	return text
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
	browser, err := harness.Open()
	if err != nil {
		return err
	}
	defer browser.Close()
	if err := browser.AddInitScript(collector); err != nil {
		return err
	}

	audit := &audit{server: server, browser: browser}
	if err := audit.walk(); err != nil {
		return err
	}
	return audit.verdict()
}

// verdict fails on any violation; the planted control was already taken out.
func (a *audit) verdict() error {
	if len(a.violations) > 0 {
		lines := make([]string, len(a.violations))
		for i, found := range a.violations {
			lines[i] = found.String()
		}
		return fmt.Errorf("CSP violations detected:\n%s", strings.Join(lines, "\n"))
	}
	fmt.Println("cspaudit: no violations across the app")
	return nil
}

type audit struct {
	server     *harness.Server
	browser    *harness.Browser
	violations []violation
}

func (a *audit) walk() error {
	feedID, err := a.seedAccount()
	if err != nil {
		return err
	}
	steps := []func() error{
		a.loggedOutPages,
		a.signIn,
		func() error { return a.loggedInPages(feedID) },
		a.readingPane,
		a.keyboardHelp,
		a.mobileSidebar,
		a.plantPositiveControl,
	}
	for _, step := range steps {
		if err := step(); err != nil {
			return err
		}
	}
	return nil
}

var username = "csp-" + harness.RandomSlug()

// seedAccount creates an admin with something to render and returns its feed.
func (a *audit) seedAccount() (int64, error) {
	if err := harness.NewAPI(a.server).SetupFirstAccount(username, harness.Password); err != nil {
		return 0, err
	}
	seed, err := harness.OpenSeed(a.server.DBPath)
	if err != nil {
		return 0, err
	}
	defer seed.Close()
	userID, err := seed.UserID(username)
	if err != nil {
		return 0, err
	}
	categoryID, err := seed.CreateCategory(userID, "Tech")
	if err != nil {
		return 0, err
	}
	feedID, err := seed.CreateFeed(categoryID, "https://example.com/feed.xml", "Example Feed")
	if err != nil {
		return 0, err
	}
	if _, err := seed.SeedTestEntries(feedID, 12); err != nil {
		return 0, err
	}
	if err := seed.ConfigureKagi(userID, "e2e-test-token"); err != nil {
		return 0, err
	}
	return feedID, seed.MakeAdmin(userID)
}

// visit waits for the document, never network idle: the SSE stream never idles.
func (a *audit) visit(label, path string) error {
	if err := a.browser.Goto(a.server.BaseURL + path); err != nil {
		return err
	}
	time.Sleep(settle)
	return a.drain(label)
}

// drain must run after every navigation: the collector's buffer is per document.
func (a *audit) drain(where string) error {
	var found []violation
	if err := harness.EvalJSON(a.browser, drainScript, &found); err != nil {
		return err
	}
	for _, item := range found {
		item.Where = where
		a.violations = append(a.violations, item)
	}
	return nil
}

func (a *audit) visitAll(pages [][2]string) error {
	for _, page := range pages {
		if err := a.visit(page[0], page[1]); err != nil {
			return err
		}
	}
	return nil
}

func (a *audit) loggedOutPages() error {
	return a.visitAll([][2]string{{"Login", "/login"}, {"Setup", "/setup"}})
}

func (a *audit) signIn() error {
	if err := a.browser.Goto(a.server.BaseURL + "/login"); err != nil {
		return err
	}
	if err := a.browser.Fill("username-input", username); err != nil {
		return err
	}
	if err := a.browser.Fill("password-input", harness.Password); err != nil {
		return err
	}
	if err := a.browser.Submit("login-submit"); err != nil {
		return err
	}
	return a.drain("Login (submit)")
}

func (a *audit) loggedInPages(feedID int64) error {
	return a.visitAll([][2]string{
		{"Unread", "/"},
		{"All entries", "/entries"},
		{"Starred", "/entries/starred"},
		{"Summarized", "/entries/summarized"},
		{"Feeds", "/feeds"},
		{"Feed edit", fmt.Sprintf("/feeds/%d/edit", feedID)},
		{"Categories", "/categories"},
		{"Import OPML", "/feeds/import"},
		{"Search", "/search?q=test"},
		{"User settings", "/user-settings"},
		{"App settings", "/settings"},
		// Bars use pct-N classes rather than inline style.
		{"Statistics", "/statistics"},
		{"Admin", "/admin"},
	})
}

// readingPane is a script-swapped fragment, parsed under the same policy.
func (a *audit) readingPane() error {
	if err := a.browser.Goto(a.server.BaseURL + "/"); err != nil {
		return err
	}
	row, err := a.browser.TestID("entry-item")
	if err != nil {
		return err
	}
	title, err := row.Find(`[data-testid="entry-title-link"]`)
	if err != nil {
		return err
	}
	if err := title.Click(); err != nil {
		return err
	}
	err = harness.Eventually("the reading pane to fill", func() (bool, error) {
		pane, err := a.browser.CSSOpt("#reading-pane:not(.reading-pane-empty)")
		return pane != nil, err
	})
	if err != nil {
		return err
	}
	time.Sleep(settle)
	return a.drain("Reading pane (swap fragment)")
}

// keyboardHelp is in a shadow root, whose <style> is still policed, hence the
// constructable stylesheet.
func (a *audit) keyboardHelp() error {
	if err := a.browser.Press("?"); err != nil {
		return err
	}
	if err := a.browser.ExpectVisible("kb-help"); err != nil {
		return err
	}
	time.Sleep(settle)
	if err := a.drain("Keyboard help overlay (shadow DOM)"); err != nil {
		return err
	}
	return a.browser.PressFocused("Escape")
}

// mobileSidebar uses a narrow viewport for the off-canvas sidebar; the same
// session keeps the sign-in.
func (a *audit) mobileSidebar() error {
	if err := a.browser.SetViewport(375, 667); err != nil {
		return err
	}
	if err := a.browser.Goto(a.server.BaseURL + "/"); err != nil {
		return err
	}
	if err := a.browser.ClickCSS(".sidebar-toggle"); err != nil {
		return err
	}
	time.Sleep(settle)
	if err := a.drain("Sidebar drawer (mobile)"); err != nil {
		return err
	}
	return a.browser.SetViewport(1280, 800)
}

// plantPositiveControl plants a violation that must surface; otherwise a clean
// result proves nothing.
func (a *audit) plantPositiveControl() error {
	_, err := harness.Eval[bool](a.browser, `(() => {
		document.body.insertAdjacentHTML('beforeend', '<div id="csp-control" style="color:red"></div>');
		return true;
	})()`)
	if err != nil {
		return err
	}
	time.Sleep(100 * time.Millisecond)
	before := len(a.violations)
	if err := a.drain(positiveControl); err != nil {
		return err
	}
	rest, err := takeOutControl(a.violations[before:])
	if err != nil {
		return err
	}
	a.violations = append(a.violations[:before], rest...)
	return nil
}

// takeOutControl removes the one style-src violation the planted style must
// cause and returns the others, which are real findings.
func takeOutControl(control []violation) ([]violation, error) {
	if len(control) == 0 {
		return nil, fmt.Errorf("positive control: a planted inline style must be blocked — the " +
			"collector was not live, so every clean result above is meaningless")
	}
	for i, found := range control {
		if !strings.Contains(found.Directive, "style-src") {
			continue
		}
		rest := append([]violation{}, control[:i]...)
		return append(rest, control[i+1:]...), nil
	}
	return nil, fmt.Errorf("positive control was blocked by %s, expected style-src", control[0].Directive)
}
