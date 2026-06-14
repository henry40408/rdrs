// Standalone config so the touch-audit spec runs outside the BDD testDir.
// Run: cd e2e && npx playwright test --config=scripts/audit.config.js
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: /touch-audit\.spec\.js/,
  globalSetup: "../global-setup.js",
  reporter: "list",
  use: { trace: "off" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
