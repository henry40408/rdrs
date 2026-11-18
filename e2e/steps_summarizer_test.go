package e2e

import (
	"fmt"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// The standalone Summarizer page.

const summarizerCard = "[data-summarizer-card]"

func registerSummarizerSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I open the Summarizer$`, func() error { return w.goto_("/summarizer") })
	sc.Step(`^I enter these URLs:$`, w.enterURLs)
	sc.Step(`^I submit the summarizer form$`, w.submitSummarizer)
	sc.Step(`^I should see (\d+) summary cards?$`, func(count int) error {
		return harness.EventuallyEq("the number of summary cards", count, func() (int, error) {
			cards, err := w.browser.CSSAll(summarizerCard)
			return len(cards), err
		})
	})
	sc.Step(`^each card resolves to a completed state containing "([^"]*)"$`, w.cardsComplete)
	sc.Step(`^I copy the first summary card$`, w.copyFirstCard)
	sc.Step(`^the first summary card's copy button reads "([^"]*)"$`, func(text string) error {
		return harness.EventuallyEq("the copy button's label", text, func() (string, error) {
			button, err := w.browser.CSS(summarizerCard + " [data-sz-copy]")
			if err != nil {
				return "", err
			}
			return button.TextContent()
		})
	})
	// Scoped to the page content: the sidebar also links /user-settings.
	sc.Step(`^I should see a link to Settings$`, func() error {
		link, err := w.browser.CSS(`.page-content a[href="/user-settings"]`)
		if err != nil {
			return err
		}
		shown, err := link.Displayed()
		if err == nil && !shown {
			err = fmt.Errorf("the Settings link is present but hidden")
		}
		return err
	})
	sc.Step(`^I should not see the summarizer form$`, func() error { return w.browser.ExpectAbsent("summarizer-form") })
}

// firstColumn is the first cell of every row of a single-column table.
func firstColumn(table *godog.Table) []string {
	cells := make([]string, 0, len(table.Rows))
	for _, row := range table.Rows {
		cells = append(cells, row.Cells[0].Value)
	}
	return cells
}

func (w *world) enterURLs(table *godog.Table) error {
	return w.browser.Fill("summarizer-input", strings.Join(firstColumn(table), "\n"))
}

func (w *world) submitSummarizer() error {
	return harness.DespiteSwaps("submitting the summarizer form", func() error {
		form, err := w.browser.TestID("summarizer-form")
		if err != nil {
			return err
		}
		button, err := form.ButtonNamed("Summarize")
		if err != nil {
			return err
		}
		if button == nil {
			return fmt.Errorf("the summarizer form has no Summarize button")
		}
		return button.ClickWhenReady()
	})
}

// cardsComplete waits on each card: they run serially and the mock upstream is
// delayed.
func (w *world) cardsComplete(text string) error {
	cards, err := w.browser.CSSAll(summarizerCard)
	if err != nil {
		return err
	}
	for index := range cards {
		selector := fmt.Sprintf("%s:nth-of-type(%d)", summarizerCard, index+1)
		completed := "completed"
		if err := w.browser.ExpectAttr(selector, "data-state", &completed); err != nil {
			return err
		}
		body := selector + " [data-sz-body]"
		err := harness.Eventually(fmt.Sprintf("card %d contains %q", index, text), func() (bool, error) {
			element, err := w.browser.CSSOpt(body)
			if err != nil || element == nil {
				return false, err
			}
			content, err := element.TextContent()
			return strings.Contains(content, text), err
		})
		if err != nil {
			return err
		}
	}
	return nil
}

func (w *world) copyFirstCard() error {
	// Headless browsers reject clipboard.writeText without the permission.
	if err := w.browser.GrantClipboard(); err != nil {
		return err
	}
	return harness.DespiteSwaps("copying the first summary card", func() error {
		card, err := w.browser.CSS(summarizerCard)
		if err != nil {
			return err
		}
		button, err := card.ButtonNamed("Copy summary")
		if err != nil {
			return err
		}
		if button == nil {
			return fmt.Errorf("the card has no copy button")
		}
		return button.ClickWhenReady()
	})
}
