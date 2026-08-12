// Config for the scriptless walkthrough — separate from audit.config.js, whose
// testMatch is scoped to the two audit specs.
//
//   cd e2e && npx playwright test --config=scripts/nojs.config.js
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: /nojs-walkthrough\.spec\.js/,
  globalSetup: "../global-setup.js",
  reporter: "list",
  use: { trace: "off" },
  projects: [
    {
      name: "chromium",
      use: {
        browserName: "chromium",
        // Escape hatch for a machine whose cached browsers lag the lockfile:
        // point RDRS_CHROMIUM at any Chromium binary rather than re-downloading
        // one. Unset in CI, where `playwright install` has already run.
        launchOptions: process.env.RDRS_CHROMIUM
          ? { executablePath: process.env.RDRS_CHROMIUM }
          : undefined,
      },
    },
  ],
});
