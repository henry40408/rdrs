package e2e

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Feeds, categories, and the flash banner.

var clockTime = regexp.MustCompile(`^\d{2}:\d{2}:\d{2}$`)

func registerOrganizeSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I have a category named "([^"]*)"$`, func(name string) error {
		_, err := w.createCategoryNamed(name)
		return err
	})
	sc.Step(`^the default "([^"]*)" category is removed$`, w.removeDefaultCategory)
	sc.Step(`^I am on the categories page$`, w.onCategoriesPage)
	sc.Step(`^I am on the import OPML page$`, w.onImportPage)
	sc.Step(`^I refresh the feed "([^"]*)"$`, w.refreshFeed)
	sc.Step(`^I edit the feed "([^"]*)" and set its title to "([^"]*)"$`, w.editFeedTitle)
	sc.Step(`^I confirm the next dialog$`, w.confirmNextDialog)
	sc.Step(`^I delete the feed "([^"]*)"$`, w.deleteFeed)
	sc.Step(`^I filter feeds by category "([^"]*)"$`, w.filterFeedsByCategory)
	sc.Step(`^the feeds list is still filtered by category$`, w.feedsListStillFiltered)
	sc.Step(`^I create a category named "([^"]*)"$`, w.createCategory)
	sc.Step(`^I rename category "([^"]*)" to "([^"]*)"$`, w.renameCategory)
	sc.Step(`^I delete category "([^"]*)"$`, w.deleteCategory)
	sc.Step(`^the categories table contains "([^"]*)"$`, w.categoriesTableContains)
	sc.Step(`^the categories table does not contain "([^"]*)"$`, w.categoriesTableLacks)
	sc.Step(`^I import the OPML fixture "([^"]*)"$`, w.importOPML)
	sc.Step(`^the exported OPML contains "([^"]*)"$`, w.exportedOPMLContains)
	sc.Step(`^the actions on a feed row line up on one axis$`, w.feedActionsShareAnAxis)
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

func (w *world) removeDefaultCategory(name string) error {
	userID, err := w.currentUserID()
	if err != nil {
		return err
	}
	return w.seed.DeleteCategory(userID, name)
}

func (w *world) onCategoriesPage() error {
	if err := w.goto_("/categories"); err != nil {
		return err
	}
	return w.browser.ExpectVisible("category-name-input")
}

func (w *world) onImportPage() error {
	if err := w.goto_("/feeds/import"); err != nil {
		return err
	}
	return w.browser.ExpectVisible("opml-file-input")
}

// confirmNextDialog accepts the next window.confirm. It overrides confirm
// rather than answering the dialog, so no alert command races the page.
func (w *world) confirmNextDialog() error {
	_, err := harness.Eval[bool](w.browser, `(() => {
		const original = window.confirm;
		window.confirm = function () { window.confirm = original; return true; };
		return true;
	})()`)
	return err
}

// rowContaining is the first row of a table whose text contains text.
func (w *world) rowContaining(tableID, text string) (*harness.Element, error) {
	table, err := w.browser.TestID(tableID)
	if err != nil {
		return nil, err
	}
	rows, err := table.FindAll("tr")
	if err != nil {
		return nil, err
	}
	for _, row := range rows {
		content, err := row.TextContent()
		if err == nil && strings.Contains(content, text) {
			return row, nil
		}
	}
	return nil, fmt.Errorf("`%s` has no row containing %q", tableID, text)
}

// categoryRow is found by the rename input's value, since names live in
// <input value>, not DOM text.
func (w *world) categoryRow(name string) (*harness.Element, error) {
	return w.browser.CSS(fmt.Sprintf(`[data-testid="categories-table"] tr:has(input[value=%s])`, harness.JSLiteral(name)))
}

func (w *world) refreshFeed(title string) error {
	row, err := w.rowContaining("feeds-table", title)
	if err != nil {
		return err
	}
	button, err := row.Find("form[action$='/refresh'] button")
	if err != nil {
		return err
	}
	return w.browser.SubmitElement(button)
}

func (w *world) editFeedTitle(oldTitle, newTitle string) error {
	row, err := w.rowContaining("feeds-table", oldTitle)
	if err != nil {
		return err
	}
	links, err := row.FindAll("a")
	if err != nil {
		return err
	}
	for _, link := range links {
		text, err := link.Text()
		if err != nil || text != "edit" {
			continue
		}
		if err := w.browser.SubmitElement(link); err != nil {
			return err
		}
		if err := w.browser.Fill("feed-edit-title-input", newTitle); err != nil {
			return err
		}
		return w.browser.Submit("feed-edit-save-btn")
	}
	return fmt.Errorf("the row of %q has no edit link", oldTitle)
}

// deleteFeed needs "I confirm the next dialog" first: the form uses confirm().
func (w *world) deleteFeed(title string) error {
	row, err := w.rowContaining("feeds-table", title)
	if err != nil {
		return err
	}
	button, err := row.Find("button.action-link-danger")
	if err != nil {
		return err
	}
	return w.browser.SubmitElement(button)
}

// filterFeedsByCategory looks the option value up first: labels carry a count
// suffix. The select auto-submits.
func (w *world) filterFeedsByCategory(name string) error {
	value, err := harness.Eval[string](w.browser, `(() => {
		const sel = document.querySelector('#filter-category');
		const opt = Array.from(sel.options).find((o) => o.text.startsWith(`+harness.JSLiteral(name+" (")+`));
		return opt ? opt.value : '';
	})()`)
	if err != nil {
		return err
	}
	if value == "" {
		return fmt.Errorf("no category option starts with `%s (`", name)
	}
	if _, err := harness.Eval[bool](w.browser, `(() => {
		const sel = document.querySelector('#filter-category');
		sel.value = `+harness.JSLiteral(value)+`;
		sel.dispatchEvent(new Event('input', { bubbles: true }));
		sel.dispatchEvent(new Event('change', { bubbles: true }));
		return true;
	})()`); err != nil {
		return err
	}
	return harness.Eventually("the feeds list to filter", func() (bool, error) {
		current, err := w.browser.URL()
		return err == nil && current.Query().Has("category"), err
	})
}

// feedsListStillFiltered: a saved edit lands on the list it was opened from,
// not on the edit page.
func (w *world) feedsListStillFiltered() error {
	return harness.Eventually("the filtered feeds list", func() (bool, error) {
		current, err := w.browser.URL()
		if err != nil {
			return false, err
		}
		return current.Path == "/feeds" && current.Query().Has("category"), nil
	})
}

func (w *world) createCategory(name string) error {
	if err := w.browser.Fill("category-name-input", name); err != nil {
		return err
	}
	return w.browser.Submit("add-category-btn")
}

func (w *world) renameCategory(oldName, newName string) error {
	row, err := w.categoryRow(oldName)
	if err != nil {
		return err
	}
	field, err := row.Find("input[type='text']")
	if err != nil {
		return err
	}
	if err := field.Fill(newName); err != nil {
		return err
	}
	save, err := row.Find("button.cat-rename-save")
	if err != nil {
		return err
	}
	// The save button appears once the field's value differs.
	if err := harness.Eventually("the rename button to show", save.Displayed); err != nil {
		return err
	}
	return w.browser.SubmitElement(save)
}

func (w *world) deleteCategory(name string) error {
	row, err := w.categoryRow(name)
	if err != nil {
		return err
	}
	button, err := row.Find("button.action-link-danger")
	if err != nil {
		return err
	}
	return w.browser.SubmitElement(button)
}

func (w *world) categoryInputs(name string) ([]*harness.Element, error) {
	return w.browser.CSSAll(fmt.Sprintf(`[data-testid="categories-table"] input[value=%s]`, harness.JSLiteral(name)))
}

func (w *world) categoriesTableContains(name string) error {
	return harness.Eventually(fmt.Sprintf("a category named %q", name), func() (bool, error) {
		inputs, err := w.categoryInputs(name)
		return len(inputs) > 0, err
	})
}

func (w *world) categoriesTableLacks(name string) error {
	return harness.Eventually(fmt.Sprintf("the category %q to be gone", name), func() (bool, error) {
		inputs, err := w.categoryInputs(name)
		return len(inputs) == 0, err
	})
}

func (w *world) importOPML(filename string) error {
	path, err := filepath.Abs(filepath.Join("fixtures", filename))
	if err != nil {
		return err
	}
	if _, err := os.Stat(path); err != nil {
		return fmt.Errorf("no OPML fixture at %s", path)
	}
	input, err := w.browser.TestID("opml-file-input")
	if err != nil {
		return err
	}
	if err := input.SetFiles(path); err != nil {
		return err
	}
	return w.browser.Click("opml-import-btn")
}

func (w *world) exportedOPMLContains(text string) error {
	body, err := w.getAsUser("/reader/api/0/subscription/export")
	if err != nil {
		return err
	}
	if !strings.Contains(body, text) {
		return fmt.Errorf("the exported OPML does not mention %q", text)
	}
	return nil
}

// feedActionsShareAnAxis: the four row actions (mixed <a> and form <button>)
// must share one axis. A block form puts its button on a line box whose strut
// varies by font (seen on iPadOS Safari only), so the check reruns with an
// inflated line-height to reproduce it in Chromium.
func (w *world) feedActionsShareAnAxis() error {
	type pass struct {
		Spread float64 `json:"spread"`
		Detail string  `json:"detail"`
	}
	var measured struct {
		Error    string `json:"error"`
		Count    int    `json:"count"`
		Rendered pass   `json:"rendered"`
		Inflated pass   `json:"inflated"`
	}
	err := harness.EvalJSON(w.browser, `(() => {
		const cell = document.querySelector('.feeds-table tbody tr td.actions');
		if (!cell) return { error: 'no actions cell' };
		// Measure a form's button, not the form.
		const items = [...cell.children].map((el) =>
			el.tagName === 'FORM' ? el.querySelector('button') : el);
		if (items.some((el) => !el)) return { error: 'a form has no button' };
		const measure = () => {
			const rows = items.map((el) => {
				const r = el.getBoundingClientRect();
				return { label: el.textContent.trim(), center: r.top + r.height / 2 };
			});
			const centers = rows.map((r) => r.center);
			return {
				spread: Math.max(...centers) - Math.min(...centers),
				detail: rows.map((r) => r.label + '@' + r.center.toFixed(1)).join(' '),
			};
		};
		const rendered = measure();
		// Simulates a taller strut; via CSSOM because CSP refuses injected <style>.
		const original = cell.style.lineHeight;
		cell.style.lineHeight = '3';
		const inflated = measure();
		cell.style.lineHeight = original;
		return { count: items.length, rendered, inflated };
	})()`, &measured)
	if err != nil {
		return err
	}
	if measured.Error != "" {
		return fmt.Errorf("could not measure the actions cell: %s", measured.Error)
	}
	if measured.Count != 4 {
		return fmt.Errorf("expected 4 actions, found %d", measured.Count)
	}
	for name, p := range map[string]pass{"rendered": measured.Rendered, "inflated": measured.Inflated} {
		if p.Spread >= 1.0 {
			return fmt.Errorf("%s: the actions are %.1fpx apart vertically: %s", name, p.Spread, p.Detail)
		}
	}
	return nil
}
