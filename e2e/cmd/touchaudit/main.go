// Command touchaudit reports interactive elements under 44px in either axis at
// iPhone-SE width (375px).
//
//	cd e2e && go run -mod=vendor ./cmd/touchaudit
//
// A report, not a gate: inline text links are exempt and findings need
// judgement, so it exits 0 and writes a JSON report.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

const (
	// minTarget is the smallest comfortable tap target, in CSS pixels.
	minTarget = 44
	// iPhone SE, the narrowest layout the app supports.
	viewportWidth  = 375
	viewportHeight = 667
)

// measureScript runs in-page; a round trip per control would take minutes. It
// is async so the sidebar branch can await its slide-in animation.
const measureScript = `(async (min, openSidebar) => {
  if (openSidebar) {
    document.querySelector('.sidebar-toggle')?.click();
    await new Promise((r) => setTimeout(r, 250));
  }
  const sel =
    'button, a[href], select, input:not([type=hidden]), textarea, ' +
    'label:has(input,select,textarea), [role="button"], summary';
  const out = [];
  for (const el of document.querySelectorAll(sel)) {
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') continue;
    if (r.width === 0 || r.height === 0) continue;
    if (r.width >= min && r.height >= min) continue;
    const label =
      el.getAttribute('data-testid') ||
      el.getAttribute('aria-label') ||
      (el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 32);
    // A checkbox or radio inside a label: the label IS the tap target.
    const type = el.getAttribute('type') || '';
    const labelWrapped =
      (type === 'checkbox' || type === 'radio') && !!el.closest('label');
    out.push({
      tag: el.tagName.toLowerCase(),
      type,
      cls: (el.className?.toString() || '').slice(0, 44),
      label,
      w: Math.round(r.width),
      h: Math.round(r.height),
      // Inline links are exempt; .entry-item-title's target is the whole row
      // (click-delegated via installRowClickToOpen).
      inlineText:
        !!el.closest('p, .entry-item-title, .entry-item-meta, .reading-pane-article, .breadcrumb') ||
        labelWrapped,
    });
  }
  return out;
})`

type finding struct {
	Tag        string `json:"tag"`
	Type       string `json:"type"`
	Class      string `json:"cls"`
	Label      string `json:"label"`
	Width      int    `json:"w"`
	Height     int    `json:"h"`
	InlineText bool   `json:"inlineText"`
}

type pageReport struct {
	Page     string    `json:"page"`
	URL      string    `json:"url"`
	Findings []finding `json:"findings"`
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

	report, err := audit(server, browser)
	if err != nil {
		return err
	}
	out := filepath.Join(harness.RepoRoot(), "e2e", "touch-audit-report.json")
	encoded, err := json.MarshalIndent(report, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(out, encoded, 0o644); err != nil {
		return err
	}
	printReport(report, out)
	return nil
}

func printReport(report []pageReport, out string) {
	total, realGaps := 0, 0
	for _, page := range report {
		if len(page.Findings) == 0 {
			fmt.Printf("\n## %s (%s) — OK (all >= %dpx)\n", page.Page, page.URL, minTarget)
			continue
		}
		fmt.Printf("\n## %s (%s) — %d sub-%dpx\n", page.Page, page.URL, len(page.Findings), minTarget)
		for _, hit := range page.Findings {
			total++
			if !hit.InlineText {
				realGaps++
			}
			printFinding(hit)
		}
	}
	fmt.Printf("\n=== %d sub-%dpx hits; %d non-inline (candidate gaps). Report: %s ===\n",
		total, minTarget, realGaps, out)
}

func printFinding(hit finding) {
	kind := ""
	if hit.Type != "" {
		kind = " type=" + hit.Type
	}
	exempt := ""
	if hit.InlineText {
		exempt = "  (inline-text exempt)"
	}
	fmt.Printf("  %3dx%3d  <%s%s> %s  [.%s]%s\n", hit.Width, hit.Height, hit.Tag, kind, hit.Label, hit.Class, exempt)
}

// seedAccount creates an admin with a realistic feed and returns the feed id
// with the username.
func seedAccount(server *harness.Server) (feedID int64, username string, err error) {
	username = "touch-" + harness.RandomSlug()
	if err := harness.NewAPI(server).SetupFirstAccount(username, harness.Password); err != nil {
		return 0, "", err
	}
	seed, err := harness.OpenSeed(server.DBPath)
	if err != nil {
		return 0, "", err
	}
	defer seed.Close()
	userID, err := seed.UserID(username)
	if err != nil {
		return 0, "", err
	}
	categoryID, err := seed.CreateCategory(userID, "Tech")
	if err != nil {
		return 0, "", err
	}
	feedID, err = seed.CreateFeed(categoryID, "https://example.com/feed.xml", "Example Feed")
	if err != nil {
		return 0, "", err
	}
	if _, err := seed.SeedTestEntries(feedID, 12); err != nil {
		return 0, "", err
	}
	if err := seed.ConfigureKagi(userID, "e2e-test-token"); err != nil {
		return 0, "", err
	}
	return feedID, username, seed.MakeAdmin(userID)
}

func signIn(server *harness.Server, browser *harness.Browser, username string) error {
	if err := browser.Goto(server.BaseURL + "/login"); err != nil {
		return err
	}
	if err := browser.Fill("username-input", username); err != nil {
		return err
	}
	if err := browser.Fill("password-input", harness.Password); err != nil {
		return err
	}
	return browser.Submit("login-submit")
}

func audit(server *harness.Server, browser *harness.Browser) ([]pageReport, error) {
	feedID, username, err := seedAccount(server)
	if err != nil {
		return nil, err
	}
	// Sign in at desktop width, then narrow the viewport.
	if err := signIn(server, browser, username); err != nil {
		return nil, err
	}
	if err := browser.SetViewport(viewportWidth, viewportHeight); err != nil {
		return nil, err
	}
	pages := [][2]string{
		{"Unread", "/"},
		{"All Entries", "/entries"},
		{"Starred", "/entries/starred"},
		{"Summarized", "/entries/summarized"},
		{"Feeds", "/feeds"},
		{"Feed edit", fmt.Sprintf("/feeds/%d/edit", feedID)},
		{"Categories", "/categories"},
		{"Import OPML", "/feeds/import"},
		{"Search", "/search?q=test"},
		{"User settings", "/user-settings"},
		{"App settings", "/settings"},
		{"Statistics", "/statistics"},
		{"Admin", "/admin"},
	}
	var report []pageReport
	for index, page := range pages {
		if err := browser.Goto(server.BaseURL + page[1]); err != nil {
			return nil, err
		}
		hits, err := measure(browser, false)
		if err != nil {
			return nil, err
		}
		report = append(report, pageReport{Page: page[0], URL: page[1], Findings: hits})
		// The off-canvas drawer is measured once, on the first page.
		if index != 0 {
			continue
		}
		drawer, err := measure(browser, true)
		if err != nil {
			return nil, err
		}
		report = append(report, pageReport{Page: "Sidebar drawer", URL: page[1], Findings: drawer})
	}
	return report, nil
}

func measure(browser *harness.Browser, openSidebar bool) ([]finding, error) {
	call := fmt.Sprintf("%s(%d, %t)", measureScript, minTarget, openSidebar)
	raw, err := harness.EvalAsync[json.RawMessage](browser, call)
	if err != nil {
		return nil, err
	}
	hits := []finding{}
	return hits, json.Unmarshal(raw, &hits)
}
