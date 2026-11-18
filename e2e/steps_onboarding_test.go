package e2e

import (
	"fmt"

	"github.com/cucumber/godog"
)

// The landing page's getting-started guide and the app settings page.

func registerOnboardingSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I open the landing page$`, func() error { return w.goto_("/") })
	sc.Step(`^I am on the settings page$`, func() error { return w.goto_("/settings") })
	sc.Step(`^the category dropdown offers "([^"]*)"$`, func(label string) error {
		return w.browser.ExpectText("feed-category-select", label)
	})
	sc.Step(`^the landing page shows the getting-started guide$`, func() error {
		return w.browser.ExpectVisible("onboarding-guide")
	})
	sc.Step(`^the landing page does not show the getting-started guide$`, func() error {
		return w.browser.ExpectAbsent("onboarding-guide")
	})
	sc.Step(`^I see an? "([^"]*)" call to action$`, w.seeCallToAction)
	sc.Step(`^I see "([^"]*)" on the landing page$`, func(text string) error { return w.browser.ExpectTextSomewhere(text) })
	sc.Step(`^I see the active WebAuthn RP origin$`, func() error {
		return w.browser.ExpectText("webauthn-rp-origin", "http")
	})
}

func (w *world) seeCallToAction(label string) error {
	guide, err := w.browser.TestID("onboarding-guide")
	if err != nil {
		return err
	}
	link, err := guide.LinkNamed(label)
	if err != nil {
		return err
	}
	if link == nil {
		return fmt.Errorf("the guide offers no `%s` link", label)
	}
	shown, err := link.Displayed()
	if err != nil {
		return err
	}
	if !shown {
		return fmt.Errorf("the `%s` link is hidden", label)
	}
	return nil
}
