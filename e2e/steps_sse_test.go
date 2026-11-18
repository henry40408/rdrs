package e2e

import (
	"fmt"
	"net/http"
	"strconv"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Live updates over SSE; every assertion polls the DOM without navigating.

func registerSSESteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^the entry row shows a pending summary badge$`, func() error {
		return w.expectDisplayed("a pending or processing summary badge", ".summary-badge-pending, .summary-badge-processing")
	})
	// The SSE event swaps #rp-summary-container in place.
	sc.Step(`^without reloading, the reading pane shows the completed summary$`, func() error {
		return harness.Eventually("the reading pane's completed summary", func() (bool, error) {
			container, err := w.browser.CSSOpt("#rp-summary-container")
			if err != nil || container == nil {
				return false, err
			}
			text, err := container.TextContent()
			return strings.Contains(text, "E2E mock summary body."), err
		})
	})
	sc.Step(`^the entry row shows the completed summary badge$`, func() error {
		return w.expectDisplayed("the completed summary badge", `.summary-badge[title="Has Summary"]`)
	})
	sc.Step(`^a background request marks "([^"]*)" as read$`, w.backgroundMarkRead)
	sc.Step(`^within 5 seconds the sidebar unread count decreases by one without a reload$`, w.unreadDecreases)
}

// expectDisplayed waits for an element matching the selector to be displayed.
func (w *world) expectDisplayed(what, selector string) error {
	return harness.Eventually(what, func() (bool, error) {
		element, err := w.browser.CSSOpt(selector)
		if err != nil || element == nil {
			return false, err
		}
		return element.Displayed()
	})
}

// backgroundMarkRead snapshots the unread count and marks read in one step, so
// the snapshot precedes the SSE event.
func (w *world) backgroundMarkRead(title string) error {
	before, err := w.unreadCount()
	if err != nil {
		return err
	}
	w.unreadBefore = before
	entryID, err := w.entryID(title)
	if err != nil {
		return err
	}
	return w.postAsUser(fmt.Sprintf("/entries/%d/read", entryID))
}

// unreadDecreases: the SSE event triggers rdrs-sidebar.refresh(), updating the
// badge in place.
func (w *world) unreadDecreases() error {
	expected := max(w.unreadBefore-1, 0)
	return harness.EventuallyEq("the sidebar unread count", expected, w.unreadCount)
}

func (w *world) unreadCount() (int, error) {
	element, err := w.browser.CSSOpt("#unread-count")
	if err != nil || element == nil {
		return 0, err
	}
	text, err := element.TextContent()
	if err != nil {
		return 0, err
	}
	count, _ := strconv.Atoi(strings.TrimSpace(text))
	return count, nil
}

// postAsUser POSTs out of band, so only the resulting SSE event can update an
// open page. Any non-error status (200 or 303) will do: only the side effect
// matters.
func (w *world) postAsUser(path string) error {
	response, err := w.requestAsUser(http.MethodPost, path)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode >= 400 {
		return fmt.Errorf("POST %s answered %s", path, response.Status)
	}
	return nil
}
