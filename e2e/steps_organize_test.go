package e2e

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Feeds, categories, and the flash banner.

var clockTime = regexp.MustCompile(`^\d{2}:\d{2}:\d{2}$`)

func registerOrganizeSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I have a feed with (\d+) test entries$`, func(count int) error {
		_, err := w.seedFeed("Mobile Feed", count)
		return err
	})
	sc.Step(`^I have a feed "([^"]*)" in category "([^"]*)"$`, w.feedInCategory)
	sc.Step(`^I have a feed from the mock RSS server in category "([^"]*)"$`, w.mockFeedInCategory)
	sc.Step(`^I am (?:on|in) the feeds page$`, w.onFeedsPage)
	sc.Step(`^I add a feed from the mock RSS server under "([^"]*)"$`, w.addMockFeed)
	sc.Step(`^the feeds table contains "([^"]*)"$`, func(text string) error {
		return w.browser.ExpectText("feeds-table", text)
	})
	sc.Step(`^the feeds table does not contain "([^"]*)"$`, w.feedsTableLacks)
	sc.Step(`^I see a success flash "([^"]*)"$`, func(message string) error {
		return w.browser.ExpectText("flash-message", message)
	})
	sc.Step(`^the flash banner shows a timestamp$`, w.flashShowsTimestamp)
}

func (w *world) createCategoryNamed(name string) (int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return 0, err
	}
	return w.seed.CreateCategory(userID, name)
}

func (w *world) feedInCategory(feedTitle, category string) error {
	categoryID, err := w.createCategoryNamed(category)
	if err != nil {
		return err
	}
	feedURL := fmt.Sprintf("https://example.com/%s-%s.xml", w.user.Username, feedTitle)
	_, err = w.seed.CreateFeed(categoryID, feedURL, feedTitle)
	return err
}

func (w *world) mockFeedInCategory(category string) error {
	categoryID, err := w.createCategoryNamed(category)
	if err != nil {
		return err
	}
	_, err = w.seed.CreateFeed(categoryID, w.server.FeedURL+"/feed.xml", "Test Feed")
	return err
}

// onFeedsPage waits for the category dropdown, which reads "Loading" until
// the sidebar data lands.
func (w *world) onFeedsPage() error {
	if err := w.goto_("/feeds"); err != nil {
		return err
	}
	return harness.Eventually("the category dropdown to load", func() (bool, error) {
		select_, err := w.browser.TestIDOpt("feed-category-select")
		if err != nil || select_ == nil {
			return false, err
		}
		text, err := select_.TextContent()
		return !strings.Contains(text, "Loading"), err
	})
}

func (w *world) addMockFeed(category string) error {
	if err := w.browser.Fill("feed-url-input", w.server.FeedURL+"/feed.xml"); err != nil {
		return err
	}
	if err := w.browser.SelectOptionByText("feed-category-select", category); err != nil {
		return err
	}
	return w.browser.Submit("add-feed-btn")
}

func (w *world) feedsTableLacks(text string) error {
	return harness.Eventually(fmt.Sprintf("the feeds table to drop %q", text), func() (bool, error) {
		table, err := w.browser.TestIDOpt("feeds-table")
		if err != nil || table == nil {
			return table == nil, err
		}
		content, err := table.TextContent()
		return !strings.Contains(content, text), err
	})
}

// flashShowsTimestamp: server- and client-rendered flashes must both produce
// a <time> in the viewer's timezone; rdrs-flash.js rewrites the server's UTC
// text.
func (w *world) flashShowsTimestamp() error {
	timeElement, err := w.browser.TestID("flash-time")
	if err != nil {
		return err
	}
	// data-localized marks the rewrite, so the UTC text is never read mid-swap.
	empty := ""
	if err := w.browser.ExpectAttr(`[data-testid="flash-time"]`, "data-localized", &empty); err != nil {
		return err
	}
	text, err := timeElement.TextContent()
	if err != nil {
		return err
	}
	if !clockTime.MatchString(strings.TrimSpace(text)) {
		return fmt.Errorf("the flash timestamp reads %q, not HH:MM:SS", text)
	}
	datetime, _, err := timeElement.Attr("datetime")
	if err != nil {
		return err
	}
	if datetime == "" {
		return fmt.Errorf("the flash timestamp carries no datetime")
	}
	expected, err := harness.Eval[string](w.browser, `new Date(`+harness.JSLiteral(datetime)+`).toLocaleTimeString(undefined, {
		hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })`)
	if err != nil {
		return err
	}
	return harness.EventuallyEq("the localised flash timestamp", expected, func() (string, error) {
		shown, err := w.browser.TextOf("flash-time")
		return strings.TrimSpace(shown), err
	})
}
