package e2e

import (
	"fmt"
	"strings"
	"time"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Theme, password, retention, timezone, sidebar order and open tracking: the
// settings page and what it changes.

// A form's test ids are shared with the password form, hence the action prefix.
const (
	preferencesForm = `form[action="/user-settings/preferences"]`
	passwordForm    = `form[action="/user-settings/password"]`
)

// reportSettle is how long the page may take to send its browser report.
const reportSettle = 500 * time.Millisecond

func registerSettingsSteps(sc *godog.ScenarioContext, w *world) {
	registerPreferenceSteps(sc, w)
	registerTimezoneSteps(sc, w)
	registerSidebarPrefSteps(sc, w)
	registerOpenTrackingSteps(sc, w)
}

// submitPreferences re-reads nothing: the page was just loaded, so submitting
// writes back the other fields' current values.
func (w *world) submitPreferences() error {
	if err := w.browser.SubmitCSS(preferencesForm + " button[type=submit]"); err != nil {
		return err
	}
	return w.expectPath("/user-settings")
}

func registerPreferenceSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I am on the user settings page$`, func() error {
		if err := w.goto_("/user-settings"); err != nil {
			return err
		}
		return w.browser.ExpectVisible("theme-select")
	})
	sc.Step(`^I switch the theme to "([^"]*)"$`, func(theme string) error {
		if err := w.browser.SelectOption("theme-select", theme); err != nil {
			return err
		}
		return w.submitPreferences()
	})
	// Also used as an And barrier after a When.
	sc.Step(`^the html element has data-theme "([^"]*)"$`, func(value string) error {
		return w.browser.ExpectAttr("html", "data-theme", &value)
	})
	sc.Step(`^the html element has no data-theme attribute$`, func() error {
		return w.browser.ExpectAttr("html", "data-theme", nil)
	})
	// Light mode uses auto smoothing, dark keeps antialiased; the computed value
	// is checkable on any platform though the effect is macOS-only.
	sc.Step(`^the body uses "([^"]*)" font smoothing$`, func(value string) error {
		return harness.EventuallyEq("the body's font smoothing", value, func() (string, error) {
			return harness.Eval[string](w.browser,
				`getComputedStyle(document.body).getPropertyValue('-webkit-font-smoothing')`)
		})
	})
	sc.Step(`^I change my password to "([^"]*)"$`, w.changePassword)
	sc.Step(`^I can sign in with "([^"]*)"$`, w.canSignInWith)
	sc.Step(`^I set entries per page to "([^"]*)"$`, func(count string) error {
		return w.fillAndSavePreferences("entries-per-page", count)
	})
	sc.Step(`^I set the retention period to "([^"]*)" days$`, func(days string) error {
		return w.fillAndSavePreferences("retention-read-days", days)
	})
	sc.Step(`^the retention period field shows "([^"]*)"$`, func(value string) error {
		return w.expectFieldValue("the retention field", "retention-read-days", value)
	})
	sc.Step(`^I save the preferences$`, w.submitPreferences)
}

func (w *world) fillAndSavePreferences(testID, value string) error {
	if err := w.browser.Fill(testID, value); err != nil {
		return err
	}
	return w.submitPreferences()
}

func (w *world) expectFieldValue(what, testID, expected string) error {
	return harness.EventuallyEq(what, expected, func() (string, error) {
		return w.browser.ValueOf(testID)
	})
}

func (w *world) changePassword(newPassword string) error {
	fields := map[string]string{
		"current-password":     w.user.Password,
		"new-password":         newPassword,
		"confirm-new-password": newPassword,
	}
	for testID, value := range fields {
		if err := w.browser.FillCSS(fmt.Sprintf(`%s [data-testid="%s"]`, passwordForm, testID), value); err != nil {
			return err
		}
	}
	if err := w.browser.SubmitCSS(passwordForm + " button[type=submit]"); err != nil {
		return err
	}
	// The server deletes every session and redirects to /login on success.
	if err := w.expectPath("/login"); err != nil {
		return err
	}
	w.user.Password = newPassword
	return nil
}

// canSignInWith: the password change already destroyed the session and landed
// on /login.
func (w *world) canSignInWith(password string) error {
	if err := w.goto_("/login"); err != nil {
		return err
	}
	if err := w.browser.Fill("username-input", w.user.Username); err != nil {
		return err
	}
	if err := w.browser.Fill("password-input", password); err != nil {
		return err
	}
	if err := w.browser.Click("login-submit"); err != nil {
		return err
	}
	return w.expectPath("/")
}

// timezoneAction is the link that suggests zone.
func timezoneAction(zone string) string {
	encoded := strings.ReplaceAll(zone, "/", "%2F")
	return fmt.Sprintf(`[data-testid="flash-action"][href$="timezone=%s"]`, encoded)
}

func registerTimezoneSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^my browser is in the timezone "([^"]*)"$`, func(zone string) error { return w.browser.SetTimezone(zone) })
	// The report is sent after the page loads, and the notice appears on that
	// same page when the reply arrives.
	sc.Step(`^I am offered to review the timezone "([^"]*)"$`, func(zone string) error {
		return harness.Eventually("the browser timezone offer", func() (bool, error) {
			action, err := w.browser.CSSOpt(timezoneAction(zone))
			return action != nil, err
		})
	})
	sc.Step(`^I am not offered to review the timezone$`, func() error { return w.browser.ExpectAbsent("flash-action") })
	sc.Step(`^I watch for browser reports$`, func() error {
		handle, err := w.routeMethod(`/api/user/client-info(\?|$)`, "PUT", harness.Action{Kind: harness.Watch})
		w.clientInfoPuts = handle
		return err
	})
	sc.Step(`^no browser report is sent$`, func() error {
		if w.clientInfoPuts == nil {
			return fmt.Errorf("no report watch was armed")
		}
		// The page sends its report right after loading.
		time.Sleep(reportSettle)
		if count := w.clientInfoPuts.Arrived(); count != 0 {
			return fmt.Errorf("%d browser report(s) were sent", count)
		}
		return nil
	})
	sc.Step(`^I follow the timezone suggestion for "([^"]*)"$`, func(zone string) error {
		if err := w.browser.ClickCSS(timezoneAction(zone)); err != nil {
			return err
		}
		return w.browser.ExpectVisible("timezone-suggestion")
	})
	sc.Step(`^the timezone field shows "([^"]*)"$`, func(zone string) error {
		return w.expectFieldValue("the timezone field", "timezone", zone)
	})
	sc.Step(`^the timezone is marked as not saved$`, func() error { return w.browser.ExpectVisible("timezone-suggestion") })
	sc.Step(`^the timezone is not marked as not saved$`, func() error { return w.browser.ExpectAbsent("timezone-suggestion") })
}

func registerSidebarPrefSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I set the sidebar order to "([^"]*)"$`, func(order string) error {
		if err := w.goto_("/user-settings"); err != nil {
			return err
		}
		if err := w.browser.SelectOption("sidebar-sort", order); err != nil {
			return err
		}
		return w.submitPreferences()
	})
	// Used as both Given and When.
	sc.Step(`^fully-read categories and feeds are hidden$`, func() error {
		if err := w.goto_("/user-settings"); err != nil {
			return err
		}
		if err := w.browser.Check("sidebar-hide-read"); err != nil {
			return err
		}
		return w.submitPreferences()
	})
	sc.Step(`^all entries in feed "([^"]*)" are marked read$`, func(title string) error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		return w.seed.MarkFeedRead(userID, title)
	})
	// Exhaustive, so a row that should be hidden fails here.
	sc.Step(`^the sidebar categories read "([^"]*)"$`, func(expected string) error {
		return w.expectLabels("#sidebar-categories a[data-category-id] .sidebar-item-label", expected)
	})
	sc.Step(`^the sidebar feeds read "([^"]*)"$`, func(expected string) error {
		return w.expectLabels(".sidebar-feed[data-feed-id] .sidebar-item-label", expected)
	})
	sc.Step(`^the sidebar leaves no gap below category "([^"]*)"$`, w.noGapBelowCategory)
	sc.Step(`^the sidebar order field shows "([^"]*)"$`, func(value string) error {
		return w.expectFieldValue("the sidebar order field", "sidebar-sort", value)
	})
	sc.Step(`^the hide-fully-read checkbox is checked$`, func() error {
		checked, err := w.browser.IsChecked("sidebar-hide-read")
		if err == nil && !checked {
			err = fmt.Errorf("the hide-fully-read checkbox is not ticked")
		}
		return err
	})
}

func (w *world) expectLabels(selector, expected string) error {
	names := strings.Split(expected, ",")
	for index := range names {
		names[index] = strings.TrimSpace(names[index])
	}
	return harness.EventuallyEq(fmt.Sprintf("the labels matching %s", selector), strings.Join(names, "\n"), func() (string, error) {
		texts, err := w.browser.TextsOf(selector)
		for index := range texts {
			texts[index] = strings.TrimSpace(texts[index])
		}
		return strings.Join(texts, "\n"), err
	})
}

// noGapBelowCategory: hiding fully-read feeds can empty the open category's
// list, whose margins then showed as a gap. Measured as distance to the next
// row, after the list mounts so it cannot pass before the feeds arrive.
func (w *world) noGapBelowCategory(name string) error {
	err := harness.Eventually("the open category's feed list mounted", func() (bool, error) {
		lists, err := w.browser.CSSAll(".sidebar-feeds[data-category-id]")
		return len(lists) > 0, err
	})
	if err != nil {
		return err
	}
	gap, err := harness.Eval[*int](w.browser, fmt.Sprintf(`(() => {
		const name = %s;
		const rows = [...document.querySelectorAll("#sidebar-categories a[data-category-id]")];
		const at = rows.findIndex(
			(row) => row.querySelector(".sidebar-item-label")?.textContent.trim() === name);
		if (at < 0 || at + 1 >= rows.length) return null;
		return Math.round(rows[at + 1].getBoundingClientRect().top - rows[at].getBoundingClientRect().bottom);
	})()`, harness.JSLiteral(name)))
	if err != nil {
		return err
	}
	if gap == nil {
		return fmt.Errorf("%q and the category below it were not both listed", name)
	}
	if *gap > 1 {
		return fmt.Errorf("the sidebar left a %dpx gap below %q", *gap, name)
	}
	return nil
}

func registerOpenTrackingSteps(sc *godog.ScenarioContext, w *world) {
	// Turns tracking on through the real form.
	sc.Step(`^I turn on open tracking$`, func() error {
		if err := w.goto_("/user-settings"); err != nil {
			return err
		}
		if err := w.browser.Check("pixel-tracking"); err != nil {
			return err
		}
		return w.submitPreferences()
	})
	// Seeded rather than driven; must run before the feed is seeded, since
	// earlier entries carry no pixel.
	sc.Step(`^I have open tracking turned on$`, func() error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		return w.seed.EnablePixelTracking(userID)
	})
	// Marks every entry of the feed opened, as an external client would.
	sc.Step(`^every entry in "([^"]*)" has been opened$`, func(feedTitle string) error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		feedID, err := w.seed.FeedIDByTitle(userID, feedTitle)
		if err != nil {
			return err
		}
		return w.seed.RecordOpensForFeed(userID, feedID)
	})
	sc.Step(`^the feeds table has an open rate column$`, func() error { return w.browser.ExpectTextSomewhere("Open Rate") })
	sc.Step(`^the feeds table has no open rate column$`, func() error {
		headers, err := w.browser.TextsOf("table.feeds-table thead th")
		if err != nil {
			return err
		}
		for _, header := range headers {
			if strings.Contains(header, "Open Rate") {
				return fmt.Errorf("an opted-out reader should not be shown a column of dashes: %q", headers)
			}
		}
		return nil
	})
	sc.Step(`^the reading pane carries a tracking pixel$`, w.paneCarriesPixel)
	sc.Step(`^the reading pane carries no tracking pixel$`, w.paneCarriesNoPixel)
	sc.Step(`^the open rate for "([^"]*)" is "([^"]*)"$`, func(feedTitle, expected string) error {
		actual, err := w.openRateCell(feedTitle)
		if err == nil && !strings.Contains(actual, expected) {
			err = fmt.Errorf("expected the open rate for %q to read %q, got %q", feedTitle, expected, actual)
		}
		return err
	})
	sc.Step(`^the open rate for "([^"]*)" is not reported yet$`, func(feedTitle string) error {
		actual, err := w.openRateCell(feedTitle)
		if err == nil && strings.Contains(actual, "%") {
			err = fmt.Errorf("a feed below the sample floor must not be given a percentage, got %q", actual)
		}
		return err
	})
	sc.Step(`^the statistics page ranks "([^"]*)" above "([^"]*)" by open rate$`, w.statisticsRanks)
}

// pixelSources are the pixel srcs in the rendered article. The sanitiser would
// have proxied a 1x1 image, so a same-origin src proves injection ran
// afterwards.
func (w *world) pixelSources() ([]string, error) {
	return harness.Eval[[]string](w.browser, `Array.from(
		document.querySelectorAll('.reading-pane img, #reading-pane img'))
		.map(i => i.getAttribute('src')).filter(s => s && s.includes('/p/'))`)
}

func (w *world) paneCarriesPixel() error {
	return harness.Eventually("the reading pane to carry a tracking pixel", func() (bool, error) {
		sources, err := w.pixelSources()
		for _, source := range sources {
			if strings.HasPrefix(source, "/p/") && strings.HasSuffix(strings.ToLower(source), ".gif") {
				return true, nil
			}
		}
		return false, err
	})
}

func (w *world) paneCarriesNoPixel() error {
	// Wait for the pane, or this passes on an empty page.
	if err := w.browser.ExpectTextSomewhere("Test Entry 1"); err != nil {
		return err
	}
	sources, err := w.pixelSources()
	if err == nil && len(sources) > 0 {
		err = fmt.Errorf("an opted-out reader's entry must carry no pixel: %q", sources)
	}
	return err
}

func (w *world) openRateCell(feedTitle string) (string, error) {
	row, err := w.browser.RowWithText(feedTitle)
	if err != nil {
		return "", err
	}
	cell, err := row.Find(`[data-testid="feed-open-rate"]`)
	if err != nil {
		return "", fmt.Errorf("no open-rate cell in the row for %q: %w", feedTitle, err)
	}
	return cell.Text()
}

func (w *world) statisticsRanks(first, second string) error {
	if err := w.browser.ExpectTextSomewhere("Feeds by Open Rate"); err != nil {
		return err
	}
	titles, err := w.browser.TextsOf(`[data-testid="stats-open-rate"] .stats-bar-row-header span:first-child`)
	if err != nil {
		return err
	}
	position := func(name string) int {
		for index, title := range titles {
			if strings.TrimSpace(title) == name {
				return index
			}
		}
		return -1
	}
	a, b := position(first), position(second)
	if a < 0 || b < 0 {
		return fmt.Errorf("expected both %q and %q to be listed, got %q", first, second, titles)
	}
	if a >= b {
		return fmt.Errorf("the least-opened feed belongs at the top, got %q", titles)
	}
	return nil
}
