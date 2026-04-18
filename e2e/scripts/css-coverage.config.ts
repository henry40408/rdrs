import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  fullyParallel: false,
  workers: 1,
  reporter: "list",
  globalSetup: "../global-setup.ts",
  use: {
    trace: "off",
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
});
