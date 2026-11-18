package e2e

import (
	"fmt"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Full-text search and the highlight layout.

func registerSearchSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I have a feed with entries titled:$`, w.feedWithTitles)
	// count entries titled "<title> N", newest first, in their own feed.
	sc.Step(`^I have (\d+) entries titled "([^"]*)"$`, w.manyEntriesTitled)
	sc.Step(`^I have an entry titled "([^"]*)"$`, w.singleEntryTitled)
	sc.Step(`^I am on the search page$`, func() error { return w.goto_("/search") })
	sc.Step(`^I use a narrow phone viewport$`, func() error { return w.browser.SetViewport(360, 720) })
	sc.Step(`^I search for "([^"]*)"$`, w.searchFor)
	// Searches on a debounce; later assertions retry long enough.
	sc.Step(`^I type "([^"]*)" into the search box$`, func(term string) error {
		return w.browser.Fill("search-input", term)
	})
	sc.Step(`^I clear the search box$`, w.clearSearchBox)
	sc.Step(`^I load more search results$`, func() error { return w.browser.Click("search-load-more-btn") })
	sc.Step(`^I see the search prompt$`, func() error { return w.browser.ExpectTextSomewhere("Search your library") })
	sc.Step(`^I see search results:$`, func(table *godog.Table) error {
		if err := w.browser.ExpectVisible("search-results"); err != nil {
			return err
		}
		for _, title := range firstColumn(table) {
			if err := w.browser.ExpectTextSomewhere(title); err != nil {
				return err
			}
		}
		return nil
	})
	sc.Step(`^the result count is (\d+)$`, func(count int) error {
		return harness.EventuallyEq("the number of search results", count, func() (int, error) {
			results, err := w.browser.CSSAll(".search-result")
			return len(results), err
		})
	})
	sc.Step(`^the search input is focused$`, func() error {
		focused, err := w.browser.IsFocused("search-input")
		if err == nil && !focused {
			err = fmt.Errorf("the search input does not have focus")
		}
		return err
	})
	sc.Step(`^I see the empty-results message$`, func() error { return w.browser.ExpectVisible("search-empty") })
	sc.Step(`^the highlighted term "([^"]*)" renders on a single line$`, w.highlightSingleLine)
	sc.Step(`^the highlighted title flows as one inline block$`, func() error {
		display, err := w.browser.ComputedStyle(".search-result-title", "display")
		if err == nil && strings.Contains(display, "flex") {
			err = fmt.Errorf("the title is a flex container (%s)", display)
		}
		return err
	})
}

// seedSearchFeed creates a category and feed for search fixtures.
func (w *world) seedSearchFeed(category, feedTitle, urlSuffix string) (int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return 0, err
	}
	categoryID, err := w.seed.CreateCategory(userID, category)
	if err != nil {
		return 0, err
	}
	return w.seed.CreateFeed(categoryID, fmt.Sprintf("https://example.com/%s-%s.xml", w.user.Username, urlSuffix), feedTitle)
}

func (w *world) feedWithTitles(table *godog.Table) error {
	feedID, err := w.seedSearchFeed("Search Category", "Search Feed", "feed")
	if err != nil {
		return err
	}
	username := w.user.Username
	var entries []harness.NewEntry
	for index, title := range firstColumn(table) {
		entries = append(entries, harness.NewEntry{
			FeedID: feedID, GUID: fmt.Sprintf("%s-%d", username, index), Title: title,
			Link:            fmt.Sprintf("https://example.com/%s/%d", username, index),
			Content:         "<p>" + title + "</p>",
			PublishedOffset: fmt.Sprintf("-%d hours", index+1),
		})
	}
	w.seededEntries, err = w.seed.InsertEntries(entries)
	return err
}

func (w *world) manyEntriesTitled(count int, title string) error {
	feedID, err := w.seedSearchFeed("Bulk Category", "Bulk Feed", "bulk")
	if err != nil {
		return err
	}
	username := w.user.Username
	entries := make([]harness.NewEntry, count)
	for index := range entries {
		numbered := fmt.Sprintf("%s %d", title, index)
		entries[index] = harness.NewEntry{
			FeedID: feedID, GUID: fmt.Sprintf("%s-bulk-%d", username, index), Title: numbered,
			Link:            fmt.Sprintf("https://example.com/%s/bulk/%d", username, index),
			Content:         "<p>" + numbered + "</p>",
			PublishedOffset: fmt.Sprintf("-%d minutes", index+1),
		}
	}
	w.seededEntries, err = w.seed.InsertEntries(entries)
	return err
}

func (w *world) singleEntryTitled(title string) error {
	feedID, err := w.seedSearchFeed("Highlight Category", "Highlight Feed", "highlight")
	if err != nil {
		return err
	}
	username := w.user.Username
	w.seededEntries, err = w.seed.InsertEntries([]harness.NewEntry{{
		FeedID: feedID, GUID: username + "-highlight", Title: title,
		Link:            fmt.Sprintf("https://example.com/%s/highlight", username),
		Content:         "<p>" + title + "</p>",
		PublishedOffset: "-1 hours",
	}})
	return err
}

// searchFor: Enter swaps the results in place; the address bar takes the query
// only once the swap has landed, so wait for that before asserting.
func (w *world) searchFor(term string) error {
	if err := w.browser.Fill("search-input", term); err != nil {
		return err
	}
	// The field keeps focus, so a stray re-render cannot swallow the Enter.
	if err := w.browser.PressFocused("Enter"); err != nil {
		return err
	}
	return harness.EventuallyEq("the searched query in the URL", term, func() (string, error) {
		return w.queryParam("q")
	})
}

// clearSearchBox uses backspace: assigning the value dispatches no input
// event, so the debounced listener would never run.
func (w *world) clearSearchBox() error {
	field, err := w.browser.TestID("search-input")
	if err != nil {
		return err
	}
	value, err := field.Value()
	if err != nil {
		return err
	}
	if err := field.Click(); err != nil {
		return err
	}
	if err := w.browser.PressFocused("End"); err != nil {
		return err
	}
	for range []rune(value) {
		if err := w.browser.PressFocused("Backspace"); err != nil {
			return err
		}
	}
	return nil
}

// highlightSingleLine: with word-break: break-word a narrow viewport splits
// the term across lines; box-decoration-break: clone merges the rects, so
// check the height instead.
func (w *world) highlightSingleLine(term string) error {
	const selector = ".search-result-title mark"
	mark, err := w.browser.CSS(selector)
	if err != nil {
		return err
	}
	text, err := mark.TextContent()
	if err != nil {
		return err
	}
	if !strings.Contains(text, term) {
		return fmt.Errorf("the first highlight is not %q", term)
	}
	box, err := w.browser.BoundingBox(selector)
	if err != nil {
		return err
	}
	if box.Height >= 30 {
		return fmt.Errorf("the highlight is %vpx tall, so it wrapped onto a second line", box.Height)
	}
	return nil
}
