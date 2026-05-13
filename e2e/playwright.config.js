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
  retries: process.env.CI ? 2 : 0,
  reporter: process.env.CI ? "html" : "list",
  use: {
    trace: "on-first-retry",
  },
  globalSetup: "./global-setup.js",
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
