import { test, expect } from "../fixtures/rdrs.js";

test.describe("Keyboard Help", () => {
  test.beforeAll(async ({ api }) => {
    await api.register("kbuser", "password123");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("kbuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  });

  // PR-12 cleanup: legacy keyboard.js ? help overlay may not mount on SSR pages. SSR-first migration changed behavior; spec § Testing schedules this for deletion in PR-12.
  test.fixme("? shows keyboard help, Escape hides it", async ({ page }) => {
    const kbHelp = page.locator("rdrs-kb-help");

    // Initially not visible
    await expect(kbHelp).not.toHaveClass(/visible/);

    // Press ? to show help
    await page.keyboard.press("?");
    await expect(kbHelp).toHaveClass(/visible/);

    // Press Escape to hide
    await page.keyboard.press("Escape");
    await expect(kbHelp).not.toHaveClass(/visible/);
  });
});
