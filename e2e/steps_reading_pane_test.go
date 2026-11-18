package e2e

import (
	"fmt"
	"regexp"
	"strings"
	"time"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Opening entries, the reading pane, neighbour navigation and the summary
// panel, including held-response races proving a stale response never wins.

// summarizeToggle is located by data-summary-toggle: its name flips to
// "Dismiss summary", which collides with the summary box's own Dismiss.
const summarizeToggle = ".reading-pane-actions [data-summary-toggle] button"

// Above 1024px the star lives in the toolbar, below it in the action bar (the
// other copy is hidden); every row has a star with the same name.
const (
	paneStarToolbar = ".reading-pane-back [id^='reading-pane-bar-star-form-'] button"
	paneStarActions = ".reading-pane-actions [id^='reading-pane-star-form-'] button"
)

// holdTime is long enough for a second click to land while the first is in
// flight.
const holdTime = 600 * time.Millisecond

// settleTime: pre-fix, the pane flipped back after this delay.
const settleTime = 100 * time.Millisecond

var paneEntryIDPattern = regexp.MustCompile(`/entries/(\d+)/`)

func registerReadingPaneSteps(sc *godog.ScenarioContext, w *world) {
	registerOpenEntrySteps(sc, w)
	registerPaneNavigationSteps(sc, w)
	registerPaneContentSteps(sc, w)
	registerPaneActionSteps(sc, w)
	registerSummaryPanelSteps(sc, w)
	sc.Step(`^every icon beside text is centred on the text$`, w.iconsCentredOnText)
	sc.Step(`^the URL has the \?entry= parameter for "([^"]*)"$`, w.urlHasEntry)
	sc.Step(`^the URL has no \?entry= parameter$`, func() error {
		return harness.EventuallyEq("the ?entry= parameter", "", w.entryParam)
	})
}

func registerOpenEntrySteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I click the entry titled "([^"]*)"$`, w.clickEntry)
	// No pane wait: this response is held, and the next step clicks another
	// entry.
	sc.Step(`^I click the entry titled "([^"]*)" without waiting for the pane$`, w.openEntry)
	// The feed link goes to /feeds/{id}/entries, so it must swap in place like
	// the sidebar.
	sc.Step(`^I click the feed name in the entry titled "([^"]*)"$`, func(title string) error {
		row, err := w.entryRow(title)
		if err != nil {
			return err
		}
		link, err := row.Find(".entry-feed")
		if err != nil {
			return err
		}
		return link.Click()
	})
	sc.Step(`^the fragment response for the entry titled "([^"]*)" is delayed$`, func(title string) error {
		handle, err := w.delayEntryRequest(title, `/entries/%d/fragment`)
		w.delayedFragment = handle
		return err
	})
	sc.Step(`^the fetch full content response for the entry titled "([^"]*)" is delayed$`, func(title string) error {
		handle, err := w.delayEntryRequest(title, `/entries/%d/fetch-full-content`)
		w.delayedFullContent = handle
		return err
	})
	// The (\?|$) keeps the cancel POST, which shares the prefix, fast.
	sc.Step(`^the summarize response for the entry titled "([^"]*)" is delayed$`, func(title string) error {
		handle, err := w.delayEntryRequest(title, `/entries/%d/summarize(\?|$)`)
		w.delayedSummarize = handle
		return err
	})
	sc.Step(`^the delayed fragment response has settled$`, func() error { return settle(w.delayedFragment, "fragment") })
	sc.Step(`^the delayed fetch full content response has settled$`, func() error {
		return settle(w.delayedFullContent, "full-content")
	})
}

func registerPaneNavigationSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I navigate to the "([^"]*)" entry in the reading pane$`, w.navigatePane)
	sc.Step(`^the reading-pane "([^"]*)" button is disabled$`, func(direction string) error {
		return w.expectEnabled(navTestID(direction), false)
	})
	sc.Step(`^the reading-pane "([^"]*)" button is enabled$`, func(direction string) error {
		return w.expectEnabled(navTestID(direction), true)
	})
}

func registerPaneContentSteps(sc *godog.ScenarioContext, w *world) {
	expectText := func(pattern, testID string) {
		sc.Step(pattern, func(text string) error { return w.browser.ExpectText(testID, text) })
	}
	expectText(`^the reading pane shows the title "([^"]*)"$`, "reading-pane-title")
	expectText(`^the reading pane shows the content "([^"]*)"$`, "reading-pane-body")
	expectText(`^the reading pane shows the feed title "([^"]*)"$`, "reading-pane-feed-title")
	sc.Step(`^the reading pane shows a published time$`, func() error {
		return w.browser.ExpectVisible("reading-pane-published-at")
	})
	sc.Step(`^the reading pane shows an image favicon$`, w.paneFavicon)
	sc.Step(`^the reading pane favicon still has its image$`, w.paneFaviconKept)
	sc.Step(`^the reading pane shows the original feed body$`, func() error {
		original := "original"
		return w.browser.ExpectAttr(`[data-testid="reading-pane-body"]`, "data-mode", &original)
	})
	sc.Step(`^the reading pane is empty$`, func() error {
		return harness.Eventually("the reading pane to be empty", func() (bool, error) {
			pane, err := w.browser.CSSOpt("#reading-pane.reading-pane-empty")
			return pane != nil, err
		})
	})
	sc.Step(`^the reading pane shows a broken-image fallback$`, w.brokenImageFallback)
	sc.Step(`^the nested code-block pre has no padding while the outer pre does$`, w.nestedPrePadding)
}

func registerPaneActionSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I click "([^"]*)"$`, w.clickButton)
	sc.Step(`^I click the "([^"]*)" button$`, w.clickButton)
	sc.Step(`^I click the reading-pane star button$`, func() error {
		star, err := w.paneStar()
		if err != nil {
			return err
		}
		return w.browser.ClickCSS(star)
	})
	sc.Step(`^the reading-pane star button reads "([^"]*)"$`, w.paneStarReads)
	sc.Step(`^I see a "([^"]*)" button$`, w.seeButton)
	sc.Step(`^I see a "([^"]*)" fetch full content action$`, w.seeFetchAction)
}

func registerSummaryPanelSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I see the summary error banner$`, func() error {
		_, err := w.browser.CSS("[data-summary-error]")
		return err
	})
	sc.Step(`^I do not see the summary error banner$`, w.noSummaryError)
	sc.Step(`^I see a "([^"]*)" summary action$`, w.seeSummaryAction)
	sc.Step(`^I click the "([^"]*)" summary action$`, w.clickSummaryAction)
}

// clickEntry clicks the title (installRowClickToOpen ignores <a> targets) and
// waits for the pane to lose .reading-pane-empty.
func (w *world) clickEntry(title string) error {
	if err := w.openEntry(title); err != nil {
		return err
	}
	err := harness.Eventually("the reading pane to fill", func() (bool, error) {
		pane, err := w.browser.CSSOpt("#reading-pane:not(.reading-pane-empty)")
		return pane != nil, err
	})
	if err != nil {
		return err
	}
	// The actions wake only once /neighbors resolves, so wait for the
	// toggle's state to stop changing (disabled while a summary is in flight,
	// else enabled). Without Kagi there is no toggle to settle.
	_, err = harness.Settles("the reading pane's actions", 3, func() (string, error) {
		toggle, err := w.browser.CSSOpt(summarizeToggle)
		if err != nil || toggle == nil {
			return "none", err
		}
		enabled, err := toggle.Enabled()
		return fmt.Sprint(enabled), err
	})
	return err
}

func (w *world) openEntry(title string) error {
	row, err := w.entryRow(title)
	if err != nil {
		return err
	}
	link, err := row.Find(`[data-testid="entry-title-link"]`)
	if err != nil {
		return err
	}
	return link.Click()
}

func (w *world) delayEntryRequest(title, pattern string) (*harness.RouteHandle, error) {
	id, err := w.entryID(title)
	if err != nil {
		return nil, err
	}
	return w.delayRequests(fmt.Sprintf(pattern, id), holdTime)
}

func settle(handle *harness.RouteHandle, what string) error {
	if handle == nil {
		return fmt.Errorf("no delayed %s route was armed", what)
	}
	if err := handle.WaitForSettled(); err != nil {
		return err
	}
	time.Sleep(settleTime)
	return nil
}

func navTestID(direction string) string {
	if strings.EqualFold(direction, "next") {
		return "reading-pane-next"
	}
	return "reading-pane-prev"
}

// paneEntryID mirrors app.js's currentPaneEntryId(), via the action forms. A
// mid-swap stale handle reads as "no entry yet", which the caller polls for.
func (w *world) paneEntryID() string {
	form, err := w.browser.CSSOpt(`#reading-pane form[action*="/entries/"]`)
	if err != nil || form == nil {
		return ""
	}
	action, _, err := form.Attr("action")
	if err != nil {
		return ""
	}
	match := paneEntryIDPattern.FindStringSubmatch(action)
	if match == nil {
		return ""
	}
	return match[1]
}

// navigatePane waits for the pane to change entry, so a follow-up cannot race
// this swap.
func (w *world) navigatePane(direction string) error {
	before := w.paneEntryID()
	// Enabled once /neighbors resolves; Click waits for clickable.
	if err := w.browser.Click(navTestID(direction)); err != nil {
		return err
	}
	return harness.Eventually("the pane to show a different entry", func() (bool, error) {
		return w.paneEntryID() != before, nil
	})
}

func (w *world) expectEnabled(id string, expected bool) error {
	state := "disabled"
	if expected {
		state = "enabled"
	}
	return harness.Eventually(fmt.Sprintf("`%s` is %s", id, state), func() (bool, error) {
		button, err := w.browser.TestIDOpt(id)
		if err != nil || button == nil {
			return false, err
		}
		enabled, err := button.Enabled()
		return enabled == expected, err
	})
}

// paneFavicon: the favicon leads the mono dispatch eyebrow.
func (w *world) paneFavicon() error {
	favicon, err := w.browser.CSS(".dispatch-eyebrow img.entry-favicon")
	if err != nil {
		return err
	}
	src, _, err := favicon.Attr("src")
	if err != nil {
		return err
	}
	if !strings.HasSuffix(src, "/icon") {
		return fmt.Errorf("the pane favicon points at %q, not an icon endpoint", src)
	}
	return nil
}

// paneFaviconKept reads once, right after the delayed click: cancelPaneImages()
// has run on the outgoing pane, and a retry would be masked by the next entry
// landing.
func (w *world) paneFaviconKept() error {
	favicon, err := w.browser.CSSOpt(".dispatch-eyebrow img.entry-favicon")
	if err != nil {
		return err
	}
	if favicon == nil {
		return fmt.Errorf("the pane has no favicon at all")
	}
	src, _, err := favicon.Attr("src")
	if err != nil {
		return err
	}
	if src == "" {
		return fmt.Errorf("the reading-pane favicon lost its src during navigation")
	}
	return nil
}

func (w *world) brokenImageFallback() error {
	if _, err := w.browser.CSS(".reading-pane-article .rp-broken-image"); err != nil {
		return err
	}
	caption, err := w.browser.CSS(".rp-broken-cap")
	if err != nil {
		return err
	}
	text, err := caption.TextContent()
	if err != nil {
		return err
	}
	if !strings.Contains(text, "Image unavailable") {
		return fmt.Errorf("the broken-image caption does not say the image is unavailable")
	}
	return nil
}

// nestedPrePadding: Rouge's inner <pre> gets zero padding; the outer keeps its
// padding.
func (w *world) nestedPrePadding() error {
	inner, err := w.browser.CSSAll(".reading-pane-article pre pre")
	if err != nil {
		return err
	}
	if len(inner) == 0 {
		return fmt.Errorf("the article renders no nested <pre> at all")
	}
	innerPad, err := w.browser.ComputedStyle(".reading-pane-article pre pre", "padding-top")
	if err != nil {
		return err
	}
	outerPad, err := w.browser.ComputedStyle(".reading-pane-article pre", "padding-top")
	if err != nil {
		return err
	}
	if innerPad != "0px" {
		return fmt.Errorf("the nested pre keeps %s of padding", innerPad)
	}
	if outerPad == "0px" || outerPad == "" {
		return fmt.Errorf("the outer pre lost its padding (%s)", outerPad)
	}
	return nil
}

// clickButton retries across swaps: the page's own refetches can detach a
// handle between the find and the click.
func (w *world) clickButton(label string) error {
	return harness.DespiteSwaps(fmt.Sprintf("clicking the `%s` button", label), func() error {
		button, err := w.browser.ButtonNamed(label)
		if err != nil {
			return err
		}
		if button == nil {
			return fmt.Errorf("the page has no `%s` button", label)
		}
		return button.ClickWhenReady()
	})
}

// paneStar is the visible copy of the pane's star button.
func (w *world) paneStar() (string, error) {
	wide, err := harness.Eval[bool](w.browser, "window.innerWidth > 1024")
	if err != nil {
		return "", err
	}
	if wide {
		return paneStarToolbar, nil
	}
	return paneStarActions, nil
}

// paneStarReads: the label flips with starred state, which affects layout.
func (w *world) paneStarReads(text string) error {
	return harness.EventuallyEq("the star button's label", text, func() (string, error) {
		star, err := w.paneStar()
		if err != nil {
			return "", err
		}
		label, err := w.browser.CSS(star + " .action-label")
		if err != nil {
			return "", err
		}
		content, err := label.TextContent()
		return strings.TrimSpace(content), err
	})
}

func (w *world) seeButton(label string) error {
	button, err := w.browser.ButtonNamed(label)
	if err != nil {
		return err
	}
	if button == nil {
		return fmt.Errorf("the page has no `%s` button", label)
	}
	shown, err := button.Displayed()
	if err != nil {
		return err
	}
	if !shown {
		return fmt.Errorf("the `%s` button is present but hidden", label)
	}
	return nil
}

func (w *world) seeFetchAction(label string) error {
	action, err := w.browser.CSS(`form[action*="/fetch-full-content"] button`)
	if err != nil {
		return err
	}
	text, err := action.TextContent()
	if err != nil {
		return err
	}
	aria, _, err := action.Attr("aria-label")
	if err != nil {
		return err
	}
	if !strings.Contains(text, label) && !strings.Contains(aria, label) {
		return fmt.Errorf("the fetch action reads %q / %q, not %q", text, aria, label)
	}
	return nil
}

func (w *world) noSummaryError() error {
	return harness.Eventually("the summary error banner to be gone", func() (bool, error) {
		banners, err := w.browser.CSSAll("[data-summary-error]")
		return len(banners) == 0, err
	})
}

func (w *world) seeSummaryAction(label string) error {
	container, err := w.browser.CSS("#rp-summary-container")
	if err != nil {
		return err
	}
	button, err := container.ButtonNamed(label)
	if err != nil {
		return err
	}
	if button == nil {
		return fmt.Errorf("the summary panel has no `%s` action", label)
	}
	shown, err := button.Displayed()
	if err != nil {
		return err
	}
	if !shown {
		return fmt.Errorf("the `%s` summary action is hidden", label)
	}
	return nil
}

func (w *world) clickSummaryAction(label string) error {
	err := harness.DespiteSwaps(fmt.Sprintf("clicking the `%s` summary action", label), func() error {
		container, err := w.browser.CSS("#rp-summary-container")
		if err != nil {
			return err
		}
		button, err := container.ButtonNamed(label)
		if err != nil {
			return err
		}
		if button == nil {
			return fmt.Errorf("the summary panel has no `%s` action", label)
		}
		return button.ClickWhenReady()
	})
	if err != nil {
		return err
	}
	// Waits for the swap, not network idle (sidebar polling never idles).
	return w.noSummaryError()
}

// iconsCentredOnText: icons must centre on the text's cap height, not its box,
// since glyph position depends on font metrics. Swept page-wide so new icons
// are covered; the stacked mobile bar is skipped.
func (w *world) iconsCentredOnText() error {
	type measurement struct {
		Label  string  `json:"label"`
		Offset float64 `json:"offset"`
	}
	var rows []measurement
	err := harness.EvalJSON(w.browser, `(() => {
		const ctx = document.createElement('canvas').getContext('2d');
		const shown = (el) =>
			el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden';
		return Array.from(document.querySelectorAll('svg.ico'))
			.filter(shown)
			.map((svg) => {
				const host = svg.closest('button, a, .summary-error-banner, .sidebar-item');
				if (!host || getComputedStyle(host).flexDirection === 'column') return null;
				const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
				let text = null;
				while (!text && walker.nextNode()) {
					const node = walker.currentNode;
					const parent = node.parentElement;
					if (node.data.trim() && !parent.closest('kbd') && shown(parent)) text = node;
				}
				if (!text) return null;
				// A zero-size inline-block marks the baseline; wrapped with the text so a
				// flex parent does not make it its own item.
				const line = document.createElement('span');
				const probe = document.createElement('span');
				probe.style.display = 'inline-block';
				text.replaceWith(line);
				line.append(text, probe);
				const baseline = probe.getBoundingClientRect().top;
				line.replaceWith(text);
				const style = getComputedStyle(text.parentElement);
				ctx.font = style.fontWeight + ' ' + style.fontSize + ' ' + style.fontFamily;
				const capCentre = baseline - ctx.measureText('H').actualBoundingBoxAscent / 2;
				// Measure ink: <use> reports CSS px, inline icons viewBox units.
				const box = svg.getBoundingClientRect();
				const use = svg.querySelector('use');
				const ink = (use || svg).getBBox();
				const scale = use ? 1 : box.height / (svg.viewBox.baseVal?.height || box.height);
				const iconCentre = box.top + (ink.y + ink.height / 2) * scale;
				return { label: text.data.trim(), offset: iconCentre - capCentre };
			})
			.filter(Boolean);
	})()`, &rows)
	if err != nil {
		return err
	}
	reachedBanner := false
	var off []string
	for _, row := range rows {
		if strings.HasPrefix(row.Label, "Summarization failed") {
			reachedBanner = true
		}
		// 1px slack for rounding; the bug this catches is 2px.
		if row.Offset > 1.0 || row.Offset < -1.0 {
			off = append(off, fmt.Sprintf("`%s` (%+.2fpx)", row.Label, row.Offset))
		}
	}
	if !reachedBanner {
		return fmt.Errorf("the sweep never reached the summary error banner: %v", rows)
	}
	if len(off) > 0 {
		return fmt.Errorf("icons off their text's cap-height centre (positive is low): %s", strings.Join(off, ", "))
	}
	return nil
}

// entryParam is the ?entry= value of the current URL, or "".
func (w *world) entryParam() (string, error) {
	current, err := w.browser.URL()
	if err != nil {
		return "", err
	}
	return current.Query().Get("entry"), nil
}

// urlHasEntry: performSwap pushes or replaces the URL after a pane swap.
func (w *world) urlHasEntry(title string) error {
	id, err := w.entryID(title)
	if err != nil {
		return err
	}
	return harness.EventuallyEq("the ?entry= parameter", fmt.Sprint(id), w.entryParam)
}
