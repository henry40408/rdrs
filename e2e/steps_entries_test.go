package e2e

import (
	"fmt"
	"net/url"
	"strconv"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Seeding entries, navigating the lists, and asserting on the rows.

// transparentPNG is 1x1, so a favicon renders as an <img>.
var transparentPNG = []byte{
	0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
	0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
	0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8, 0xcf, 0x00, 0x00,
	0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
	0xae, 0x42, 0x60, 0x82,
}

// lineNumberedCode mirrors Rouge's line-numbered output: nested <pre>s in a
// <table>.
const lineNumberedCode = `<div class="highlight"><pre class="highlight"><code>` +
	`<table class="rouge-table"><tbody><tr>` +
	"<td class=\"rouge-gutter\"><pre class=\"lineno\">1\n2\n3\n</pre></td>" +
	"<td class=\"rouge-code\"><pre>line one\nline two\nline three\n</pre></td>" +
	`</tr></tbody></table></code></pre></div>`

func registerEntrySteps(sc *godog.ScenarioContext, w *world) {
	registerEntrySeeding(sc, w)
	registerEntryNavigation(sc, w)
	registerEntryLocation(sc, w)
	registerEntryRows(sc, w)
}

func registerEntrySeeding(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I have a feed "([^"]*)" with (\d+) test entries in category "([^"]*)"$`, w.feedWithEntriesInCategory)
	sc.Step(`^the "([^"]*)" feed has a favicon$`, w.feedHasFavicon)
	// A loopback link is refused by the SSRF guard before any I/O, so the
	// error flash is immediate rather than depending on internet access.
	sc.Step(`^the entry titled "([^"]*)" cannot have its full content fetched$`, func(title string) error {
		return w.withEntry(title, func(id int64) error { return w.seed.SetEntryLink(id, "http://127.0.0.1/blocked") })
	})
	sc.Step(`^the entry titled "([^"]*)" is marked read$`, func(title string) error {
		return w.withEntry(title, func(id int64) error { return w.seed.MarkRead(id, "0 seconds") })
	})
	// Backdated so the read precedes the render snapshot; a same-second read
	// falls inside its >= boundary and flakes.
	sc.Step(`^the entry titled "([^"]*)" was marked read an hour ago$`, func(title string) error {
		return w.withEntry(title, func(id int64) error { return w.seed.MarkRead(id, "-1 hour") })
	})
	sc.Step(`^the entry titled "([^"]*)" is starred$`, func(title string) error {
		return w.withEntry(title, func(id int64) error { return w.seed.MarkStarred(id, "0 seconds") })
	})
	sc.Step(`^the entry titled "([^"]*)" has a summary$`, func(title string) error {
		return w.withEntryAndUser(title, func(id, user int64) error { return w.seed.InsertSummary(id, user, "summary.") })
	})
	sc.Step(`^the entry titled "([^"]*)" has a failed summary$`, func(title string) error {
		return w.withEntryAndUser(title, func(id, user int64) error {
			return w.seed.InsertFailedSummary(id, user, "Kagi API returned 503.")
		})
	})
	sc.Step(`^the entry titled "([^"]*)" has a pending summary$`, func(title string) error {
		return w.withEntryAndUser(title, w.seed.InsertPendingSummary)
	})
	sc.Step(`^the entry titled "([^"]*)" has content with a broken image$`, func(title string) error {
		return w.withEntry(title, func(id int64) error {
			return w.seed.SetEntryContent(id, `<p>x</p><img src="https://images.internal/missing.jpg" alt="Missing diagram">`)
		})
	})
	sc.Step(`^the entry titled "([^"]*)" contains a line-numbered code block$`, func(title string) error {
		return w.withEntry(title, func(id int64) error { return w.seed.SetEntryContent(id, lineNumberedCode) })
	})
	sc.Step(`^all entries in category "([^"]*)" are marked read$`, func(category string) error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		return w.seed.MarkCategoryRead(userID, category)
	})
	sc.Step(`^the feed has (\d+) entries$`, w.feedHasEntries)
	sc.Step(`^I have (\d+) more categories$`, w.moreCategories)
	// With unread entries, so hide-fully-read keeps them and the sidebar scrolls.
	sc.Step(`^I have (\d+) more categories with unread entries$`, w.moreCategoriesWithUnread)
}

func registerEntryNavigation(sc *godog.ScenarioContext, w *world) {
	open := func(pattern, path string) {
		sc.Step(pattern, func() error { return w.goto_(path) })
	}
	open(`^I open the all entries page$`, "/entries")
	open(`^I open the read entries page$`, "/entries/read")
	open(`^I open the starred entries page$`, "/entries/starred")
	open(`^I open the summarized entries page$`, "/entries/summarized")
	sc.Step(`^I open the entries page for feed "([^"]*)"$`, func(title string) error {
		id, err := w.feedID(title)
		return w.gotoIf(err, "/feeds/%d/entries", id)
	})
	sc.Step(`^I open the entries page for category "([^"]*)"$`, func(name string) error {
		id, err := w.categoryID(name)
		return w.gotoIf(err, "/categories/%d/entries", id)
	})
	// ?status=all keeps read rows, so the morph scenarios can check them.
	sc.Step(`^I open the entries page for category "([^"]*)" showing all statuses$`, func(name string) error {
		id, err := w.categoryID(name)
		return w.gotoIf(err, "/categories/%d/entries?status=all", id)
	})
	sc.Step(`^I open the entries page for category "([^"]*)" searching for "([^"]*)"$`, func(name, query string) error {
		id, err := w.categoryID(name)
		return w.gotoIf(err, "/categories/%d/entries?q=%s", id, url.QueryEscape(query))
	})
	sc.Step(`^I open the inbox deep-linked to entry titled "([^"]*)"$`, func(title string) error {
		id, err := w.entryID(title)
		return w.gotoIf(err, "/?entry=%d", id)
	})
	sc.Step(`^I reload the page$`, func() error { return w.browser.Reload() })
	sc.Step(`^I go back in the browser$`, func() error { return w.browser.Back() })
	sc.Step(`^I click the breadcrumb link "([^"]*)"$`, w.clickBreadcrumb)
}

func registerEntryLocation(sc *godog.ScenarioContext, w *world) {
	expect := func(pattern, path string) {
		sc.Step(pattern, func() error { return w.expectPath(path) })
	}
	expect(`^I am on the unread inbox$`, "/")
	expect(`^I am on the all entries page$`, "/entries")
	expect(`^I am on the starred entries page$`, "/entries/starred")
	sc.Step(`^I am on the entries page for feed "([^"]*)"$`, func(title string) error {
		id, err := w.feedID(title)
		return w.expectPathIf(err, "/feeds/%d/entries", id)
	})
	sc.Step(`^I am on the Read filter for feed "([^"]*)"$`, func(title string) error {
		id, err := w.feedID(title)
		return w.expectPathIf(err, "/feeds/%d/entries?status=read", id)
	})
	// Also an And barrier after a When.
	sc.Step(`^I am on the entries page for category "([^"]*)"$`, func(name string) error {
		id, err := w.categoryID(name)
		return w.expectPathIf(err, "/categories/%d/entries", id)
	})
	sc.Step(`^the browser is on "([^"]*)"$`, func(path string) error {
		return harness.Eventually(fmt.Sprintf("the URL to end in %q", path), func() (bool, error) {
			current, err := w.browser.URL()
			return err == nil && strings.HasSuffix(current.Path, path), err
		})
	})
}

func registerEntryRows(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I see (\d+) (?:entries|entry) in the entry list$`, func(count int) error {
		return harness.EventuallyEq("the number of entry rows", count, func() (int, error) {
			rows, err := w.browser.TestIDs("entry-item")
			return len(rows), err
		})
	})
	sc.Step(`^I see more than (\d+) entries in the entry list$`, func(count int) error {
		return harness.Eventually(fmt.Sprintf("more than %d entry rows", count), func() (bool, error) {
			rows, err := w.browser.TestIDs("entry-item")
			return len(rows) > count, err
		})
	})
	sc.Step(`^the first entry is titled "([^"]*)"$`, w.firstEntryTitled)
	sc.Step(`^the entry row for "([^"]*)" shows as read$`, func(title string) error {
		return w.expectRowClass(title, "entry-read", true)
	})
	sc.Step(`^the entry row for "([^"]*)" shows as unread$`, func(title string) error {
		return w.expectRowClass(title, "entry-read", false)
	})
	sc.Step(`^the entry row for "([^"]*)" shows as starred$`, w.rowShowsStarred)
	sc.Step(`^every entry in the list is marked read$`, w.everyEntryRead)
	sc.Step(`^the list header shows "([^"]*)"$`, func(title string) error {
		return harness.Eventually(fmt.Sprintf("the list header shows %q", title), func() (bool, error) {
			header, err := w.browser.CSSOpt(".list-pane-header h1")
			if err != nil || header == nil {
				return false, err
			}
			text, err := header.TextContent()
			return strings.Contains(text, title), err
		})
	})
	sc.Step(`^I see no flash message$`, func() error { return w.browser.ExpectAbsent("flash-message") })
	// A barrier for swaps whose only signal is a toast: the flash follows the
	// pane swap, so later navigation waits for it.
	sc.Step(`^I see a flash message$`, func() error { return w.browser.ExpectVisible("flash-message") })
	sc.Step(`^the feed link does not span the full meta row$`, w.feedLinkShrinkWraps)
}

func (w *world) gotoIf(err error, format string, args ...any) error {
	if err != nil {
		return err
	}
	return w.goto_(fmt.Sprintf(format, args...))
}

func (w *world) expectPathIf(err error, format string, args ...any) error {
	if err != nil {
		return err
	}
	return w.expectPath(fmt.Sprintf(format, args...))
}

func (w *world) entryID(title string) (int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return 0, err
	}
	return w.seed.EntryIDByTitle(userID, title)
}

func (w *world) feedID(title string) (int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return 0, err
	}
	return w.seed.FeedIDByTitle(userID, title)
}

func (w *world) categoryID(name string) (int64, error) {
	userID, err := w.currentUserID()
	if err != nil {
		return 0, err
	}
	return w.seed.CategoryID(userID, name)
}

func (w *world) withEntry(title string, apply func(entryID int64) error) error {
	id, err := w.entryID(title)
	if err != nil {
		return err
	}
	return apply(id)
}

func (w *world) withEntryAndUser(title string, apply func(entryID, userID int64) error) error {
	userID, err := w.currentUserID()
	if err != nil {
		return err
	}
	return w.withEntry(title, func(id int64) error { return apply(id, userID) })
}

func (w *world) feedWithEntriesInCategory(feedTitle string, count int, category string) error {
	categoryID, err := w.createCategoryNamed(category)
	if err != nil {
		return err
	}
	feedURL := fmt.Sprintf("https://example.com/%s-%s.xml", w.user.Username, feedTitle)
	feedID, err := w.seed.CreateFeed(categoryID, feedURL, feedTitle)
	if err != nil {
		return err
	}
	w.seededEntries, err = w.seed.SeedTestEntries(feedID, count)
	return err
}

func (w *world) feedHasFavicon(feedTitle string) error {
	feedID, err := w.feedID(feedTitle)
	if err != nil {
		return err
	}
	return w.seed.InsertIcon(feedID, transparentPNG, "image/png", "https://example.com/icon.png")
}

func (w *world) feedHasEntries(count int) error {
	userID, err := w.currentUserID()
	if err != nil {
		return err
	}
	feedID, err := w.seed.FirstFeedID(userID)
	if err != nil {
		return err
	}
	w.seededEntries, err = w.seed.SeedTestEntries(feedID, count)
	return err
}

func (w *world) moreCategories(count int) error {
	for i := 1; i <= count; i++ {
		if _, err := w.createCategoryNamed("Filler " + strconv.Itoa(i)); err != nil {
			return err
		}
	}
	return nil
}

func (w *world) moreCategoriesWithUnread(count int) error {
	for i := 1; i <= count; i++ {
		categoryID, err := w.createCategoryNamed("Filler " + strconv.Itoa(i))
		if err != nil {
			return err
		}
		feedURL := fmt.Sprintf("https://example.com/%s-filler-%d.xml", w.user.Username, i)
		feedID, err := w.seed.CreateFeed(categoryID, feedURL, fmt.Sprintf("Filler Feed %d", i))
		if err != nil {
			return err
		}
		if _, err := w.seed.SeedTestEntries(feedID, 1); err != nil {
			return err
		}
	}
	return nil
}

func (w *world) clickBreadcrumb(label string) error {
	breadcrumb, err := w.browser.TestID("breadcrumb")
	if err != nil {
		return err
	}
	link, err := breadcrumb.LinkNamed(label)
	if err != nil {
		return err
	}
	if link == nil {
		return fmt.Errorf("the breadcrumb has no `%s` link", label)
	}
	return link.Click()
}

// entryRowOpt is the row containing title, or nil.
func (w *world) entryRowOpt(title string) (*harness.Element, error) {
	rows, err := w.browser.TestIDs("entry-item")
	if err != nil {
		return nil, err
	}
	for _, row := range rows {
		text, err := row.TextContent()
		if err == nil && strings.Contains(text, title) {
			return row, nil
		}
	}
	return nil, nil
}

// entryRow is the row containing title, waiting for it.
func (w *world) entryRow(title string) (*harness.Element, error) {
	var row *harness.Element
	err := harness.Eventually(fmt.Sprintf("an entry row containing %q", title), func() (bool, error) {
		var err error
		row, err = w.entryRowOpt(title)
		return row != nil, err
	})
	return row, err
}

func (w *world) firstEntryTitled(title string) error {
	return harness.Eventually(fmt.Sprintf("the first entry is %q", title), func() (bool, error) {
		first, err := w.browser.TestIDOpt("entry-item")
		if err != nil || first == nil {
			return false, err
		}
		text, err := first.TextContent()
		return strings.Contains(text, title), err
	})
}

func hasClass(classes []string, wanted string) bool {
	return contains(classes, wanted)
}

func (w *world) expectRowClass(title, class string, present bool) error {
	state := "lacks"
	if present {
		state = "has"
	}
	return harness.Eventually(fmt.Sprintf("`%s` %s `%s`", title, state, class), func() (bool, error) {
		row, err := w.entryRowOpt(title)
		if err != nil || row == nil {
			return false, err
		}
		classes, err := row.Classes()
		return hasClass(classes, class) == present, err
	})
}

// rowShowsStarred: starred rows show a star with aria-label="Unstar".
func (w *world) rowShowsStarred(title string) error {
	return harness.Eventually(fmt.Sprintf("`%s` is starred", title), func() (bool, error) {
		row, err := w.entryRowOpt(title)
		if err != nil || row == nil {
			return false, err
		}
		action, err := row.FindOptional(`[data-testid="entry-star-action"]`)
		if err != nil || action == nil {
			return false, err
		}
		label, _, err := action.Attr("aria-label")
		return label == "Unstar", err
	})
}

func (w *world) everyEntryRead() error {
	return harness.Eventually("every row carries `entry-read`", func() (bool, error) {
		rows, err := w.browser.CSSAll("[data-entries-list] [data-entry-row]")
		if err != nil || len(rows) == 0 {
			return false, err
		}
		for _, row := range rows {
			classes, err := row.Classes()
			if err != nil || !hasClass(classes, "entry-read") {
				return false, err
			}
		}
		return true, nil
	})
}

// feedLinkShrinkWraps: a full-width feed link would steal clicks from the
// row's open handler.
func (w *world) feedLinkShrinkWraps() error {
	link, err := w.browser.BoundingBox(`[data-testid="entry-item"] .entry-feed`)
	if err != nil {
		return err
	}
	container, err := w.browser.BoundingBox(`[data-testid="entry-item"] .entry-meta-text`)
	if err != nil {
		return err
	}
	if link.Width >= container.Width {
		return fmt.Errorf("the feed link is %vpx wide inside a %vpx row, so it spans it", link.Width, container.Width)
	}
	return nil
}
