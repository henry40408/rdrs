import { test, expect } from "../fixtures/rdrs.js";

test.describe("Entry Detail Page", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("detailuser", "password123");

    const userId = seed.getUserId("detailuser");
    const categoryId = seed.createCategory(userId, "Detail Category");
    const feedId = seed.createFeed(categoryId, "https://example.com/detail-feed.xml", "Detail Feed");

    seed.seedTestEntries(feedId, 5);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("detailuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();

    // Open first entry
    await page.getByTestId("entry-title-link").first().click();
    await page.waitForURL(/\/entries\/\d+/);
    await expect(page.getByTestId("entry-title")).toBeVisible();
  });

  test("opening entry auto-marks as read", async ({ page, serverUrl }) => {
    // Go back to unread list
    await page.getByTestId("back-link").click();
    await page.waitForURL(`${serverUrl}/`);

    // The entry we just viewed should no longer be in the unread list
    // (count should be 4 instead of 5)
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
    await expect(page.getByTestId("entry-item")).toHaveCount(4);
  });

  test("s toggles star button text", async ({ page }) => {
    const starBtn = page.getByTestId("star-btn");
    await expect(starBtn).toHaveText("Star");

    await page.keyboard.press("s");
    await expect(starBtn).toHaveText("Unstar");

    await page.keyboard.press("s");
    await expect(starBtn).toHaveText("Star");
  });

  test("u marks as unread and shows flash", async ({ page }) => {
    await page.keyboard.press("u");

    await expect(page.getByTestId("flash-message")).toBeVisible();
    await expect(page.getByTestId("flash-message")).toContainText(
      "Marked as unread"
    );
  });
});
