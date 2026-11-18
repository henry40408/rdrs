package e2e

import (
	"fmt"
	"math"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// The entry list's own search drawer.

const (
	scopedToggle = `[data-testid="scoped-search-toggle"]`
	scopedInput  = `[data-testid="scoped-search-input"]`
	scopedClose  = `[data-testid="scoped-search-close"]`
)

func registerScopedSearchSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^a category "([^"]*)" containing entries titled "([^"]*)" and "([^"]*)"$`, w.categoryWithEntries)
	sc.Step(`^I open the scoped search box$`, func() error {
		if err := w.browser.Click("scoped-search-toggle"); err != nil {
			return err
		}
		return harness.Eventually("the scoped search input to take focus", func() (bool, error) {
			return w.browser.IsFocused("scoped-search-input")
		})
	})
	sc.Step(`^I close the scoped search box$`, func() error { return w.browser.Click("scoped-search-close") })
	sc.Step(`^the scoped search box is open$`, func() error { return w.scopedSearchState(true) })
	sc.Step(`^the scoped search box is closed$`, func() error { return w.scopedSearchState(false) })
	// Auto-submits on a 250 ms debounce; later assertions retry long enough.
	sc.Step(`^I type "([^"]*)" into the scoped search box$`, func(term string) error {
		return w.browser.Fill("scoped-search-input", term)
	})
	sc.Step(`^I clear the scoped search box$`, w.clearScopedSearch)

	sc.Step(`^the search toggle shares a row with the page title$`, w.toggleSharesRowWithTitle)
	sc.Step(`^the search close button is as tall as the search box$`, func() error {
		return w.expectSameHeight(scopedClose, scopedInput)
	})
	sc.Step(`^the scoped search box shares its midline with the hamburger$`, w.drawerSharesHamburgerMidline)

	sc.Step(`^the mark-above button is hidden$`, func() error { return w.browser.ExpectAbsent("mark-above-btn") })
	sc.Step(`^the mark-above button is shown$`, func() error { return w.browser.ExpectVisible("mark-above-btn") })
	// Arms the confirm override and clicks in one step; a separate step races
	// the native submit.
	sc.Step(`^I mark matching entries as read$`, func() error {
		if err := w.confirmNextDialog(); err != nil {
			return err
		}
		return w.browser.Click("mark-matching-btn")
	})

	sc.Step(`^the entry list shows "([^"]*)"$`, func(title string) error {
		return harness.Eventually(fmt.Sprintf("the list to show %q", title), func() (bool, error) {
			row, err := w.entryRowOpt(title)
			return row != nil, err
		})
	})
	sc.Step(`^the entry list does not show "([^"]*)"$`, w.listDoesNotShow)
	// Redirects to the same unread q= URL, where read entries drop out.
	sc.Step(`^"([^"]*)" is no longer in the unread list$`, w.listDoesNotShow)

	// Polled: replaceState runs after the debounced swap. Also an And barrier.
	sc.Step(`^the URL has the "([^"]*)" query parameter set to "([^"]*)"$`, func(key, value string) error {
		return harness.EventuallyEq(fmt.Sprintf("the %q query parameter", key), value, func() (string, error) {
			return w.queryParam(key)
		})
	})
	sc.Step(`^the URL has no "([^"]*)" query parameter$`, func(key string) error {
		return harness.EventuallyEq(fmt.Sprintf("the %q query parameter", key), "", func() (string, error) {
			return w.queryParam(key)
		})
	})

	sc.Step(`^I open the scoped search help$`, func() error { return w.browser.Click("scoped-search-help") })
	sc.Step(`^I open the search page help$`, func() error { return w.browser.Click("search-help") })
	sc.Step(`^the scoped search box reports a syntax error$`, w.scopedSearchReportsError)
	sc.Step(`^the help overlay shows the search syntax tab$`, w.helpShowsSearchSyntax)
}

func (w *world) categoryWithEntries(category, titleA, titleB string) error {
	userID, err := w.currentUserID()
	if err != nil {
		return err
	}
	username := w.user.Username
	categoryID, err := w.seed.CreateCategory(userID, category)
	if err != nil {
		return err
	}
	feedID, err := w.seed.CreateFeed(categoryID, fmt.Sprintf("https://example.com/%s-scoped-search.xml", username), category+" Feed")
	if err != nil {
		return err
	}
	entry := func(slug, title, offset string) harness.NewEntry {
		return harness.NewEntry{
			FeedID: feedID, GUID: username + "-scoped-" + slug, Title: title,
			Link:            fmt.Sprintf("https://example.com/%s/scoped-%s", username, slug),
			Content:         "<p>" + title + "</p>",
			PublishedOffset: offset,
		}
	}
	w.seededEntries, err = w.seed.InsertEntries([]harness.NewEntry{
		entry("a", titleA, "-1 hours"),
		entry("b", titleB, "-2 hours"),
	})
	return err
}

// scopedSearchState: the collapsed drawer keeps the input in the DOM, just not
// visible.
func (w *world) scopedSearchState(open bool) error {
	expanded := "false"
	if open {
		expanded = "true"
		if err := w.browser.ExpectVisible("scoped-search-input"); err != nil {
			return err
		}
	} else if err := w.browser.ExpectHidden("scoped-search-input"); err != nil {
		return err
	}
	return w.browser.ExpectAttr(scopedToggle, "aria-expanded", &expanded)
}

// clearScopedSearch uses backspace: assigning the value dispatches no input
// event, so the debounced listener would never run.
func (w *world) clearScopedSearch() error {
	field, err := w.browser.TestID("scoped-search-input")
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

// toggleSharesRowWithTitle: in the filter bar the toggle once left a lone chip
// on its own row on views without selects.
func (w *world) toggleSharesRowWithTitle() error {
	toggle, err := w.browser.BoundingBox(scopedToggle)
	if err != nil {
		return err
	}
	title, err := w.browser.BoundingBox(".list-pane-header h1")
	if err != nil {
		return err
	}
	delta := math.Abs((toggle.Y + toggle.Height/2) - (title.Y + title.Height/2))
	if delta >= 2 {
		return fmt.Errorf("search toggle midline is %vpx from the title's (toggle y=%v h=%v, title y=%v h=%v)",
			delta, toggle.Y, toggle.Height, title.Y, title.Height)
	}
	return nil
}

// expectSameHeight is polled: the drawer grows over a 0.16s transition.
func (w *world) expectSameHeight(one, other string) error {
	heights := func() (float64, float64, error) {
		first, err := w.browser.BoundingBox(one)
		if err != nil {
			return 0, 0, err
		}
		second, err := w.browser.BoundingBox(other)
		return first.Height, second.Height, err
	}
	err := harness.Eventually(fmt.Sprintf("%s and %s to match in height", one, other), func() (bool, error) {
		first, second, err := heights()
		return math.Abs(first-second) < 1, err
	})
	if err == nil {
		return nil
	}
	first, second, heightErr := heights()
	if heightErr != nil {
		return heightErr
	}
	return fmt.Errorf("%s is %vpx tall and %s is %vpx", one, first, other, second)
}

// drawerSharesHamburgerMidline: on mobile only the row padding keeps the two
// midlines aligned. Polled through the expand transition.
func (w *world) drawerSharesHamburgerMidline() error {
	return harness.Eventually("the drawer and hamburger midlines to agree", func() (bool, error) {
		input, err := w.browser.BoundingBox(scopedInput)
		if err != nil {
			return false, err
		}
		toggle, err := w.browser.BoundingBox(".sidebar-toggle")
		if err != nil {
			return false, err
		}
		return math.Abs((input.Y+input.Height/2)-(toggle.Y+toggle.Height/2)) < 1, nil
	})
}

func (w *world) listDoesNotShow(title string) error {
	return harness.Eventually(fmt.Sprintf("the list to drop %q", title), func() (bool, error) {
		row, err := w.entryRowOpt(title)
		return row == nil, err
	})
}

// queryParam is "" when the parameter is absent.
func (w *world) queryParam(key string) (string, error) {
	current, err := w.browser.URL()
	if err != nil {
		return "", err
	}
	return current.Query().Get(key), nil
}

func (w *world) scopedSearchReportsError() error {
	err := harness.Eventually("the syntax error banner under the search box", func() (bool, error) {
		visible, err := w.browser.IsVisible("scoped-search-error")
		if err != nil || !visible {
			return false, err
		}
		text, err := w.browser.TextOf("scoped-search-error")
		return strings.Contains(text, "Search syntax error"), err
	})
	if err != nil {
		return err
	}
	// The input mirrors it, for assistive tech and the red border.
	invalid := "true"
	return w.browser.ExpectAttr(scopedInput, "aria-invalid", &invalid)
}

// helpShowsSearchSyntax probes by script: the overlay's content is in a shadow
// root.
func (w *world) helpShowsSearchSyntax() error {
	if err := w.browser.ExpectVisible("kb-help"); err != nil {
		return err
	}
	var probe struct {
		Selected string `json:"selected"`
		Visible  bool   `json:"visible"`
		Text     string `json:"text"`
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const root = document.querySelector('rdrs-kb-help').shadowRoot;
		return {
			selected: root.getElementById('tab-syntax').getAttribute('aria-selected'),
			visible: !root.getElementById('syntax').hidden,
			text: root.getElementById('syntax').textContent,
		};
	})()`, &probe)
	if err != nil {
		return err
	}
	if probe.Selected != "true" || !probe.Visible {
		return fmt.Errorf("the Search syntax tab is not the one showing: %+v", probe)
	}
	if !strings.Contains(probe.Text, "is:starred") {
		return fmt.Errorf("the Search syntax tab lacks the syntax reference: %+v", probe)
	}
	return nil
}
