import { defineConfig } from "@playwright/test";

// Standalone config for the README screenshot generator (scripts/screenshots.js).
// The default playwright.config.js points testDir at the BDD-generated dir, so
// the generator needs its own testDir. Run via `npm run screenshots`.
export default defineConfig({
  testDir: "./scripts",
  testMatch: "screenshots.js",
  globalSetup: "./global-setup.js",
  workers: 1,
  reporter: "list",
  use: { viewport: { width: 1920, height: 1080 } },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
