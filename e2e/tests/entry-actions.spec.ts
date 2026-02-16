import { test, expect } from "../fixtures/rdrs.js";

test.describe("Entry Actions", () => {
  test.beforeAll(async ({ api, seed }) => {
    await api.register("actuser", "password123");

    const userId = seed.getUserId("actuser");
    const categoryId = seed.createCategory(userId, "Actions Category");
    const feedId = seed.createFeed(categoryId, "https://example.com/actions-feed.xml", "Actions Feed");

    seed.seedTestEntries(feedId, 5);
  });

  test.beforeEach(async ({ page, serverUrl }) => {
    await page.goto(`${serverUrl}/login`);
    await page.getByTestId("username-input").fill("actuser");
    await page.getByTestId("password-input").fill("password123");
    await page.getByTestId("login-submit").click();
    await page.waitForURL(`${serverUrl}/`);
    await expect(page.getByTestId("entry-item").first()).toBeVisible();
  });

  test("click read action removes entry from unread list", async ({
    page,
  }) => {
    const initialCount = await page.getByTestId("entry-item").count();

    // Click the read action on the first entry
    await page.getByTestId("entry-read-action").first().click();

    // Entry count should decrease
    await expect(page.getByTestId("entry-item")).toHaveCount(
      initialCount - 1
    );
  });

  test("keyboard m marks entry as read", async ({ page }) => {
    const initialCount = await page.getByTestId("entry-item").count();

    // Select first entry and press m
    await page.keyboard.press("j");
    await page.keyboard.press("m");

    // Entry should be removed from unread list
    await expect(page.getByTestId("entry-item")).toHaveCount(
      initialCount - 1
    );
  });

  test("click star action toggles star text", async ({ page }) => {
    const starAction = page.getByTestId("entry-star-action").first();
    await expect(starAction).toHaveText("star");

    await starAction.click();
    await expect(starAction).toHaveText("unstar");

    await starAction.click();
    await expect(starAction).toHaveText("star");
  });

  test("keyboard s toggles star", async ({ page }) => {
    // Select first entry
    await page.keyboard.press("j");

    const starAction = page.getByTestId("entry-star-action").first();
    await expect(starAction).toHaveText("star");

    // Press s to star
    await page.keyboard.press("s");
    await expect(starAction).toHaveText("unstar");

    // Press s again to unstar
    await page.keyboard.press("s");
    await expect(starAction).toHaveText("star");
  });
});
