package e2e

import (
	"github.com/cucumber/godog"
)

// Bulk triage, per-row controls, and the reading-pane summary toggle.

func registerTriageSteps(sc *godog.ScenarioContext, w *world) {
	// A fake Kagi token makes the Summarize button render; requests hit the mock.
	sc.Step(`^the user has Kagi configured$`, func() error {
		userID, err := w.currentUserID()
		if err != nil {
			return err
		}
		return w.seed.ConfigureKagi(userID, "e2e-test-token")
	})
}
