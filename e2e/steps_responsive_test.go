package e2e

import (
	"github.com/cucumber/godog"
)

// Viewports, the mobile drawer, the statistics chart and the flash banner.

func registerResponsiveSteps(sc *godog.ScenarioContext, w *world) {
	sc.Step(`^I open the inbox$`, func() error { return w.goto_("/") })
}
