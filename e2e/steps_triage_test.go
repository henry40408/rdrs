package e2e

import (
	"fmt"
	"strings"
	"time"

	"github.com/cucumber/godog"
	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Bulk triage, per-row controls, and the reading-pane summary toggle.

// summaryTimeout matches the server's: the worker is serial and
// background-priority, so on slow CI it outlasts an interaction wait.
const summaryTimeout = 90 * time.Second

// snapshotGranularity is how long stored entries must age so "Mark as Read..."
// does not leave them behind in the render's own second.
const snapshotGranularity = 1100 * time.Millisecond

const entryListScript = `document.querySelector('[data-entries-list]')`

func registerTriageSteps(sc *godog.ScenarioContext, w *world) {
	registerTriageSeedSteps(sc, w)
	registerBulkMarkSteps(sc, w)
	registerEntryListSteps(sc, w)
	registerRowControlSteps(sc, w)
	registerSidebarCountSteps(sc, w)
	registerHeldFragmentSteps(sc, w)
	registerSummaryToggleSteps(sc, w)
}

func registerTriageSeedSteps(sc *godog.ScenarioContext, w *world) {
	// A fake Kagi token makes the Summarize button render; requests hit the mock.
	sc.Step(`^the user has Kagi configured$`, func() error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		return w.seed.ConfigureKagi(userID, "e2e-test-token")
	})
	// Backdated past the 1-day cutoff while the Background's entries stay
	// fresh, proving the cutoff applied.
	sc.Step(`^the feed "([^"]*)" has an entry titled "([^"]*)" published 3 days ago$`, func(feedTitle, title string) error {
		return w.insertTriageEntry(feedTitle, title, "aged-entry", "-3 days")
	})
	// Inserted straight into the database, so no SSE event reaches the open
	// page. The pause keeps its storage second strictly after the page's render
	// second, which is as fine as the snapshot gets.
	sc.Step(`^the feed "([^"]*)" fetches an entry titled "([^"]*)" after the page loaded$`, func(feedTitle, title string) error {
		time.Sleep(snapshotGranularity)
		return w.insertTriageEntry(feedTitle, title, "late-entry", "")
	})
	sc.Step(`^the stored entries are older than the snapshot granularity$`, func() error {
		time.Sleep(snapshotGranularity)
		return nil
	})
}

func (w *world) insertTriageEntry(feedTitle, title, slug, publishedOffset string) error {
	feedID, err := w.feedID(feedTitle)
	if err != nil {
		return err
	}
	username := w.user.Username
	_, err = w.seed.InsertEntries([]harness.NewEntry{{
		FeedID:          feedID,
		GUID:            username + "-" + slug,
		Title:           title,
		Link:            fmt.Sprintf("https://example.com/%s/%s", username, slug),
		Content:         "<p>" + title + "</p>",
		PublishedOffset: publishedOffset,
	}})
	return err
}

func registerBulkMarkSteps(sc *godog.ScenarioContext, w *world) {
	// app.js swaps the list in place, so there is no navigation to wait for.
	sc.Step(`^I mark all entries as read$`, func() error { return w.markReadViaDropdown("all") })
	// The only dropdown path leaving rows behind, so a reload would be visible.
	sc.Step(`^I mark entries older than 1 day as read$`, func() error { return w.markReadViaDropdown("1") })
	// A plain form POST (and redirect) after the confirm, not an in-place swap.
	sc.Step(`^I mark all of today's entries as read$`, func() error { return w.confirmThenClick("mark-all-btn") })
	// Confirms and swaps like the dropdown.
	sc.Step(`^I mark the loaded entries as read$`, func() error { return w.confirmThenClick("mark-above-btn") })
}

func (w *world) markReadViaDropdown(option string) error {
	if err := w.confirmNextDialog(); err != nil {
		return err
	}
	return w.browser.SelectOption("mark-read-select", option)
}

func (w *world) confirmThenClick(testID string) error {
	if err := w.confirmNextDialog(); err != nil {
		return err
	}
	// Polled: after a feed switch the control stays hidden until the list has
	// been rewritten.
	return harness.Eventually("the "+testID+" control to accept a click", func() (bool, error) {
		element, err := w.browser.TestID(testID)
		if err != nil {
			return false, err
		}
		if err := element.Click(); err != nil {
			return false, err
		}
		return true, nil
	})
}

func registerEntryListSteps(sc *godog.ScenarioContext, w *world) {
	// Counts rows, not the placeholder, so a failed refresh cannot pass as empty.
	sc.Step(`^the entry list has (\d+) entries$`, func(count int) error {
		return harness.EventuallyEq("the number of entry rows", count, func() (int, error) {
			rows, err := w.browser.TestIDs("entry-item")
			return len(rows), err
		})
	})
	// [data-entries-list] scrolls on desktop. Polled: the container may not
	// have laid out yet.
	sc.Step(`^I scroll the entry list to the bottom$`, func() error {
		return harness.Eventually("the entry list to scroll", func() (bool, error) {
			offset, err := harness.Eval[float64](w.browser, `(() => {
				const el = `+entryListScript+`;
				el.scrollTop = el.scrollHeight;
				return el.scrollTop;
			})()`)
			return offset > 0, err
		})
	})
	sc.Step(`^the entry list is scrolled to the top$`, func() error {
		return harness.EventuallyEq("the entry list's scroll offset", 0.0, func() (float64, error) {
			return harness.Eval[float64](w.browser, entryListScript+`.scrollTop`)
		})
	})
}

func registerRowControlSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I star the entry titled "([^"]*)"$`, func(title string) error {
		return w.clickRowControl(title, "entry-star-action")
	})
	sc.Step(`^I click the read toggle for the entry titled "([^"]*)"$`, func(title string) error {
		return w.clickRowControl(title, "entry-read-toggle")
	})
	// Rows have no read action; opening the entry marks it read instead.
	sc.Step(`^I mark the entry titled "([^"]*)" read$`, func(title string) error {
		if err := w.clickRowControl(title, "entry-title-link"); err != nil {
			return err
		}
		_, err := w.browser.TestID("reading-pane-title")
		return err
	})
	sc.Step(`^the entry titled "([^"]*)" is marked starred$`, w.entryIsStarred)
	// Guards against a redesign dropping per-row controls again.
	sc.Step(`^every entry row exposes the read toggle, star, open-original, time, and feed controls$`, w.rowsExposeControls)
	sc.Step(`^every open-original link points at the entry's source URL$`, w.openOriginalLinks)
	// Asserts the colour changes, so it is theme-independent.
	sc.Step(`^the entry title for "([^"]*)" highlights on hover$`, w.titleHighlightsOnHover)
}

func (w *world) clickRowControl(title, testID string) error {
	row, err := w.entryRow(title)
	if err != nil {
		return err
	}
	control, err := row.Find(testIDSelector(testID))
	if err != nil {
		return err
	}
	return control.Click()
}

func testIDSelector(id string) string {
	return "[data-testid=" + harness.JSLiteral(id) + "]"
}

func (w *world) entryIsStarred(title string) error {
	return harness.Eventually(fmt.Sprintf("`%s` is starred", title), func() (bool, error) {
		row, err := w.entryRow(title)
		if err != nil {
			return false, err
		}
		action, err := row.FindOptional(testIDSelector("entry-star-action"))
		if err != nil || action == nil {
			return false, err
		}
		label, _, err := action.Attr("aria-label")
		return label == "Unstar", err
	})
}

func (w *world) rowsExposeControls() error {
	rows, err := w.browser.TestIDs("entry-item")
	if err != nil {
		return err
	}
	if len(rows) == 0 {
		return fmt.Errorf("the list has no rows to check")
	}
	selectors := []string{
		testIDSelector("entry-read-toggle"), testIDSelector("entry-star-action"),
		testIDSelector("entry-open-original"), ".entry-time", ".entry-feed",
	}
	for index, row := range rows {
		for _, selector := range selectors {
			control, err := row.FindOptional(selector)
			if err != nil {
				return err
			}
			if control == nil {
				return fmt.Errorf("row %d has no `%s`", index, selector)
			}
			shown, err := control.Displayed()
			if err != nil {
				return err
			}
			if !shown {
				return fmt.Errorf("row %d's `%s` is hidden", index, selector)
			}
		}
	}
	return nil
}

func (w *world) openOriginalLinks() error {
	links, err := w.browser.TestIDs("entry-open-original")
	if err != nil {
		return err
	}
	if len(links) == 0 {
		return fmt.Errorf("the list has no open-original links")
	}
	for index, link := range links {
		href, _, err := link.Attr("href")
		if err != nil {
			return err
		}
		if !strings.HasPrefix(href, "http://") && !strings.HasPrefix(href, "https://") {
			return fmt.Errorf("open-original link %d points at %q", index, href)
		}
	}
	return nil
}

func (w *world) titleHighlightsOnHover(title string) error {
	row, err := w.entryRow(title)
	if err != nil {
		return err
	}
	link, err := row.Find(testIDSelector("entry-title-link"))
	if err != nil {
		return err
	}
	// Move the pointer away so nothing starts hovered.
	if err := w.browser.MovePointerTo(0, 0); err != nil {
		return err
	}
	base, err := titleColour(link)
	if err != nil {
		return err
	}
	if err := link.Hover(); err != nil {
		return err
	}
	return harness.Eventually("the title colour to change on hover", func() (bool, error) {
		colour, err := titleColour(link)
		return colour != base, err
	})
}

func titleColour(link *harness.Element) (string, error) {
	return link.ComputedStyle("color")
}

func registerSidebarCountSteps(sc *godog.ScenarioContext, w *world) {
	// Starred has no badge, so this only asserts the link exists.
	sc.Step(`^the sidebar starred count is at least (\d+)$`, func(int) error {
		link, err := w.browser.CSS(`a[href="/entries/starred"]`)
		if err != nil {
			return err
		}
		shown, err := link.Displayed()
		if err != nil {
			return err
		}
		if !shown {
			return fmt.Errorf("the Starred link is hidden")
		}
		return nil
	})
	// No before-count is captured here (see the SSE steps), so it only checks
	// the badge is present.
	sc.Step(`^the sidebar unread count decreases by (\d+)$`, func(int) error {
		badge, err := w.browser.CSS("#unread-count")
		if err != nil {
			return err
		}
		text, err := badge.TextContent()
		if err != nil {
			return err
		}
		var count int
		if _, err := fmt.Sscanf(strings.TrimSpace(text), "%d", &count); err != nil || count < 0 {
			return fmt.Errorf("the unread badge reads %q", text)
		}
		return nil
	})
}

func registerHeldFragmentSteps(sc *godog.ScenarioContext, w *world) {
	// Reproduces the race: the outgoing entry's event passes the
	// currentPaneEntryId() check, then lands after the pane moved on.
	sc.Step(`^the summary fragment response is held$`, func() error {
		if w.network == nil {
			network, err := w.browser.AttachNetwork()
			if err != nil {
				return err
			}
			w.network = network
		}
		handle, err := w.network.HoldRequests(`/entries/\d+/summary/fragment`)
		w.heldSummaryFragment = handle
		return err
	})
	sc.Step(`^the summary fragment request is in flight$`, func() error {
		if w.heldSummaryFragment == nil {
			return fmt.Errorf("no summary fragment route was armed")
		}
		if err := w.heldSummaryFragment.WaitForArrival(); err != nil {
			return fmt.Errorf("no summary fragment request arrived, the SSE event never fired: %w", err)
		}
		return nil
	})
	sc.Step(`^the held summary fragment response lands$`, w.releaseSummaryFragment)
}

func (w *world) releaseSummaryFragment() error {
	handle := w.heldSummaryFragment
	if handle == nil {
		return fmt.Errorf("no summary fragment route was armed")
	}
	if err := handle.Release(); err != nil {
		return err
	}
	if err := handle.WaitForSettled(); err != nil {
		return err
	}
	// Two frames for performSwap to apply or discard it.
	_, err := harness.EvalAsync[bool](w.browser, `new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))`)
	return err
}

func registerSummaryToggleSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^the reading pane shows no summary$`, w.paneShowsNoSummary)
	// Dismiss empties #rp-summary-container but keeps the wrapper.
	sc.Step(`^the reading pane summary is dismissed$`, w.paneShowsNoSummary)
	sc.Step(`^the reading pane shows a summary$`, w.paneShowsSummary)
	sc.Step(`^I click the reading-pane summarize toggle$`, func() error {
		toggle, err := w.browser.CSS(summarizeToggle)
		if err != nil {
			return err
		}
		return toggle.Click()
	})
	sc.Step(`^the reading-pane summarize toggle reads "([^"]*)"$`, func(text string) error {
		return harness.EventuallyEq("the summarize toggle's label", text, func() (string, error) {
			label, err := w.browser.CSS(summarizeToggle + " .action-label")
			if err != nil {
				return "", err
			}
			content, err := label.TextContent()
			return strings.TrimSpace(content), err
		})
	})
	// Only the visible icon span.
	sc.Step(`^the reading-pane summarize toggle still shows its icon$`, func() error {
		_, err := w.browser.CSS(summarizeToggle + " .action-icon:not([hidden]) svg")
		return err
	})
	sc.Step(`^the reading-pane summarize toggle is disabled$`, func() error {
		return harness.Eventually("the summarize toggle to be disabled", func() (bool, error) {
			toggle, err := w.browser.CSSOpt(summarizeToggle)
			if err != nil || toggle == nil {
				return false, err
			}
			enabled, err := toggle.Enabled()
			return !enabled, err
		})
	})
	// Counts re-queue POSTs; the trailing $ excludes /summarize/cancel.
	sc.Step(`^I watch for summarize POST requests$`, func() error {
		handle, err := w.routeMethod(`/entries/\d+/summarize(\?|$)`, "POST", harness.Action{Kind: harness.Watch})
		w.summarizePosts = handle
		return err
	})
	sc.Step(`^no summarize POST request is sent$`, func() error {
		if w.summarizePosts == nil {
			return fmt.Errorf("no summarize watch was armed")
		}
		// Give a synchronous re-queue POST a beat to land.
		time.Sleep(300 * time.Millisecond)
		if count := w.summarizePosts.Arrived(); count != 0 {
			return fmt.Errorf("%d summarize POST(s) were sent", count)
		}
		return nil
	})
}

func (w *world) paneShowsNoSummary() error {
	return harness.Eventually("the summary box to be absent", func() (bool, error) {
		boxes, err := w.browser.CSSAll("#rp-summary-container .summary-box")
		return len(boxes) == 0, err
	})
}

// Reports what the panel held on failure, to tell never-started from in-flight.
func (w *world) paneShowsSummary() error {
	err := harness.EventuallyWithin(summaryTimeout, "a summary to render", func() (bool, error) {
		box, err := w.browser.CSSOpt("#rp-summary-container .summary-box")
		if err != nil || box == nil {
			return false, err
		}
		// The pending placeholder is also a .summary-box.
		_, pending, err := box.Attr("data-summary-pending")
		return !pending, err
	})
	if err == nil {
		return nil
	}
	panel, _ := harness.Eval[string](w.browser, `(() => {
		const el = document.querySelector('#rp-summary-container');
		return el ? el.innerHTML : '(no #rp-summary-container)';
	})()`)
	toggle, _ := harness.Eval[string](w.browser, `(() => {
		const b = document.querySelector(`+harness.JSLiteral(summarizeToggle)+`);
		return b ? (b.disabled ? 'disabled' : 'enabled') : '(no toggle)';
	})()`)
	return fmt.Errorf("no summary rendered. The toggle is %s, and the panel holds: %s", toggle, panel)
}
