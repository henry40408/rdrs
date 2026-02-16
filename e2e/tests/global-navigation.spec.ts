import { test, expect } from "../fixtures/rdrs.js";

test.describe("Global Navigation Shortcuts", () => {
  test.beforeAll(async ({ api }) => {
    await api.register("gnuser", "password123");
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("gnuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
  });

  test("g then h navigates to home (unread)", async ({ page, serverUrl }) => {
    // First navigate away from home
    await page.goto(`${serverUrl}/entries`);
    await expect(page.getByTestId("main-nav")).toBeVisible();

    await page.keyboard.press("g");
    await page.keyboard.press("h");
    await page.waitForURL(`${serverUrl}/`);
  });

  test("g then e navigates to entries", async ({ page, serverUrl }) => {
    await page.keyboard.press("g");
    await page.keyboard.press("e");
    await page.waitForURL(`${serverUrl}/entries`);
  });

  test("g then s navigates to search", async ({ page, serverUrl }) => {
    await page.keyboard.press("g");
    await page.keyboard.press("s");
    await page.waitForURL(`${serverUrl}/search`);
  });
});
