// Standalone config so the audit specs run outside the BDD testDir.
// Run one: cd e2e && npx playwright test --config=scripts/audit.config.js scripts/csp-audit.spec.js
//
// touch-audit is a report generator — it prints findings and always passes.
// csp-audit is a gate — it fails the run. Only the latter is wired into CI, so
// name the spec explicitly rather than running the whole config.
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: /(touch|csp)-audit\.spec\.js/,
  globalSetup: "../global-setup.js",
  reporter: "list",
  use: { trace: "off" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
