package e2e

import (
	"fmt"
	"math"
	"regexp"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Viewports, the mobile drawer, the statistics chart and the flash banner.

// subpixel: getBoundingClientRect is fractional (44px can read 43.99999); real
// regressions are whole pixels.
const subpixel = 0.5

var viewports = map[string][2]int64{
	"mobile":  {375, 667},
	"tablet":  {768, 1024},
	"desktop": {1280, 800},
	"wide":    {1400, 900},
}

// actionBarControl is one reading-pane action button's label and slot.
type actionBarControl struct {
	Label string
	X     float64
	Width float64
}

func registerResponsiveSteps(sc *godog.ScenarioContext, w *world) {
	registerViewportSteps(sc, w)
	registerNavigationSteps(sc, w)
	registerDrawerSteps(sc, w)
	registerLayoutSteps(sc, w)
	registerChartSteps(sc, w)
	registerBannerSteps(sc, w)
	registerDocumentScrollSteps(sc, w)
}

func registerViewportSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I am viewing on a (\w+) screen$`, func(kind string) error {
		size, ok := viewports[kind]
		if !ok {
			return fmt.Errorf("unknown viewport: %s", kind)
		}
		return w.browser.SetViewport(size[0], size[1])
	})
	sc.Step(`^I have read entries across several days$`, func() error { return w.seedReadHistory(8) })
	sc.Step(`^I have read entries spanning several weeks$`, func() error { return w.seedReadHistory(30) })
}

func (w *world) seedReadHistory(days int) error {
	ids, err := w.seedNamedFeed("Stats Feed", days)
	if err != nil {
		return err
	}
	for day, id := range ids {
		if err := w.seed.MarkRead(id, fmt.Sprintf("-%d days", day)); err != nil {
			return err
		}
	}
	return nil
}

func (w *world) seedNamedFeed(title string, count int) ([]int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return nil, err
	}
	username := w.user.Username
	categoryID, err := w.seed.CreateCategory(userID, "Cat-"+username)
	if err != nil {
		return nil, err
	}
	feedID, err := w.seed.CreateFeed(categoryID, fmt.Sprintf("https://example.com/%s.xml", username), title)
	if err != nil {
		return nil, err
	}
	w.seededEntries, err = w.seed.SeedTestEntries(feedID, count)
	return w.seededEntries, err
}

func registerNavigationSteps(sc *godog.ScenarioContext, w *world) {
	for step, path := range map[string]string{
		"I open the inbox":            "/",
		"I open the categories page":  "/categories",
		"I open the all-entries page": "/entries",
		"I open the feeds page":       "/feeds",
		"I open the import page":      "/feeds/import",
	} {
		sc.Step("^"+regexp.QuoteMeta(step)+"$", func() error { return w.goto_(path) })
	}
	sc.Step(`^I open the statistics page for the "([^"]*)" period$`, func(period string) error {
		return w.goto_("/statistics?period=" + period)
	})
	sc.Step(`^I open the edit page for feed "([^"]*)"$`, w.openFeedEdit)
	sc.Step(`^I expand the "([^"]*)" disclosure$`, w.expandDisclosure)
}

func (w *world) openFeedEdit(feedTitle string) error {
	table, err := w.browser.TestID("feeds-table")
	if err != nil {
		return err
	}
	rows, err := table.FindAll("tr")
	if err != nil {
		return err
	}
	for _, row := range rows {
		text, err := row.TextContent()
		if err != nil || !strings.Contains(text, feedTitle) {
			continue
		}
		link, err := row.LinkNamed("edit")
		if err != nil {
			return err
		}
		if link == nil {
			return fmt.Errorf("the row for %q has no edit link", feedTitle)
		}
		return link.Click()
	}
	return fmt.Errorf("the feeds table has no row for %q", feedTitle)
}

func (w *world) expandDisclosure(label string) error {
	summaries, err := w.browser.CSSAll("summary")
	if err != nil {
		return err
	}
	for _, summary := range summaries {
		text, _ := summary.TextContent()
		if strings.Contains(text, label) {
			return summary.Click()
		}
	}
	return fmt.Errorf("no disclosure is labelled %q", label)
}

func registerDrawerSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I tap the hamburger$`, func() error { return w.browser.ClickCSS(".sidebar-toggle") })
	sc.Step(`^I tap the sidebar close button$`, func() error { return w.browser.ClickCSS(".sidebar-close") })
	// The right edge is clear of the drawer, so it hits the tap-outside close.
	sc.Step(`^I tap outside the sidebar$`, func() error {
		size, err := harness.Eval[[]float64](w.browser, `[window.innerWidth, window.innerHeight]`)
		if err != nil {
			return err
		}
		return w.browser.ClickAt(size[0]-10, size[1]/2)
	})
	sc.Step(`^the sidebar is visible$`, func() error { return w.expectSidebarOpen(true) })
	sc.Step(`^the sidebar is not visible$`, func() error { return w.expectSidebarOpen(false) })
	sc.Step(`^the hamburger button is visible$`, func() error {
		toggle, err := w.browser.CSS(".sidebar-toggle")
		if err != nil {
			return err
		}
		return expectDisplayedElement(toggle, "the hamburger", true)
	})
	sc.Step(`^the sidebar is always-visible$`, w.sidebarAlwaysVisible)
	sc.Step(`^the reading pane is visible on mobile$`, func() error { return w.expectPaneOverlay(true) })
	sc.Step(`^the reading pane overlay is dismissed$`, func() error { return w.expectPaneOverlay(false) })
	sc.Step(`^I tap the reading-pane back button$`, func() error { return w.browser.Click("reading-pane-back") })
}

func expectDisplayedElement(element *harness.Element, what string, expected bool) error {
	shown, err := element.Displayed()
	if err != nil {
		return err
	}
	if shown != expected {
		return fmt.Errorf("%s: displayed is %v, expected %v", what, shown, expected)
	}
	return nil
}

func (w *world) expectSidebarOpen(open bool) error {
	state := map[bool]string{true: "open", false: "closed"}[open]
	return harness.Eventually("the sidebar to be "+state, func() (bool, error) {
		sidebar, err := w.browser.CSSOpt("#sidebar")
		if err != nil {
			return false, err
		}
		if sidebar == nil {
			return !open, nil
		}
		classes, err := sidebar.Classes()
		if err != nil {
			return false, err
		}
		return hasClass(classes, "open") == open, nil
	})
}

func (w *world) sidebarAlwaysVisible() error {
	sidebar, err := w.browser.CSS("#sidebar")
	if err != nil {
		return err
	}
	if err := expectDisplayedElement(sidebar, "the sidebar", true); err != nil {
		return err
	}
	toggle, err := w.browser.CSSOpt(".sidebar-toggle")
	if err != nil || toggle == nil {
		return err
	}
	return expectDisplayedElement(toggle, "the hamburger at this width", false)
}

// expectPaneOverlay: at 1024px and narrower the pane is hidden unless
// .reading-pane-active; both class and visibility are asserted.
func (w *world) expectPaneOverlay(open bool) error {
	return harness.Eventually(fmt.Sprintf("the reading pane overlay open=%v", open), func() (bool, error) {
		pane, err := w.browser.CSSOpt("#reading-pane")
		if err != nil {
			return false, err
		}
		if pane == nil {
			return !open, nil
		}
		classes, err := pane.Classes()
		if err != nil {
			return false, err
		}
		shown, err := pane.Displayed()
		return hasClass(classes, "reading-pane-active") == open && shown == open, err
	})
}

func registerLayoutSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^the entry list pane is at least (\d+)px wide$`, func(minWidth int) error {
		box, err := w.browser.BoundingBox(".list-pane")
		if err != nil {
			return err
		}
		if box.Width < float64(minWidth) {
			return fmt.Errorf("the list pane is %vpx wide", box.Width)
		}
		return nil
	})
	sc.Step(`^the entry list pane is narrower than the viewport$`, func() error {
		viewportWidth, err := harness.Eval[float64](w.browser, `window.innerWidth`)
		if err != nil {
			return err
		}
		box, err := w.browser.BoundingBox(".list-pane")
		if err != nil {
			return err
		}
		if box.Width >= viewportWidth*0.9 {
			return fmt.Errorf("the list pane is %vpx wide in a %vpx viewport", box.Width, viewportWidth)
		}
		return nil
	})
	sc.Step(`^the categories table is shown as cards$`, w.tableAsCards)
	sc.Step(`^the categories table is shown as a table$`, func() error {
		display, err := w.browser.ComputedStyle("table.mobile-cards thead", "display")
		if err == nil && display == "none" {
			err = fmt.Errorf("the table head is still hidden")
		}
		return err
	})
	sc.Step(`^the page has no horizontal scroll$`, func() error {
		overflow, err := harness.Eval[float64](w.browser,
			`document.documentElement.scrollWidth - document.documentElement.clientWidth`)
		if err == nil && overflow > 0 {
			err = fmt.Errorf("the page overflows by %vpx", overflow)
		}
		return err
	})
	sc.Step(`^the entry-row actions are vertically centered on the meta line$`, w.rowActionsCentred)
	sc.Step(`^the entry-list filter bar does not overflow the list pane$`, w.filterBarFits)
	sc.Step(`^the "([^"]*)" control is at least (\d+)px tall$`, func(selector string, min int) error {
		box, err := w.browser.BoundingBox(selector)
		if err == nil && box.Height < float64(min)-subpixel {
			err = fmt.Errorf("%s is %vpx tall, short of %d", selector, box.Height, min)
		}
		return err
	})
	sc.Step(`^the "([^"]*)" control is at least (\d+)px wide$`, func(selector string, min int) error {
		box, err := w.browser.BoundingBox(selector)
		if err == nil && box.Width < float64(min)-subpixel {
			err = fmt.Errorf("%s is %vpx wide, short of %d", selector, box.Width, min)
		}
		return err
	})
	sc.Step(`^the "([^"]*)" element is visible$`, func(selector string) error {
		element, err := w.browser.CSS(selector)
		if err != nil {
			return err
		}
		return expectDisplayedElement(element, selector, true)
	})
	sc.Step(`^I record the reading-pane action bar layout$`, func() error {
		boxes, err := w.actionBarBoxes()
		w.actionBar = boxes
		return err
	})
	sc.Step(`^the reading-pane action bar layout is unchanged$`, w.actionBarUnchanged)
	sc.Step(`^every checkbox hint starts on its own line$`, w.checkboxHintsOnOwnLine)
}

func (w *world) tableAsCards() error {
	display, err := w.browser.ComputedStyle("table.mobile-cards thead", "display")
	if err != nil {
		return err
	}
	if display != "none" {
		return fmt.Errorf("the table head is %q, not hidden", display)
	}
	cell, err := w.browser.CSS("table.mobile-cards td[data-label]")
	if err != nil {
		return err
	}
	return expectDisplayedElement(cell, "the card cells", true)
}

// rowActionsCentred: .rail-actions and .entry-item-meta share grid row 2 and
// must share a vertical centre (an old hand-tuned offset drifted on mobile).
func (w *world) rowActionsCentred() error {
	delta, err := harness.Eval[float64](w.browser, `(() => {
		const row = document.querySelector('[data-testid="entry-item"]');
		const centre = (sel) => {
			const r = row.querySelector(sel).getBoundingClientRect();
			return r.y + r.height / 2;
		};
		return Math.abs(centre('.rail-actions') - centre('.entry-item-meta'));
	})()`)
	if err == nil && delta > 1.5 {
		err = fmt.Errorf("the actions sit %vpx off the meta line", delta)
	}
	return err
}

// filterBarFits: wrapping is by design; the invariant is no horizontal
// overflow.
func (w *world) filterBarFits() error {
	var overflow struct {
		Count            int     `json:"count"`
		HorizontalScroll float64 `json:"horizontalScroll"`
		PastRight        float64 `json:"pastRight"`
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const pane = document.querySelector('.list-pane');
		const paneRight = pane.getBoundingClientRect().right;
		const groups = [...pane.querySelectorAll('.filter-bar > .form-group')];
		return {
			count: groups.length,
			horizontalScroll: pane.scrollWidth - pane.clientWidth,
			pastRight: Math.max(0, ...groups.map((g) => g.getBoundingClientRect().right - paneRight)),
		};
	})()`, &overflow)
	if err != nil {
		return err
	}
	if overflow.Count < 2 {
		return fmt.Errorf("the filter bar renders only %d control(s)", overflow.Count)
	}
	if overflow.PastRight > 1 {
		return fmt.Errorf("a control reaches %vpx past the pane", overflow.PastRight)
	}
	if overflow.HorizontalScroll > 1 {
		return fmt.Errorf("the pane scrolls %vpx horizontally", overflow.HorizontalScroll)
	}
	return nil
}

func (w *world) actionBarBoxes() ([]actionBarControl, error) {
	var boxes []actionBarControl
	err := harness.EvalJSON(w.browser, `[...document.querySelectorAll('.reading-pane-actions .rp-action')]
		.map((b) => {
			const r = b.getBoundingClientRect();
			const l = b.querySelector('.action-label');
			return { Label: l ? l.textContent.trim() : '', X: r.x, Width: r.width };
		})`, &boxes)
	if err == nil && len(boxes) == 0 {
		err = fmt.Errorf("the reading pane has no action bar")
	}
	return boxes, err
}

func describeBar(boxes []actionBarControl) string {
	parts := make([]string, len(boxes))
	for index, box := range boxes {
		parts[index] = fmt.Sprintf("%s @%.1f+%.1f", box.Label, box.X, box.Width)
	}
	return strings.Join(parts, ", ")
}

// actionBarUnchanged: slots must not shift as labels change, or buttons slide
// under the thumb.
func (w *world) actionBarUnchanged() error {
	before := w.actionBar
	if before == nil {
		return fmt.Errorf("no action-bar layout was recorded")
	}
	after, err := w.actionBarBoxes()
	if err != nil {
		return err
	}
	if len(before) != len(after) {
		return fmt.Errorf("the bar had %d controls and now has %d:\n  before: %s\n  after:  %s",
			len(before), len(after), describeBar(before), describeBar(after))
	}
	for index, old := range before {
		moved := math.Abs(old.X - after[index].X)
		resized := math.Abs(old.Width - after[index].Width)
		if moved > subpixel || resized > subpixel {
			return fmt.Errorf("%q moved %.1fpx and resized %.1fpx:\n  before: %s\n  after:  %s",
				old.Label, moved, resized, describeBar(before), describeBar(after))
		}
	}
	return nil
}

// checkboxHintsOnOwnLine covers every checkbox label, so new checkboxes are
// covered. The 44px rule makes labels inline-flex, letting a hint flow inline.
func (w *world) checkboxHintsOnOwnLine() error {
	var rows []struct {
		ID          string  `json:"id"`
		LabelBottom float64 `json:"labelBottom"`
		LabelHeight float64 `json:"labelHeight"`
		HintTop     float64 `json:"hintTop"`
	}
	err := harness.EvalJSON(w.browser, `Array.from(document.querySelectorAll('input[type="checkbox"]'))
		.map((input) => {
			const label = input.closest('label');
			const hint = label && label.nextElementSibling;
			if (!label || !hint) return null;
			const box = label.getBoundingClientRect();
			return { id: input.id, labelBottom: box.bottom, labelHeight: box.height,
				hintTop: hint.getBoundingClientRect().top };
		})
		.filter(Boolean)`, &rows)
	if err != nil {
		return err
	}
	if len(rows) == 0 {
		return fmt.Errorf("no checkbox is followed by a hint: this assertion would pass vacuously")
	}
	for _, row := range rows {
		if row.HintTop < row.LabelBottom-0.5 {
			return fmt.Errorf("%q's hint starts at %v, above the label's bottom edge at %v: it runs alongside the label instead of under it",
				row.ID, row.HintTop, row.LabelBottom)
		}
		if row.LabelHeight < 44 {
			return fmt.Errorf("%q's tap row is only %vpx tall", row.ID, row.LabelHeight)
		}
	}
	return nil
}

func registerChartSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I hover the last daily-read bar$`, func() error {
		bars, err := w.browser.CSSAll(".stats-bar-col")
		if err != nil {
			return err
		}
		if len(bars) == 0 {
			return fmt.Errorf("the chart renders no bars")
		}
		return bars[len(bars)-1].Hover()
	})
	sc.Step(`^I hover daily-read bar number (\d+)$`, func(number int) error {
		bars, err := w.browser.CSSAll(".stats-bar-col")
		if err != nil {
			return err
		}
		if number < 1 || number > len(bars) {
			return fmt.Errorf("the chart has fewer than %d bars", number)
		}
		return bars[number-1].Hover()
	})
	sc.Step(`^the visible daily-read tooltip is within the viewport$`, w.tooltipWithinViewport)
	sc.Step(`^the daily-read chart is visible$`, func() error {
		chart, err := w.browser.CSS(".stats-chart")
		if err != nil {
			return err
		}
		return expectDisplayedElement(chart, "the chart", true)
	})
	sc.Step(`^the daily-read chart has at most (\d+) bars$`, func(max int) error {
		bars, err := w.browser.CSSAll(".stats-bar-col")
		if err != nil {
			return err
		}
		if len(bars) == 0 {
			return fmt.Errorf("the chart renders no bars")
		}
		if len(bars) > max {
			return fmt.Errorf("the chart renders %d bars", len(bars))
		}
		return nil
	})
	sc.Step(`^some daily-read axis labels are hidden$`, w.someLabelsHidden)
	sc.Step(`^the daily-read bars are each at least (\d+)px wide$`, func(min int) error {
		bars, err := w.browser.CSSAll(".stats-bar-col")
		if err != nil {
			return err
		}
		if len(bars) == 0 {
			return fmt.Errorf("the chart renders no bars")
		}
		for index, bar := range bars {
			box, err := bar.Box()
			if err != nil {
				return err
			}
			if box.Width < float64(min) {
				return fmt.Errorf("bar %d is %vpx wide", index, box.Width)
			}
		}
		return nil
	})
}

func (w *world) tooltipWithinViewport() error {
	var rect *struct {
		Left  float64 `json:"left"`
		Right float64 `json:"right"`
		Width float64 `json:"vw"`
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const tip = [...document.querySelectorAll('.stats-bar-tip')]
			.find((t) => getComputedStyle(t).visibility === 'visible');
		if (!tip) return null;
		const r = tip.getBoundingClientRect();
		return { left: r.left, right: r.right, vw: document.documentElement.clientWidth };
	})()`, &rect)
	if err != nil {
		return err
	}
	if rect == nil {
		return fmt.Errorf("no tooltip is visible")
	}
	if rect.Left < -0.5 {
		return fmt.Errorf("the tooltip starts %vpx off the left edge", rect.Left)
	}
	if rect.Right > rect.Width+0.5 {
		return fmt.Errorf("the tooltip reaches %vpx in a %vpx viewport", rect.Right, rect.Width)
	}
	return nil
}

// someLabelsHidden asks each label, which works with scripting off.
func (w *world) someLabelsHidden() error {
	labels, err := w.browser.CSSAll(".stats-bar-label")
	if err != nil {
		return err
	}
	if len(labels) == 0 {
		return fmt.Errorf("the chart renders no axis labels")
	}
	visible := 0
	for _, label := range labels {
		shown, err := label.Displayed()
		if err != nil {
			return err
		}
		if shown {
			visible++
		}
	}
	if visible >= len(labels) {
		return fmt.Errorf("all %d labels are shown, so none were thinned", len(labels))
	}
	return nil
}

func registerBannerSteps(sc *godog.ScenarioContext, w *world) {
	// Calls <rdrs-flash> directly, as the app's JS does.
	sc.Step(`^a flash banner is shown$`, func() error {
		if _, err := harness.Eval[bool](w.browser,
			`(window.flash.show('success', 'Marked older than 1 week entries as read.'), true)`); err != nil {
			return err
		}
		_, err := w.browser.CSS(".banner")
		return err
	})
	sc.Step(`^the flash banner's dot and dismiss are centred on its first line$`, w.bannerDotAndDismissCentred)
	sc.Step(`^the flash banner sits below the hamburger$`, w.bannerBelowHamburger)
	sc.Step(`^the flash banner is vertically centered on a wide touch tablet$`, w.bannerCentredOnTouchTablet)
}

// bannerDotAndDismissCentred: the dot and dismiss are grid items in an
// align-items: start row; both must centre on the first line box.
func (w *world) bannerDotAndDismissCentred() error {
	var measured map[string]float64
	err := harness.EvalJSON(w.browser, `(() => {
		const banner = document.querySelector('.banner');
		const body = banner.querySelector('.banner-body').getBoundingClientRect();
		const line = parseFloat(getComputedStyle(banner).lineHeight);
		const dot = banner.querySelector('.banner-icon').getBoundingClientRect();
		const svg = banner.querySelector('.banner-dismiss svg.ico');
		const box = svg.getBoundingClientRect();
		const use = svg.querySelector('use');
		const ink = (use || svg).getBBox();
		const vb = svg.viewBox.baseVal;
		const scale = use ? 1 : box.height / (vb && vb.height ? vb.height : box.height);
		return {
			lineCentre: body.top + line / 2,
			dotCentre: dot.top + dot.height / 2,
			dismissCentre: box.top + (ink.y + ink.height / 2) * scale,
		};
	})()`, &measured)
	if err != nil {
		return err
	}
	centre := measured["lineCentre"]
	for _, part := range []string{"dotCentre", "dismissCentre"} {
		at, ok := measured[part]
		if !ok {
			return fmt.Errorf("the banner has no %s", part)
		}
		if math.Abs(at-centre) > 0.5 {
			return fmt.Errorf("the banner's %s sits at %v, %+.2fpx off the first line's centre at %v", part, at, at-centre, centre)
		}
	}
	return nil
}

func (w *world) bannerBelowHamburger() error {
	toggle, err := w.browser.BoundingBox(".sidebar-toggle")
	if err != nil {
		return err
	}
	banner, err := w.browser.BoundingBox(".banner")
	if err != nil {
		return err
	}
	if banner.Y < toggle.Y+toggle.Height {
		return fmt.Errorf("the banner overlaps the hamburger (%v vs %v)", banner.Y, toggle.Y+toggle.Height)
	}
	// Full-width, not indented to clear the floating button.
	if banner.X >= toggle.X {
		return fmt.Errorf("the banner is indented to %v, clear of the button at %v", banner.X, toggle.X)
	}
	return nil
}

// bannerCentredOnTouchTablet is iPad landscape: a wide (>1024px) touch
// viewport. (hover: none) makes .banner-dismiss 44px, and align-items: start
// pinned the message above the centred timestamp.
func (w *world) bannerCentredOnTouchTablet() error {
	if err := w.browser.SetViewport(1180, 820); err != nil {
		return err
	}
	if err := w.browser.SetTouch(true); err != nil {
		return err
	}
	if err := w.goto_("/"); err != nil {
		return err
	}
	if _, err := harness.Eval[bool](w.browser, `(window.flash.show('success', 'Marked as unread.'), true)`); err != nil {
		return err
	}
	if _, err := w.browser.CSS(".banner"); err != nil {
		return err
	}
	var m struct {
		Message  float64 `json:"msg"`
		Time     float64 `json:"time"`
		DismissW float64 `json:"dismissW"`
		DismissH float64 `json:"dismissH"`
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const n = document.querySelector('.banner');
		const centreY = (sel) => {
			const r = n.querySelector(sel).getBoundingClientRect();
			return r.y + r.height / 2;
		};
		const d = n.querySelector('.banner-dismiss').getBoundingClientRect();
		return { msg: centreY('.banner-message'), time: centreY('.banner-time'),
			dismissW: d.width, dismissH: d.height };
	})()`, &m)
	if err != nil {
		return err
	}
	if math.Abs(m.Message-m.Time) > 1.5 {
		return fmt.Errorf("the message and timestamp centres are %v and %v", m.Message, m.Time)
	}
	if m.DismissW < 44 || m.DismissH < 44 {
		return fmt.Errorf("the dismiss target is %v×%v", m.DismissW, m.DismissH)
	}
	return nil
}

func registerDocumentScrollSteps(sc *godog.ScenarioContext, w *world) {
	// Above 1024px the split view fits the viewport; an empty reading pane has
	// nothing to scroll, so a drag must not chain to the page (iPad).
	sc.Step(`^the document cannot scroll$`, func() error {
		probe, err := w.documentScrollProbe()
		if err == nil && (probe.HTML != "hidden" || probe.Body != "hidden" || probe.ScrollY != 0) {
			err = fmt.Errorf("the document can still scroll: %+v", probe)
		}
		return err
	})
	// At 1024px and narrower the entry list scrolls with the document.
	sc.Step(`^the document is not locked$`, func() error {
		probe, err := w.documentScrollProbe()
		if err == nil && (probe.HTML == "hidden" || probe.Body == "hidden") {
			err = fmt.Errorf("the document is locked at tablet width: %+v", probe)
		}
		return err
	})
}

type scrollProbe struct {
	HTML    string  `json:"html"`
	Body    string  `json:"body"`
	ScrollY float64 `json:"scrollY"`
}

// documentScrollProbe reads [html, body] computed overflow-y and what a scroll
// attempt achieves.
func (w *world) documentScrollProbe() (scrollProbe, error) {
	var probe scrollProbe
	err := harness.EvalJSON(w.browser, `(() => {
		window.scrollTo(0, 500);
		return { html: getComputedStyle(document.documentElement).overflowY,
			body: getComputedStyle(document.body).overflowY, scrollY: window.scrollY };
	})()`, &probe)
	return probe, err
}
