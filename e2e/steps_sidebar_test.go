package e2e

import (
	"fmt"
	"math"
	"regexp"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Sidebar category/feed rows, badges, and in-place reconciliation.

const (
	categoryLinkSelector = "#sidebar-categories a[data-category-id]"
	feedLinkSelector     = ".sidebar-feed[data-feed-id]"
)

var feedIconSource = regexp.MustCompile(`/api/feeds/\d+/icon`)

func registerSidebarSteps(sc *godog.ScenarioContext, w *world) {
	// The category shortcuts no-op on an empty list, so a keypress racing the
	// sidebar fetch would fail for unrelated reasons.
	sc.Step(`^the sidebar has loaded its categories$`, func() error {
		_, err := w.browser.CSS("#sidebar-categories a")
		return err
	})
	sc.Step(`^I click the sidebar category "([^"]*)"$`, func(name string) error {
		return w.clickSidebarLink(categoryLinkSelector, name)
	})
	sc.Step(`^I click the sidebar feed "([^"]*)"$`, func(title string) error {
		return w.clickSidebarLink(feedLinkSelector, title)
	})
	sc.Step(`^the sidebar highlights category "([^"]*)"$`, func(name string) error {
		return w.expectActive(categoryLinkSelector, name)
	})
	// Also used as an And after a When to wait for the sidebar.
	sc.Step(`^the sidebar lists feed "([^"]*)"$`, func(title string) error {
		link, err := w.sidebarLink(feedLinkSelector, title)
		if err != nil {
			return err
		}
		return expectDisplayedElement(link, "feed "+title, true)
	})
	sc.Step(`^the sidebar does not list feed "([^"]*)"$`, func(title string) error {
		return harness.Eventually(fmt.Sprintf("feed %q is gone", title), func() (bool, error) {
			link, err := w.browser.WithTextContaining(feedLinkSelector, title)
			return link == nil, err
		})
	})
	sc.Step(`^the sidebar highlights feed "([^"]*)"$`, func(title string) error {
		return w.expectActive(feedLinkSelector, title)
	})
	sc.Step(`^the sidebar feed "([^"]*)" shows (\d+) unread$`, func(title string, count int) error {
		return w.expectFeedPart(title, ".sidebar-badge", fmt.Sprint(count))
	})
	sc.Step(`^the sidebar feed "([^"]*)" shows its icon$`, w.feedShowsIcon)
	// No-icon fallback: the first letter, uppercased.
	sc.Step(`^the sidebar feed "([^"]*)" shows an initial chip$`, func(title string) error {
		initial := strings.ToUpper(string([]rune(title)[:1]))
		return w.expectFeedPart(title, ".entry-favicon-chip", initial)
	})
	sc.Step(`^I tag the sidebar feed rows$`, w.tagFeedRows)
	sc.Step(`^the sidebar feed rows are still the ones I tagged$`, w.feedRowsStillTagged)
	sc.Step(`^the sidebar shows no unread for category "([^"]*)"$`, w.categoryHasNoUnread)
	sc.Step(`^the sidebar highlights All Entries$`, func() error { return w.expectTestIDActive("nav-entries") })
	sc.Step(`^the sidebar highlights Summarized$`, func() error { return w.expectTestIDActive("nav-summarized") })
	// The Starred item has no data-testid; address it by href.
	sc.Step(`^the sidebar highlights Starred$`, func() error {
		return harness.Eventually("the Starred item is active", func() (bool, error) {
			link, err := w.browser.CSSOpt(`rdrs-sidebar a[href="/entries/starred"]`)
			return w.isActive(link, err)
		})
	})
	sc.Step(`^the sidebar Summarized item shows a count of "([^"]*)"$`, func(count string) error {
		return w.expectTrimmedText("the Summarized count", `[data-testid="nav-summarized"] #summarized-count`, count)
	})
	sc.Step(`^the sidebar Today item shows a count of "([^"]*)"$`, func(count string) error {
		return w.expectTrimmedText("the Today count", `[data-testid="nav-today"] #today-count`, count)
	})
	// An empty badge is hidden, so the link's visible text is just its label.
	sc.Step(`^the sidebar Today item shows no count$`, func() error {
		return w.expectTrimmedText("the Today link text", `[data-testid="nav-today"]`, "Today")
	})
	sc.Step(`^I scroll the sidebar categories to the bottom$`, w.scrollSidebarToBottom)
	sc.Step(`^I click the last sidebar category$`, w.clickLastCategory)
	sc.Step(`^the sidebar is still scrolled where it was$`, w.sidebarStillScrolled)
}

// sidebarLink waits for the first link matching selector whose text contains
// text.
func (w *world) sidebarLink(selector, text string) (*harness.Element, error) {
	var link *harness.Element
	err := harness.Eventually(fmt.Sprintf("a sidebar link named %q", text), func() (bool, error) {
		found, err := w.browser.WithTextContaining(selector, text)
		link = found
		return found != nil, err
	})
	return link, err
}

func (w *world) clickSidebarLink(selector, text string) error {
	return harness.DespiteSwaps("clicking the sidebar link "+text, func() error {
		link, err := w.sidebarLink(selector, text)
		if err != nil {
			return err
		}
		return link.Click()
	})
}

func (w *world) isActive(element *harness.Element, err error) (bool, error) {
	if err != nil || element == nil {
		return false, err
	}
	classes, err := element.Classes()
	return hasClass(classes, "active"), err
}

func (w *world) expectActive(selector, text string) error {
	return harness.Eventually(fmt.Sprintf("%q is active", text), func() (bool, error) {
		link, err := w.browser.WithTextContaining(selector, text)
		return w.isActive(link, err)
	})
}

func (w *world) expectTestIDActive(id string) error {
	return harness.Eventually(fmt.Sprintf("%s is active", id), func() (bool, error) {
		item, err := w.browser.TestIDOpt(id)
		return w.isActive(item, err)
	})
}

func (w *world) expectTrimmedText(what, selector, expected string) error {
	return harness.EventuallyEq(what, expected, func() (string, error) {
		element, err := w.browser.CSS(selector)
		if err != nil {
			return "", err
		}
		text, err := element.TextContent()
		return strings.TrimSpace(text), err
	})
}

// expectFeedPart polls the trimmed text of a part of the feed's row.
func (w *world) expectFeedPart(title, partSelector, expected string) error {
	return harness.EventuallyEq(fmt.Sprintf("feed %q's %s", title, partSelector), expected, func() (string, error) {
		link, err := w.sidebarLink(feedLinkSelector, title)
		if err != nil {
			return "", err
		}
		part, err := link.Find(partSelector)
		if err != nil {
			return "", err
		}
		text, err := part.TextContent()
		return strings.TrimSpace(text), err
	})
}

func (w *world) feedShowsIcon(title string) error {
	link, err := w.sidebarLink(feedLinkSelector, title)
	if err != nil {
		return err
	}
	icon, err := link.FindOptional("img.entry-favicon")
	if err != nil {
		return err
	}
	if icon == nil {
		return fmt.Errorf("feed %q renders no image favicon", title)
	}
	if err := expectDisplayedElement(icon, fmt.Sprintf("feed %q's icon", title), true); err != nil {
		return err
	}
	source, _, err := icon.Attr("src")
	if err != nil {
		return err
	}
	if !feedIconSource.MatchString(source) {
		return fmt.Errorf("feed %q's icon points at %q, not the icon endpoint", title, source)
	}
	return nil
}

// tagFeedRows: a re-rendered row has no data-e2e-tag, so a surviving tag proves
// the row (and favicon) was patched in place; a rebuilt <img> blanks a frame in
// WebKit.
func (w *world) tagFeedRows() error {
	tagged, err := harness.Eval[int](w.browser, `(() => {
		const rows = [...document.querySelectorAll(".sidebar-feed[data-feed-id]")];
		for (const row of rows) {
			row.dataset.e2eTag = "1";
			const icon = row.querySelector(".entry-favicon");
			if (icon) icon.dataset.e2eTag = "1";
		}
		return rows.length;
	})()`)
	if err == nil && tagged == 0 {
		err = fmt.Errorf("there were no sidebar feed rows to tag")
	}
	return err
}

func (w *world) feedRowsStillTagged() error {
	for _, selector := range []string{
		".sidebar-feed[data-feed-id]:not([data-e2e-tag])",
		".sidebar-feed[data-feed-id] .entry-favicon:not([data-e2e-tag])",
	} {
		rebuilt, err := w.browser.CSSAll(selector)
		if err != nil {
			return err
		}
		if len(rebuilt) != 0 {
			return fmt.Errorf("%d node(s) matching %s were rebuilt", len(rebuilt), selector)
		}
	}
	return nil
}

// categoryHasNoUnread: <rdrs-sidebar> re-fetches /api/sidebar after mount; wait
// until the badge is gone so the freshest counts are on screen.
func (w *world) categoryHasNoUnread(name string) error {
	return harness.Eventually(fmt.Sprintf("category %q has no unread badge", name), func() (bool, error) {
		link, err := w.browser.WithTextContaining(`rdrs-sidebar a[href^="/categories/"]`, name)
		if err != nil || link == nil {
			// A link that is gone has no badge either.
			return err == nil, err
		}
		badge, err := link.FindOptional(".sidebar-badge")
		return badge == nil, err
	})
}

// scrollSidebarToBottom guards a regression: a reload or innerHTML re-render
// reset .sidebar-nav scroll to the top, hiding the just-clicked category.
func (w *world) scrollSidebarToBottom() error {
	offset, err := harness.Eval[float64](w.browser, `(() => {
		const nav = document.querySelector('.sidebar-nav');
		nav.scrollTop = nav.scrollHeight;
		window.__rdrsSidebarScroll = nav.scrollTop;
		return nav.scrollTop;
	})()`)
	if err == nil && offset <= 0 {
		err = fmt.Errorf("the sidebar nav must actually overflow for this scenario")
	}
	return err
}

// clickLastCategory clicks the last one, so the click's scroll-into-view does
// not move .sidebar-nav and confound the assertion.
func (w *world) clickLastCategory() error {
	links, err := w.browser.CSSAll(categoryLinkSelector)
	if err != nil {
		return err
	}
	if len(links) == 0 {
		return fmt.Errorf("the sidebar lists no categories")
	}
	label, err := links[len(links)-1].Find(".sidebar-item-label")
	if err != nil {
		return err
	}
	text, err := label.TextContent()
	if err != nil {
		return err
	}
	name := strings.TrimSpace(text)
	if err := links[len(links)-1].Click(); err != nil {
		return err
	}

	return harness.Eventually(fmt.Sprintf("the list header shows %q", name), func() (bool, error) {
		header, err := w.browser.TextOfCSS(".list-pane-header h1")
		return header != nil && strings.Contains(*header, name), err
	})
}

func (w *world) sidebarStillScrolled() error {
	probe, err := harness.Eval[[]float64](w.browser,
		`[window.__rdrsSidebarScroll ?? 0, document.querySelector('.sidebar-nav').scrollTop]`)
	if err != nil {
		return err
	}
	noted, now := probe[0], probe[1]
	if noted <= 0 {
		return fmt.Errorf("the noted offset is gone: the document reloaded")
	}
	// Not exact: the open category's feed list changes the scroll extent. A
	// reload or full re-render lands on 0, which this catches.
	if math.Abs(now-noted) >= 80 {
		return fmt.Errorf("the sidebar moved from %v to %v", noted, now)
	}
	return nil
}
