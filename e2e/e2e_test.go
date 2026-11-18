package e2e

import (
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/cucumber/godog"

	"github.com/henry40408/rdrs/e2e/internal/harness"
)

const defaultFeatures = "features"

// TestE2E is the suite. Scenarios run one at a time: each starts its own
// server and browser, and a small machine drives one Chromium best.
func TestE2E(t *testing.T) {
	if err := preflight(); err != nil {
		t.Fatal(err)
	}
	suite := godog.TestSuite{
		Name:                "rdrs",
		ScenarioInitializer: initializeScenario,
		Options: &godog.Options{
			Format: "pretty",
			// ANSI codes make CI logs unreadable.
			NoColors:    os.Getenv("CI") != "",
			Output:      os.Stdout,
			Paths:       featurePaths(),
			Concurrency: 1,
			// Undefined and pending steps fail rather than skip.
			Strict: true,
		},
	}
	if status := suite.Run(); status != 0 {
		t.Fatalf("the suite failed with status %d", status)
	}
}

// preflight opens and closes one browser, so a missing Chromium gives one
// clear error rather than one per scenario.
func preflight() error {
	fmt.Fprintf(os.Stderr, "e2e: browser: %s\n", harness.BrowserChoice())
	browser, err := harness.Open()
	if err != nil {
		return err
	}
	browser.Close()
	return nil
}

// featurePaths is E2E_FEATURES, a comma-separated list of feature paths, for
// running one file locally. It defaults to every feature.
func featurePaths() []string {
	value := os.Getenv("E2E_FEATURES")
	if value == "" {
		return []string{defaultFeatures}
	}
	return strings.Split(value, ",")
}
