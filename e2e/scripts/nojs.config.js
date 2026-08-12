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
        // This machine has the chromium-1228 build but not the headless shell
        // Playwright 1.62 asks for (1234), so point at the full binary rather
        // than download another. Override with RDRS_CHROMIUM= if it moves.
        launchOptions: process.env.RDRS_CHROMIUM
          ? { executablePath: process.env.RDRS_CHROMIUM }
          : undefined,
      },
    },
  ],
});
