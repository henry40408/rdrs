package e2e

import (
	"fmt"
	"math"
	"strings"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

// Keyboard shortcuts, selection, the go-to hint and the help overlay.

func registerKeyboardSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I press the "([^"]*)" key$`, func(key string) error { return w.browser.Press(key) })
	sc.Step(`^I press the "([^"]*)" key without refocusing$`, func(key string) error { return w.browser.PressFocused(key) })

	sc.Step(`^the first entry is selected$`, func() error { return w.expectSelectedIndex(0) })
	sc.Step(`^the second entry is selected$`, func() error { return w.expectSelectedIndex(1) })
	// .selected is client-side, so a stale highlight shows as a count of 2.
	sc.Step(`^exactly one entry is selected$`, func() error {
		return harness.Eventually("exactly one selected row", func() (bool, error) {
			rows, err := w.browser.CSSAll("[data-entry-row].selected")
			return len(rows) == 1, err
		})
	})
	sc.Step(`^the selected entry is titled "([^"]*)"$`, func(title string) error {
		return harness.Eventually(fmt.Sprintf("the selected row is %q", title), func() (bool, error) {
			row, err := w.browser.CSSOpt("[data-entry-row].selected")
			if err != nil || row == nil {
				return false, err
			}
			text, err := row.TextContent()
			return strings.Contains(text, title), err
		})
	})

	sc.Step(`^the go-to hint is visible$`, func() error {
		_, err := w.browser.CSS(".kbd-hint")
		return err
	})
	sc.Step(`^the go-to hint is gone$`, func() error {
		return harness.Eventually("the go-to hint to disappear", func() (bool, error) {
			hints, err := w.browser.CSSAll(".kbd-hint")
			return len(hints) == 0, err
		})
	})

	sc.Step(`^the keyboard shortcut help overlay is visible$`, func() error { return w.browser.ExpectVisible("kb-help") })
	sc.Step(`^the keyboard shortcut help overlay is hidden$`, func() error { return w.browser.ExpectHidden("kb-help") })
	sc.Step(`^the help overlay descriptions are aligned$`, w.helpAligned)
	sc.Step(`^the help overlay resolves its design tokens$`, w.helpTokens)
	sc.Step(`^pressing the "([^"]*)" key opens a new tab at "([^"]*)"$`, w.keyOpensTab)
}

func (w *world) expectSelectedIndex(index int) error {
	return harness.Eventually(fmt.Sprintf("entry %d is selected", index), func() (bool, error) {
		rows, err := w.browser.TestIDs("entry-item")
		if err != nil || index >= len(rows) {
			return false, err
		}
		classes, err := rows[index].Classes()
		if err != nil {
			return false, err
		}
		for _, class := range classes {
			if class == "selected" || class == "active" {
				return true, nil
			}
		}
		return false, nil
	})
}

// helpAligned: the wide Space key cell once pushed its description right.
// Measured in a script because selectors do not pierce shadow roots.
func (w *world) helpAligned() error {
	offsets, err := harness.Eval[[]float64](w.browser, `(() => {
		const root = document.querySelector('rdrs-kb-help').shadowRoot;
		return [...root.querySelectorAll('.shortcut-desc')].slice(0, 4)
			.map((el) => el.getBoundingClientRect().x);
	})()`)
	if err != nil {
		return err
	}
	if len(offsets) != 4 {
		return fmt.Errorf("the overlay renders %d descriptions, expected at least 4", len(offsets))
	}
	for index, x := range offsets[1:] {
		if math.Abs(x-offsets[0]) >= 1 {
			return fmt.Errorf("description %d starts at %v, but the first starts at %v", index+1, x, offsets[0])
		}
	}
	return nil
}

// helpTokens: tokens have no var() fallbacks, so a rename would unstyle the
// modal. Both sides are computed values so formatting normalises equally.
func (w *world) helpTokens() error {
	var probe map[string]string
	err := harness.EvalJSON(w.browser, `(() => {
		const modal = document.querySelector('rdrs-kb-help').shadowRoot.querySelector('.modal');
		const probeValue = (property, token) => {
			const el = document.createElement('span');
			el.style[property] = 'var(' + token + ')';
			document.body.appendChild(el);
			const value = getComputedStyle(el)[property];
			el.remove();
			return value;
		};
		const computed = getComputedStyle(modal);
		return {
			background: computed.backgroundColor,
			expectedBackground: probeValue('backgroundColor', '--color-panel'),
			color: computed.color,
			expectedColor: probeValue('color', '--color-text'),
			fontFamily: computed.fontFamily,
			expectedFontFamily: probeValue('fontFamily', '--font-ui'),
		};
	})()`, &probe)
	if err != nil {
		return err
	}
	for _, check := range []struct{ actual, expected, what string }{
		{"background", "expectedBackground", "background colour"},
		{"color", "expectedColor", "text colour"},
		{"fontFamily", "expectedFontFamily", "font stack"},
	} {
		if probe[check.actual] != probe[check.expected] {
			return fmt.Errorf("the overlay's %s is %q, but the token resolves to %q",
				check.what, probe[check.actual], probe[check.expected])
		}
	}
	return nil
}

// keyOpensTab asserts on the URL the page asked for: where the network is
// unreachable the popup's own URL collapses to chrome-error://.
func (w *world) keyOpensTab(key, expected string) error {
	before := w.browser.WindowOpens()
	if err := w.browser.Press(key); err != nil {
		return err
	}
	err := harness.Eventually("a new tab to open", func() (bool, error) {
		return w.browser.WindowOpens() > before, nil
	})
	if err != nil {
		return err
	}
	if err := w.browser.ClosePopups(); err != nil {
		return err
	}
	requested := w.browser.WindowOpenURL(before)
	if !strings.Contains(requested, expected) {
		return fmt.Errorf("the new tab opened at %s, expected %q", requested, expected)
	}
	return nil
}
