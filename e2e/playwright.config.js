import { defineConfig } from "@playwright/test";
import { defineBddConfig } from "playwright-bdd";

const testDir = defineBddConfig({
  features: "features/*.feature",
  steps: ["steps/*.js", "support/fixtures.js"],
});

export default defineConfig({
  testDir,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  workers: process.env.CI ? "50%" : "75%",
  // No retries anywhere: CI runs the same way a local run does. Retries were
  // previously 2 on CI, and a survey of the last twelve runs found every one of
  // the 25 retries they consumed was masking a single bug — the sidebar cache
  // serving a half-seeded world (fixed in #427) — with not one attributable to
  // runner or network noise. A retry that turns an intermittent failure green
  // buys nothing here except a quieter dashboard.
  //
  // If genuine infrastructure flakiness ever does show up, the answer is
  // `retries: 2` plus `failOnFlakyTests` — which keeps the retry as a
  // broken-vs-intermittent classifier while still failing the run — rather than
  // a bare retry that hides it again.
  retries: 0,
  reporter: process.env.CI ? "html" : "list",
  use: {
    // Paired with `retries: 0`: "on-first-retry" would never fire without a
    // retry to attach to, leaving failures with no trace at all.
    trace: "retain-on-failure",
  },
  globalSetup: "./global-setup.js",
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
